//! The Linux **network** backend for the network domain — the one host target
//! per `knowledge/02-HOST-CAPABILITIES.md` §2. It is **kernel-first** (charter
//! law #9): links, addresses, routes, counters, carrier and radio blocks are
//! read from the kernel over netlink (`super::netlink`), event-fed into a local
//! state store, so a read costs no daemon hop. NetworkManager over D-Bus
//! (`/run/dbus/system_bus_socket`) is the **counted fallback and the control
//! home**: it owns what the kernel does not (the master switch, radio policy,
//! per-device management) and the mutating verbs only NM exposes.
//!
//! ## Law #8 (the host is read-only)
//!
//! Every **read** path here is safe to run live on the founder's box: netlink
//! dumps and event subscriptions, the NM policy properties (`NetworkingEnabled`
//! / `WirelessEnabled` / `WwanEnabled`), `/sys`, and `/dev/rfkill`. The
//! **write** paths (`Enable`, `Properties.Set` on `WirelessEnabled`/
//! `WwanEnabled`) sit behind the [`NmClient`] seam and are only ever exercised by
//! (`a`) the mock in the tests and (`b`) the founder's `examples/network_probe.rs`
//! live bring-back. No build, test, or agent turns a host switch; the read-only
//! `#[ignore]`d test at the bottom can be run by whoever has the bus.
//!
//! ## The seams
//!
//! [`NetlinkReader`] is the kernel seam (a real
//! rtnetlink socket in production, a scripted kernel in tests) feeding
//! [`NetworkStore`]. [`NmClient`] is what remains of the NetworkManager surface:
//! ONE policy sweep plus the three control verbs. The production implementation
//! is [`RealNmClient`] (a small hand-rolled D-Bus client — no `nmcli`/`ip`
//! shellouts, the condemned pattern P5); tests swap in a `Mock` that scripts the
//! same seam, records every call and injects faults, so the four-state
//! diagnostic, the zero-NmClient-calls-on-a-warm-store proof and every mutation
//! mapping are proven without touching the host.
//!
//! ## The iwd variant
//!
//! A second backend (iwd D-Bus) is a *designed seam*, not built now:
//! [`NmClient`] is the seam it would implement, and
//! [`crate::domains::network::NetworkBackend`] would delegate identically.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::{Path, PathBuf};
use crate::core::ids::NetworkInterfaceId;
use crate::core::values::Duration;
use crate::core::{Error, Result};
use crate::domains::network::backend::NetworkBackend;
use crate::domains::network::types::{
    ConnStatus, HostState, HostStateSource, InterfaceCounters, InterfaceSnapshot, Page, Pagination,
    RadioState, SubscriptionId,
};
use crate::platforms::linux::netlink::{
    self, KernelAddress, KernelEvent, KernelLink, KernelRoute, LinkKind, LinkProvenance,
    NetlinkReader, OperState, RealNetlinkReader, RfkillState,
};

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// A minimal D-Bus system-bus client (read/write method calls).
// ═══════════════════════════════════════════════════════════════════════

/// The D-Bus manager identity (NetworkManager).
const NM_DEST: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const NM_DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRED_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wired";
const DBUS_PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";

/// A D-Bus value — only the arms the NM surface needs.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Boolean(bool),
    Byte(u8),
    Int32(i32),
    UInt32(u32),
    UInt64(u64),
    Text(String),
    ObjectPath(String),
    Array(Vec<Val>),
    Struct(Vec<Val>),
    /// A variant carries its own inner signature (needed to marshal it back out).
    Variant(Vec<u8>, Box<Val>),
}

/// Alignment of a D-Bus signature char, measured from the start of the message.
fn align_of(sig: u8) -> usize {
    match sig {
        b'y' | b'g' => 1,
        b'n' | b'q' => 2,
        b'i' | b'u' | b'b' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'v' | b'{' => 8,
        _ => 1,
    }
}

fn pad_to(n: usize, a: usize) -> usize {
    let r = n % a;
    if r == 0 {
        n
    } else {
        n + (a - r)
    }
}

/// Marshal `val` at absolute offset `buf.len()`, following the type in `sig`.
fn marshal(buf: &mut Vec<u8>, sig: &[u8], val: &Val) -> Result<()> {
    let s = sig[0];
    // A variant is not pre-padded: its signature byte follows immediately and
    // only the value inside is aligned.
    if s != b'v' {
        let target = pad_to(buf.len(), align_of(s));
        buf.resize(target, 0);
    }
    match (s, val) {
        (b'y', Val::Byte(v)) => buf.push(*v),
        (b'b', Val::Boolean(v)) => buf.extend_from_slice(&u32::from(*v).to_le_bytes()),
        (b'i', Val::Int32(v)) => buf.extend_from_slice(&v.to_le_bytes()),
        (b'q', _) => {} // uint16, unused
        (b'u', Val::UInt32(v)) => buf.extend_from_slice(&v.to_le_bytes()),
        (b'x', Val::Int32(v)) => buf.extend_from_slice(&i64::from(*v).to_le_bytes()),
        (b't', Val::UInt64(v)) => buf.extend_from_slice(&v.to_le_bytes()),
        (b's', Val::Text(v)) => marshal_blob(buf, v.as_bytes()),
        (b'o', Val::Text(v)) | (b'o', Val::ObjectPath(v)) => marshal_blob(buf, v.as_bytes()),
        (b'g', Val::Text(v)) => {
            buf.push(v.len() as u8);
            buf.extend_from_slice(v.as_bytes());
            buf.push(0);
        }
        (b'a', Val::Array(items)) => {
            // element signature is sig[1..next-complete-type]
            let (elem_sig, rest) = split_one_type(&sig[1..]);
            let content = buf.len();
            buf.extend_from_slice(&0u32.to_le_bytes()); // length placeholder
            let start = buf.len();
            for item in items {
                marshal(buf, elem_sig, item)?;
            }
            let len = (buf.len() - start) as u32;
            // arrays that are struct members are padded to 8; not needed here.
            buf[content..content + 4].copy_from_slice(&len.to_le_bytes());
            let _ = rest;
        }
        (b'(', Val::Struct(members)) => {
            let mut rest = &sig[1..];
            for (i, m) in members.iter().enumerate() {
                let (member_sig, r) = split_one_type(rest);
                rest = r;
                marshal(buf, member_sig, m)
                    .map_err(|_| Error::failed("struct member marshalling failed"))?;
                let _ = i;
            }
        }
        (b'v', Val::Variant(inner_sig, inner)) => {
            write_variant(buf, inner_sig, inner)?;
        }
        _ => return Err(Error::failed("marshalling: type/signature mismatch")),
    }
    Ok(())
}

fn marshal_blob(buf: &mut Vec<u8>, bytes: &[u8]) {
    // D-Bus string / object-path: the length field is the byte count WITHOUT
    // the terminating NUL; the NUL is a separate trailing byte.
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    buf.push(0);
}

/// Write a variant: `[siglen:u8][inner_sig][nul]` then the value aligned to its
/// inner type. The wrapper carries no 8-alignment of its own.
fn write_variant(buf: &mut Vec<u8>, inner_sig: &[u8], inner: &Val) -> Result<()> {
    buf.push(inner_sig.len() as u8);
    buf.extend_from_slice(inner_sig);
    buf.push(0);
    let t = pad_to(buf.len(), align_of(inner_sig[0]));
    buf.resize(t, 0);
    marshal(buf, inner_sig, inner)
}

/// Split a signature into (one complete type, the rest), handling containers.
fn split_one_type(sig: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0;
    let mut depth = 0usize;
    loop {
        let c = sig[i];
        match c {
            b'a' => {}
            b'(' | b'{' => depth += 1,
            b')' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
            }
            _ if depth == 0 => {
                i += 1;
                break;
            }
            _ => {}
        }
        i += 1;
    }
    sig.split_at(i)
}

/// Borrow `buf[at..at+n]`, or fail honestly if out of range (never a partial
/// read silently padded).
fn take<'a>(b: &'a [u8], at: usize, n: usize) -> Result<&'a [u8]> {
    b.get(at..at + n).ok_or_else(|| Error::failed("unmarshal: out of range"))
}

/// Unmarshal one value of type `sig` from `buf` at `*off` (absolute, message
/// aligned), advancing `*off`.
fn unmarshal(buf: &[u8], off: &mut usize, sig: &[u8]) -> Result<Val> {
    let Some(&s) = sig.first() else {
        return Err(Error::failed("unmarshal: empty signature (wire desync)"));
    };
    // A variant is not pre-padded (its signature byte follows immediately).
    if s != b'v' {
        *off = pad_to(*off, align_of(s));
    }
    let val = match s {
        b'y' => {
            let v = take(buf, *off, 1)?[0];
            *off += 1;
            Val::Byte(v)
        }
        b'b' => {
            let v = u32::from_le_bytes(take(buf, *off, 4)?.try_into().unwrap());
            *off += 4;
            Val::Boolean(v != 0)
        }
        b'i' | b'n' => {
            let v = i32::from_le_bytes(take(buf, *off, 4)?.try_into().unwrap());
            *off += 4;
            Val::Int32(v)
        }
        b'u' => {
            let v = u32::from_le_bytes(take(buf, *off, 4)?.try_into().unwrap());
            *off += 4;
            Val::UInt32(v)
        }
        b'x' => {
            let v = i64::from_le_bytes(take(buf, *off, 8)?.try_into().unwrap());
            *off += 8;
            Val::Int32(v as i32)
        }
        b't' => {
            let v = u64::from_le_bytes(take(buf, *off, 8)?.try_into().unwrap());
            *off += 8;
            Val::UInt64(v)
        }
        b's' | b'o' => {
            let len = u32::from_le_bytes(take(buf, *off, 4)?.try_into().unwrap()) as usize;
            *off += 4;
            let s = take(buf, *off, len)?;
            *off += len;
            *off += 1; // the NUL byte is always present and not part of the length
            let text = String::from_utf8_lossy(s).into_owned();
            if sig[0] == b'o' {
                Val::ObjectPath(text)
            } else {
                Val::Text(text)
            }
        }
        b'g' => {
            let len = take(buf, *off, 1)?[0] as usize;
            *off += 1;
            let s = take(buf, *off, len)?;
            *off += len + 1; // skip nul
            Val::Text(String::from_utf8_lossy(&s[..len.min(s.len())]).into_owned())
        }
        b'a' => {
            let (elem_sig, _rest) = split_one_type(&sig[1..]);
            *off = pad_to(*off, 4);
            let byte_len = u32::from_le_bytes(take(buf, *off, 4)?.try_into().unwrap()) as usize;
            *off += 4;
            let end = *off + byte_len;
            let mut items = Vec::new();
            // array elements align to their type from message start, but the
            // array's element region starts here; NM marshals with absolute
            // alignment, so we keep absolute offsets.
            while *off < end {
                items.push(unmarshal(buf, off, elem_sig)?);
            }
            *off = end;
            Val::Array(items)
        }
        b'(' => {
            let mut members = Vec::new();
            let mut rest = &sig[1..];
            while rest[0] != b')' {
                let (member_sig, r) = split_one_type(rest);
                rest = r;
                members.push(unmarshal(buf, off, member_sig)?);
            }
            *off += 0; // ')' contributes nothing
            Val::Struct(members)
        }
        b'v' => {
            let slen = take(buf, *off, 1)?[0] as usize;
            *off += 1;
            let vsig = take(buf, *off, slen)?;
            *off += slen + 1; // nul
            let inner = unmarshal(buf, off, vsig)?;
            Val::Variant(vsig.to_vec(), Box::new(inner))
        }
        other => return Err(Error::failed(format!("unmarshal: unsupported sig {other}"))),
    };
    Ok(val)
}

/// Header-field codes we write/read.
const FIELD_PATH: u8 = 1;
const FIELD_INTERFACE: u8 = 2;
const FIELD_MEMBER: u8 = 3;
const FIELD_SIGNATURE: u8 = 8;
const FIELD_REPLY_SERIAL: u8 = 5;

/// Build one method-call message wire bytes.
fn build_call(serial: u32, dest: &str, path: &str, iface: &str, member: &str, body_sig: &str, body: &[Val]) -> Vec<u8> {
    // Body first (its length is a fixed field, so header size is deterministic).
    let mut body_bytes = Vec::new();
    let mut rest = body_sig.as_bytes();
    for item in body {
        let (one, r) = split_one_type(rest);
        rest = r;
        marshal(&mut body_bytes, one, item).unwrap_or(());
    }

    // Header fields (marshal with an absolute base of 16, the fixed header).
    let mut fields: Vec<u8> = Vec::new();
    // ORDER does not matter; append DESTINATION(6? no) — path/interface/member/signature.
    write_field(&mut fields, FIELD_PATH, b"o", &Val::ObjectPath(path.to_string()));
    write_field(&mut fields, FIELD_INTERFACE, b"s", &Val::Text(iface.to_string()));
    write_field(&mut fields, FIELD_MEMBER, b"s", &Val::Text(member.to_string()));
    // DESTINATION field (code 6) so the bus routes to NM.
    write_field(&mut fields, 6, b"s", &Val::Text(dest.to_string()));
    if !body_sig.is_empty() {
        write_field(&mut fields, FIELD_SIGNATURE, b"g", &Val::Text(body_sig.to_string()));
    }
    let field_bytes = fields.len() as u32;

    let header_len = 16 + field_bytes as usize;
    let body_start = pad_to(header_len, 8);
    let total = body_start + body_bytes.len();

    let mut msg = Vec::with_capacity(total);
    msg.push(b'l'); // little-endian
    msg.push(1); // method_call
    msg.push(0); // flags
    msg.push(1); // protocol version
    msg.extend_from_slice(&(body_bytes.len() as u32).to_le_bytes()); // body length
    msg.extend_from_slice(&serial.to_le_bytes()); // serial
    msg.extend_from_slice(&field_bytes.to_le_bytes()); // fields array length
    debug_assert_eq!(msg.len(), 16);
    msg.extend_from_slice(&fields);
    msg.resize(body_start, 0);
    msg.extend_from_slice(&body_bytes);
    msg
}

/// Write one header field: an 8-aligned struct of (byte code, variant value).
fn write_field(buf: &mut Vec<u8>, code: u8, inner_sig: &[u8], value: &Val) {
    let at = pad_to(buf.len(), 8);
    buf.resize(at, 0);
    buf.push(code);
    write_variant(buf, inner_sig, value).unwrap_or(());
}

/// D-Bus message types and the ERROR_NAME header field.
const MSG_METHOD_CALL: u8 = 1;
const MSG_ERROR: u8 = 3;
const MSG_SIGNAL: u8 = 4;
const FIELD_ERROR_NAME: u8 = 4;

/// Read exactly one complete D-Bus message off the stream and return its bytes.
fn read_raw_message(stream: &mut UnixStream) -> Result<Vec<u8>> {
    use std::io::Read;
    let classify = |e: std::io::Error| -> Error {
        match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                Error::temporary(format!("dbus read timed out: {e}"))
            }
            _ => Error::failed(format!("dbus read header: {e}")),
        }
    };
    let mut fixed = [0u8; 16];
    stream.read_exact(&mut fixed).map_err(classify)?;
    if fixed[0] != b'l' {
        return Err(Error::failed("dbus: only little-endian messages are supported"));
    }
    let body_len = u32::from_le_bytes(fixed[4..8].try_into().unwrap()) as usize;
    let fields_len = u32::from_le_bytes(fixed[12..16].try_into().unwrap()) as usize;
    let body_start = pad_to(16 + fields_len, 8);
    let total = body_start + body_len;
    let mut rest = alloc::vec![0u8; total - 16];
    stream
        .read_exact(&mut rest)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                Error::temporary(format!("dbus read timed out: {e}"))
            }
            _ => Error::failed(format!("dbus read body: {e}")),
        })?;
    let mut msg = fixed.to_vec();
    msg.extend_from_slice(&rest);
    Ok(msg)
}

/// Read the next method reply. Skips interleaved signals (e.g. the `NameAcquired`
/// a daemon emits right after `Hello`) and turns an ERROR reply into a mapped
/// `core::Error` (a polkit denial becomes `permissionDenied`, never a silent
/// zero). Returns `(reply_serial, body_signature, body_bytes)` on METHOD_RETURN.
fn read_message(stream: &mut UnixStream) -> Result<(u32, String, Vec<u8>)> {
    loop {
        let msg = read_raw_message(stream)?;
        let kind = msg[1];
        if kind == MSG_SIGNAL || kind == MSG_METHOD_CALL {
            continue; // not a reply to our pending call — skip it.
        }
        let body_len = u32::from_le_bytes(msg[4..8].try_into().unwrap()) as usize;
        let fields_len = u32::from_le_bytes(msg[12..16].try_into().unwrap()) as usize;
        let body_start = pad_to(16 + fields_len, 8);
        // The header-fields array is `fields_len` bytes and the LAST element is
        // NOT padded, so fields end exactly at `16 + fields_len`.
        let fields_end = 16 + fields_len;
        let mut off = 16usize;
        let mut reply_serial = 0u32;
        let mut body_sig = String::new();
        let mut error_name = String::new();
        while off < fields_end {
            off = pad_to(off, 8);
            if off >= msg.len() {
                break;
            }
            let code = msg[off];
            off += 1;
            let slen = msg[off] as usize;
            off += 1;
            let vsig = &msg[off..off + slen];
            off += slen + 1;
            if code == FIELD_REPLY_SERIAL {
                off = pad_to(off, 4);
                reply_serial = u32::from_le_bytes(msg[off..off + 4].try_into().unwrap());
                off += 4;
            } else if code == FIELD_SIGNATURE {
                let g = unmarshal(&msg, &mut off, b"g")?;
                if let Val::Text(s) = g {
                    body_sig = s;
                }
            } else if code == FIELD_ERROR_NAME {
                let e = unmarshal(&msg, &mut off, b"s")?;
                if let Val::Text(s) = e {
                    error_name = s;
                }
            } else {
                // Skip the value by unmarshalling with its own signature.
                let _ = unmarshal(&msg, &mut off, vsig)?;
            }
        }
        if kind == MSG_ERROR {
            // Map the D-Bus error name onto a contract code honestly.
            if error_name.contains("AccessDenied") || error_name.contains("NotAuthorized") {
                return Err(Error::permission_denied()
                    .with_context(format!("NetworkManager refused: {error_name}")));
            }
            return Err(Error::failed(format!("NetworkManager D-Bus error: {error_name}")));
        }
        let body = msg[body_start..body_start + body_len].to_vec();
        return Ok((reply_serial, body_sig, body));
    }
}

/// This process's real uid, read from `/proc/self/status` (no libc dependency).
fn current_uid() -> u32 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // "Uid:\treal\teffective\tsaved\tfs"
            if let Some(first) = rest.split_whitespace().next() {
                if let Ok(uid) = first.parse::<u32>() {
                    return uid;
                }
            }
        }
    }
    0
}

/// Perform EXTERNAL SASL auth and BEGIN on a freshly connected stream. EXTERNAL
/// needs this process's uid (from `/proc/self/status`), which dbus-daemon checks
/// against the socket peer credential.
fn authenticate(stream: &mut UnixStream) -> Result<()> {
    use std::io::{Read, Write};
    stream
        .write_all(b"\0")
        .map_err(|e| Error::failed(format!("dbus auth write: {e}")))?;
    let hex: String = current_uid()
        .to_string()
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect();
    let hello = format!("AUTH EXTERNAL {hex}\r\n");
    stream
        .write_all(hello.as_bytes())
        .map_err(|e| Error::failed(format!("dbus auth: {e}")))?;
    // Read until "OK" line.
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(|e| Error::failed(format!("dbus auth read: {e}")))?;
        if n == 0 {
            break;
        }
        line.push(byte[0]);
        if line.ends_with(b"\r\n") {
            if line.starts_with(b"OK") {
                break;
            }
            if line.starts_with(b"ER") || line.starts_with(b"REJECTED") {
                return Err(Error::failed("dbus auth rejected"));
            }
            line.clear();
        }
    }
    // Negotiate Unix FD passing the way a full client does (the connection is
    // left half-activated on some daemons without it).
    stream
        .write_all(b"NEGOTIATE_UNIX_FD\r\n")
        .map_err(|e| Error::failed(format!("dbus negotiate: {e}")))?;
    // Read the AGREE_UNIX_FD (or ERROR) line.
    let mut line2 = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .map_err(|e| Error::failed(format!("dbus negotiate read: {e}")))?;
        if n == 0 {
            return Err(Error::failed("dbus: peer closed during NEGOTIATE_UNIX_FD"));
        }
        line2.push(byte[0]);
        if line2.ends_with(b"\r\n") {
            if line2.starts_with(b"AGREE") || line2.starts_with(b"ERROR") {
                break;
            }
            line2.clear();
        }
    }
    stream
        .write_all(b"BEGIN\r\n")
        .map_err(|e| Error::failed(format!("dbus begin: {e}")))?;
    Ok(())
}

/// Synchronous call: connect (lazily), auth, marshal, send, read reply, and
/// unmarshal the reply body against the reply's own signature.
fn dbus_call(
    stream: &mut UnixStream,
    serial: &AtomicU32,
    dest: &str,
    path: &str,
    iface: &str,
    member: &str,
    body_sig: &str,
    body: &[Val],
) -> Result<Vec<Val>> {
    use std::io::Write;
    let s = serial.fetch_add(1, Ordering::Relaxed);
    let wire = build_call(s, dest, path, iface, member, body_sig, body);
    stream
        .write_all(&wire)
        .map_err(|e| Error::failed(format!("dbus write: {e}")))?;
    let (_reply_serial, reply_sig, reply_body) = read_message(stream)?;
    // Unmarshal the reply body into values, matching reply_sig.
    let mut off = 0usize;
    let mut out = Vec::new();
    let mut rest = reply_sig.as_bytes();
    while !rest.is_empty() {
        let (one, r) = split_one_type(rest);
        rest = r;
        out.push(unmarshal(&reply_body, &mut off, one)?);
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════
// The mockable NetworkManager seam.
// ═══════════════════════════════════════════════════════════════════════

/// One NetworkManager device, the fields the read surface maps. Mirrors the
/// D-Bus device properties consumed here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub path: String,
    /// `Interface` name (the OS ifname).
    pub iface: String,
    /// NM `DeviceType` (1=ethernet, 2=wifi, 14=modem, 32=loopback, ...).
    pub device_type: u32,
    /// NM `State` (10=unmanaged, 20=unavailable, 30=disconnected, 100=activated).
    pub state: u32,
    pub firmware_missing: bool,
    pub managed: bool,
    /// Wired `Carrier`, `None` for a non-wired device.
    pub carrier: Option<bool>,
    /// NM `Real` (false for p2p/generic pseudo-devices, which we filter out).
    pub real: bool,
    pub hw_address: String,
    pub mtu: u32,
    pub driver: String,
}

/// Everything NetworkManager owns that the kernel does not — the answer to the
/// ONE counted fallback sweep. This is the whole D-Bus surface a read can
/// touch, and it is fetched as a single seam call, cached against the store's
/// topology revision (a topology change — a link or address arriving or going
/// away, or a link's administrative state flipping — re-sweeps).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NmPolicy {
    /// `NetworkingEnabled` — the field that once persisted `false`.
    pub networking_enabled: bool,
    /// `WirelessEnabled` — wifi radio policy.
    pub wireless_enabled: bool,
    /// `WirelessHardwareEnabled` — the wifi hardware switch, as NM sees it.
    pub wireless_hardware_enabled: bool,
    /// `WwanEnabled` — cellular radio / mobile-data policy.
    pub wwan_enabled: bool,
    /// `WwanHardwareEnabled` — the cellular hardware switch, as NM sees it.
    pub wwan_hardware_enabled: bool,
    /// NM's device view: the `Managed` policy bit per interface, and the facts a
    /// *fallback* link record needs when the kernel link table cannot answer.
    pub devices: Vec<DeviceSnapshot>,
}

impl NmPolicy {
    /// NM's `Managed` for one ifname; `None` when NM does not enumerate it (a
    /// link NM never saw — a container veth, a bridge) — never a guess.
    pub fn managed(&self, iface: &str) -> Option<bool> {
        self.device(iface).map(|d| d.managed)
    }

    /// NM's device view entry for one ifname.
    pub fn device(&self, iface: &str) -> Option<&DeviceSnapshot> {
        self.devices.iter().find(|d| d.iface == iface)
    }

    /// NM's device view filtered to `Real` devices with a name — the set a
    /// fallback link table is synthesised from.
    pub fn real_devices(&self) -> Vec<DeviceSnapshot> {
        self.devices
            .iter()
            .filter(|d| d.real && !d.iface.is_empty())
            .cloned()
            .collect()
    }
}

/// Everything the network backend needs from NetworkManager. After the
/// transport correction (law #9) this seam is **policy and control only**:
///
/// * [`NmClient::policy`] — the ONE sweep that answers everything the kernel
///   does not own (the master switch, radio policy, per-device management).
///   It is the counted fallback: a read that needs policy state issues exactly
///   one seam call, and every subsequent read is served from the store's
///   snapshot.
/// * [`NmClient::counters`] / [`NmClient::rfkill`] — kernel reads behind the
///   seam (`/sys` statistics, `/dev/rfkill`), used ONLY when netlink could not
///   answer. Counted like every other seam call.
/// * the three control verbs — host-mutating, so they stay behind the seam
///   (law #8: the mock proves the mapping, the founder runs the live proof).
///
/// What is deliberately GONE: per-read `networking_enabled` / `wireless_enabled`
/// / `devices` / property readers. Those were the D-Bus-per-read path the
/// correction removed — the kernel first answers links, addresses, routes,
/// counters, carrier and radio blocks now (`platforms::linux::netlink`).
pub trait NmClient {
    /// One policy sweep (see [`NmPolicy`]). Errors are total: a fault here is
    /// the seam's error, carried to the caller unchanged.
    fn policy(&self) -> Result<NmPolicy>;

    /// Counters for an interface the kernel link table did not report
    /// (`IFLA_STATS64` absent). A `/sys` read, not a daemon hop — counted all
    /// the same, because it is a seam call.
    fn counters(&self, iface: &str) -> Result<InterfaceCounters>;

    /// The kernel's rfkill view when the netlink snapshot could not read it.
    fn rfkill(&self) -> Result<Vec<RfkillState>>;

    /// `org.freedesktop.NetworkManager.Enable(bool)` — the host-global master
    /// switch. Host-mutating; founder-run live, mock-verified here.
    fn set_networking_enabled(&self, enabled: bool) -> Result<()>;

    /// `Properties.Set WirelessEnabled` — the wifi radio switch.
    fn set_wireless_enabled(&self, enabled: bool) -> Result<()>;

    /// `Properties.Set WwanEnabled` — the cellular radio / mobile-data switch.
    fn set_wwan_enabled(&self, enabled: bool) -> Result<()>;
}

// ── NM state → open-token mapping (host map §1.2 N1) ──────────────────────

/// NM's `Device.State` for an activated (usable) connection — one of the two NM
/// state numbers kept after the kernel took over the rest of the read surface.
const NM_DEVICE_STATE_ACTIVATED: u32 = 100;

/// NM's `Device.State` for a device it cannot use with no named cause — the
/// state whose honest status token is `unknown`, never `managed`.
const NM_DEVICE_STATE_UNAVAILABLE: u32 = 20;

/// Whether the kernel reports this radio blocked (`/dev/rfkill`, hard or soft).
/// A kernel fact, kept apart from NM's policy `enabled` bit.
fn radio_blocked(rfkill: &[RfkillState], radio: &str) -> bool {
    rfkill
        .iter()
        .any(|r| r.name == radio && (r.hard_blocked || r.soft_blocked))
}

/// The four-state diagnostic as a pure function of the host facts, returning
/// BOTH the state and the SOURCE that decided it (law #9 made auditable):
/// `carrier-down` / `no-firmware` / a kernel-rfkill `radio-off` arrive from
/// netlink (kernel facts); `networking-disabled` / `radio-off` via NM policy /
/// `unmanaged` arrive from the counted D-Bus fallback.
///
/// The order is load-bearing. First the two NM policy facts that describe the
/// host rather than the wire — the master switch outranks every per-device
/// reason, so a switched-off host reads `networking-disabled` even when a device
/// also looks unmanaged. Then the kernel decides what only it can see: a radio
/// the kernel itself blocked is `radio-off`, and a link the host asked up whose
/// hardware never delivered is `no-firmware` (or, on a wire, `carrier-down`).
/// NM policy then decides the radio bits, the kernel's operstate is the final
/// witness for `managed`, and anything nothing named is an honest `unknown`.
///
/// This is the incident as a function: `networking-disabled` (the master
/// switch), `radio-off` (policy on the radio), `no-firmware` (the device cannot
/// come up), `carrier-down` (nothing on the wire) are four DISTINCT answers,
/// never collapsed into a bare `connected: false` — and each now names the
/// layer that produced it.
fn classify_host_state(
    link: &KernelLink,
    rfkill: &[RfkillState],
    policy: &NmPolicy,
) -> (HostState, HostStateSource) {
    let device = policy.device(&link.name);
    let wifi = link.is_wifi();
    let radio = link.is_radio();
    // The kernel's own stuck-radio marker: asked up, operstate DOWN, no carrier
    // (see `KernelLink::stalled`). NM refutes it when it enumerates the device
    // and reports no firmware fault — a radio that is merely not associated
    // looks identical from netlink alone, and NM already knows the difference.
    let kernel_stalled = radio && link.stalled();
    let firmware_missing = match device {
        Some(d) => d.firmware_missing,
        None => kernel_stalled,
    };

    // 1. The host-global master switch (NM policy) outranks everything.
    if !policy.networking_enabled {
        return (HostState::NetworkingDisabled, HostStateSource::NmPolicy);
    }
    // 2. NM does not drive this device (policy).
    if let Some(false) = policy.managed(&link.name) {
        return (HostState::Unmanaged, HostStateSource::NmPolicy);
    }
    // 3. A radio the kernel itself blocked is radio-off, and the kernel is the
    //    source (the rfkill bit, never NM's policy `enabled`). It also explains
    //    a link that never came up, so it outranks the firmware verdict.
    let kernel_radio_blocked =
        (wifi && radio_blocked(rfkill, "wifi")) || (link.is_cellular() && radio_blocked(rfkill, "cellular"));
    if kernel_radio_blocked {
        return (HostState::RadioOff, HostStateSource::Kernel);
    }
    // 4. The device cannot come up: the kernel's stuck-radio marker when NM
    //    cannot answer, or NM's probe-derived verdict when it can. Both name a
    //    kernel-visible condition.
    if firmware_missing {
        let source = if kernel_stalled { HostStateSource::Kernel } else { HostStateSource::NmPolicy };
        return (HostState::NoFirmware, source);
    }
    // 5. Radio policy (NM): the radio is on for the host but NM has it off.
    if wifi && !policy.wireless_enabled {
        return (HostState::RadioOff, HostStateSource::NmPolicy);
    }
    // 6. Nothing on the wire — a non-radio link, administratively up, with
    //    IFLA_CARRIER == 0. On a radio, carrier means "associated", not
    //    "plugged", so it is deliberately not read as carrier-down here.
    if !radio && link.up && link.carrier == Some(false) {
        return (HostState::CarrierDown, HostStateSource::Kernel);
    }
    // 7. Running, or NM reports the device available with no named cause.
    if link.is_running() || device.is_some_and(|d| d.state != NM_DEVICE_STATE_UNAVAILABLE) {
        return (HostState::Managed, HostStateSource::Kernel);
    }
    // 8. Nothing above named a cause: report the gap, never a fabricated
    //    `managed`. The witness is whichever layer last spoke.
    let source = if device.is_some() {
        HostStateSource::NmPolicy
    } else {
        HostStateSource::Kernel
    };
    (HostState::Unknown, source)
}

/// A stable interface id derived from the OS ifname (FNV-1a/64). NOT a host
/// credential and NOT fabricated data: the ifname is the measured, stable key
/// the backend already holds, so the id is deterministic across calls (the
/// contract's id is a `UInt64`; this mapping keeps `getStatus(id)` reproducible
/// without a hidden session table).
fn iface_id(name: &str) -> NetworkInterfaceId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    NetworkInterfaceId::new(hash)
}

// ═══════════════════════════════════════════════════════════════════════
// The event-fed kernel state store (charter law #9: kernel first).
// ═══════════════════════════════════════════════════════════════════════

/// The most kernel events one read drains. Bounded, so an event flood can never
/// make a read unbounded; what arrives afterwards is picked up by the next read
/// (the store keeps the LATEST state per link, so nothing is lost — only the
/// intermediate revisions a subscription pull collapses, and those are counted).
const MAX_DRAIN_EVENTS: usize = 64;

/// One link / address / route / rfkill table, fed by netlink events and read
/// with no daemon hop. A read brings it current (a bounded, non-blocking event
/// drain plus a prime dump when the table has never been filled) and then serves
/// from memory — the kernel's facts, held once.
pub struct NetworkStore {
    /// The kernel seam (the real rtnetlink reader in production, a scripted
    /// mock in tests).
    reader: Box<dyn NetlinkReader>,
    state: Mutex<StoreState>,
    /// Bumped once per applied event that changed the table — the revision a
    /// subscription pull measures its collapsed updates against.
    revision: AtomicU64,
}

/// The mutable table, behind one lock: one read, one lock, no daemon hop.
struct StoreState {
    /// A kernel sweep has answered at least once (or the fallback installed).
    primed: bool,
    /// NM's device view is installed as the fallback link table.
    fallback_installed: bool,
    /// Why the kernel seam could not answer, if it could not — reported on every
    /// accessor path, never silently empty facts.
    kernel_error: Option<String>,
    links: Vec<KernelLink>,
    addresses: Vec<KernelAddress>,
    routes: Vec<KernelRoute>,
    rfkill: Vec<RfkillState>,
    /// Bumped when the link set, a link's administrative state, or the address
    /// set changed — the signal that NM's policy view may have gone stale.
    topology_revision: u64,
}

impl StoreState {
    /// Apply one kernel event; `true` when the table changed. A link appearing
    /// or disappearing, a link's administrative flag flipping, or an address
    /// appearing or going away also bumps `topology_revision` — that is what
    /// invalidates the cached NM policy sweep, so a topology change re-sweeps.
    fn apply(&mut self, event: &KernelEvent) -> bool {
        match event {
            KernelEvent::Link(link) => match self.links.iter_mut().find(|l| l.index == link.index) {
                Some(existing) => {
                    let topology = existing.name != link.name
                        || existing.kind != link.kind
                        || existing.info_kind != link.info_kind
                        || existing.up != link.up;
                    let changed = topology || *existing != *link;
                    *existing = link.clone();
                    if topology {
                        self.topology_revision += 1;
                    }
                    changed
                }
                None => {
                    self.links.push(link.clone());
                    self.links.sort_by(|a, b| a.name.cmp(&b.name));
                    self.topology_revision += 1;
                    true
                }
            },
            KernelEvent::LinkGone(index) => {
                let before = self.links.len();
                self.links.retain(|l| l.index != *index);
                let changed = self.links.len() != before;
                if changed {
                    self.topology_revision += 1;
                }
                changed
            }
            KernelEvent::Address(address) => {
                if self.addresses.iter().any(|a| a == address) {
                    false
                } else {
                    self.addresses.push(address.clone());
                    self.topology_revision += 1;
                    true
                }
            }
            KernelEvent::AddressGone(address) => {
                let before = self.addresses.len();
                self.addresses.retain(|a| a != address);
                let changed = self.addresses.len() != before;
                if changed {
                    self.topology_revision += 1;
                }
                changed
            }
        }
    }
}

impl NetworkStore {
    /// A store over an explicit kernel seam (tests script a mock kernel).
    pub fn with_reader(reader: Box<dyn NetlinkReader>) -> Self {
        NetworkStore {
            reader,
            state: Mutex::new(StoreState {
                primed: false,
                fallback_installed: false,
                kernel_error: None,
                links: Vec::new(),
                addresses: Vec::new(),
                routes: Vec::new(),
                rfkill: Vec::new(),
                topology_revision: 0,
            }),
            revision: AtomicU64::new(0),
        }
    }

    /// The production store: the real rtnetlink reader over a live netlink
    /// socket (read-only; law #8).
    pub fn kernel_first() -> Self {
        Self::with_reader(Box::new(RealNetlinkReader::new()))
    }

    /// Bring the store current and serve from memory. This never blocks (the
    /// drain is non-blocking) and never polls (there is no timer — the kernel
    /// pushes and the read applies).
    fn refresh(&self) {
        let mut state = self.state.lock().unwrap();
        match self.reader.drain_events(MAX_DRAIN_EVENTS) {
            Ok(events) => {
                // The channel is alive. If the table is NM's synthesised
                // fallback (the kernel could not answer earlier), the kernel can
                // answer now: un-prime so the re-prime below replaces the
                // daemon's link view with the kernel's own.
                if state.fallback_installed {
                    state.primed = false;
                }
                let mut changed = 0usize;
                for event in &events {
                    if state.apply(event) {
                        changed += 1;
                    }
                }
                if changed > 0 {
                    self.revision.fetch_add(changed as u64, Ordering::Relaxed);
                }
            }
            Err(e) => {
                // The channel is untrustworthy (socket died / ENOBUFS overrun):
                // un-prime so the re-prime below rebuilds the table from a full
                // dump. The reason is kept and reported, never swallowed.
                state.primed = false;
                state.kernel_error = Some(e.to_string());
            }
        }
        if !state.primed {
            self.prime_from_kernel(&mut state);
        }
    }

    /// One full kernel sweep: the store's initial fill, and the recovery path
    /// after a channel fault or an event overrun.
    fn prime_from_kernel(&self, state: &mut StoreState) {
        match self.reader.snapshot() {
            Ok(snapshot) => {
                state.links = snapshot.links;
                state.links.sort_by(|a, b| a.name.cmp(&b.name));
                state.addresses = snapshot.addresses;
                state.routes = snapshot.routes;
                state.rfkill = snapshot.rfkill;
                state.primed = true;
                state.fallback_installed = false;
                state.kernel_error = None;
            }
            Err(e) => {
                state.primed = false;
                state.kernel_error = Some(e.to_string());
            }
        }
    }

    /// Install NM's device view as the link table — the counted fallback, used
    /// ONLY when the kernel link table cannot answer at all. Every record
    /// carries [`LinkProvenance::NmFallback`], so a consumer can see the daemon
    /// speaking instead of the kernel.
    fn install_nm_fallback(&self, devices: &[DeviceSnapshot]) {
        let mut state = self.state.lock().unwrap();
        if state.fallback_installed {
            return;
        }
        state.links = devices.iter().map(fallback_link).collect();
        state.links.sort_by(|a, b| a.name.cmp(&b.name));
        state.fallback_installed = true;
        state.primed = true;
    }

    /// Every link the store holds, sorted by ifname (the stable enumeration).
    pub fn links(&self) -> Vec<KernelLink> {
        self.state.lock().unwrap().links.clone()
    }

    pub fn link_by_id(&self, id: NetworkInterfaceId) -> Option<KernelLink> {
        self.state
            .lock()
            .unwrap()
            .links
            .iter()
            .find(|l| iface_id(&l.name) == id)
            .cloned()
    }

    /// The kernel's radio-block view (`/dev/rfkill` via the netlink snapshot).
    pub fn rfkill(&self) -> Vec<RfkillState> {
        self.state.lock().unwrap().rfkill.clone()
    }

    /// `IFLA_STATS64` for one ifname, as the kernel last reported it; `None`
    /// when the kernel reported no counters for it (never a zeroed counter).
    pub fn counters_by_name(&self, name: &str) -> Option<InterfaceCounters> {
        self.state.lock().unwrap().links.iter().find(|l| l.name == name).and_then(|l| l.stats)
    }

    /// The store revision: one per applied, state-changing kernel event.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// The topology revision the cached NM policy sweep is valid for.
    pub fn topology_revision(&self) -> u64 {
        self.state.lock().unwrap().topology_revision
    }

    pub fn primed(&self) -> bool {
        self.state.lock().unwrap().primed
    }

    pub fn fallback_installed(&self) -> bool {
        self.state.lock().unwrap().fallback_installed
    }

    /// Every route the kernel reported, as the last sweep saw it. Routes are NOT
    /// event-fed (the kernel's routing-table change storms are not status), so
    /// this is the dump's view and re-primes with the table.
    pub fn routes(&self) -> Vec<KernelRoute> {
        self.state.lock().unwrap().routes.clone()
    }

    /// Why the kernel seam could not answer, if it could not.
    pub fn kernel_error(&self) -> Option<String> {
        self.state.lock().unwrap().kernel_error.clone()
    }

    /// Where the link record for `name` came from (kernel netlink, or the
    /// counted NM fallback).
    pub fn provenance(&self, name: &str) -> Option<LinkProvenance> {
        self.state.lock().unwrap().links.iter().find(|l| l.name == name).map(|l| l.provenance)
    }
}

/// Synthesise a kernel-shaped link record from NM's device view. Used ONLY on
/// the fallback path (the kernel link table cannot answer): NM's `FirmwareMissing`
/// is expressed in the kernel's shape (administratively up, operstate DOWN, no
/// carrier) so the same four-state function decides, and every field that is not
/// a kernel fact stays at the record's neutral value. `provenance` says which
/// layer is speaking.
fn fallback_link(device: &DeviceSnapshot) -> KernelLink {
    let activated = device.state == NM_DEVICE_STATE_ACTIVATED;
    KernelLink {
        // NM has no ifindex to give; 0 is "unknown", never a fabricated index.
        index: 0,
        name: device.iface.clone(),
        kind: LinkKind::from_nm_device_type(device.device_type),
        // NM's `DeviceType` 14 (modem) is the wwan net device the kernel already
        // names with `IFLA_LINKINFO kind "wwan"`.
        info_kind: if device.device_type == 14 { "wwan".to_string() } else { String::new() },
        up: activated || device.managed,
        carrier: if device.firmware_missing { Some(false) } else { device.carrier },
        oper_state: if activated { OperState::Up } else { OperState::Down },
        mtu: device.mtu,
        hardware_address: device.hw_address.clone(),
        // No IFLA_STATS64 reached us: the /sys counters behind the seam answer.
        stats: None,
        provenance: LinkProvenance::NmFallback,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The Linux backend: kernel-first store, policy + control seam.
// ═══════════════════════════════════════════════════════════════════════

/// One live subscription: the interface it watches and the store revision it
/// last delivered (so a bounded pull can count what it collapsed).
#[derive(Clone, Copy, Debug)]
struct Subscription {
    iface: NetworkInterfaceId,
    last_revision: u64,
}

/// The Linux network [`NetworkBackend`]. Kernel-authoritative reads — links,
/// addresses, routes, counters, carrier, radio blocks — are served from the
/// event-fed [`NetworkStore`]; [`NmClient`] is the counted policy + control
/// fallback (charter law #9: kernel first, daemon for policy, contract for
/// consumers).
pub struct LinuxNetworkBackend<C: NmClient = RealNmClient> {
    client: C,
    store: NetworkStore,
    next_token: AtomicU32,
    live: Mutex<alloc::collections::BTreeMap<u64, Subscription>>,
    /// The NM policy sweep, cached against the store's topology revision:
    /// `None` until the first read that needs policy state.
    policy: Mutex<Option<(u64, NmPolicy)>>,
    /// Every NmClient call issued on a read's behalf — the counted fallback.
    fallback_calls: AtomicU64,
    /// Store revisions a bounded pull collapsed — counted, never silent.
    dropped_updates: AtomicU64,
}

impl<C: NmClient> LinuxNetworkBackend<C> {
    /// Build a backend over an explicit seam and the production kernel reader
    /// (what the founder's probe binary uses).
    pub fn with_client(client: C) -> Self {
        Self::with_client_and_netlink(client, Box::new(RealNetlinkReader::new()))
    }

    /// Build a backend over an explicit kernel seam (tests script the kernel
    /// here; the host is never touched — law #8).
    pub fn with_client_and_netlink(client: C, reader: Box<dyn NetlinkReader>) -> Self {
        LinuxNetworkBackend {
            client,
            store: NetworkStore::with_reader(reader),
            next_token: AtomicU32::new(1),
            live: Mutex::new(alloc::collections::BTreeMap::new()),
            policy: Mutex::new(None),
            fallback_calls: AtomicU64::new(0),
            dropped_updates: AtomicU64::new(0),
        }
    }

    // ── The transport-hierarchy accounting (law #9 made measurable) ─────────

    /// Every NmClient call this backend issued on a read's behalf. On a warm
    /// store this is zero; on a cold store it is exactly one per read op.
    pub fn nm_fallback_calls(&self) -> u64 {
        self.fallback_calls.load(Ordering::Relaxed)
    }

    /// Store revisions a bounded subscription pull collapsed (the intermediate
    /// states a caller did not see because it pulled less often than the kernel
    /// spoke).
    pub fn dropped_updates(&self) -> u64 {
        self.dropped_updates.load(Ordering::Relaxed)
    }

    /// The store's revision: one per applied, state-changing kernel event.
    pub fn store_revision(&self) -> u64 {
        self.store.revision()
    }

    /// Why the kernel seam could not answer, if it could not (the fallback then
    /// answers instead, visibly).
    pub fn kernel_error(&self) -> Option<String> {
        self.store.kernel_error()
    }

    /// The kernel-first store (read-only access to the link/address tables).
    pub fn store(&self) -> &NetworkStore {
        &self.store
    }

    /// Force a fresh NM policy sweep. The cached snapshot is invalidated by a
    /// topology change (a link or address arriving or going away); this is the
    /// explicit form for a caller that knows policy moved and the topology did
    /// not. One counted seam call.
    pub fn refresh_policy(&self) -> Result<()> {
        *self.policy.lock().unwrap() = None;
        self.ensure_policy().map(|_| ())
    }

    // ── Read-path internals ─────────────────────────────────────────────────

    /// Bring the kernel store current. When the kernel cannot answer at all
    /// (netlink unavailable / overrun), install NM's device view as the fallback
    /// link table — ONE counted seam call, once per backend.
    fn ensure_kernel_state(&self) -> Result<()> {
        self.store.refresh();
        if self.store.primed() || self.store.fallback_installed() {
            return Ok(());
        }
        let policy = self.ensure_policy()?;
        self.store.install_nm_fallback(&policy.real_devices());
        Ok(())
    }

    /// The NM policy snapshot, or the ONE counted fallback sweep that fetches
    /// it. Cached against the store's topology revision: a warm store makes
    /// zero seam calls, and a topology change re-sweeps exactly once.
    fn ensure_policy(&self) -> Result<NmPolicy> {
        let topology = self.store.topology_revision();
        if let Some((cached_at, policy)) = self.policy.lock().unwrap().as_ref() {
            if *cached_at == topology {
                return Ok(policy.clone());
            }
        }
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        let policy = self.client.policy()?;
        *self.policy.lock().unwrap() = Some((topology, policy.clone()));
        Ok(policy)
    }

    fn find_link(&self, id: NetworkInterfaceId) -> Result<KernelLink> {
        self.store
            .link_by_id(id)
            .ok_or_else(|| Error::not_found().with_context(format!("interface id {id:?}")))
    }

    /// Build the contract status for one link record. The kernel answers the
    /// link facts; NM policy is fetched (one counted call) to classify, and the
    /// state's SOURCE is carried alongside it.
    fn status_of(&self, link: &KernelLink) -> Result<ConnStatus> {
        let rfkill = self.store.rfkill();
        let policy = self.ensure_policy()?;
        let (host_state, host_state_source) = classify_host_state(link, &rfkill, &policy);
        Ok(ConnStatus {
            connected: link.is_running(),
            interface: iface_id(&link.name),
            kind: link.kind_token().to_string(),
            // Signal strength / speed / quality are nl80211 facts this milestone
            // does not read: 0 means "not measured", never a fabricated value.
            strength_dbm: 0,
            speed_bps: 0,
            quality: 0,
            metered: false,
            behind_captive: false,
            host_state,
            host_state_source,
            // The SSID lives in NM's connection profile / nl80211 — not read
            // here; empty, never invented.
            ssid: String::new(),
        })
    }

    fn radio(&self, name: &str, enabled: bool, hw_enabled: bool, rf: Option<&RfkillState>) -> RadioState {
        let (rf_hard, rf_soft) = rf.map(|r| (r.hard_blocked, r.soft_blocked)).unwrap_or((false, false));
        RadioState {
            name: name.to_string(),
            enabled,
            // A radio is hard-blocked if the kernel rfkill says so OR NM reports
            // the hardware switch off. Soft block is ONLY the rfkill soft bit —
            // never derived from `enabled`, which is policy.
            hard_blocked: rf_hard || !hw_enabled,
            soft_blocked: rf_soft,
        }
    }

    /// The radio-block view for `listRadioStates`: the kernel's rfkill as the
    /// netlink snapshot reported it, or — only when netlink could not answer at
    /// all — the char device behind the seam (counted like every seam call).
    fn rfkill_view(&self) -> Result<Vec<RfkillState>> {
        let from_store = self.store.rfkill();
        if !from_store.is_empty() || !self.store.fallback_installed() {
            return Ok(from_store);
        }
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.rfkill()
    }

    fn mint(&self, iface: NetworkInterfaceId) -> SubscriptionId {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed) as u64;
        self.live.lock().unwrap().insert(
            token,
            Subscription { iface, last_revision: self.store.revision() },
        );
        SubscriptionId(token)
    }

    fn subscription(&self, sub: SubscriptionId) -> Result<Subscription> {
        self.live
            .lock()
            .unwrap()
            .get(&sub.0)
            .copied()
            .ok_or_else(|| {
                Error::not_found().with_context(format!("subscription {} not live or already cancelled", sub.0))
            })
    }

    /// Deliver the store's current revision to one subscription, counting the
    /// revisions it collapsed on the way (never silent).
    fn advance(&self, sub: SubscriptionId, current: u64) {
        let mut live = self.live.lock().unwrap();
        let Some(entry) = live.get_mut(&sub.0) else { return };
        if current > entry.last_revision {
            let collapsed = current - entry.last_revision - 1;
            if collapsed > 0 {
                self.dropped_updates.fetch_add(collapsed, Ordering::Relaxed);
            }
        }
        entry.last_revision = current;
    }
}

impl LinuxNetworkBackend<RealNmClient> {
    /// Connect to the live NetworkManager on this host, with the kernel-first
    /// store over a live netlink socket. The WRITES reach the bus and are the
    /// founder's live-proof vehicle; nothing in build or test turns a host
    /// switch (law #8).
    pub fn connect() -> Result<Self> {
        let client = RealNmClient::new(None, None)?;
        Ok(LinuxNetworkBackend::with_client(client))
    }
}

impl<C: NmClient> NetworkBackend for LinuxNetworkBackend<C> {
    fn get_interfaces(&self, page: Pagination) -> Result<Page<InterfaceSnapshot>> {
        self.ensure_kernel_state()?;
        // `managed` is the one policy bit a link record carries: the counted
        // sweep fills it (zero calls once the policy snapshot is current).
        let policy = self.ensure_policy()?;
        let links = self.store.links();
        let total = links.len() as u64;
        let items: Vec<InterfaceSnapshot> = links
            .iter()
            .map(|link| InterfaceSnapshot {
                id: iface_id(&link.name),
                name: link.name.clone(),
                kind: link.kind_token().to_string(),
                hardware_address: crate::domains::network::types::MacAddress(link.hardware_address.clone()),
                up: link.up,
                connected: link.is_running(),
                mtu: link.mtu,
                // NM's `Managed`; a link NM never enumerates reports false — the
                // honest gap, never a guess dressed as a kernel fact.
                managed: policy.managed(&link.name).unwrap_or(false),
            })
            .collect();
        Ok(paginate(items, total, page))
    }

    fn get_status(&self, interface: NetworkInterfaceId) -> Result<ConnStatus> {
        self.ensure_kernel_state()?;
        let link = self.find_link(interface)?;
        self.status_of(&link)
    }

    fn subscribe_status(&self, interface: NetworkInterfaceId) -> Result<SubscriptionId> {
        // Validating the interface keeps the token honest (no phantom pulls).
        self.ensure_kernel_state()?;
        self.find_link(interface)?;
        Ok(self.mint(interface))
    }

    fn pull_status(&self, subscription: SubscriptionId) -> Result<ConnStatus> {
        let sub = self.subscription(subscription)?;
        self.ensure_kernel_state()?;
        let link = self.find_link(sub.iface)?;
        // The store holds the latest state per link: every revision since the
        // last delivered one is collapsed into this pull. Counted, never silent.
        self.advance(subscription, self.store.revision());
        self.status_of(&link)
    }

    fn cancel_subscription(&self, subscription: SubscriptionId) -> Result<()> {
        self.live
            .lock()
            .unwrap()
            .remove(&subscription.0)
            .map(|_| ())
            .ok_or_else(|| Error::not_found().with_context("subscription not live or already cancelled"))
    }

    fn get_counters(&self, interface: NetworkInterfaceId, _window: Duration) -> Result<InterfaceCounters> {
        self.ensure_kernel_state()?;
        let link = self.find_link(interface)?;
        if let Some(counters) = self.store.counters_by_name(&link.name) {
            return Ok(counters);
        }
        // The kernel link table reported no IFLA_STATS64 for this link: the /sys
        // counters behind the seam are the fallback (a kernel read, not a daemon
        // hop — counted all the same).
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.counters(&link.name)
    }

    fn subscribe_counters(&self, interface: NetworkInterfaceId) -> Result<SubscriptionId> {
        self.ensure_kernel_state()?;
        self.find_link(interface)?;
        Ok(self.mint(interface))
    }

    fn pull_counters(&self, subscription: SubscriptionId) -> Result<InterfaceCounters> {
        let sub = self.subscription(subscription)?;
        self.ensure_kernel_state()?;
        let link = self.find_link(sub.iface)?;
        self.advance(subscription, self.store.revision());
        if let Some(counters) = self.store.counters_by_name(&link.name) {
            return Ok(counters);
        }
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.counters(&link.name)
    }

    fn is_networking_enabled(&self) -> Result<bool> {
        // The master switch is NM policy: the kernel does not own it. One
        // counted sweep (cached against the topology revision), zero once warm.
        Ok(self.ensure_policy()?.networking_enabled)
    }

    fn set_networking_enabled(&self, enabled: bool) -> Result<()> {
        self.client.set_networking_enabled(enabled)
    }

    fn set_radio_enabled(&self, radio: &str, enabled: bool) -> Result<()> {
        match radio {
            "wifi" => self.client.set_wireless_enabled(enabled),
            // The one home: the "cellular" radio token and setMobileData both
            // route to NM WwanEnabled — no rival path.
            "cellular" => self.client.set_wwan_enabled(enabled),
            "" => Err(Error::invalid_argument("radio token must not be empty")),
            "bluetooth" => Err(Error::not_supported(
                "not-applicable: the 'bluetooth' radio is owned by the bluetooth domain",
            )),
            other => Err(Error::not_supported(format!(
                "no radio mapping for token '{other}' on this backend"
            ))),
        }
    }

    fn list_radio_states(&self, page: Pagination) -> Result<Page<RadioState>> {
        // The radio-block view is a kernel fact: bring the store current first
        // (and, if netlink cannot answer, install the counted fallback table).
        self.ensure_kernel_state()?;
        let policy = self.ensure_policy()?;
        let rf = self.rfkill_view()?;
        let rf_wifi = rf.iter().find(|r| r.name == "wifi");
        let rf_cell = rf.iter().find(|r| r.name == "cellular");
        let radios = [
            self.radio("wifi", policy.wireless_enabled, policy.wireless_hardware_enabled, rf_wifi),
            self.radio("cellular", policy.wwan_enabled, policy.wwan_hardware_enabled, rf_cell),
        ];
        let total = radios.len() as u64;
        Ok(paginate(radios.to_vec(), total, page))
    }

    fn is_mobile_data_enabled(&self) -> Result<bool> {
        Ok(self.ensure_policy()?.wwan_enabled)
    }

    fn set_mobile_data_enabled(&self, enabled: bool) -> Result<()> {
        self.client.set_wwan_enabled(enabled)
    }

    fn is_data_roaming_enabled(&self) -> Result<bool> {
        self.ensure_kernel_state()?;
        // A cellular modem is a KERNEL fact now: the wwan subsystem registers
        // the wwan net device (`IFLA_LINKINFO` kind "wwan"), so the store
        // answers with no daemon hop.
        let has_wwan = self.store.links().iter().any(|l| l.is_cellular());
        if has_wwan {
            Err(Error::unavailable(
                "data roaming is a ModemManager setting; ModemManager absent on this host",
            ))
        } else {
            Ok(false)
        }
    }

    fn set_data_roaming_enabled(&self, _enabled: bool) -> Result<()> {
        Err(Error::not_supported(
            "not-applicable: no cellular modem on this host (data roaming is a ModemManager setting; ModemManager absent)",
        ))
    }
}

/// Paginate a sorted, fully-materialised list per the contract rule: `limit ==
/// 0` means "one", and `offset + limit` never exceeds `total`.
fn paginate<T>(items: Vec<T>, total: u64, page: Pagination) -> Page<T> {
    let limit = page.effective_limit() as usize;
    let offset = (page.offset as usize).min(items.len());
    let end = offset.saturating_add(limit).min(items.len());
    let window: Vec<T> = items.into_iter().skip(offset).take(end - offset).collect();
    Page { items: window, total }
}

// ═══════════════════════════════════════════════════════════════════════
// The production seam: a NetworkManager D-Bus client over the system bus.
// ═══════════════════════════════════════════════════════════════════════

/// Connect to NetworkManager over the system bus and read/write its surface.
/// Every method here maps to exactly one NM property or call (see the mapping
/// table in `knowledge/reports/09-network.md`); there are no shellouts.
pub struct RealNmClient {
    stream: Mutex<std::cell::RefCell<UnixStream>>,
    serial: AtomicU32,
    sysfs_root: PathBuf,
    rfkill_path: PathBuf,
}

impl RealNmClient {
    /// Connect and authenticate against `/run/dbus/system_bus_socket`. `sysfs`
    /// and `rfkill` can be overridden for tests.
    pub fn new(sysfs: Option<PathBuf>, rfkill: Option<PathBuf>) -> Result<Self> {
        use std::io::Write;
        let path =
            std::env::var("SMART_LIB_SYSTEM_BUS").unwrap_or_else(|_| "/run/dbus/system_bus_socket".to_string());
        let mut stream =
            UnixStream::connect(&path).map_err(|e| Error::unavailable(format!("dbus system bus at {path}: {e}")))?;
        // A bounded read keeps a vanished daemon from hanging the caller; the
        // timeout is classified as retryable `temporary`, not a bare failure.
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .ok();
        // Law #8: this is a read/write session, but every method we invoke is
        // either a property read or a seam method the founder runs deliberately.
        authenticate(&mut stream)?;
        // Establish the unique connection name (`Hello`) before any other
        // method call — a reference daemon closes a connection that calls a
        // routed service before Hello (this is the one step a real client does).
        let hello = build_call(
            1,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
            "",
            &[],
        );
        stream
            .write_all(&hello)
            .map_err(|e| Error::failed(format!("dbus hello write: {e}")))?;
        // The reply carries the unique name (e.g. ":1.206"); consume it so it
        // does not desynchronise the next method reply.
        read_message(&mut stream)?;
        Ok(RealNmClient {
            stream: Mutex::new(std::cell::RefCell::new(stream)),
            serial: AtomicU32::new(2),
            sysfs_root: sysfs.unwrap_or_else(|| PathBuf::from("/sys/class/net")),
            rfkill_path: rfkill.unwrap_or_else(|| PathBuf::from("/dev/rfkill")),
        })
    }

    /// Read one NM manager property as the target type.
    fn prop_bool(&self, prop: &str) -> Result<bool> {
        self.get_variant(NM_PATH, NM_IFACE, prop).map(|v| match v {
            Val::Boolean(b) => b,
            _ => false,
        })
    }

    fn set_manager_bool(&self, prop: &str, value: bool) -> Result<()> {
        self.call(
            NM_PATH,
            DBUS_PROPS_IFACE,
            "Set",
            &[b's', b's', b'v'],
            &[
                Val::Text(NM_IFACE.to_string()),
                Val::Text(prop.to_string()),
                Val::Variant(b"b".to_vec(), Box::new(Val::Boolean(value))),
            ],
        )?;
        Ok(())
    }

    /// `Control.Enable(bool)` — the host-global networking master switch.
    fn enable(&self, enabled: bool) -> Result<()> {
        self.call(NM_PATH, NM_IFACE, "Enable", &[b'b'], &[Val::Boolean(enabled)])?;
        Ok(())
    }

    /// `org.freedesktop.DBus.Properties.Get` → the variant value.
    fn get_variant(&self, path: &str, iface: &str, prop: &str) -> Result<Val> {
        let out = self.call(
            path,
            DBUS_PROPS_IFACE,
            "Get",
            &[b's', b's'],
            &[Val::Text(iface.to_string()), Val::Text(prop.to_string())],
        )?;
        out.into_iter()
            .next()
            .and_then(|v| match v {
                Val::Variant(_sig, inner) => Some(*inner),
                other => Some(other),
            })
            .ok_or_else(|| Error::failed(format!("Properties.Get({path}.{prop}) returned nothing")))
    }

    fn devices(&self) -> Result<Vec<DeviceSnapshot>> {
        let out = self.call(NM_PATH, NM_IFACE, "GetDevices", &[], &[])?;
        let paths: Vec<String> = out
            .into_iter()
            .filter_map(|v| match v {
                Val::Array(items) => Some(
                    items
                        .into_iter()
                        .filter_map(|p| match p {
                            Val::ObjectPath(p) => Some(p),
                            Val::Text(p) => Some(p),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                ),
                Val::ObjectPath(p) => Some(alloc::vec![p]),
                _ => None,
            })
            .flatten()
            .collect();
        paths.iter().map(|p| self.read_device(p)).collect()
    }

    fn read_device(&self, path: &str) -> Result<DeviceSnapshot> {
        let text = |prop: &str| -> String {
            match self.get_variant(path, NM_DEVICE_IFACE, prop) {
                Ok(Val::Text(s)) => s,
                _ => String::new(),
            }
        };
        let num = |prop: &str| -> u32 {
            match self.get_variant(path, NM_DEVICE_IFACE, prop) {
                Ok(Val::UInt32(n)) => n,
                Ok(Val::UInt64(n)) => n as u32,
                Ok(Val::Int32(n)) => n as u32,
                _ => 0,
            }
        };
        let flag = |prop: &str| -> bool {
            matches!(self.get_variant(path, NM_DEVICE_IFACE, prop), Ok(Val::Boolean(true)))
        };
        // State+reason arrive as a `(uu)` struct.
        let state = match self.get_variant(path, NM_DEVICE_IFACE, "State") {
            Ok(Val::UInt32(s)) => s,
            Ok(Val::UInt64(s)) => s as u32,
            Ok(Val::Struct(members)) if members.len() >= 2 => match members[0] {
                Val::UInt32(s) => s,
                Val::UInt64(s) => s as u32,
                _ => 0,
            },
            _ => 0,
        };
        // Carrier: only a wired device has one.
        let carrier = match self.get_variant(path, NM_WIRED_IFACE, "Carrier") {
            Ok(Val::Boolean(c)) => Some(c),
            _ => None,
        };
        Ok(DeviceSnapshot {
            path: path.to_string(),
            iface: text("Interface"),
            device_type: num("DeviceType"),
            state,
            firmware_missing: flag("FirmwareMissing"),
            managed: flag("Managed"),
            carrier,
            real: flag("Real") && !text("Interface").is_empty(),
            hw_address: text("HwAddress"),
            mtu: num("Mtu"),
            driver: text("Driver"),
        })
    }

    /// Send a method call and return its reply values.
    fn call(&self, path: &str, iface: &str, member: &str, sig: &[u8], args: &[Val]) -> Result<Vec<Val>> {
        let body_sig: String = sig.iter().map(|&b| b as char).collect();
        let guard = self.stream.lock().unwrap();
        let mut stream = guard.borrow_mut();
        dbus_call(&mut *stream, &self.serial, NM_DEST, path, iface, member, &body_sig, args)
    }

    fn read_counters(&self, iface: &str) -> Result<InterfaceCounters> {
        Ok(read_counters_from_sysfs(&self.sysfs_root, iface))
    }

    /// Read the kernel rfkill device (a stream of 8-byte events, no header) and
    /// parse it through the netlink module's one rfkill vocabulary.
    fn read_rfkill(&self) -> Result<Vec<RfkillState>> {
        use std::io::Read;
        let mut buf = [0u8; 8 * 128];
        let n = match std::fs::File::open(&self.rfkill_path) {
            Ok(mut f) => f.read(&mut buf).unwrap_or(0),
            Err(_) => 0,
        };
        Ok(netlink::parse_rfkill_events(&buf[..n]))
    }
}

impl NmClient for RealNmClient {
    /// The ONE counted fallback sweep: the manager policy bits plus NM's device
    /// view. Several D-Bus round trips on the wire, but exactly one seam call —
    /// the unit the fallback is counted in — and it happens only when the kernel
    /// could not answer or the topology changed since the last sweep.
    fn policy(&self) -> Result<NmPolicy> {
        Ok(NmPolicy {
            networking_enabled: self.prop_bool("NetworkingEnabled")?,
            wireless_enabled: self.prop_bool("WirelessEnabled")?,
            wireless_hardware_enabled: self.prop_bool("WirelessHardwareEnabled")?,
            wwan_enabled: self.prop_bool("WwanEnabled")?,
            wwan_hardware_enabled: self.prop_bool("WwanHardwareEnabled")?,
            devices: self.devices()?,
        })
    }

    fn counters(&self, iface: &str) -> Result<InterfaceCounters> {
        self.read_counters(iface)
    }

    fn rfkill(&self) -> Result<Vec<RfkillState>> {
        self.read_rfkill()
    }

    fn set_networking_enabled(&self, enabled: bool) -> Result<()> {
        self.enable(enabled)
    }

    fn set_wireless_enabled(&self, enabled: bool) -> Result<()> {
        self.set_manager_bool("WirelessEnabled", enabled)
    }

    fn set_wwan_enabled(&self, enabled: bool) -> Result<()> {
        self.set_manager_bool("WwanEnabled", enabled)
    }
}

/// Read one interface's counters from a sysfs root
/// (`<root>/<iface>/statistics/*`). A missing or unparsable field reads 0 —
/// "not reported", never fabricated.
fn read_counters_from_sysfs(root: &Path, iface: &str) -> InterfaceCounters {
    let read_u64 = |field: &str| -> u64 {
        std::fs::read_to_string(root.join(iface).join("statistics").join(field))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    InterfaceCounters {
        bytes_in: read_u64("rx_bytes"),
        bytes_out: read_u64("tx_bytes"),
        packets_in: read_u64("rx_packets"),
        packets_out: read_u64("tx_packets"),
        errors_in: read_u64("rx_errors"),
        errors_out: read_u64("tx_errors"),
        drops_in: read_u64("rx_dropped"),
        drops_out: read_u64("tx_dropped"),
        sampled_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    }
}

/// Whether a NetworkManager system-bus session can be opened and authenticated
/// (used by the read-only live proof to decide `REAL` honestly at runtime).
pub fn system_bus_available() -> bool {
    RealNmClient::new(None, None).is_ok()
}



// ═══════════════════════════════════════════════════════════════════════
// Hermetic, mock-based behavior tests (law #8: none of these touch the host).
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::network::availability::{claimed_l3_tests, AvailStatus, NETWORK_AVAILABILITY};
    use crate::platforms::linux::netlink::{KernelSnapshot, MockNetlink};
    use alloc::collections::BTreeSet;
    use std::cell::RefCell;
    use std::sync::Arc;

    // ── The scripted kernel (netlink vocabulary) ────────────────────────────

    /// A healthy ethernet link as the kernel reports it: administratively up,
    /// carrier present, operstate UP, counters attached.
    fn ethernet_link(name: &str, index: u32, carrier: bool) -> KernelLink {
        KernelLink {
            index,
            name: name.to_string(),
            kind: LinkKind::Ethernet,
            info_kind: String::new(),
            up: true,
            carrier: Some(carrier),
            oper_state: OperState::Up,
            mtu: 1500,
            hardware_address: "08:8F:C3:E1:21:71".to_string(),
            stats: Some(InterfaceCounters {
                bytes_in: 11_223_344,
                bytes_out: 55_667_788,
                packets_in: 90_001,
                packets_out: 90_002,
                errors_in: 1,
                errors_out: 2,
                drops_in: 3,
                drops_out: 4,
                sampled_at: 1,
            }),
            provenance: LinkProvenance::Kernel,
        }
    }

    /// A healthy wifi link: administratively up, operstate UP, carrier on — the
    /// shape this host's wlp0s20f3 reports when associated.
    fn wifi_link(name: &str, index: u32) -> KernelLink {
        KernelLink {
            index,
            name: name.to_string(),
            kind: LinkKind::Wifi,
            info_kind: String::new(),
            up: true,
            carrier: Some(true),
            oper_state: OperState::Up,
            mtu: 1500,
            hardware_address: "98:59:7A:E4:A0:F2".to_string(),
            stats: None,
            provenance: LinkProvenance::Kernel,
        }
    }

    /// A wifi link the host asked up whose hardware never came up — the kernel's
    /// no-firmware shape (`stalled()`: admin up, operstate DOWN, no carrier).
    fn wifi_stalled_link(name: &str, index: u32) -> KernelLink {
        KernelLink {
            carrier: Some(false),
            oper_state: OperState::Down,
            ..wifi_link(name, index)
        }
    }

    /// The same link with a fresh counter sample (a state change: the kernel
    /// really did report new numbers, and the store holds the latest).
    fn resampled(link: &KernelLink, bytes_in: u64) -> KernelLink {
        let mut moved = link.clone();
        if let Some(stats) = moved.stats.as_mut() {
            stats.bytes_in = bytes_in;
            stats.sampled_at += 1;
        }
        moved
    }

    /// A wifi link NM administratively took down when the radio went off.
    fn wifi_radio_off_link(name: &str, index: u32) -> KernelLink {
        KernelLink {
            up: false,
            carrier: None,
            oper_state: OperState::Down,
            ..wifi_link(name, index)
        }
    }

    /// The wwan net device the kernel registers for a cellular modem.
    fn wwan_link(name: &str, index: u32) -> KernelLink {
        KernelLink {
            index,
            name: name.to_string(),
            kind: LinkKind::Other,
            info_kind: "wwan".to_string(),
            up: true,
            carrier: Some(true),
            oper_state: OperState::Up,
            mtu: 1500,
            hardware_address: String::new(),
            stats: None,
            provenance: LinkProvenance::Kernel,
        }
    }

    /// Loopback, the link every kernel link table has.
    fn loopback_link() -> KernelLink {
        KernelLink {
            index: 1,
            name: "lo".to_string(),
            kind: LinkKind::Loopback,
            info_kind: String::new(),
            up: true,
            carrier: None,
            oper_state: OperState::Up,
            mtu: 65_536,
            hardware_address: "00:00:00:00:00:00".to_string(),
            stats: None,
            provenance: LinkProvenance::Kernel,
        }
    }

    fn snapshot(links: Vec<KernelLink>) -> KernelSnapshot {
        KernelSnapshot {
            links,
            addresses: Vec::new(),
            routes: Vec::new(),
            rfkill: Vec::new(),
        }
    }

    fn kernel(links: Vec<KernelLink>) -> MockNetlink {
        MockNetlink::with_snapshot(snapshot(links))
    }

    /// A scripted kernel whose rfkill view carries the given radio blocks (the
    /// kernel's own radio-block source — never NM policy).
    fn kernel_with_rfkill(links: Vec<KernelLink>, rfkill: Vec<RfkillState>) -> MockNetlink {
        let mut snap = snapshot(links);
        snap.rfkill = rfkill;
        MockNetlink::with_snapshot(snap)
    }

    // ── The scripted NetworkManager (policy + control seam) ─────────────────

    /// A scripted NM behind the shrunk seam. `calls` records every seam call a
    /// read issued (so the counting mock can assert zero / exactly one), and
    /// `writes` records every host-mutating verb (so a mapping test can assert
    /// the right NM call would be made). `fault` injects a `core::Error` on any
    /// seam method (error injection over all 20 codes).
    #[derive(Default)]
    struct Mock {
        policy: NmPolicy,
        counters: InterfaceCounters,
        rfkill: Vec<RfkillState>,
        writes: RefCell<Vec<String>>,
        calls: RefCell<Vec<String>>,
        fault: RefCell<Option<Error>>,
    }

    impl Mock {
        fn check(&self) -> Result<()> {
            if let Some(e) = self.fault.borrow().as_ref() {
                Err(e.clone())
            } else {
                Ok(())
            }
        }

        fn record(&self, call: String) {
            self.calls.borrow_mut().push(call);
        }

        fn written(&self, needle: &str) -> bool {
            self.writes.borrow().iter().any(|w| w == needle)
        }
    }

    /// An NM policy with the manager bits set and no device view (`managed` is
    /// then "NM does not enumerate it", which is not "unmanaged").
    fn policy(networking_enabled: bool, wireless_enabled: bool) -> NmPolicy {
        NmPolicy {
            networking_enabled,
            wireless_enabled,
            wireless_hardware_enabled: true,
            wwan_enabled: false,
            wwan_hardware_enabled: true,
            devices: Vec::new(),
        }
    }

    impl NmClient for Mock {
        fn policy(&self) -> Result<NmPolicy> {
            self.check()?;
            self.record("policy".to_string());
            Ok(self.policy.clone())
        }

        fn counters(&self, iface: &str) -> Result<InterfaceCounters> {
            self.check()?;
            self.record(format!("counters({iface})"));
            Ok(self.counters)
        }

        fn rfkill(&self) -> Result<Vec<RfkillState>> {
            self.check()?;
            self.record("rfkill".to_string());
            Ok(self.rfkill.clone())
        }

        fn set_networking_enabled(&self, enabled: bool) -> Result<()> {
            self.check()?;
            self.writes.borrow_mut().push(format!("Enable({enabled})"));
            Ok(())
        }

        fn set_wireless_enabled(&self, enabled: bool) -> Result<()> {
            self.check()?;
            self.writes.borrow_mut().push(format!("WirelessEnabled({enabled})"));
            Ok(())
        }

        fn set_wwan_enabled(&self, enabled: bool) -> Result<()> {
            self.check()?;
            self.writes.borrow_mut().push(format!("WwanEnabled({enabled})"));
            Ok(())
        }
    }

    /// A backend over a scripted kernel and a scripted NM (never the host).
    fn backend(kernel: MockNetlink, nm: Mock) -> LinuxNetworkBackend<Mock> {
        LinuxNetworkBackend::with_client_and_netlink(nm, Box::new(kernel))
    }

    /// The common fixture: one healthy ethernet, one healthy wifi, loopback, and
    /// an NM policy that has both radios on.
    fn happy_backend() -> LinuxNetworkBackend<Mock> {
        backend(
            kernel(alloc::vec![
                loopback_link(),
                ethernet_link("enp0s31f6", 2, true),
                wifi_link("wlp0s20f3", 3),
            ]),
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
        )
    }

    fn id_of(iface: &str) -> NetworkInterfaceId {
        iface_id(iface)
    }

    // ── The four-state diagnostic with explicit sources (the incident) ───────

    /// The incident's four failure modes are four DISTINCT statuses, and each
    /// names the layer that produced it: `carrier-down` / `no-firmware` come
    /// from netlink (kernel facts), `networking-disabled` / `radio-off` come
    /// from NetworkManager policy (the counted fallback).
    #[test]
    fn get_status_four_states_distinct_with_explicit_sources() {
        // (1) networking-disabled: the master switch outranks everything, even
        //     a link the kernel reports perfectly healthy.
        let nd = backend(
            kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true)]),
            Mock {
                policy: policy(false, true),
                ..Default::default()
            },
        );
        // (2) radio-off: NM policy off the wifi radio; the kernel shows the link
        //     administratively down, which is the policy's shadow, not a cause.
        let ro = backend(
            kernel(alloc::vec![wifi_radio_off_link("wlp0s20f3", 3)]),
            Mock {
                policy: policy(true, false),
                ..Default::default()
            },
        );
        // (3) no-firmware: asked up, operstate DOWN, no carrier, no radio block.
        let nf = backend(
            kernel(alloc::vec![wifi_stalled_link("wlp0s20f3", 3)]),
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
        );
        // (4) carrier-down: an unplugged cable, the host asking the link up.
        let cd = backend(
            kernel(alloc::vec![ethernet_link("enp0s31f6", 2, false)]),
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
        );

        let states: Vec<(HostState, HostStateSource)> = [
            (nd.get_status(id_of("enp0s31f6")).unwrap(),),
            (ro.get_status(id_of("wlp0s20f3")).unwrap(),),
            (nf.get_status(id_of("wlp0s20f3")).unwrap(),),
            (cd.get_status(id_of("enp0s31f6")).unwrap(),),
        ]
        .into_iter()
        .map(|(s,)| (s.host_state, s.host_state_source))
        .collect();

        assert_eq!(
            states,
            [
                (HostState::NetworkingDisabled, HostStateSource::NmPolicy),
                (HostState::RadioOff, HostStateSource::NmPolicy),
                (HostState::NoFirmware, HostStateSource::Kernel),
                (HostState::CarrierDown, HostStateSource::Kernel),
            ],
            "the incident's four failure modes must be four distinct statuses, each with its source"
        );
        // A fifth value proves they are not accidentally equal, and the two
        // sources are both really produced (kernel facts AND policy facts).
        let distinct: BTreeSet<HostState> = states.iter().map(|(s, _)| *s).collect();
        assert_eq!(distinct.len(), 4);
        assert_eq!(
            states.iter().filter(|(_, src)| *src == HostStateSource::Kernel).count(),
            2,
            "carrier-down and no-firmware are kernel facts (netlink)"
        );
        assert_eq!(
            states.iter().filter(|(_, src)| *src == HostStateSource::NmPolicy).count(),
            2,
            "networking-disabled and radio-off are NM policy (the counted fallback)"
        );
    }

    /// A rfkill-blocked radio is `radio-off` with the KERNEL as its source: the
    /// rfkill bit is a kernel fact, never conflated with NM's policy `enabled`.
    #[test]
    fn rfkill_blocked_radio_is_radio_off_from_the_kernel() {
        let b = backend(
            kernel_with_rfkill(
                alloc::vec![wifi_link("wlp0s20f3", 3)],
                alloc::vec![RfkillState {
                    name: "wifi".to_string(),
                    hard_blocked: false,
                    soft_blocked: true,
                }],
            ),
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
        );
        let status = b.get_status(id_of("wlp0s20f3")).unwrap();
        assert_eq!(status.host_state, HostState::RadioOff);
        assert_eq!(status.host_state_source, HostStateSource::Kernel);
        // A blocked radio is not "missing firmware": the kernel held it.
        let stalled = backend(
            kernel_with_rfkill(
                alloc::vec![wifi_stalled_link("wlp0s20f3", 3)],
                alloc::vec![RfkillState {
                    name: "wifi".to_string(),
                    hard_blocked: true,
                    soft_blocked: false,
                }],
            ),
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
        );
        let blocked = stalled.get_status(id_of("wlp0s20f3")).unwrap();
        assert_eq!(blocked.host_state, HostState::RadioOff, "a hard-blocked radio is radio-off, not no-firmware");
    }

    /// A healthy activated link reads `managed` — the fifth value the diagnostic
    /// must be able to tell apart from all four failures.
    #[test]
    fn connected_managed_device_reads_managed() {
        let b = happy_backend();
        let status = b.get_status(id_of("wlp0s20f3")).unwrap();
        assert_eq!(status.host_state, HostState::Managed);
        assert!(status.connected, "operstate UP is the kernel's connection fact");
        assert_eq!(status.kind, "wifi");
        // Not measured, never fabricated: nl80211 signal facts are not read.
        assert_eq!(status.strength_dbm, 0);
        assert!(status.ssid.is_empty());
    }

    /// A link NM does not drive reads `unmanaged` (policy), not `carrier-down`.
    #[test]
    fn unmanaged_device_is_its_own_state() {
        let mut p = policy(true, true);
        p.devices = alloc::vec![DeviceSnapshot {
            iface: "enp0s31f6".to_string(),
            device_type: 1,
            managed: false,
            real: true,
            ..Default::default()
        }];
        let b = backend(kernel(alloc::vec![ethernet_link("enp0s31f6", 2, false)]), Mock {
            policy: p,
            ..Default::default()
        });
        let status = b.get_status(id_of("enp0s31f6")).unwrap();
        assert_eq!(status.host_state, HostState::Unmanaged);
        assert_eq!(status.host_state_source, HostStateSource::NmPolicy);
    }

    /// A link that is not running with no named cause is an honest `unknown`,
    /// never a fabricated `managed`.
    #[test]
    fn a_silent_link_with_no_named_cause_is_unknown() {
        let mut link = ethernet_link("enp0s31f6", 2, true);
        link.up = false;
        link.oper_state = OperState::Down;
        let b = backend(kernel(alloc::vec![link]), Mock {
            policy: policy(true, true),
            ..Default::default()
        });
        let status = b.get_status(id_of("enp0s31f6")).unwrap();
        assert_eq!(status.host_state, HostState::Unknown);
        assert!(!status.connected);
    }

    // ── The transport-hierarchy proof (law #9: kernel first) ─────────────────

    /// The acceptance assertion: a WARM store issues ZERO NmClient calls on the
    /// read path — every kernel-authoritative read is served from the store.
    /// (Run with `--nocapture` for the printed per-op table.)
    #[test]
    fn warm_store_reads_issue_zero_nm_calls() {
        let b = happy_backend();
        // Warm the store: the first read primes the policy snapshot.
        assert_eq!(b.nm_fallback_calls(), 0, "nothing has been read yet");
        let _ = b.get_status(id_of("wlp0s20f3")).unwrap();
        let after_first = b.nm_fallback_calls();
        assert_eq!(after_first, 1, "the cold read issues exactly one counted fallback");

        // Every subsequent read op on the warm store issues zero NmClient calls.
        let ops = [
            ("getStatus", b.get_status(id_of("wlp0s20f3")).is_ok()),
            ("getInterfaces", b.get_interfaces(Pagination::default()).is_ok()),
            (
                "pullStatus",
                b.pull_status(b.subscribe_status(id_of("wlp0s20f3")).unwrap()).is_ok(),
            ),
            (
                "pullCounters",
                b.pull_counters(b.subscribe_counters(id_of("enp0s31f6")).unwrap()).is_ok(),
            ),
            ("isNetworkingEnabled", b.is_networking_enabled().is_ok()),
            ("listRadioStates", b.list_radio_states(Pagination::default()).is_ok()),
            ("isMobileDataEnabled", b.is_mobile_data_enabled().is_ok()),
        ];
        for (name, ok) in ops {
            assert!(ok, "{name}");
        }
        assert_eq!(
            b.nm_fallback_calls(),
            after_first,
            "a warm store makes zero further NmClient calls: the kernel answers"
        );

        // The kernel facts themselves are the store's, provably: the link table
        // came from the netlink mock, not from NM's device view.
        assert!(b.store().provenance("wlp0s20f3") == Some(LinkProvenance::Kernel));
        assert!(b.kernel_error().is_none());
    }

    /// The seam-count table the report cites: per read op, NmClient calls on a
    /// COLD store (the first read) versus a WARM one (the next). Printed so the
    /// founder can read the before/after without a debugger.
    #[test]
    fn cold_store_reads_issue_exactly_one_counted_fallback() {
        // (read op, cold count, warm count)
        let mut rows: Vec<(&str, u64, u64)> = Vec::new();

        // getStatus: needs the master switch + radio policy ⇒ one cold call.
        let b = happy_backend();
        let _ = b.get_status(id_of("enp0s31f6")).unwrap();
        let cold = b.nm_fallback_calls();
        let _ = b.get_status(id_of("enp0s31f6")).unwrap();
        rows.push(("getStatus", cold, b.nm_fallback_calls() - cold));

        // getInterfaces: needs NM's `managed` bit ⇒ one cold call.
        let b = happy_backend();
        let _ = b.get_interfaces(Pagination::default()).unwrap();
        let cold = b.nm_fallback_calls();
        let _ = b.get_interfaces(Pagination::default()).unwrap();
        rows.push(("getInterfaces", cold, b.nm_fallback_calls() - cold));

        // getCounters: the kernel's own IFLA_STATS64 ⇒ zero, cold or warm.
        let b = happy_backend();
        let _ = b.get_counters(id_of("enp0s31f6"), Duration::from_nanos(1_000_000_000)).unwrap();
        let cold = b.nm_fallback_calls();
        let _ = b.get_counters(id_of("enp0s31f6"), Duration::from_nanos(1_000_000_000)).unwrap();
        rows.push(("getCounters", cold, b.nm_fallback_calls() - cold));

        // isNetworkingEnabled: NM policy ⇒ one cold call.
        let b = happy_backend();
        let _ = b.is_networking_enabled().unwrap();
        let cold = b.nm_fallback_calls();
        let _ = b.is_networking_enabled().unwrap();
        rows.push(("isNetworkingEnabled", cold, b.nm_fallback_calls() - cold));

        // listRadioStates: policy + kernel rfkill ⇒ one cold call.
        let b = happy_backend();
        let _ = b.list_radio_states(Pagination::default()).unwrap();
        let cold = b.nm_fallback_calls();
        let _ = b.list_radio_states(Pagination::default()).unwrap();
        rows.push(("listRadioStates", cold, b.nm_fallback_calls() - cold));

        // A kernel-fact read that never needs policy at all: the counters of a
        // link carrying IFLA_STATS64 are served with zero seam calls, cold or warm.
        std::println!("read op                  cold  warm");
        for (name, cold, warm) in &rows {
            std::println!("{name:<24} {cold:>4}  {warm:>4}");
        }
        assert_eq!(rows[0], ("getStatus", 1, 0), "cold getStatus issues exactly one counted fallback");
        assert_eq!(rows[1], ("getInterfaces", 1, 0), "cold getInterfaces issues exactly one counted fallback");
        assert_eq!(rows[2], ("getCounters", 0, 0), "kernel counters never reach the seam");
        assert_eq!(rows[3], ("isNetworkingEnabled", 1, 0));
        assert_eq!(rows[4], ("listRadioStates", 1, 0));
    }

    /// Netlink events feed the store: one read drains them, the revision rises,
    /// and the served status is the LATEST fact, not a stale one.
    #[test]
    fn kernel_events_feed_the_store() {
        let k = Arc::new(kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true)]));
        let b = LinuxNetworkBackend::with_client_and_netlink(
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
            Box::new(k.clone()),
        );
        assert!(b.get_status(id_of("enp0s31f6")).unwrap().connected);
        let revision = b.store_revision();

        // No event arrived: nothing changed, and no daemon hop was made either.
        b.get_status(id_of("enp0s31f6")).unwrap();
        assert_eq!(b.store_revision(), revision);
        assert_eq!(b.nm_fallback_calls(), 1, "still only the first cold sweep");

        // The kernel says the carrier went away: one event, one applied change.
        k.queue_event(KernelEvent::Link(ethernet_link("enp0s31f6", 2, false)));
        let status = b.get_status(id_of("enp0s31f6")).unwrap();
        assert_eq!(status.host_state, HostState::CarrierDown, "the event reached the store");
        assert_eq!(b.store_revision(), revision + 1, "one state-changing event, one revision");
        // The link went away entirely: the store forgets it, the read is notFound.
        k.queue_event(KernelEvent::LinkGone(2));
        assert_eq!(b.get_status(id_of("enp0s31f6")).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(b.get_interfaces(Pagination::default()).unwrap().total, 0);
    }

    // ── Reads -----------------------------------------------------------------

    /// `getInterfaces` enumerates the kernel link table (every netdev the kernel
    /// knows), with the kernel's own kind, MTU and hardware address.
    #[test]
    fn get_interfaces_enumerates_the_kernel_link_table() {
        let b = happy_backend();
        let page = b.get_interfaces(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 3);
        let names: Vec<&str> = page.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["enp0s31f6", "lo", "wlp0s20f3"], "sorted by ifname");
        let lo = page.items.iter().find(|i| i.name == "lo").unwrap();
        assert_eq!(lo.kind, "loopback");
        assert_eq!(lo.mtu, 65_536);
        assert!(lo.up && lo.connected);
        // `managed` is NM policy: a device NM does not enumerate reports false,
        // never a guess dressed as a kernel fact.
        assert!(!lo.managed);
    }

    /// A `wwan` link reports the "cellular" token derived from the kernel's
    /// `IFLA_LINKINFO`, so `isDataRoamingEnabled` sees the modem without a
    /// daemon hop.
    #[test]
    fn data_roaming_reads_the_kernel_wwan_link() {
        let with_modem = backend(kernel(alloc::vec![wwan_link("wwan0", 4)]), Mock::default());
        let err = with_modem.is_data_roaming_enabled().unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("ModemManager"));
        let without = happy_backend();
        assert!(!without.is_data_roaming_enabled().unwrap(), "no modem ⇒ measured-absent false");
    }

    #[test]
    fn get_counters_serves_the_kernel_stats() {
        let b = happy_backend();
        let counters = b.get_counters(id_of("enp0s31f6"), Duration::from_nanos(1_000_000_000)).unwrap();
        assert_eq!(counters.bytes_in, 11_223_344);
        assert_eq!(counters.packets_out, 90_002);
        assert_eq!(counters.drops_out, 4);
        assert_eq!(counters.sampled_at, 1);
        assert_eq!(b.nm_fallback_calls(), 0, "the kernel's own counters, no seam call");
    }

    /// A link whose IFLA_STATS64 the kernel did not report falls back to the
    /// seam's /sys counters — counted, never silent.
    #[test]
    fn counters_fall_back_to_the_seam_when_the_kernel_reports_none() {
        let b = backend(kernel(alloc::vec![wifi_link("wlp0s20f3", 3)]), Mock {
            policy: policy(true, true),
            counters: InterfaceCounters {
                bytes_in: 777,
                ..Default::default()
            },
            ..Default::default()
        });
        let counters = b.get_counters(id_of("wlp0s20f3"), Duration::from_nanos(1_000_000_000)).unwrap();
        assert_eq!(counters.bytes_in, 777, "the /sys fallback answered");
        assert_eq!(b.nm_fallback_calls(), 1, "exactly one counted counters fallback (counters need no policy)");
    }

    /// When netlink cannot answer at all, the link table is synthesised from
    /// NM's device view — the counted fallback, marked `NmFallback` so a
    /// consumer can see the daemon speaking.
    #[test]
    fn netlink_unavailable_falls_back_to_the_nm_device_view() {
        let k = kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true)]);
        k.fail_with(Error::unavailable("netlink dump socket: operation not permitted"));
        let mut p = policy(true, true);
        p.devices = alloc::vec![DeviceSnapshot {
            iface: "enp0s31f6".to_string(),
            device_type: 1,
            state: NM_DEVICE_STATE_ACTIVATED,
            firmware_missing: false,
            managed: true,
            carrier: Some(true),
            real: true,
            hw_address: "08:8F:C3:E1:21:71".to_string(),
            mtu: 1500,
            ..Default::default()
        }];
        let b = backend(k, Mock {
            policy: p,
            ..Default::default()
        });
        let page = b.get_interfaces(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 1, "NM's device view became the link table");
        assert_eq!(page.items[0].name, "enp0s31f6");
        assert_eq!(page.items[0].kind, "ethernet");
        assert_eq!(b.store().provenance("enp0s31f6"), Some(LinkProvenance::NmFallback));
        assert_eq!(b.nm_fallback_calls(), 1, "exactly one counted fallback, cold");
        // The reason the kernel could not answer is reported, not swallowed.
        assert!(b.kernel_error().is_some(), "the netlink failure is visible");
        // Warm reads on the fallback table still make zero further calls.
        let _ = b.get_status(id_of("enp0s31f6")).unwrap();
        assert_eq!(b.nm_fallback_calls(), 1);
    }

    /// When netlink comes back, the kernel's link table replaces the NM-synthesised
    /// fallback one (and never mixes with it): the fallback is a measured gap, not
    /// a permanent second source.
    #[test]
    fn a_returning_kernel_replaces_the_fallback_link_table() {
        let k = Arc::new(kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true)]));
        let b = LinuxNetworkBackend::with_client_and_netlink(
            Mock {
                policy: NmPolicy {
                    networking_enabled: true,
                    wireless_enabled: true,
                    devices: alloc::vec![DeviceSnapshot {
                        iface: "nm-only".to_string(),
                        device_type: 1,
                        state: NM_DEVICE_STATE_ACTIVATED,
                        managed: true,
                        carrier: Some(true),
                        real: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            Box::new(k.clone()),
        );
        // Netlink is dead: NM's device view becomes the link table.
        k.fail_with(Error::unavailable("netlink event socket unavailable"));
        let page = b.get_interfaces(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].name, "nm-only");
        assert_eq!(b.store().provenance("nm-only"), Some(LinkProvenance::NmFallback));
        // The kernel answers again: the fallback table is replaced wholesale.
        k.clear_failure();
        let page = b.get_interfaces(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].name, "enp0s31f6", "the kernel's table wins the moment it can answer");
        assert_eq!(b.store().provenance("enp0s31f6"), Some(LinkProvenance::Kernel));
        assert!(!b.store().fallback_installed());
    }

    /// `hardBlocked` / `softBlocked` come from the kernel's rfkill and are NEVER
    /// conflated with the policy `enabled` bit (the schema's explicit warning).
    #[test]
    fn radio_hard_and_soft_are_not_conflated_with_policy() {
        // Policy ON but soft-blocked by the kernel's rfkill: enabled stays true,
        // soft_blocked true, hard_blocked false.
        let b = backend(
            kernel_with_rfkill(
                alloc::vec![],
                alloc::vec![RfkillState {
                    name: "wifi".to_string(),
                    hard_blocked: false,
                    soft_blocked: true,
                }],
            ),
            Mock {
            policy: NmPolicy {
                wireless_enabled: true,
                wireless_hardware_enabled: true,
                ..policy(true, true)
            },
            ..Default::default()
        });
        let radios = b.list_radio_states(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        let r = radios.items.iter().find(|r| r.name == "wifi").unwrap();
        assert!(r.enabled, "policy enabled is reported verbatim");
        assert!(!r.hard_blocked);
        assert!(r.soft_blocked, "the rfkill soft bit is reported, not folded into enabled");
        // Hardware switch off (NM says so): hard blocked, independent of policy.
        let b2 = backend(kernel(alloc::vec![]), Mock {
            policy: NmPolicy {
                wireless_enabled: false,
                wireless_hardware_enabled: false,
                ..policy(true, false)
            },
            ..Default::default()
        });
        let r2 = b2
            .list_radio_states(Pagination { offset: 0, limit: u32::MAX })
            .unwrap()
            .items
            .into_iter()
            .find(|r| r.name == "wifi")
            .unwrap();
        assert!(r2.hard_blocked);
    }

    #[test]
    fn pagination_limit_zero_means_one() {
        let b = backend(
            kernel(alloc::vec![
                ethernet_link("eth1", 10, true),
                ethernet_link("eth2", 11, true),
                wifi_link("wlan0", 12),
            ]),
            Mock::default(),
        );
        let one = b.get_interfaces(Pagination { offset: 0, limit: 0 }).unwrap();
        assert_eq!(one.items.len(), 1, "limit 0 means one, not all");
        assert_eq!(one.total, 3, "total is the stable full count");
        let two = b.get_interfaces(Pagination { offset: 2, limit: u32::MAX }).unwrap();
        assert_eq!(two.items.len(), 1, "offset 2 of 3 returns the last");
    }

    // ── Subscriptions: bounded batches, counted drops ───────────────────────

    #[test]
    fn status_subscription_bounded_pull() {
        let b = happy_backend();
        let sub = b.subscribe_status(id_of("wlp0s20f3")).unwrap();
        // The caller drives the bound: pull exactly three, no unbounded push.
        let mut seen = 0;
        for _ in 0..3 {
            let status = b.pull_status(sub).unwrap();
            assert_eq!(status.host_state, HostState::Managed);
            seen += 1;
        }
        assert_eq!(seen, 3, "bounded pull ran exactly the caller's bound");
        // Nothing was dropped: the kernel said nothing new between the pulls.
        assert_eq!(b.dropped_updates(), 0);
        // Cancelling retires the token; a later pull is notFound, not a silent success.
        b.cancel_subscription(sub).unwrap();
        let after = b.pull_status(sub).unwrap_err();
        assert_eq!(after.code, ErrorCode::NotFound);
    }

    /// A pull that arrives after the kernel spoke more than once serves the
    /// LATEST state and COUNTS the revisions it collapsed — never silent.
    #[test]
    fn subscription_pulls_count_the_updates_they_collapse() {
        let k = Arc::new(kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true)]));
        let b = LinuxNetworkBackend::with_client_and_netlink(
            Mock {
                policy: policy(true, true),
                ..Default::default()
            },
            Box::new(k.clone()),
        );
        let sub = b.subscribe_status(id_of("enp0s31f6")).unwrap();
        // Three kernel events land while the caller is not pulling.
        let base = ethernet_link("enp0s31f6", 2, true);
        k.queue_event(KernelEvent::Link(resampled(&base, 11_223_345)));
        k.queue_event(KernelEvent::Link(resampled(&base, 11_223_346)));
        k.queue_event(KernelEvent::Link(ethernet_link("enp0s31f6", 2, false)));
        // One bounded pull: the latest fact, with the two collapsed revisions
        // counted rather than silently dropped.
        let status = b.pull_status(sub).unwrap();
        assert_eq!(status.host_state, HostState::CarrierDown, "the pull serves the LATEST fact");
        assert_eq!(b.dropped_updates(), 2, "the two intermediate revisions are counted, not silent");
        // A pull with nothing new drops nothing.
        b.pull_status(sub).unwrap();
        assert_eq!(b.dropped_updates(), 2);
    }

    #[test]
    fn counter_subscription_bounded_pull() {
        let b = happy_backend();
        let sub = b.subscribe_counters(id_of("enp0s31f6")).unwrap();
        let mut last = 0u64;
        for _ in 0..5 {
            last = b.pull_counters(sub).unwrap().bytes_in;
        }
        assert_eq!(last, 11_223_344);
        b.cancel_subscription(sub).unwrap();
    }

    #[test]
    fn cancel_retires_token() {
        let b = happy_backend();
        let sub = b.subscribe_status(id_of("wlp0s20f3")).unwrap();
        b.cancel_subscription(sub).unwrap();
        assert_eq!(b.cancel_subscription(sub).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(b.pull_status(sub).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn get_status_unknown_interface_is_not_found() {
        let b = happy_backend();
        assert_eq!(
            b.get_status(id_of("nosuchdev")).unwrap_err().code,
            ErrorCode::NotFound,
            "an interface the kernel does not know is notFound, never a fabricated status"
        );
    }

    // ── Host-global networking + radio reads (NM policy, counted) ───────────

    #[test]
    fn is_networking_enabled_reads_the_policy_sweep_once() {
        let on = backend(kernel(alloc::vec![]), Mock {
            policy: policy(true, true),
            ..Default::default()
        });
        assert!(on.is_networking_enabled().unwrap());
        let off = backend(kernel(alloc::vec![]), Mock {
            policy: policy(false, true),
            ..Default::default()
        });
        assert!(!off.is_networking_enabled().unwrap());
        // Policy is snapshotted, not re-read per call: one counted sweep.
        assert_eq!(off.nm_fallback_calls(), 1);
        assert!(!off.is_networking_enabled().unwrap());
        assert_eq!(off.nm_fallback_calls(), 1, "the warm snapshot made zero further calls");
        // The explicit form re-sweeps (one more counted call).
        off.refresh_policy().unwrap();
        assert_eq!(off.nm_fallback_calls(), 2);
    }

    #[test]
    fn mobile_data_enabled_reads_the_policy_sweep() {
        let b = backend(kernel(alloc::vec![]), Mock {
            policy: NmPolicy {
                wwan_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(b.is_mobile_data_enabled().unwrap());
    }

    // ── Control verbs (mock only; law #8) ───────────────────────────────────

    #[test]
    fn set_networking_calls_enable() {
        let b = backend(kernel(alloc::vec![]), Mock::default());
        b.set_networking_enabled(false).unwrap();
        b.set_networking_enabled(true).unwrap();
        assert!(b.client.written("Enable(false)"), "setNetworking(false) must reach NM Enable(false)");
        assert!(b.client.written("Enable(true)"), "setNetworking(true) must reach NM Enable(true)");
        // Control verbs are not reads: no fallback sweep was needed.
        assert_eq!(b.nm_fallback_calls(), 0);
    }

    #[test]
    fn set_radio_wifi_calls_wireless_enabled() {
        let b = backend(kernel(alloc::vec![]), Mock::default());
        b.set_radio_enabled("wifi", false).unwrap();
        b.set_radio_enabled("wifi", true).unwrap();
        assert!(b.client.written("WirelessEnabled(false)"));
        assert!(b.client.written("WirelessEnabled(true)"));
    }

    #[test]
    fn set_radio_cellular_routes_to_mobile_data() {
        // "cellular" and setMobileData share one home: NM WwanEnabled.
        let b = backend(kernel(alloc::vec![]), Mock::default());
        b.set_radio_enabled("cellular", true).unwrap();
        b.set_mobile_data_enabled(false).unwrap();
        assert!(b.client.written("WwanEnabled(true)"), "setRadioEnabled(cellular) must reach WwanEnabled(true)");
        assert!(b.client.written("WwanEnabled(false)"), "setMobileData(false) must reach WwanEnabled(false)");
    }

    #[test]
    fn set_radio_rejects_unknown_token_and_owns_bluetooth() {
        let b = backend(kernel(alloc::vec![]), Mock::default());
        let empty = b.set_radio_enabled("", true).unwrap_err();
        assert_eq!(empty.code, ErrorCode::InvalidArgument);
        let bt = b.set_radio_enabled("bluetooth", true).unwrap_err();
        assert_eq!(bt.code, ErrorCode::NotSupported);
        let future = b.set_radio_enabled("a-2046-radio", true).unwrap_err();
        assert_eq!(future.code, ErrorCode::NotSupported);
        // None of the refused writes touched the host.
        assert!(b.client.writes.borrow().is_empty());
    }

    #[test]
    fn set_data_roaming_is_not_applicable() {
        let b = happy_backend();
        assert_eq!(
            b.set_data_roaming_enabled(true).unwrap_err().code,
            ErrorCode::NotSupported,
            "no cellular modem on this host: a vacuous success would be a lie"
        );
    }

    // ── Error injection over all 20 codes (mapping total through the seam) ───

    #[test]
    fn error_injection_is_total_over_all_20_codes() {
        for code in ErrorCode::ALL {
            let b = happy_backend();
            *b.client.fault.borrow_mut() = Some(Error::new(code));
            assert_eq!(b.get_status(id_of("wlp0s20f3")).unwrap_err().code, code, "get_status carried {code:?}");
            assert_eq!(
                b.get_interfaces(Pagination::default()).unwrap_err().code,
                code,
                "get_interfaces carried {code:?}"
            );
            assert_eq!(b.is_networking_enabled().unwrap_err().code, code, "is_networking_enabled carried {code:?}");
            assert_eq!(
                b.list_radio_states(Pagination::default()).unwrap_err().code,
                code,
                "list_radio_states carried {code:?}"
            );
            assert_eq!(
                b.set_networking_enabled(true).unwrap_err().code,
                code,
                "setNetworking carried {code:?}"
            );
            assert_eq!(
                b.set_radio_enabled("wifi", true).unwrap_err().code,
                code,
                "setRadioEnabled carried {code:?}"
            );
        }
    }

    // ── sysfs counters (the fallback reader, hermetic: a real temp dir) ──────

    #[test]
    fn get_counters_from_sysfs() {
        let dir = std::env::temp_dir().join(format!("smart-lib-net-{}", std::process::id()));
        let stats = dir.join("eth1").join("statistics");
        std::fs::create_dir_all(&stats).unwrap();
        let put = |field: &str, v: &str| std::fs::write(stats.join(field), v).unwrap();
        put("rx_bytes", "11223344\n");
        put("tx_bytes", "55667788\n");
        put("rx_packets", "90001\n");
        put("tx_packets", "90002\n");
        put("rx_errors", "1\n");
        put("tx_errors", "2\n");
        put("rx_dropped", "3\n");
        put("tx_dropped", "4\n");
        let c = read_counters_from_sysfs(&dir, "eth1");
        assert_eq!(c.bytes_in, 11_223_344);
        assert_eq!(c.bytes_out, 55_667_788);
        assert_eq!(c.packets_in, 90_001);
        assert_eq!(c.errors_in, 1);
        assert_eq!(c.drops_out, 4);
        assert!(c.sampled_at > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Availability ↔ tests linkage ────────────────────────────────────────

    /// Every REAL/DEGRADED row names an `l3_test`; each named suffix is one of
    /// the test functions above. A REAL claim with a vanished test is a FAIL
    /// (conformance §3.5).
    #[test]
    fn availability_real_rows_have_a_backing_test() {
        const IMPLEMENTED_TESTS: &[&str] = &[
            "get_interfaces_enumerates_the_kernel_link_table",
            "get_status_four_states_distinct_with_explicit_sources",
            "rfkill_blocked_radio_is_radio_off_from_the_kernel",
            "connected_managed_device_reads_managed",
            "status_subscription_bounded_pull",
            "subscription_pulls_count_the_updates_they_collapse",
            "cancel_retires_token",
            "get_counters_serves_the_kernel_stats",
            "counters_fall_back_to_the_seam_when_the_kernel_reports_none",
            "netlink_unavailable_falls_back_to_the_nm_device_view",
            "counter_subscription_bounded_pull",
            "is_networking_enabled_reads_the_policy_sweep_once",
            "data_roaming_reads_the_kernel_wwan_link",
            "cold_store_reads_issue_exactly_one_counted_fallback",
            "warm_store_reads_issue_zero_nm_calls",
            "set_networking_calls_enable",
            "set_radio_wifi_calls_wireless_enabled",
            "set_radio_cellular_routes_to_mobile_data",
            "mobile_data_enabled_reads_the_policy_sweep",
            "radio_hard_and_soft_are_not_conflated_with_policy",
        ];
        for (name, l3) in claimed_l3_tests() {
            let suffix = l3.rsplit("::").next().unwrap();
            assert!(
                IMPLEMENTED_TESTS.contains(&suffix),
                "method {name} claims {l3} but no such test exists"
            );
        }
        // The host-global verbs are actually present and (for the reads) == REAL.
        for verb in ["isNetworkingEnabled", "listRadioStates", "getStatus", "getInterfaces"] {
            let row = NETWORK_AVAILABILITY.iter().find(|r| r.name == verb).unwrap();
            assert_eq!(row.status, AvailStatus::Real, "{verb}");
        }
    }

    // ── D-Bus wire codec self-consistency (marshal → unmarshal) ──────────────

    #[test]
    fn dbus_marshal_unmarshal_round_trips() {
        // A Properties.Get reply body "v" wrapping a boolean stays a boolean.
        let mut buf = Vec::new();
        // variant of bool: siglen(1)+b"+nul, then aligned bool.
        buf.push(1);
        buf.push(b'b');
        buf.push(0);
        buf.resize(pad_to(buf.len(), 4), 0);
        buf.extend_from_slice(&1u32.to_le_bytes());
        let mut off = 0usize;
        let v = unmarshal(&buf, &mut off, b"v").unwrap();
        assert_eq!(v, Val::Variant(b"b".to_vec(), Box::new(Val::Boolean(true))));

        // A "ss" body round-trips.
        let mut body = Vec::new();
        marshal(&mut body, b"s", &Val::Text("org.freedesktop.NetworkManager".to_string())).unwrap();
        marshal(&mut body, b"s", &Val::Text("NetworkingEnabled".to_string())).unwrap();
        let mut o = 0usize;
        let a = unmarshal(&body, &mut o, b"s").unwrap();
        assert_eq!(a, Val::Text("org.freedesktop.NetworkManager".to_string()));
        let b = unmarshal(&body, &mut o, b"s").unwrap();
        assert_eq!(b, Val::Text("NetworkingEnabled".to_string()));

        // An array of object paths ("ao") round-trips element-for-element —
        // this is the exact shape of GetDevices' reply.
        let paths = [
            Val::ObjectPath("/Devices/1".to_string()),
            Val::ObjectPath("/Devices/7".to_string()),
        ];
        let mut body = Vec::new();
        let content = body.len();
        body.extend_from_slice(&0u32.to_le_bytes()); // byte-length placeholder
        let start = body.len();
        for p in &paths {
            marshal(&mut body, b"o", p).unwrap();
        }
        let blen = (body.len() - start) as u32;
        body[content..content + 4].copy_from_slice(&blen.to_le_bytes());
        let mut o2 = 0usize;
        match unmarshal(&body, &mut o2, b"ao").unwrap() {
            Val::Array(items) => {
                assert_eq!(items.len(), 2, "both device paths survive the wire");
                assert_eq!(items[0], Val::ObjectPath("/Devices/1".to_string()));
                assert_eq!(items[1], Val::ObjectPath("/Devices/7".to_string()));
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    /// A read-only live integration test over the real system bus and the real
    /// netlink socket. Law #8: this only READS (kernel sweeps, NM properties,
    /// rfkill) — it never turns a host switch. `#[ignore]`d so the default
    /// hermetic suite never touches the network, and run explicitly with
    /// `cargo test -- --ignored` when a bus is present. If NetworkManager is
    /// absent it fails loudly (never silently skipped).
    #[test]
    #[ignore = "read-only live probe against the real NetworkManager and netlink; run with `cargo test -- --ignored` when a system bus is present"]
    fn live_reads_networkmanager() {
        let backend = match LinuxNetworkBackend::connect() {
            Ok(b) => b,
            Err(e) => panic!(
                "cannot reach NetworkManager on the system bus: {e} \
                 (is org.freedesktop.NetworkManager running? reads only — nothing changed)"
            ),
        };
        // The read surface answers without error and without fabricating.
        assert!(backend.is_networking_enabled().is_ok(), "isNetworkingEnabled read");
        let page = backend
            .get_interfaces(Pagination { offset: 0, limit: u32::MAX })
            .expect("getInterfaces read");
        // The kernel's routing table is in the store too: the default route is
        // the kernel's own fact, read from the same dump.
        assert!(
            backend
                .store()
                .routes()
                .iter()
                .any(|r| r.destination.is_empty()),
            "the kernel's default route must be in the store"
        );
        // The kernel answers first: the link table came from netlink, so the
        // counted NM fallback table was never installed.
        assert!(
            backend.store().links().iter().any(|l| l.name == "lo"),
            "the kernel link table must include loopback"
        );
        assert!(
            !backend.store().fallback_installed(),
            "netlink answered, so the NM fallback table was never installed"
        );
        assert!(page.total >= 1, "expected at least the loopback interface");
        assert!(!page.items.is_empty());
        // Every enumerated interface yields a status whose hostState is one of
        // the contract's seven tokens (never an invented state), each with a
        // source naming the layer that decided it.
        for iface in &page.items {
            let s = backend.get_status(iface.id).expect("getStatus read");
            assert!(
                HostState::ALL.contains(&s.host_state),
                "hostState {:?} must be a contract token",
                s.host_state
            );
            assert!(
                s.host_state_source.as_token() == "kernel" || s.host_state_source.as_token() == "nm-policy",
                "source {:?} must be one of the two layers",
                s.host_state_source
            );
        }
        let radios = backend
            .list_radio_states(Pagination { offset: 0, limit: u32::MAX })
            .expect("listRadioStates read");
        assert!(radios.total >= 1, "wifi + cellular radios are always enumerated");
        // Hard and soft are reported separately (never folded into enabled).
        assert!(radios.items.iter().any(|r| r.name == "wifi"));
        std::println!(
            "live: netlink links = {:?}, NM seam calls = {}, store revision = {}",
            backend.store().links().iter().map(|l| format!("{}({})", l.name, l.kind_token())).collect::<Vec<_>>(),
            backend.nm_fallback_calls(),
            backend.store_revision()
        );
    }
}
