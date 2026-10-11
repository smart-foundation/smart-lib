//! The Linux **NetworkManager** backend for the network domain — the one host
//! target per `knowledge/02-HOST-CAPABILITIES.md` §2 ("NM D-Bus for privileged /
//! wifi-connect ops"). It speaks D-Bus over `/run/dbus/system_bus_socket`.
//!
//! ## Law #8 (the host is read-only)
//!
//! Every **read** path here is safe to run live on the founder's box:
//! `NetworkingEnabled`/`WirelessEnabled`/`WwanEnabled` properties, the device
//! enumeration and per-device `State`, `/sys/class/net/*/statistics/*` counters,
//! and `/dev/rfkill`. The **write** paths (`Enable`, `Properties.Set` on
//! `WirelessEnabled`/`WwanEnabled`) sit behind the [`NmClient`] seam and are only
//! ever exercised by (`a`) the mock in the tests and (`b`) the founder's
//! `examples/network_probe.rs` live bring-back. No build, test, or agent turns a
//! host switch; the read-only `#[ignore]`d test at the bottom can be run by
//! whoever has the bus.
//!
//! ## The seam
//!
//! [`NmClient`] is the whole NetworkManager surface the backend needs. The
//! production implementation is [`RealNmClient`] (a ~small hand-rolled D-Bus
//! client — no `nmcli`/`ip` shellouts, the condemned pattern P5). Tests swap in a
//! [`MockNmClient`] that scripts the same surface and injects faults, so the
//! four-state diagnostic and every mutation mapping are proven without touching
//! the host.
//!
//! ## The iwd/netlink variant
//!
//! A second backend (iwd D-Bus, or raw netlink for the read surface) is a
//! *designed seam*, not built now: [`NmClient`] is the seam it would implement,
//! and [`crate::domains::network::NetworkBackend`] would delegate identically.

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
    ConnStatus, HostState, InterfaceCounters, InterfaceSnapshot, Page, Pagination, RadioState,
    SubscriptionId,
};

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
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

/// The kernel rfkill view of one radio, kept apart from NM policy so the two
/// are never conflated (`RadioState.hardBlocked`/`softBlocked`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RfkillState {
    /// Open token: "wifi", "cellular", "bluetooth".
    pub name: String,
    pub hard_blocked: bool,
    pub soft_blocked: bool,
}

/// Everything the network backend needs from NetworkManager. The mutating
/// surface (host-global enable, radio enable, mobile data) lives here, behind
/// the seam — so no build or test ever turns a host switch (law #8).
pub trait NmClient {
    fn networking_enabled(&self) -> Result<bool>;
    fn wireless_enabled(&self) -> Result<bool>;
    fn wireless_hardware_enabled(&self) -> Result<bool>;
    fn wwan_enabled(&self) -> Result<bool>;
    fn wwan_hardware_enabled(&self) -> Result<bool>;

    fn set_networking_enabled(&self, enabled: bool) -> Result<()>;
    fn set_wireless_enabled(&self, enabled: bool) -> Result<()>;
    fn set_wwan_enabled(&self, enabled: bool) -> Result<()>;

    fn devices(&self) -> Result<Vec<DeviceSnapshot>>;
    fn rfkill(&self) -> Result<Vec<RfkillState>>;
    fn counters(&self, iface: &str) -> Result<InterfaceCounters>;
}

// ── NM state → open-token mapping (host map §1.2 N1) ──────────────────────

fn kind_token(device_type: u32) -> &'static str {
    match device_type {
        1 => "ethernet",
        2 => "wifi",
        14 => "cellular",
        22 => "team",
        32 => "loopback",
        _ => "unknown",
    }
}

const NM_DEVICE_STATE_UNAVAILABLE: u32 = 20;
const NM_DEVICE_STATE_ACTIVATED: u32 = 100;

fn is_wifi(d: &DeviceSnapshot) -> bool {
    d.device_type == 2
}

/// The four-state diagnostic, as a pure function of the host facts. Order is
/// load-bearing: host-global off outranks every per-device reason, so a switched
/// off host is `networking-disabled` even when a device also looks unmanaged.
///
/// This is the incident as a function: `networking-disabled` (the master
/// switch), `radio-off` (policy on the wifi radio), `no-firmware` (the device
/// cannot come up), `carrier-down` (nothing on the wire) are four DISTINCT
/// answers, never collapsed into a bare `connected: false`.
fn classify_host_state(networking_enabled: bool, wireless_enabled: bool, d: &DeviceSnapshot) -> HostState {
    if !networking_enabled {
        HostState::NetworkingDisabled
    } else if !d.managed {
        HostState::Unmanaged
    } else if d.firmware_missing {
        HostState::NoFirmware
    } else if is_wifi(d) && !wireless_enabled {
        HostState::RadioOff
    } else if d.carrier == Some(false) {
        HostState::CarrierDown
    } else if d.state == NM_DEVICE_STATE_UNAVAILABLE {
        // Unavailable, but none of the named causes above — report the gap.
        HostState::Unknown
    } else {
        HostState::Managed
    }
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
// The Linux backend over the seam.
// ═══════════════════════════════════════════════════════════════════════

/// The Linux NetworkManager [`NetworkBackend`], generic over any [`NmClient`].
/// Defaults to [`RealNmClient`] so `LinuxNetworkBackend::default()` talks to the
/// live system bus.
pub struct LinuxNetworkBackend<C: NmClient = RealNmClient> {
    client: C,
    next_token: AtomicU32,
    live: Mutex<alloc::collections::BTreeMap<u64, NetworkInterfaceId>>,
}

impl<C: NmClient> LinuxNetworkBackend<C> {
    /// Build a backend over an explicit seam (tests inject a mock here).
    pub fn with_client(client: C) -> Self {
        LinuxNetworkBackend {
            client,
            next_token: AtomicU32::new(1),
            live: Mutex::new(alloc::collections::BTreeMap::new()),
        }
    }

    /// The set of `Real`, truthful devices we enumerate (skip p2p/generic).
    fn real_devices(&self) -> Result<Vec<DeviceSnapshot>> {
        let mut devices: Vec<DeviceSnapshot> = self
            .client
            .devices()?
            .into_iter()
            .filter(|d| d.real && !d.iface.is_empty())
            .collect();
        devices.sort_by(|a, b| a.iface.cmp(&b.iface));
        Ok(devices)
    }

    fn find_device(&self, id: NetworkInterfaceId) -> Result<DeviceSnapshot> {
        self.real_devices()?
            .into_iter()
            .find(|d| iface_id(&d.iface) == id)
            .ok_or_else(|| Error::not_found().with_context(format!("interface id {id:?}")))
    }

    fn mint(&self, iface: NetworkInterfaceId) -> SubscriptionId {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed) as u64;
        self.live.lock().unwrap().insert(token, iface);
        SubscriptionId(token)
    }

    fn iface_of(&self, sub: SubscriptionId) -> Result<NetworkInterfaceId> {
        self.live
            .lock()
            .unwrap()
            .get(&sub.0)
            .copied()
            .ok_or_else(|| {
                Error::not_found().with_context(format!("subscription {} not live or already cancelled", sub.0))
            })
    }

    fn status_of(&self, d: &DeviceSnapshot) -> Result<ConnStatus> {
        let networking_enabled = self.client.networking_enabled()?;
        let wireless_enabled = if is_wifi(d) {
            self.client.wireless_enabled().unwrap_or(false)
        } else {
            false
        };
        let connected = d.state == NM_DEVICE_STATE_ACTIVATED;
        Ok(ConnStatus {
            connected,
            interface: iface_id(&d.iface),
            kind: kind_token(d.device_type).to_string(),
            // Signal/speed are not read from NM here; 0 means "not measured",
            // which is honest (never a fabricated 0 dBm).
            strength_dbm: 0,
            speed_bps: 0,
            quality: 0,
            metered: false,
            behind_captive: false,
            host_state: classify_host_state(networking_enabled, wireless_enabled, d),
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
}

impl LinuxNetworkBackend<RealNmClient> {
    /// Connect to the live NetworkManager on this host (reads AND writes reach
    /// the bus; the writes are the founder's live-proof vehicle, never run here
    /// during build/test).
    pub fn connect() -> Result<Self> {
        let client = RealNmClient::new(None, None)?;
        Ok(LinuxNetworkBackend::with_client(client))
    }
}

impl<C: NmClient> NetworkBackend for LinuxNetworkBackend<C> {
    fn get_interfaces(&self, page: Pagination) -> Result<Page<InterfaceSnapshot>> {
        let devices = self.real_devices()?;
        let total = devices.len() as u64;
        let items: Vec<InterfaceSnapshot> = devices
            .into_iter()
            .map(|d| InterfaceSnapshot {
                id: iface_id(&d.iface),
                name: d.iface.clone(),
                kind: kind_token(d.device_type).to_string(),
                hardware_address: crate::domains::network::types::MacAddress(d.hw_address),
                up: d.state == NM_DEVICE_STATE_ACTIVATED || d.managed,
                connected: d.state == NM_DEVICE_STATE_ACTIVATED,
                mtu: d.mtu,
                managed: d.managed,
            })
            .collect();
        Ok(paginate(items, total, page))
    }

    fn get_status(&self, interface: NetworkInterfaceId) -> Result<ConnStatus> {
        let d = self.find_device(interface)?;
        self.status_of(&d)
    }

    fn subscribe_status(&self, interface: NetworkInterfaceId) -> Result<SubscriptionId> {
        // Validating the interface keeps the token honest (no phantom pulls).
        self.find_device(interface)?;
        Ok(self.mint(interface))
    }

    fn pull_status(&self, subscription: SubscriptionId) -> Result<ConnStatus> {
        let iface = self.iface_of(subscription)?;
        let d = self.find_device(iface)?;
        self.status_of(&d)
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
        let d = self.find_device(interface)?;
        self.client.counters(&d.iface)
    }

    fn subscribe_counters(&self, interface: NetworkInterfaceId) -> Result<SubscriptionId> {
        self.find_device(interface)?;
        Ok(self.mint(interface))
    }

    fn pull_counters(&self, subscription: SubscriptionId) -> Result<InterfaceCounters> {
        let iface = self.iface_of(subscription)?;
        let d = self.find_device(iface)?;
        self.client.counters(&d.iface)
    }

    fn is_networking_enabled(&self) -> Result<bool> {
        self.client.networking_enabled()
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
        let wireless_enabled = self.client.wireless_enabled()?;
        let wireless_hw = self.client.wireless_hardware_enabled()?;
        let wwan_enabled = self.client.wwan_enabled()?;
        let wwan_hw = self.client.wwan_hardware_enabled()?;
        let rf = self.client.rfkill().unwrap_or_default();
        let rf_wifi = rf.iter().find(|r| r.name == "wifi");
        let rf_cell = rf.iter().find(|r| r.name == "cellular");
        let radios = [
            self.radio("wifi", wireless_enabled, wireless_hw, rf_wifi),
            self.radio("cellular", wwan_enabled, wwan_hw, rf_cell),
        ];
        let total = radios.len() as u64;
        Ok(paginate(radios.to_vec(), total, page))
    }

    fn is_mobile_data_enabled(&self) -> Result<bool> {
        self.client.wwan_enabled()
    }

    fn set_mobile_data_enabled(&self, enabled: bool) -> Result<()> {
        self.client.set_wwan_enabled(enabled)
    }

    fn is_data_roaming_enabled(&self) -> Result<bool> {
        // Absence of a cellular modem is a *measurement*: roaming is not enabled
        // because there is no data path to roam. A present modem would need
        // ModemManager, which is absent — reported, not fabricated.
        let has_wwan = self.real_devices()?.iter().any(|d| d.device_type == 14);
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

    /// Parse the kernel rfkill device (a stream of 8-byte events, no header).
    fn read_rfkill(&self) -> Result<Vec<RfkillState>> {
        use std::io::Read;
        let mut buf = [0u8; 8 * 128];
        let n = match std::fs::File::open(&self.rfkill_path) {
            Ok(mut f) => f.read(&mut buf).unwrap_or(0),
            Err(_) => 0,
        };
        Ok(parse_rfkill_events(&buf[..n]))
    }
}

impl NmClient for RealNmClient {
    fn networking_enabled(&self) -> Result<bool> {
        self.prop_bool("NetworkingEnabled")
    }
    fn wireless_enabled(&self) -> Result<bool> {
        self.prop_bool("WirelessEnabled")
    }
    fn wireless_hardware_enabled(&self) -> Result<bool> {
        self.prop_bool("WirelessHardwareEnabled")
    }
    fn wwan_enabled(&self) -> Result<bool> {
        self.prop_bool("WwanEnabled")
    }
    fn wwan_hardware_enabled(&self) -> Result<bool> {
        self.prop_bool("WwanHardwareEnabled")
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
    fn devices(&self) -> Result<Vec<DeviceSnapshot>> {
        self.devices()
    }
    fn rfkill(&self) -> Result<Vec<RfkillState>> {
        self.read_rfkill()
    }
    fn counters(&self, iface: &str) -> Result<InterfaceCounters> {
        self.read_counters(iface)
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

/// Parse a batch of kernel `struct rfkill_event` records (8 bytes each:
/// `{ u32 idx; u8 type; u8 op; u8 soft; u8 hard; }`) into per-radio states,
/// keeping `hard` and `soft` separate from one another. Types mapped: 1=wifi,
/// 2=bluetooth, 5=cellular; others dropped (no honest name).
pub fn parse_rfkill_events(buf: &[u8]) -> Vec<RfkillState> {
    #[derive(Default)]
    struct Acc {
        name: &'static str,
        hard: bool,
        soft: bool,
    }
    let name_for = |t: u8| -> &'static str {
        match t {
            1 => "wifi",
            2 => "bluetooth",
            5 => "cellular",
            _ => "unknown",
        }
    };
    let mut map: alloc::collections::BTreeMap<u8, Acc> = alloc::collections::BTreeMap::new();
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let kind = buf[off + 4];
        let soft = buf[off + 6] != 0;
        let hard = buf[off + 7] != 0;
        let e = map.entry(kind).or_default();
        e.name = name_for(kind);
        e.hard = hard;
        e.soft = soft;
        off += 8;
    }
    map.into_values()
        .filter(|a| a.name != "unknown")
        .map(|a| RfkillState {
            name: a.name.to_string(),
            hard_blocked: a.hard,
            soft_blocked: a.soft,
        })
        .collect()
}

/// Return the rfkill entries discovered on the host (public, for the live read
/// proof and for callers that want the raw radio-block view).
pub fn read_rfkill_readonly() -> Result<Vec<RfkillState>> {
    let client = RealNmClient::new(None, None)?;
    client.read_rfkill()
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
    use alloc::collections::BTreeSet;
    use std::cell::RefCell;
    /// A scripted NetworkManager behind the seam. Scalar fields carry the NM
    /// state; `writes` records every host-mutating call (so a mapping test can
    /// assert the right NM call would be made); `fault` injects a `core::Error`
    /// on any seam method (error injection over all 20 codes).
    #[derive(Default)]
    struct Mock {
        networking_enabled: bool,
        wireless_enabled: bool,
        wireless_hw: bool,
        wwan_enabled: bool,
        wwan_hw: bool,
        devices: Vec<DeviceSnapshot>,
        rfkill: Vec<RfkillState>,
        counters: InterfaceCounters,
        writes: RefCell<Vec<String>>,
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
            self.writes.borrow_mut().push(call);
        }
        fn written(&self, needle: &str) -> bool {
            self.writes.borrow().iter().any(|w| w == needle)
        }
    }

    impl NmClient for Mock {
        fn networking_enabled(&self) -> Result<bool> {
            self.check()?;
            Ok(self.networking_enabled)
        }
        fn wireless_enabled(&self) -> Result<bool> {
            self.check()?;
            Ok(self.wireless_enabled)
        }
        fn wireless_hardware_enabled(&self) -> Result<bool> {
            self.check()?;
            Ok(self.wireless_hw)
        }
        fn wwan_enabled(&self) -> Result<bool> {
            self.check()?;
            Ok(self.wwan_enabled)
        }
        fn wwan_hardware_enabled(&self) -> Result<bool> {
            self.check()?;
            Ok(self.wwan_hw)
        }
        fn set_networking_enabled(&self, enabled: bool) -> Result<()> {
            self.check()?;
            self.record(format!("Enable({enabled})"));
            Ok(())
        }
        fn set_wireless_enabled(&self, enabled: bool) -> Result<()> {
            self.check()?;
            self.record(format!("WirelessEnabled({enabled})"));
            Ok(())
        }
        fn set_wwan_enabled(&self, enabled: bool) -> Result<()> {
            self.check()?;
            self.record(format!("WwanEnabled({enabled})"));
            Ok(())
        }
        fn devices(&self) -> Result<Vec<DeviceSnapshot>> {
            self.check()?;
            Ok(self.devices.clone())
        }
        fn rfkill(&self) -> Result<Vec<RfkillState>> {
            self.check()?;
            Ok(self.rfkill.clone())
        }
        fn counters(&self, _iface: &str) -> Result<InterfaceCounters> {
            self.check()?;
            Ok(self.counters)
        }
    }

    fn wifi(iface: &str, state: u32) -> DeviceSnapshot {
        DeviceSnapshot {
            path: format!("/org/freedesktop/NetworkManager/Devices/{iface}"),
            iface: iface.to_string(),
            device_type: 2,
            state,
            firmware_missing: false,
            managed: true,
            carrier: None,
            real: true,
            hw_address: "AA:BB:CC:DD:EE:FF".to_string(),
            mtu: 1500,
            driver: "iwlwifi".to_string(),
        }
    }

    fn ethernet(iface: &str, carrier: bool, state: u32) -> DeviceSnapshot {
        DeviceSnapshot {
            device_type: 1,
            carrier: Some(carrier),
            ..wifi(iface, state)
        }
    }

    fn id_of(iface: &str) -> NetworkInterfaceId {
        iface_id(iface)
    }

    // ── The four-state diagnostic (the incident as a regression test) ────────

    #[test]
    fn get_status_four_states_distinct() {
        // (1) networking-disabled: the master switch outranks everything.
        let nd = Mock {
            networking_enabled: false,
            devices: alloc::vec![wifi("wlan0", 100)],
            wireless_enabled: true,
            ..Default::default()
        };
        // (2) radio-off: policy off the wifi radio, networking up.
        let ro = Mock {
            networking_enabled: true,
            wireless_enabled: false,
            devices: alloc::vec![DeviceSnapshot {
                state: 30,
                ..wifi("wlan0", 30)
            }],
            ..Default::default()
        };
        // (3) no-firmware: the device cannot come up, radio on.
        let nf = Mock {
            networking_enabled: true,
            wireless_enabled: true,
            devices: alloc::vec![DeviceSnapshot {
                firmware_missing: true,
                state: 20,
                ..wifi("wlan0", 20)
            }],
            ..Default::default()
        };
        // (4) carrier-down: an unplugged ethernet cable, everything else fine.
        let cd = Mock {
            networking_enabled: true,
            devices: alloc::vec![ethernet("enp0s31f6", false, 20)],
            ..Default::default()
        };

        let states = [
            LinuxNetworkBackend::with_client(nd).get_status(id_of("wlan0")).unwrap().host_state,
            LinuxNetworkBackend::with_client(ro).get_status(id_of("wlan0")).unwrap().host_state,
            LinuxNetworkBackend::with_client(nf).get_status(id_of("wlan0")).unwrap().host_state,
            LinuxNetworkBackend::with_client(cd).get_status(id_of("enp0s31f6")).unwrap().host_state,
        ];
        assert_eq!(
            states,
            [HostState::NetworkingDisabled, HostState::RadioOff, HostState::NoFirmware, HostState::CarrierDown],
            "the incident's four failure modes must be four distinct statuses"
        );
        // A fifth value proves they are not accidentally equal.
        let mut distinct: BTreeSet<HostState> = BTreeSet::new();
        for s in states {
            distinct.insert(s);
        }
        assert_eq!(distinct.len(), 4);
    }

    /// A healthy activated wifi reads `managed` — the fifth value the diagnostic
    /// must be able to tell apart from all four failures.
    #[test]
    fn connected_managed_device_reads_managed() {
        let m = Mock {
            networking_enabled: true,
            wireless_enabled: true,
            devices: alloc::vec![wifi("wlan0", 100)],
            ..Default::default()
        };
        let status = LinuxNetworkBackend::with_client(m).get_status(id_of("wlan0")).unwrap();
        assert_eq!(status.host_state, HostState::Managed);
        assert!(status.connected);
    }

    // ── Host-global networking + radio writes (mock only; law #8) ────────────

    #[test]
    fn set_networking_calls_enable() {
        let m = Mock::default();
        let backend = LinuxNetworkBackend::with_client(m);
        backend.set_networking_enabled(false).unwrap();
        backend.set_networking_enabled(true).unwrap();
        assert!(backend.client.written("Enable(false)"), "setNetworking(false) must reach NM Enable(false)");
        assert!(backend.client.written("Enable(true)"), "setNetworking(true) must reach NM Enable(true)");
    }

    #[test]
    fn set_radio_wifi_calls_wireless_enabled() {
        let m = Mock::default();
        let backend = LinuxNetworkBackend::with_client(m);
        backend.set_radio_enabled("wifi", false).unwrap();
        backend.set_radio_enabled("wifi", true).unwrap();
        assert!(backend.client.written("WirelessEnabled(false)"));
        assert!(backend.client.written("WirelessEnabled(true)"));
    }

    #[test]
    fn set_radio_cellular_routes_to_mobile_data() {
        // "cellular" and setMobileData share one home: NM WwanEnabled.
        let m = Mock::default();
        let backend = LinuxNetworkBackend::with_client(m);
        backend.set_radio_enabled("cellular", true).unwrap();
        backend.set_mobile_data_enabled(false).unwrap();
        assert!(backend.client.written("WwanEnabled(true)"), "setRadioEnabled(cellular) must reach WwanEnabled(true)");
        assert!(backend.client.written("WwanEnabled(false)"), "setMobileData(false) must reach WwanEnabled(false)");
    }

    #[test]
    fn set_radio_rejects_unknown_token_and_owns_bluetooth() {
        let m = Mock::default();
        let backend = LinuxNetworkBackend::with_client(m);
        let empty = backend.set_radio_enabled("", true).unwrap_err();
        assert_eq!(empty.code, ErrorCode::InvalidArgument);
        let bt = backend.set_radio_enabled("bluetooth", true).unwrap_err();
        assert_eq!(bt.code, ErrorCode::NotSupported);
        let future = backend.set_radio_enabled("a-2046-radio", true).unwrap_err();
        assert_eq!(future.code, ErrorCode::NotSupported);
        // None of the refused writes touched the host.
        assert!(backend.client.writes.borrow().is_empty());
    }

    // ── Reads -----------------------------------------------------------------

    #[test]
    fn is_networking_enabled_reads_property() {
        let m = Mock {
            networking_enabled: true,
            ..Default::default()
        };
        assert!(LinuxNetworkBackend::with_client(m).is_networking_enabled().unwrap());
        let off = Mock {
            networking_enabled: false,
            ..Default::default()
        };
        assert!(!LinuxNetworkBackend::with_client(off).is_networking_enabled().unwrap());
    }

    #[test]
    fn mobile_data_enabled_reads_wwan() {
        let m = Mock {
            wwan_enabled: true,
            ..Default::default()
        };
        assert!(LinuxNetworkBackend::with_client(m).is_mobile_data_enabled().unwrap());
    }

    #[test]
    fn data_roaming_enabled_reads_wwan() {
        // No cellular modem (ModemManager absent) ⇒ roaming is measurably off.
        let m = Mock {
            devices: alloc::vec![wifi("wlan0", 100)],
            ..Default::default()
        };
        assert!(!LinuxNetworkBackend::with_client(m).is_data_roaming_enabled().unwrap());
        // A present modem with no roaming access is an honest Unavailable, not false.
        let with_wwan = Mock {
            devices: alloc::vec![DeviceSnapshot {
                device_type: 14,
                ..wifi("wwan0", 100)
            }],
            ..Default::default()
        };
        let err = LinuxNetworkBackend::with_client(with_wwan).is_data_roaming_enabled().unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
    }

    #[test]
    fn get_interfaces_and_radio_states() {
        let m = Mock {
            networking_enabled: true,
            wireless_enabled: true,
            wireless_hw: true,
            wwan_enabled: false,
            wwan_hw: true,
            devices: alloc::vec![
                wifi("wlan0", 100),
                ethernet("enp0s31f6", false, 20),
                DeviceSnapshot {
                    iface: "p2p-dev-wlan0".to_string(),
                    real: false,
                    ..wifi("p2p-dev-wlan0", 30)
                },
            ],
            ..Default::default()
        };
        let backend = LinuxNetworkBackend::with_client(m);
        // getInterfaces filters pseudo-devices (p2p) and paginates.
        let page = backend.get_interfaces(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 2, "the p2p pseudo-device is not Real and is filtered");
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].name, "enp0s31f6"); // sorted by name
        // listRadioStates reports wifi and cellular with their NM policy.
        let radios = backend.list_radio_states(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(radios.total, 2);
        let wifi_radio = radios.items.iter().find(|r| r.name == "wifi").unwrap();
        assert!(wifi_radio.enabled);
        assert!(!wifi_radio.hard_blocked);
        assert!(!wifi_radio.soft_blocked);
    }

    /// `hardBlocked` / `softBlocked` come from rfkill and are NEVER conflated
    /// with the policy `enabled` bit (the schema's explicit warning).
    #[test]
    fn radio_hard_and_soft_are_not_conflated_with_policy() {
        // Policy ON (enabled) but soft-blocked by rfkill: enabled stays true,
        // soft_blocked true, hard_blocked false.
        let m = Mock {
            wireless_enabled: true,
            wireless_hw: true,
            rfkill: alloc::vec![RfkillState {
                name: "wifi".to_string(),
                hard_blocked: false,
                soft_blocked: true,
            }],
            ..Default::default()
        };
        let radios = LinuxNetworkBackend::with_client(m)
            .list_radio_states(Pagination { offset: 0, limit: u32::MAX })
            .unwrap();
        let r = radios.items.iter().find(|r| r.name == "wifi").unwrap();
        assert!(r.enabled, "policy enabled is reported verbatim");
        assert!(!r.hard_blocked);
        assert!(r.soft_blocked, "the rfkill soft bit is reported, not folded into enabled");
        // Hardware switch off: hard blocked, independent of policy.
        let m2 = Mock {
            wireless_enabled: false,
            wireless_hw: false,
            rfkill: alloc::vec![RfkillState {
                name: "wifi".to_string(),
                hard_blocked: true,
                soft_blocked: false,
            }],
            ..Default::default()
        };
        let radios2 = LinuxNetworkBackend::with_client(m2)
            .list_radio_states(Pagination { offset: 0, limit: u32::MAX })
            .unwrap();
        let r2 = radios2.items.iter().find(|r| r.name == "wifi").unwrap();
        assert!(r2.hard_blocked);
    }

    #[test]
    fn pagination_limit_zero_means_one() {
        let mut devices = alloc::vec![wifi("wlan0", 100), ethernet("eth1", true, 100), ethernet("eth2", true, 100)];
        devices[1].iface = "eth1".to_string();
        let m = Mock {
            devices,
            ..Default::default()
        };
        let one = LinuxNetworkBackend::with_client(m).get_interfaces(Pagination { offset: 0, limit: 0 }).unwrap();
        assert_eq!(one.items.len(), 1, "limit 0 means one, not all");
        assert_eq!(one.total, 3, "total is the stable full count");
        let two = LinuxNetworkBackend::with_client(Mock { devices: alloc::vec![wifi("wlan0",100), ethernet("eth1", true, 100), ethernet("eth2",true,100)], ..Default::default() })
            .get_interfaces(Pagination { offset: 2, limit: u32::MAX })
            .unwrap();
        assert_eq!(two.items.len(), 1, "offset 2 of 3 returns the last");
    }

    #[test]
    fn status_subscription_bounded_pull() {
        let m = Mock {
            networking_enabled: true,
            wireless_enabled: true,
            devices: alloc::vec![wifi("wlan0", 100)],
            ..Default::default()
        };
        let backend = LinuxNetworkBackend::with_client(m);
        let sub = backend.subscribe_status(id_of("wlan0")).unwrap();
        // The caller drives the bound: pull exactly three, no unbounded push.
        let mut seen = 0;
        for _ in 0..3 {
            let status = backend.pull_status(sub).unwrap();
            assert_eq!(status.host_state, HostState::Managed);
            seen += 1;
        }
        assert_eq!(seen, 3, "bounded pull ran exactly the caller's bound");
        // Cancelling retires the token; a later pull is notFound, not a silent success.
        backend.cancel_subscription(sub).unwrap();
        let after = backend.pull_status(sub).unwrap_err();
        assert_eq!(after.code, ErrorCode::NotFound);
    }

    #[test]
    fn counter_subscription_bounded_pull() {
        let m = Mock {
            devices: alloc::vec![ethernet("eth1", true, 100)],
            counters: InterfaceCounters {
                bytes_in: 1_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let backend = LinuxNetworkBackend::with_client(m);
        let sub = backend.subscribe_counters(id_of("eth1")).unwrap();
        let mut last = 0u64;
        for _ in 0..5 {
            last = backend.pull_counters(sub).unwrap().bytes_in;
        }
        assert_eq!(last, 1_000);
        backend.cancel_subscription(sub).unwrap();
    }

    #[test]
    fn cancel_retires_token() {
        let m = Mock {
            devices: alloc::vec![wifi("wlan0", 100)],
            ..Default::default()
        };
        let backend = LinuxNetworkBackend::with_client(m);
        let sub = backend.subscribe_status(id_of("wlan0")).unwrap();
        backend.cancel_subscription(sub).unwrap();
        assert_eq!(backend.cancel_subscription(sub).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(backend.pull_status(sub).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn get_status_unknown_interface_is_not_found() {
        let m = Mock {
            devices: alloc::vec![wifi("wlan0", 100)],
            ..Default::default()
        };
        assert_eq!(
            LinuxNetworkBackend::with_client(m).get_status(id_of("nosuchdev")).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    // ── Error injection over all 20 codes (mapping total through the seam) ────

    #[test]
    fn error_injection_is_total_over_all_20_codes() {
        for code in ErrorCode::ALL {
            let m = Mock {
                networking_enabled: true,
                wireless_enabled: true,
                wwan_enabled: true,
                devices: alloc::vec![wifi("wlan0", 100)],
                ..Default::default()
            };
            let backend = LinuxNetworkBackend::with_client(m);
            *backend.client.fault.borrow_mut() = Some(Error::new(code));
            assert_eq!(backend.get_status(id_of("wlan0")).unwrap_err().code, code, "get_status carried {code:?}");
            assert_eq!(backend.get_interfaces(Pagination::default()).unwrap_err().code, code, "get_interfaces carried {code:?}");
            assert_eq!(backend.is_networking_enabled().unwrap_err().code, code, "is_networking_enabled carried {code:?}");
            assert_eq!(backend.list_radio_states(Pagination::default()).unwrap_err().code, code, "list_radio_states carried {code:?}");
            assert_eq!(backend.set_networking_enabled(true).unwrap_err().code, code, "setNetworking carried {code:?}");
        }
    }

    // ── sysfs + rfkill readers (hermetic: real files in a temp dir) ──────────

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

    #[test]
    fn parse_rfkill_events_splits_hard_and_soft() {
        // event: idx=u32, type=1(wifi), op=2, soft=1, hard=0
        let mut wifi = [0u8; 8];
        wifi[0] = 0; wifi[1] = 0; wifi[2] = 0; wifi[3] = 0; // idx=0
        wifi[4] = 1; // type wifi
        wifi[5] = 2; // op change
        wifi[6] = 1; // soft=1
        wifi[7] = 0; // hard=0
        let events = parse_rfkill_events(&wifi);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "wifi");
        assert!(events[0].soft_blocked);
        assert!(!events[0].hard_blocked);
    }

    // ── Availability ↔ tests linkage ────────────────────────────────────────

    /// Every REAL/DEGRADED row names an `l3_test`; each named suffix is one of
    /// the test functions above. A REAL claim with a vanished test is a FAIL
    /// (conformance §3.5).
    #[test]
    fn availability_real_rows_have_a_backing_test() {
        const IMPLEMENTED_TESTS: &[&str] = &[
            "get_interfaces_and_radio_states",
            "get_status_four_states_distinct",
            "status_subscription_bounded_pull",
            "cancel_retires_token",
            "set_radio_cellular_routes_to_mobile_data",
            "mobile_data_enabled_reads_wwan",
            "data_roaming_enabled_reads_wwan",
            "get_counters_from_sysfs",
            "counter_subscription_bounded_pull",
            "is_networking_enabled_reads_property",
            "set_networking_calls_enable",
            "set_radio_wifi_calls_wireless_enabled",
        ];
        for (name, l3) in claimed_l3_tests() {
            let suffix = l3.rsplit("::").next().unwrap();
            assert!(
                IMPLEMENTED_TESTS.contains(&suffix),
                "method {name} claims {l3} but no such test exists"
            );
        }
        // The host-global verbs are actually present and (for the reads) == REAL.
        for verb in ["isNetworkingEnabled", "listRadioStates", "getStatus"] {
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

    /// A read-only live integration test over the real system bus. Law #8: this
    /// only READS (properties, devices, rfkill) — it never turns a host switch.
    /// `#[ignore]`d so the default hermetic suite never touches the network, and
    /// run explicitly with `cargo test -- --ignored` when a bus is present. If
    /// NetworkManager is absent it fails loudly (never silently skipped).
    #[test]
    #[ignore = "read-only live probe against the real NetworkManager; run with `cargo test -- --ignored` when a system bus is present"]
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
        assert!(page.total >= 1, "expected at least the loopback device");
        assert!(!page.items.is_empty());
        // Every enumerated device yields a status whose hostState is one of the
        // contract's seven tokens (never an invented state).
        for iface in &page.items {
            let s = backend.get_status(iface.id).expect("getStatus read");
            assert!(
                HostState::ALL.contains(&s.host_state),
                "hostState {:?} must be a contract token",
                s.host_state
            );
        }
        let radios = backend
            .list_radio_states(Pagination { offset: 0, limit: u32::MAX })
            .expect("listRadioStates read");
        assert!(radios.total >= 1, "wifi + cellular radios are always enumerated");
        // Hard and soft are reported separately (never folded into enabled).
        assert!(radios.items.iter().any(|r| r.name == "wifi"));
    }
}
