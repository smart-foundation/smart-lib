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
    ConnStatus, HostState, HostStateSource, InterfaceCounters, InterfaceSnapshot, KnownWifiNetwork,
    MacAddress, Page, Pagination, ProxyConfigInfo, RadioState, RouteInfo, SubscriptionId,
    WifiNetwork, WifiWritePrivilege,
};
use crate::platforms::linux::netlink::{
    self, KernelAddress, KernelEvent, KernelLink, KernelRoute, LinkKind, LinkProvenance,
    NetlinkReader, NetlinkWriter, OperState, RealNetlinkReader, RealNetlinkWriter, RfkillState,
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
/// The wireless-device object: `RequestScan` (an active scan — a read-shaped
/// trigger, it changes no configuration) + `GetAllAccessPoints` (the scan
/// result). This is the whole of `wifiScan @6`.
const NM_WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
/// One access point as NM reports it (`Ssid`, `Strength`, `Frequency`,
/// `HwAddress`, `Flags`, `WpaFlags`, `RsnFlags`).
const NM_AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
/// An active connection: its `Connection` is the (permanent) profile object
/// whose settings carry the SSID — the chain `wifiScan`/status reads to put a
/// network name on the screen.
const NM_ACTIVE_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
/// The polkit authority on the system bus: `CheckAuthorization` is the
/// READ-shaped probe that measures what this uid may actually do, without
/// performing any host write (law #8).
const POLKIT_DEST: &str = "org.freedesktop.PolicyKit1";
const POLKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const POLKIT_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
/// NM `DeviceType` for a wifi device.
const NM_DEVICE_TYPE_WIFI: u32 = 2;
const DBUS_PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
/// The settings surface: `Settings` enumerates the profiles,
/// `Settings.Connection` reads and deletes one. This is where the
/// remembered-network bookkeeping (`listKnownWifiNetworks @78`,
/// `forgetWifiNetwork @77`) lives — the kernel owns no connection profiles.
const NM_SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const NM_SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const NM_CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
/// The NM setting names a wifi profile carries. Open tokens in the contract;
/// fixed keys here because these are NetworkManager's own names.
const NM_SETTING_CONNECTION: &str = "connection";
const NM_SETTING_WIFI: &str = "802-11-wireless";
const NM_SETTING_WIFI_SECURITY: &str = "802-11-wireless-security";
/// NM's connection type for a wifi profile.
const NM_CONNECTION_TYPE_WIFI: &str = "802-11-wireless";
/// The file that carries the host's system-wide proxy variables (pam_env's
/// `KEY=VAL` syntax). Verified: NetworkManager owns no proxy property at all
/// (its D-Bus interface documentation names none), so the host's proxy is a
/// file-level fact here rather than a daemon read.
const DEFAULT_PROXY_PATH: &str = "/etc/environment";
/// The proxy variables the host file spells, in the order the read reports
/// them. Open tokens on the wire; these are the file's own names.
const PROXY_KEYS: [&str; 4] = ["http_proxy", "https_proxy", "all_proxy", "no_proxy"];

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
    /// One dict entry `{KV}`: the key and the value, in signature order.
    /// `a{sa{sv}}` — the shape of every NM settings reply — nests these two
    /// deep, so the entry is a first-class arm rather than a special case.
    Entry(Box<Val>, Box<Val>),
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
            let elem_align = align_of(elem_sig[0]);
            // Mirror the reader (and the D-Bus spec): the length field is
            // 4-aligned, the elements start at the element-type's alignment
            // boundary, and the length counts ONLY the element bytes — never
            // the padding after the length field. The payload is padded out
            // to the element alignment at the end, which is what dbus-daemon
            // writes and what the reader's trailing-padding guard expects.
            let target = pad_to(buf.len(), 4);
            buf.resize(target, 0);
            let content = buf.len();
            buf.extend_from_slice(&0u32.to_le_bytes()); // length placeholder
            let start = pad_to(buf.len(), elem_align);
            buf.resize(start, 0);
            for item in items {
                marshal(buf, elem_sig, item)?;
            }
            buf.resize(pad_to(buf.len(), elem_align), 0);
            let len = (buf.len() - start) as u32;
            buf[content..content + 4].copy_from_slice(&len.to_le_bytes());
            let _ = rest;
        }
        (b'(', Val::Struct(members)) => {
            let mut rest = &sig[1..];
            for m in members {
                let (member_sig, r) = split_one_type(rest);
                rest = r;
                marshal(buf, member_sig, m)
                    .map_err(|_| Error::failed("struct member marshalling failed"))?;
            }
        }
        (b'{', Val::Entry(key, value)) => {
            // A dict entry is an 8-aligned struct of exactly two members.
            let target = pad_to(buf.len(), align_of(b'{'));
            buf.resize(target, 0);
            let (key_sig, rest) = split_one_type(&sig[1..]);
            marshal(buf, key_sig, key)?;
            let (value_sig, _) = split_one_type(rest);
            marshal(buf, value_sig, value)?;
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
    if std::env::var_os("SMART_LIB_DBUS_TRACE").is_some() {
        std::println!("DEBUG u sig={:?} off={} len={}", core::str::from_utf8(sig), *off, buf.len());
    }
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
            let elem_align = align_of(elem_sig[0]);
            // An array's LENGTH field is aligned to 4 (the D-Bus spec's ARRAY
            // alignment); the ELEMENTS inside are then aligned individually to
            // their own type (a dict entry to 8, a string to 4). The live probe
            // proved this by hexdump: an `a{sv}` value's length field sits at
            // an offset that is a multiple of 4 but NOT of 8, so padding the
            // length field to the element alignment desynchronises the reply.
            *off = pad_to(*off, 4);
            let byte_len = u32::from_le_bytes(take(buf, *off, 4)?.try_into().unwrap()) as usize;
            *off += 4;
            // The spec is explicit: the length does NOT include the padding
            // after the length field — the elements begin at the element
            // type's alignment boundary. The @78 live desync was exactly
            // this: counting the pre-element padding inside the length made
            // every element boundary drift, and the `a{sa{sv}}` settings
            // reply walked off the end of the message.
            *off = pad_to(*off, elem_align);
            let end = *off + byte_len;
            let mut items = Vec::new();
            // The payload is padded out to the element alignment, so the
            // trailing padding is never a phantom element: an element only
            // exists when the next alignment boundary still falls inside the
            // payload (an element occupies at least one byte past it).
            while pad_to(*off, elem_align) < end {
                items.push(unmarshal(buf, off, elem_sig)?);
            }
            *off = end;
            Val::Array(items)
        }
        b'(' => {
            // A struct always starts on an 8-byte boundary, even when its
            // contents are less strictly aligned (D-Bus spec: STRUCT and
            // DICT_ENTRY are always 8-aligned).
            *off = pad_to(*off, 8);
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
        b'{' => {
            // A dict entry: 8-aligned from the message start, key then value.
            *off = pad_to(*off, 8);
            let (key_sig, rest) = split_one_type(&sig[1..]);
            let key = unmarshal(buf, off, key_sig)?;
            let (value_sig, _) = split_one_type(rest);
            let value = unmarshal(buf, off, value_sig)?;
            Val::Entry(Box::new(key), Box::new(value))
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
    if std::env::var_os("SMART_LIB_DBUS_HEXDUMP").is_some() {
        let hex: alloc::string::String = reply_body.iter().take(1024).enumerate().map(|(i, b)| alloc::format!("{}{:02x}", if i % 32 == 0 { alloc::format!("\n{:04}: ", i) } else { String::new() }, b)).collect();
        std::println!("HEX sig={reply_sig} len={}\n{hex}", reply_body.len());
    }
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

/// A fresh, isolated bus connection used ONLY by the one-shot privilege probe.
/// A probe is a discrete read; keeping it off the shared NM session means a
/// probe-side close or timeout can never disturb the NM read stream.
struct ProbeConnection {
    stream: UnixStream,
    serial: u32,
    unique_name: String,
}

impl ProbeConnection {
    /// Connect to the system bus, authenticate, and `Hello`.
    fn connect() -> Result<Self> {
        use std::io::Write;
        let path = std::env::var("SMART_LIB_SYSTEM_BUS")
            .unwrap_or_else(|_| "/run/dbus/system_bus_socket".to_string());
        let mut stream = UnixStream::connect(&path)
            .map_err(|e| Error::unavailable(format!("dbus system bus at {path}: {e}")))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok();
        authenticate(&mut stream)?;
        let hello = build_call(1, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", "", &[]);
        stream.write_all(&hello).map_err(|e| Error::failed(format!("dbus hello write: {e}")))?;
        let (_s, _sig, body) = read_message(&mut stream)?;
        let unique_name = {
            let mut off = 0usize;
            unmarshal(&body, &mut off, b"s").ok().and_then(|v| match v {
                Val::Text(s) => Some(s),
                _ => None,
            })
        }
        .unwrap_or_default();
        Ok(ProbeConnection { stream, serial: 2, unique_name })
    }

    /// One method call to `dest` against this probe session, returning the
    /// reply body values.
    fn call(&mut self, dest: &str, path: &str, iface: &str, member: &str, sig: &[u8], args: &[Val]) -> Result<Vec<Val>> {
        use std::io::Write;
        let body_sig: String = sig.iter().map(|&b| b as char).collect();
        let wire = build_call(self.serial, dest, path, iface, member, &body_sig, args);
        self.serial += 1;
        self.stream.write_all(&wire).map_err(|e| Error::failed(format!("dbus write: {e}")))?;
        let (_rs, reply_sig, body) = read_message(&mut self.stream)?;
        let mut off = 0usize;
        let mut out = Vec::new();
        let mut rest = reply_sig.as_bytes();
        while !rest.is_empty() {
            let (one, r) = split_one_type(rest);
            rest = r;
            out.push(unmarshal(&body, &mut off, one)?);
        }
        Ok(out)
    }

    /// Ask the polkit authority whether THIS uid would be granted `action`,
    /// with no interaction (flags = 0) — no write, no prompt. Returns the open
    /// verdict token.
    fn check_authorization(&mut self, action: &str) -> Result<String> {
        let subject = Val::Struct(alloc::vec![
            Val::Text("system-bus-name".to_string()),
            Val::Array(alloc::vec![Val::Entry(
                Box::new(Val::Text("name".to_string())),
                Box::new(Val::Variant(b"s".to_vec(), Box::new(Val::Text(self.unique_name.clone())))),
            )]),
        ]);
        let sig = [b'(', b's', b'a', b'{', b's', b'v', b'}', b')', b's', b'a', b'{', b's', b's', b'}', b'u', b's'];
        let args = [
            subject,
            Val::Text(action.to_string()),
            Val::Array(Vec::new()),
            Val::UInt32(0),
            Val::Text(String::new()),
        ];
        let out = self.call(POLKIT_DEST, POLKIT_PATH, POLKIT_IFACE, "CheckAuthorization", &sig, &args)?;
        Ok(polkit_verdict(out))
    }
}

/// The read-shaped privilege measurement: what THIS uid may actually do to the
/// wifi control surface, as the host's authorisation authority reports it — the
/// measured truth that replaced the "polkit-gated" assumption. Two read-only
/// sources, both host state: the LIVE authority query (`CheckAuthorization`,
/// no interaction) when it answers, and — when the authority declines the
/// non-interactive query (a polkitd that closes an un-privileged, agent-less
/// self-authorization, as this box's does) — the action's polkit DEFAULT
/// resolved against the uid's measured active-session fact. Either way the
/// verdict is derived from the host, never assumed.
fn probe_wifi_write_privilege() -> Result<WifiWritePrivilege> {
    let uid = current_uid();
    // The action's polkit defaults, read from the host's own policy — the
    // qualitative fact that decides a no-interaction query on an active session.
    let policy_path = "/usr/share/polkit-1/actions/org.freedesktop.NetworkManager.policy";
    let active = uid_has_active_session(uid);
    let network_control = measure_action(
        "org.freedesktop.NetworkManager.network-control",
        uid,
        active,
        policy_path,
    );
    let modify_connection = measure_action(
        "org.freedesktop.NetworkManager.settings.modify.system",
        uid,
        active,
        policy_path,
    );
    let detail = format!(
        "uid {uid} ({} session) → network-control={network_control}, \
         settings.modify.system={modify_connection}",
        if active { "active" } else { "no active" }
    );
    Ok(WifiWritePrivilege {
        network_control,
        modify_connection,
        measured: true,
        uid,
        detail,
    })
}

/// Measure one action's no-interaction verdict: ask the LIVE authority first;
/// when it declines the query, resolve the action's polkit DEFAULT for the
/// uid's session qualifier. Both are read-only host facts.
fn measure_action(action: &str, uid: u32, active_session: bool, policy_path: &str) -> String {
    // Primary: the authoritative live `CheckAuthorization` (no interaction).
    if let Ok(mut probe) = ProbeConnection::connect() {
        match probe.check_authorization(action) {
            Ok(v) if v != "unknown" => return v,
            _ => {} // the authority declined a no-interaction answer; fall back
        }
    }
    // Fallback: the action's polkit default, resolved for this uid's session.
    match polkit_default_verdict(policy_path, action, active_session) {
        Some(v) => v,
        None => format!("unresolved(uid {uid}, no action default)"),
    }
}

/// Whether `uid` holds an ACTIVE local session — polkit's `allow_active`
/// qualifier. A logind runtime dir under `/run/user/<uid>` is created for an
/// active local session and torn down when it ends, so its presence is the
/// host's own, measured statement that this uid is active.
fn uid_has_active_session(uid: u32) -> bool {
    std::path::Path::new(&format!("/run/user/{uid}")).is_dir()
}

/// Resolve one action's polkit DEFAULT to a no-interaction verdict token.
/// Reads the action's `<defaults>` block from the `.policy` XML and applies
/// polkit's own resolution: `allow_active` for an active session, else
/// `allow_any`. `yes` → authorised; `no` → refused; `auth_*` → would prompt
/// (`challenge`). `None` when the action's default is not in the file —
/// reported, never guessed.
fn polkit_default_verdict(policy_path: &str, action: &str, active_session: bool) -> Option<String> {
    let text = std::fs::read_to_string(policy_path).ok()?;
    let (mut allow_any, mut allow_active) = (String::new(), String::new());
    // Find the action's opening tag, then its defaults before the closing tag.
    let open = format!("id=\"{action}\"");
    let start = text.find(&open)?;
    let end = text[start..].find("</action>").map(|e| start + e)?;
    let block = &text[start..end];
    let value_of = |tag: &str| -> Option<String> {
        let v_open = format!("<{tag}>");
        let v_close = format!("</{tag}>");
        let s = block.find(&v_open)? + v_open.len();
        let e = block[s..].find(&v_close)? + s;
        Some(block[s..e].trim().to_string())
    };
    allow_any = value_of("allow_any")?;
    allow_active = value_of("allow_active")?;
    let chosen = if active_session { allow_active } else { allow_any };
    Some(match chosen.as_str() {
        "yes" => "yes".to_string(),
        "no" => "no".to_string(),
        // auth_admin, auth_admin_keep, auth_self, auth_self_keep, auth_biometric...
        other if other.starts_with("auth") => "challenge".to_string(),
        _ => "unknown".to_string(),
    })
}


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
/// * [`NmClient::known_wifi_networks`] / [`NmClient::forget_wifi_network`] —
///   the remembered-network bookkeeping: profiles are NM's own, so no kernel
///   path owns them.
/// * the three control verbs — host-mutating, so they stay behind the seam
///   (law #8: the mock proves the mapping, the founder runs the live proof).
///
/// What is deliberately GONE: per-read `networking_enabled` / `wireless_enabled`
/// / `devices` / property readers. Those were the D-Bus-per-read path the
/// correction removed — the kernel first answers links, addresses, routes,
/// counters, carrier and radio blocks now (`platforms::linux::netlink`).
///
/// What is deliberately **not** here: a proxy verb. NetworkManager owns no
/// proxy configuration (its D-Bus interface documentation names no such
/// property, verified against `/usr/share/dbus-1/interfaces/`), so the host's
/// proxy is read as a file fact by the backend directly rather than pretended
/// into this seam.
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

    /// Every network the host remembers (`Settings.ListConnections` →
    /// per-profile `Connection.GetSettings`), filtered to wifi profiles. One
    /// counted seam call, however many profiles it enumerates.
    ///
    /// `GetSettings` never returns secrets (NetworkManager's own documentation
    /// says so), so a `KnownWifiNetwork` built from it structurally cannot
    /// carry a credential — the contract's shape and the host's behaviour agree.
    fn known_wifi_networks(&self) -> Result<Vec<KnownWifiNetwork>>;

    /// Forget one remembered network (`Settings.Connection.Delete`), the
    /// mutating half of the bookkeeping. Host-mutating; founder-run live,
    /// mock-verified here. An SSID the host does not know is `notFound`, never
    /// a vacuous success.
    fn forget_wifi_network(&self, ssid: &str) -> Result<()>;

    /// An active scan (`wifiScan @6`): `RequestScan` on the wireless device
    /// (a read-shaped trigger — it asks the radio to look, it changes no host
    /// configuration) followed by `GetAllAccessPoints`, each AP read into the
    /// contract's `WifiNetwork` (ssid / bssid / strength / frequency /
    /// security / hidden). One counted seam call, however many APs.
    fn wifi_scan(&self) -> Result<Vec<WifiNetwork>>;

    /// The SSID of the wifi device's ACTIVE connection, or `None` when the
    /// device has none. NM's chain: `Device.ActiveConnection` → its
    /// `Connection` profile → `802-11-wireless.ssid`. This is the network name
    /// that finally reaches the screen (`ConnStatus.ssid`) — the founder's
    /// "there is no network name anywhere".
    fn active_wifi_ssid(&self, iface: &str) -> Result<Option<String>>;

    /// `org.freedesktop.PolicyKit1.Authority.CheckAuthorization` for the NM
    /// verbs a wifi write needs, against THIS connection's uid. The READ-shaped
    /// privilege measurement (law #8): it asks the authority whether a write
    /// WOULD be granted — it never performs one, and with no-interaction flags
    /// it triggers no agent prompt. The measured verdict, never an assumption.
    fn measure_wifi_write_privilege(&self) -> Result<WifiWritePrivilege>;

    /// Activate a remembered wifi connection by SSID (`ActivateConnection` on
    /// the profile NM already holds). The secret is already IN the profile, so
    /// this path carries none — a reference is enough because there is nothing
    /// to (re)supply. Host-mutating; founder-run live, mock-verified here.
    fn wifi_connect_remembered(&self, ssid: &str) -> Result<()>;

    /// Disconnect the wifi device (`Device.Disconnect`). Host-mutating;
    /// founder-run live, mock-verified here.
    fn wifi_disconnect_active(&self) -> Result<()>;

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
    /// this is the dump's view; call [`NetworkStore::refresh_routes`] first for
    /// a table current as of now.
    pub fn routes(&self) -> Vec<KernelRoute> {
        self.state.lock().unwrap().routes.clone()
    }

    /// Re-read the kernel's routing table with a fresh `RTM_GETROUTE` dump and
    /// install it as the store's route table. A kernel read, no daemon hop, and
    /// the reason `getRoutes` is always current rather than a table that went
    /// stale while the link table stayed fresh. On failure the previous table
    /// is left alone and the error is carried out — an empty table would be the
    /// lie "the host has no routes".
    pub fn refresh_routes(&self) -> Result<()> {
        let routes = self.reader.routes()?;
        self.state.lock().unwrap().routes = routes;
        Ok(())
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
/// consumers). The one kernel *write* path (`setInterfaceMtu`) goes through a
/// [`NetlinkWriter`], the same mockable-seam shape as every other verb.
pub struct LinuxNetworkBackend<C: NmClient = RealNmClient> {
    client: C,
    store: NetworkStore,
    writer: Box<dyn NetlinkWriter>,
    next_token: AtomicU32,
    live: Mutex<alloc::collections::BTreeMap<u64, Subscription>>,
    /// The NM policy sweep, cached against the store's topology revision:
    /// `None` until the first read that needs policy state.
    policy: Mutex<Option<(u64, NmPolicy)>>,
    /// Each wifi device's active-connection SSID, cached against the store
    /// revision (an ifname → (revision, ssid) table). The SSID is a daemon fact
    /// NM owns, so a cold read fetches it once; a WARM read whose revision has
    /// not moved serves it locally — the same "no daemon hop on a warm store"
    /// rule the policy snapshot holds. A wifi reconnect moves the revision (the
    /// link's administrative state or its addresses change), so the cache never
    /// goes stale across a real reconnection.
    wifi_ssid: Mutex<Vec<(String, u64, String)>>,
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
        Self::with_client_netlink_and_writer(client, reader, Box::new(RealNetlinkWriter::new()))
    }

    /// Build a backend over an explicit kernel read seam AND an explicit
    /// kernel write seam (tests: a scripted writer records the MTU writes a
    /// backend asks for and never touches the host).
    pub fn with_client_netlink_and_writer(
        client: C,
        reader: Box<dyn NetlinkReader>,
        writer: Box<dyn NetlinkWriter>,
    ) -> Self {
        LinuxNetworkBackend {
            client,
            store: NetworkStore::with_reader(reader),
            writer,
            next_token: AtomicU32::new(1),
            live: Mutex::new(alloc::collections::BTreeMap::new()),
            policy: Mutex::new(None),
            wifi_ssid: Mutex::new(Vec::new()),
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

    /// A wifi device's active-connection SSID, fetched once per store revision
    /// and served from the cache afterwards (the SSID is an NM fact, but a warm
    /// store must not make a daemon hop — same rule as the policy snapshot).
    /// Empty when the device has no active connection, never a guessed name.
    fn active_wifi_ssid(&self, iface: &str) -> Result<String> {
        let revision = self.store.revision();
        {
            let cache = self.wifi_ssid.lock().unwrap();
            if let Some((_, _, ssid)) = cache.iter().find(|(name, rev, _)| name == iface && *rev == revision) {
                return Ok(ssid.clone());
            }
        }
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        let ssid = self.client.active_wifi_ssid(iface)?.unwrap_or_default();
        let mut cache = self.wifi_ssid.lock().unwrap();
        match cache.iter_mut().find(|(name, _, _)| name == iface) {
            Some(entry) => *entry = (iface.to_string(), revision, ssid.clone()),
            None => cache.push((iface.to_string(), revision, ssid.clone())),
        }
        Ok(ssid)
    }

    /// Build the contract status for one link record. The kernel answers the
    /// link facts; NM policy is fetched (one counted call) to classify, and the
    /// state's SOURCE is carried alongside it. For a connected wifi link, one
    /// further counted seam call reads NM's active-connection SSID — the
    /// network name that finally reaches the screen (the founder's complaint).
    fn status_of(&self, link: &KernelLink) -> Result<ConnStatus> {
        let rfkill = self.store.rfkill();
        let policy = self.ensure_policy()?;
        let (host_state, host_state_source) = classify_host_state(link, &rfkill, &policy);
        // The SSID exists only on a connected wifi link: NM's active connection
        // carries it. One counted seam call (cached against the store revision),
        // empty everywhere else — never a value invented for a disconnected or
        // non-wifi link.
        let ssid = if link.is_wifi() && link.is_running() {
            self.active_wifi_ssid(&link.name)?
        } else {
            String::new()
        };
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
            ssid,
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

    // ── The seven methods the route pass added (`@76`–`@82`) ────────────────
    //
    // Each one's availability row names which kind of answer this is. The
    // pattern the whole set obeys: a kernel-answerable fact (the routing
    // table, a link's MTU) goes to netlink; NM policy state (the remembered
    // networks) goes to the seam; the two writes no kernel path can own
    // honestly (a full route-table rewrite, the host proxy) are refused with
    // a reason rather than faked.

    /// `setInterfaceMtu @76` — the one kernel write this backend owns. The
    /// attribute lives on the link, NetworkManager has no MTU setter of its
    /// own, and the kernel reads it straight back as `IFLA_MTU`. DEGRADED: the
    /// mapping is proven against the scripted writer, the live proof is
    /// founder-run (law #8).
    fn set_interface_mtu(&self, interface: NetworkInterfaceId, mtu: u32) -> Result<()> {
        self.ensure_kernel_state()?;
        let link = self.find_link(interface)?;
        // The interface must exist: writing MTU onto an ifindex the kernel does
        // not know is a fabricated success.
        if mtu == 0 {
            return Err(Error::invalid_argument("mtu must be greater than zero"));
        }
        self.writer.set_link_mtu(link.index, mtu)
    }

    /// `forgetWifiNetwork @77` — DEGRADED: the remembered set is NM's
    /// connection profiles, and deleting one is polkit-gated. The mapping is
    /// proven against the scripted seam; the live proof is founder-run.
    fn forget_wifi_network(&self, ssid: &str) -> Result<()> {
        if ssid.is_empty() {
            return Err(Error::invalid_argument("ssid must not be empty"));
        }
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.forget_wifi_network(ssid)
    }

    /// `listKnownWifiNetworks @78` — REAL: NetworkManager's connection
    /// profiles, one counted seam call, no per-profile crawl. Carries no
    /// credential (`GetSettings` returns none), and the wiring of that is
    /// structural rather than promised.
    fn list_known_wifi_networks(&self, page: Pagination) -> Result<Page<KnownWifiNetwork>> {
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        let networks = self.client.known_wifi_networks()?;
        let total = networks.len() as u64;
        Ok(paginate(networks, total, page))
    }

    /// `wifiScan @6` — REAL over NM: `RequestScan` (a read-shaped trigger) +
    /// `GetAllAccessPoints`, one counted seam call. Every AP becomes a
    /// `WifiNetwork` (ssid / bssid / strength-as-dBm / frequency / security).
    /// The mapping is mock-proven; the live proof is the founder's read.
    fn wifi_scan(&self, page: Pagination) -> Result<Page<WifiNetwork>> {
        self.ensure_kernel_state()?;
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        let mut aps = self.client.wifi_scan()?;
        // A scan result is de-duplicated by SSID: a wifi radio reports one AP
        // per SSID to a scanner, and NM may list the same SSID across bands.
        // The strongest wins; the order is then by descending strength.
        aps.sort_by(|a, b| {
            b.strength_dbm
                .cmp(&a.strength_dbm)
                .then_with(|| a.ssid.cmp(&b.ssid))
        });
        aps.dedup_by(|a, b| a.ssid == b.ssid);
        let total = aps.len() as u64;
        Ok(paginate(aps, total, page))
    }

    /// `wifiConnect @7` — activate a remembered network over NM, by SSID. The
    /// credential is a reference and the secret is already IN the remembered
    /// profile, so nothing cleartext crosses. Host-mutating: mock-proven here,
    /// founder-run live, and gated by the shell. A not-remembered SSID returns
    /// a measured `no-privilege` (a NEW association needs secret material the
    /// reference does not carry on this host).
    fn wifi_connect(&self, ssid: &str, _credential: crate::core::ids::SecretKeyId) -> Result<()> {
        if ssid.is_empty() {
            return Err(Error::invalid_argument("ssid must not be empty"));
        }
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.wifi_connect_remembered(ssid)
    }

    /// `wifiDisconnect @8` — disconnect the wifi device over NM. Host-mutating:
    /// mock-proven here, founder-run live, gated by the shell.
    fn wifi_disconnect(&self) -> Result<()> {
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.wifi_disconnect_active()
    }

    /// The read-shaped privilege measurement — the truth that replaced the
    /// "polkit-gated" assumption. One counted seam call asks the authority what
    /// this uid may do; the measured words are carried out verbatim.
    fn measure_wifi_write_privilege(&self) -> Result<WifiWritePrivilege> {
        self.fallback_calls.fetch_add(1, Ordering::Relaxed);
        self.client.measure_wifi_write_privilege()
    }

    /// `getRoutes @79` — REAL: the kernel's routing table, a fresh
    /// `RTM_GETROUTE` dump through the read seam. No daemon hop, and current
    /// as of the read rather than as of the last link-table change.
    fn get_routes(&self, page: Pagination) -> Result<Page<RouteInfo>> {
        self.ensure_kernel_state()?;
        self.store.refresh_routes()?;
        let routes = self.store.routes();
        // A route's interface is the ifindex the kernel named; the contract's
        // `Route.interfaceId` is the derived id this backend uses everywhere
        // else, so the mapping is the same `iface_id` a `getStatus(id)` uses.
        let items: Vec<RouteInfo> = routes
            .iter()
            .map(|route| RouteInfo {
                destination_cidr: route.destination.clone(),
                gateway: route.gateway.clone(),
                metric: route.metric,
                interface_id: self
                    .store
                    .links()
                    .into_iter()
                    .find(|l| l.index == route.index)
                    .map(|l| iface_id(&l.name))
                    .unwrap_or_default(),
            })
            .collect();
        let total = items.len() as u64;
        Ok(paginate(items, total, page))
    }

    /// `setRoutes @80` — a full route-table rewrite is a `CAP_NET_ADMIN`
    /// netlink sequence with no honest half-measure, so this backend refuses
    /// it with the reason. The read half (`getRoutes`) is real and
    /// kernel-first, which is the half a caller can act on.
    fn set_routes(&self, _interface: NetworkInterfaceId, _routes: &[RouteInfo]) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: a route-table rewrite is a netlink RTM_NEWROUTE/RTM_DELROUTE sequence \
             (CAP_NET_ADMIN); not implemented — the read half, getRoutes, is the kernel's own table",
        ))
    }

    /// `getProxyConfig @81` — DEGRADED: reads the host's system-wide proxy
    /// block from `/etc/environment`. Honest about what it does not see: a
    /// desktop settings daemon's per-session proxy (the other half of most
    /// desktop hosts' proxy state) is not read, and the host's full proxy is
    /// the union of the two.
    fn get_proxy_config(&self) -> Result<ProxyConfigInfo> {
        Ok(read_host_proxy(Path::new(DEFAULT_PROXY_PATH)))
    }

    /// `setProxyConfig @82` — writing the host proxy means writing `/etc`
    /// (root) or driving a desktop settings daemon. Charter law #8 forbids
    /// the first, and this library is not a privilege broker, so the honest
    /// answer is a refusal naming the administrator who can.
    fn set_proxy_config(&self, _config: &ProxyConfigInfo) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: the host proxy is written by the system administrator (/etc) or a \
             desktop settings daemon; smart-lib never writes /etc (charter law #8)",
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

    /// Send a method call to NetworkManager and return its reply values.
    fn call(&self, path: &str, iface: &str, member: &str, sig: &[u8], args: &[Val]) -> Result<Vec<Val>> {
        self.call_dest(NM_DEST, path, iface, member, sig, args)
    }

    /// Send a method call to an explicit destination (the system bus also hosts
    /// the polkit authority, which the privilege probe queries) and return the
    /// reply values.
    fn call_dest(
        &self,
        dest: &str,
        path: &str,
        iface: &str,
        member: &str,
        sig: &[u8],
        args: &[Val],
    ) -> Result<Vec<Val>> {
        let body_sig: String = sig.iter().map(|&b| b as char).collect();
        let guard = self.stream.lock().unwrap();
        let mut stream = guard.borrow_mut();
        dbus_call(&mut *stream, &self.serial, dest, path, iface, member, &body_sig, args)
    }

    /// The object path of the first wifi device (`DeviceType == 2`), or `None`
    /// when the host has no wifi device at all (an honest empty scan).
    fn wifi_device_path(&self) -> Result<Option<String>> {
        let devices = self.devices()?;
        Ok(devices
            .into_iter()
            .find(|d| d.device_type == NM_DEVICE_TYPE_WIFI)
            .map(|d| d.path))
    }

    /// Read one access point into the contract's `WifiNetwork`. NM's AP
    /// properties: `Ssid` (`ay`), `Strength` (`y`, a 0–100 quality), `Frequency`
    /// (`u`, Hz), `HwAddress` (`s`), and the security word `Flags`/`WpaFlags`/
    /// `RsnFlags` (`u`) that decide the open security token.
    fn read_access_point(&self, path: &str) -> Result<WifiNetwork> {
        let uint = |prop: &str| -> u32 {
            match self.get_variant(path, NM_AP_IFACE, prop) {
                Ok(Val::UInt32(n)) => n,
                Ok(Val::UInt64(n)) => n as u32,
                _ => 0,
            }
        };
        let bssid = match self.get_variant(path, NM_AP_IFACE, "HwAddress") {
            Ok(Val::Text(s)) => s,
            _ => String::new(),
        };
        let strength_q = match self.get_variant(path, NM_AP_IFACE, "Strength") {
            Ok(Val::Byte(q)) => q,
            Ok(Val::UInt32(q)) => q.min(100) as u8,
            _ => 0,
        };
        let ssid = match self.get_variant(path, NM_AP_IFACE, "Ssid") {
            Ok(Val::Array(bytes)) => bytes
                .iter()
                .filter_map(|b| if let Val::Byte(x) = b { Some(*x) } else { None })
                .collect::<Vec<u8>>(),
            _ => Vec::new(),
        };
        let flags = uint("Flags");
        let wpa = uint("WpaFlags");
        let rsn = uint("RsnFlags");
        Ok(WifiNetwork {
            ssid: alloc::string::String::from_utf8_lossy(&ssid).into_owned(),
            bssid: MacAddress(bssid),
            // NM reports AP signal as a 0–100 quality; the contract's field is
            // dBm. The map is NM's own linear rule, documented — never a
            // fabricated absolute measurement.
            strength_dbm: ap_quality_to_dbm(strength_q),
            frequency_hz: uint("Frequency"),
            security: ap_security_token(flags, wpa, rsn),
            hidden: ssid.is_empty(),
        })
    }

    /// The SSID of a wifi device's active connection, via the NM object chain.
    fn active_connection_ssid(&self, iface: &str) -> Result<Option<String>> {
        let devices = self.devices()?;
        let Some(device) = devices
            .iter()
            .find(|d| d.iface == iface && d.device_type == NM_DEVICE_TYPE_WIFI)
        else {
            return Ok(None);
        };
        let object = |v: Result<Val>| -> String {
            match v {
                Ok(Val::ObjectPath(p)) | Ok(Val::Text(p)) => p,
                _ => String::new(),
            }
        };
        let active = object(self.get_variant(&device.path, NM_DEVICE_IFACE, "ActiveConnection"));
        if active.is_empty() || active == "/" {
            return Ok(None);
        }
        let conn = object(self.get_variant(&active, NM_ACTIVE_CONN_IFACE, "Connection"));
        if conn.is_empty() || conn == "/" {
            return Ok(None);
        }
        let settings = self.connection_settings(&conn)?;
        let ssid = settings
            .iter()
            .find(|(name, _)| name == NM_SETTING_WIFI)
            .and_then(|(_, props)| wifi_ssid(props))
            .filter(|s| !s.is_empty());
        Ok(ssid)
    }

    fn read_counters(&self, iface: &str) -> Result<InterfaceCounters> {
        Ok(read_counters_from_sysfs(&self.sysfs_root, iface))
    }

    /// `Settings.ListConnections` → the object paths of every profile.
    fn list_connections(&self) -> Result<Vec<String>> {
        let out = self.call(NM_SETTINGS_PATH, NM_SETTINGS_IFACE, "ListConnections", &[], &[])?;
        // The reply is `ao`: one array value whose items are the object paths.
        // (The first version of this helper matched a bare `ObjectPath` and so
        // silently returned an EMPTY profile list on a host with ten — the same
        // class of bug as the link-table dump fold, caught only by the live
        // probe. Flattening the array is the fix.)
        Ok(out
            .into_iter()
            .filter_map(|v| match v {
                Val::Array(items) => Some(items),
                Val::ObjectPath(p) => Some(alloc::vec![Val::ObjectPath(p)]),
                Val::Text(p) => Some(alloc::vec![Val::Text(p)]),
                _ => None,
            })
            .flatten()
            .filter_map(|v| match v {
                Val::ObjectPath(p) => Some(p),
                Val::Text(p) => Some(p),
                _ => None,
            })
            .collect())
    }

    /// `Connection.GetSettings` on one profile → the `a{sa{sv}}` settings map,
    /// flattened into (setting name, [(property, value)]).
    fn connection_settings(&self, path: &str) -> Result<Vec<(String, Vec<(String, Val)>)>> {
        let out = self.call(path, NM_CONNECTION_IFACE, "GetSettings", &[], &[])?;
        if let Some(Val::Variant(_, _)) = out.first() { let _ = out; }
        if std::env::var_os("SMART_LIB_DBUS_TRACE").is_some() {
            std::println!("DEBUG GetSettings ok");
        }
        let Some(reply) = out.into_iter().next() else {
            return Err(Error::failed(format!(
                "GetSettings({path}) returned nothing"
            )));
        };
        flatten_settings(&reply)
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

/// Flatten an `a{sa{sv}}` settings map into (setting name, [(property, value)]).
///
/// NM's own shape: the outer array's entries are `s` (setting name) → the inner
/// array's entries, each `s` (property name) → `v` (the variant value). A
/// property whose value is not the expected kind is skipped rather than guessed
/// at, and a setting with no readable properties is kept with an empty list so
/// the caller can still see the profile exists.
fn flatten_settings(value: &Val) -> Result<Vec<(String, Vec<(String, Val)>)>> {
    let Val::Array(entries) = value else {
        return Err(Error::failed(
            "GetSettings reply is not a settings map (a{sa{sv}})",
        ));
    };
    let mut settings = Vec::new();
    for entry in entries {
        let Val::Entry(key, inner) = entry else {
            return Err(Error::failed("settings map entry is not a dict entry"));
        };
        let name = match key.as_ref() {
            Val::Text(s) => s.clone(),
            _ => return Err(Error::failed("settings map key is not text")),
        };
        let mut properties = Vec::new();
        if let Val::Array(props) = inner.as_ref() {
            for prop in props {
                match prop {
                    Val::Entry(k, v) => {
                        if let Val::Text(pk) = k.as_ref() {
                            // A `{sv}` entry's value is a VARIANT — the wrapper
                            // the wire always carries. The reader keeps that
                            // wrapper (it is the honest `v`), so a property's
                            // value is the VARIANT here, and the consumer wants
                            // the value INSIDE it. Unwrapping one level is the
                            // whole mapping: leaving the wrapper in made every
                            // `Some(Val::Text(t) if t == "802-11-wireless")`
                            // match fail, so a host with ten profiles read as
                            // ZERO remembered wifi — the @78 "10 profiles, 0
                            // remembered" gap. A non-variant value (a mock that
                            // scripts the inner value directly) passes through
                            // unchanged, so the seam stays total.
                            let value = match v.as_ref() {
                                Val::Variant(_, inner) => (**inner).clone(),
                                other => other.clone(),
                            };
                            properties.push((pk.clone(), value));
                        }
                    }
                    Val::Variant(_, v) => properties.push((String::new(), (**v).clone())),
                    _ => {}
                }
            }
        }
        settings.push((name, properties));
    }
    Ok(settings)
}

/// One property's value out of a flattened settings section.
fn setting_prop<'a>(section: &'a [(String, Val)], prop: &str) -> Option<&'a Val> {
    section.iter().find(|(k, _)| k == prop).map(|(_, v)| v)
}

/// The SSID bytes of a wifi profile (`802-11-wireless.ssid` is `ay`, a byte
/// array — an SSID is not necessarily UTF-8). Rendered lossily: the contract's
/// `KnownWifiNetwork.ssid` is `Text`, so a non-UTF-8 SSID is the one place this
/// surface cannot be exact, and it says so in the report rather than inventing
/// an escaping.
fn wifi_ssid(section: &[(String, Val)]) -> Option<String> {
    let Val::Array(bytes) = setting_prop(section, "ssid")? else {
        return None;
    };
    let raw: Vec<u8> = bytes
        .iter()
        .filter_map(|b| match b {
            Val::Byte(b) => Some(*b),
            _ => None,
        })
        .collect();
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// The NM key-management token as one of the contract's open security tokens.
/// NM's own `key-mgmt` values are already the vocabulary the contract's open
/// token set expects; an unrecognised one is passed through verbatim rather
/// than flattened to "unknown", because the token set is open by design.
fn wifi_security(security: Option<&[(String, Val)]>) -> String {
    let Some(section) = security else {
        // No `802-11-wireless-security` setting at all: an open network.
        return "open".to_string();
    };
    match setting_prop(section, "key-mgmt") {
        Some(Val::Text(mgmt)) if !mgmt.is_empty() => mgmt.clone(),
        _ => "open".to_string(),
    }
}

/// NM's AP signal is a 0–100 quality; the contract's `strength` is dBm. NM's
/// own linear rule maps the two (`quality/2 - 100`, clamped to the −100..−50
/// dBm a wifi radio reports). Documented, symmetric with NetworkManager's
/// `nm_utils` — not an absolute measurement the host never made.
fn ap_quality_to_dbm(quality: u8) -> i32 {
    let q = quality.min(100) as i32;
    (q / 2 - 100).clamp(-100, -50)
}

/// The open security token for one AP, from NM's three security words:
/// `RsnFlags` (WPA2/WPA3), `WpaFlags` (WPA1), `Flags` (the PRIVACY bit for
/// WEP). The decision order is strongest-first so a WPA3 AP reads `wpa3`, not
/// the WPA2 word it also sets.
fn ap_security_token(flags: u32, wpa_flags: u32, rsn_flags: u32) -> String {
    const AP_FLAGS_PRIVACY: u32 = 0x1;
    const RSN_KEY_MGMT_SAE: u32 = 0x400;
    if rsn_flags & RSN_KEY_MGMT_SAE != 0 {
        "wpa3".to_string()
    } else if rsn_flags != 0 {
        "wpa2".to_string()
    } else if wpa_flags != 0 {
        "wpa".to_string()
    } else if flags & AP_FLAGS_PRIVACY != 0 {
        "wep".to_string()
    } else {
        "open".to_string()
    }
}

/// The polkit `CheckAuthorization` reply as one open verdict token. The reply
/// is `(b authorized, b challenged, a{ss} details)`: the struct arrives either
/// bare or folded into a one-element array (the same reply shape the NM
/// `ListConnections` / `GetAllAccessPoints` replies use), so both are unwrapped
/// here. `yes` (authorised) outranks `challenge` (would prompt) outranks `no`
/// (refused); a reply with no struct is an honest `unknown` — never a guess.
fn polkit_verdict(out: Vec<Val>) -> String {
    let verdict = out.into_iter().find_map(|v| match v {
        Val::Struct(members) => Some(members),
        Val::Array(mut one) if one.len() == 1 => match one.pop() {
            Some(Val::Struct(members)) => Some(members),
            _ => None,
        },
        _ => None,
    });
    let Some(members) = verdict else {
        return "unknown".to_string();
    };
    match (members.first(), members.get(1)) {
        (Some(Val::Boolean(true)), _) => "yes".to_string(),
        (_, Some(Val::Boolean(true))) => "challenge".to_string(),
        _ => "no".to_string(),
    }
}

/// Parse one profile's settings into a `KnownWifiNetwork`, or `None` when the
/// profile is not a wifi network (the enumeration covers every connection type
/// NM holds — ethernet, VPN, bridge, ... — and only wifi ones are networks).
fn known_wifi_from_settings(
    settings: &[(String, Vec<(String, Val)>)],
) -> Option<KnownWifiNetwork> {
    let connection = settings
        .iter()
        .find(|(name, _)| name == NM_SETTING_CONNECTION)
        .map(|(_, props)| props)?;
    // The profile's type decides: a wifi profile is the only kind this
    // contract surface remembers.
    match setting_prop(connection, "type") {
        Some(Val::Text(t)) if t == NM_CONNECTION_TYPE_WIFI => {}
        _ => return None,
    }
    let wifi = settings
        .iter()
        .find(|(name, _)| name == NM_SETTING_WIFI)
        .map(|(_, props)| props)?;
    let ssid = wifi_ssid(wifi)?;
    if ssid.is_empty() {
        return None; // a profile with no name cannot be reported as a network
    }
    let security = settings
        .iter()
        .find(|(name, _)| name == NM_SETTING_WIFI_SECURITY)
        .map(|(_, props)| props);
    let auto_connect = matches!(
        setting_prop(connection, "autoconnect"),
        Some(Val::Boolean(true))
    );
    // NM's `connection.timestamp` is seconds since the epoch; the contract's
    // `lastConnected` is nanoseconds. The conversion is exact, and a profile
    // NM never connected reports 0 — "not connected yet", never a guess.
    let last_connected = match setting_prop(connection, "timestamp") {
        Some(Val::UInt64(s)) => s.saturating_mul(1_000_000_000),
        Some(Val::UInt32(s)) => u64::from(*s).saturating_mul(1_000_000_000),
        _ => 0,
    };
    Some(KnownWifiNetwork {
        ssid,
        auto_connect,
        last_connected,
        security: wifi_security(security.as_ref().map(|v| v.as_slice())),
    })
}

/// Read the host's system-wide proxy configuration from a pam_env-style
/// `KEY=VAL` file. The `no_proxy` list is reported as an ordered list of
/// bypass entries under the `bypass` key, so the caller sees the host's exact
/// order and duplicates rather than a map that collapses them.
///
/// A file that is absent or unreadable is not an error: the host simply has no
/// system-wide proxy, which reads as a disabled configuration — but an
/// *unparsable* file is a stated gap, not silently-empty proxy state.
fn read_host_proxy(path: &Path) -> ProxyConfigInfo {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return ProxyConfigInfo::default(),
    };
    let mut url = String::new();
    let mut bypass: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if PROXY_KEYS.contains(&key) {
            if key == "no_proxy" {
                for host in value.split(',').map(str::trim).filter(|h| !h.is_empty()) {
                    bypass.push((host.to_string(), String::new()));
                }
            } else if url.is_empty() && !value.is_empty() {
                url = value.to_string();
            }
        }
    }
    ProxyConfigInfo {
        enabled: !url.is_empty(),
        url,
        bypass,
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

    fn known_wifi_networks(&self) -> Result<Vec<KnownWifiNetwork>> {
        let paths = self.list_connections()?;
        let mut networks = Vec::new();
        for path in &paths {
            let settings = self.connection_settings(path)?;
            if let Some(network) = known_wifi_from_settings(&settings) {
                networks.push(network);
            }
        }
        networks.sort_by(|a, b| a.ssid.cmp(&b.ssid));
        Ok(networks)
    }

    fn wifi_scan(&self) -> Result<Vec<WifiNetwork>> {
        let Some(device) = self.wifi_device_path()? else {
            return Ok(Vec::new()); // no wifi device: an honest empty scan
        };
        // Trigger the active scan: `RequestScan` with an empty options hash `a{sv}`.
        // It asks the radio to look and returns when the scan is underway; it
        // changes NO configuration. A best-effort trigger: if the radio refuses
        // (a scan was requested moments ago), the read below still returns the
        // APs NM last saw, so the answer is never invented.
        let _ = self.call(
            &device,
            NM_WIRELESS_IFACE,
            "RequestScan",
            &[b'a', b'{', b's', b'v', b'}'],
            &[Val::Array(Vec::new())],
        );
        let out = self.call(&device, NM_WIRELESS_IFACE, "GetAllAccessPoints", &[], &[])?;
        let paths: Vec<String> = out
            .into_iter()
            .filter_map(|v| match v {
                Val::Array(items) => Some(items),
                _ => None,
            })
            .flatten()
            .filter_map(|v| match v {
                Val::ObjectPath(p) | Val::Text(p) => Some(p),
                _ => None,
            })
            .collect();
        let mut aps = Vec::new();
        for path in &paths {
            aps.push(self.read_access_point(path)?);
        }
        Ok(aps)
    }

    fn active_wifi_ssid(&self, iface: &str) -> Result<Option<String>> {
        self.active_connection_ssid(iface)
    }

    fn measure_wifi_write_privilege(&self) -> Result<WifiWritePrivilege> {
        probe_wifi_write_privilege()
    }

    fn wifi_connect_remembered(&self, ssid: &str) -> Result<()> {
        // The profile NM already holds, found by SSID; the wifi device to
        // activate it on.
        let paths = self.list_connections()?;
        let mut profile = None;
        for path in &paths {
            let settings = self.connection_settings(path)?;
            if let Some(network) = known_wifi_from_settings(&settings) {
                if network.ssid == ssid {
                    profile = Some(path.clone());
                    break;
                }
            }
        }
        let Some(connection) = profile else {
            // Not remembered: a NEW wifi association needs the secret material,
            // and a SecretKeyId reference does not carry it on this host — the
            // honest answer names the gap instead of a blocked or fake success.
            return Err(Error::not_supported(format!(
                "no-privilege: {ssid} is not a remembered network; connecting to a NEW wifi \
                 network needs the secret material, which stays behind the SecretKeyId \
                 reference and is not present on this host — activate a remembered network, \
                 or add the profile through the system network settings"
            )));
        };
        let Some(device) = self.wifi_device_path()? else {
            return Err(Error::not_found().with_context("no wifi device to activate on"));
        };
        // `ActivateConnection(connection, device, specific_object)`: a "/"
        // specific object means "any AP of this network".
        self.call(
            NM_PATH,
            NM_IFACE,
            "ActivateConnection",
            &[b'o', b'o', b'o'],
            &[
                Val::ObjectPath(connection),
                Val::ObjectPath(device),
                Val::ObjectPath("/".to_string()),
            ],
        )?;
        Ok(())
    }

    fn wifi_disconnect_active(&self) -> Result<()> {
        let Some(device) = self.wifi_device_path()? else {
            return Err(Error::not_found().with_context("no wifi device to disconnect"));
        };
        self.call(&device, NM_DEVICE_IFACE, "Disconnect", &[], &[])?;
        Ok(())
    }


    fn forget_wifi_network(&self, ssid: &str) -> Result<()> {
        let paths = self.list_connections()?;
        for path in &paths {
            let settings = self.connection_settings(path)?;
            let Some(network) = known_wifi_from_settings(&settings) else {
                continue; // not a wifi profile: not the network we were asked to forget
            };
            if network.ssid == ssid {
                // Found: delete the profile. NM answers `AccessDenied` for a
                // polkit-gated delete, which the reply mapping already turns
                // into `permissionDenied`.
                self.call(path, NM_CONNECTION_IFACE, "Delete", &[], &[])?;
                return Ok(());
            }
        }
        // Not remembered: an honest notFound, never a vacuous success.
        Err(Error::not_found().with_context(format!("no remembered network named {ssid}")))
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
    use crate::platforms::linux::netlink::{
        KernelSnapshot, MockNetlink, MockNetlinkWriter,
    };
    use alloc::collections::BTreeSet;
    use alloc::vec;
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
    /// seam method (error injection over all 20 codes). `profiles` scripts the
    /// remembered-network set and `deletes` records which SSID the seam was
    /// asked to forget.
    #[derive(Default)]
    struct Mock {
        policy: NmPolicy,
        counters: InterfaceCounters,
        rfkill: Vec<RfkillState>,
        writes: RefCell<Vec<String>>,
        calls: RefCell<Vec<String>>,
        fault: RefCell<Option<Error>>,
        profiles: Vec<KnownWifiNetwork>,
        deletes: RefCell<Vec<String>>,
        /// Scripted access-point set `wifi_scan` enumerates.
        aps: Vec<WifiNetwork>,
        /// The scripted SSID of the wifi device's active connection (`None` =
        /// no active connection — an honest empty, never a guessed name).
        active_ssid: Option<String>,
        /// The measured privilege verdict `measure_wifi_write_privilege`
        /// reports (defaults to the measured-granted shape).
        privilege: WifiWritePrivilege,
        /// SSIDs `wifi_connect_remembered` was asked to activate.
        connects: RefCell<Vec<String>>,
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

        fn known_wifi_networks(&self) -> Result<Vec<KnownWifiNetwork>> {
            self.check()?;
            self.record("known_wifi_networks".to_string());
            let mut networks = self.profiles.clone();
            networks.sort_by(|a, b| a.ssid.cmp(&b.ssid));
            Ok(networks)
        }

        fn forget_wifi_network(&self, ssid: &str) -> Result<()> {
            self.check()?;
            if self.profiles.iter().any(|p| p.ssid == ssid) {
                self.deletes.borrow_mut().push(ssid.to_string());
                Ok(())
            } else {
                Err(Error::not_found().with_context(format!("no remembered network named {ssid}")))
            }
        }

        fn wifi_scan(&self) -> Result<Vec<WifiNetwork>> {
            self.check()?;
            self.record("wifi_scan".to_string());
            Ok(self.aps.clone())
        }

        fn active_wifi_ssid(&self, _iface: &str) -> Result<Option<String>> {
            self.check()?;
            self.record("active_wifi_ssid".to_string());
            Ok(self.active_ssid.clone())
        }

        fn measure_wifi_write_privilege(&self) -> Result<WifiWritePrivilege> {
            self.check()?;
            self.record("measure_wifi_write_privilege".to_string());
            Ok(self.privilege.clone())
        }

        fn wifi_connect_remembered(&self, ssid: &str) -> Result<()> {
            self.check()?;
            if self.profiles.iter().any(|p| p.ssid == ssid) {
                self.writes.borrow_mut().push(format!("ActivateConnection({ssid})"));
                self.connects.borrow_mut().push(ssid.to_string());
                Ok(())
            } else {
                Err(Error::not_supported(format!(
                    "no-privilege: {ssid} is not a remembered network; connecting to a NEW wifi \
                     network needs the secret material, which stays behind the SecretKeyId \
                     reference and is not present on this host"
                )))
            }
        }

        fn wifi_disconnect_active(&self) -> Result<()> {
            self.check()?;
            self.writes.borrow_mut().push("wifi_disconnect_active".to_string());
            Ok(())
        }
    }

    /// A backend over a scripted kernel and a scripted NM (never the host).
    fn backend(kernel: MockNetlink, nm: Mock) -> LinuxNetworkBackend<Mock> {
        LinuxNetworkBackend::with_client_and_netlink(nm, Box::new(kernel))
    }

    /// A backend over a scripted kernel, a scripted NM and a scripted kernel
    /// writer — the MTU-write tests use this so no path reaches the host.
    fn backend_with_writer(
        kernel: MockNetlink,
        nm: Mock,
        writer: Box<dyn NetlinkWriter>,
    ) -> LinuxNetworkBackend<Mock> {
        LinuxNetworkBackend::with_client_netlink_and_writer(nm, Box::new(kernel), writer)
    }

    /// A shareable handle on a [`MockNetlinkWriter`], so a test can build the
    /// backend and still inspect what the writer recorded. The production
    /// writer is never shared: the backend owns its socket.
    struct SharedWriter(alloc::sync::Arc<MockNetlinkWriter>);

    impl NetlinkWriter for SharedWriter {
        fn set_link_mtu(&self, index: u32, mtu: u32) -> Result<()> {
            self.0.set_link_mtu(index, mtu)
        }
    }

    /// A scripted kernel whose routing table carries a default route, a /24
    /// subnet route and a link-local route.
    fn kernel_with_routes(links: Vec<KernelLink>) -> MockNetlink {
        let mut snapshot = snapshot(links);
        snapshot.routes = alloc::vec![
            KernelRoute {
                index: 2,
                destination: "0.0.0.0/0".to_string(),
                gateway: "192.168.18.1".to_string(),
                table: 254,
                metric: 600,
            },
            KernelRoute {
                index: 2,
                destination: "10.0.0.0/24".to_string(),
                gateway: String::new(),
                table: 254,
                metric: 100,
            },
            KernelRoute {
                index: 3,
                destination: "::/0".to_string(),
                gateway: "fe80::1".to_string(),
                table: 254,
                metric: 1024,
            },
        ];
        MockNetlink::with_snapshot(snapshot)
    }

    /// The common fixture: one healthy ethernet, one healthy wifi, loopback, and
    /// an NM policy that has both radios on.
    fn happy_backend() -> LinuxNetworkBackend<Mock> {        backend(
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
        // Warm the store: the first read primes the policy snapshot AND, for a
        // connected wifi link, reads its active-connection SSID. Both are
        // counted daemon facts cold; both are served locally once warm.
        assert_eq!(b.nm_fallback_calls(), 0, "nothing has been read yet");
        let _ = b.get_status(id_of("wlp0s20f3")).unwrap();
        let after_first = b.nm_fallback_calls();
        assert_eq!(
            after_first, 2,
            "the cold wifi read issues exactly two counted fallbacks: the policy sweep + \
             the active-connection SSID (both NM facts the kernel cannot answer)"
        );

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

    /// BUG 1's hermetic lock: the two surfaces that once disagreed on a link's
    /// type read the SAME record. `getStatus(id)` must report the type
    /// `getInterfaces` enumerated — every interface, and in particular the
    /// wifi link must be `wifi` in the status path too, never a defaulted
    /// `ethernet` (the divergence the live verification caught).
    #[test]
    fn status_and_enumeration_agree_on_the_type() {
        let b = happy_backend();
        let page = b.get_interfaces(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        let mut checked = 0;
        for iface in &page.items {
            let status = b.get_status(iface.id).expect("getStatus read");
            assert_eq!(
                status.kind, iface.kind,
                "getStatus typed {} as {:?} while getInterfaces typed it {:?} — \
                 the two paths must read the type from the same source",
                iface.name, status.kind, iface.kind
            );
            checked += 1;
        }
        assert_eq!(checked, 3, "every enumerated interface was cross-checked");
        let wifi = page.items.iter().find(|i| i.name == "wlp0s20f3").unwrap();
        assert_eq!(wifi.kind, "wifi");
        assert_eq!(b.get_status(wifi.id).unwrap().kind, "wifi");
        let ether = page.items.iter().find(|i| i.name == "enp0s31f6").unwrap();
        assert_eq!(b.get_status(ether.id).unwrap().kind, "ethernet");
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
            "set_interface_mtu_writes_the_kernel_link_attribute",
            "set_interface_mtu_carries_a_writer_fault_over_all_20_codes",
            "forget_wifi_network_deletes_the_nm_profile",
            "list_known_wifi_networks_reads_nm_profiles",
            "the_nm_settings_to_known_wifi_mapping_keeps_wifi_profiles",
            "wifi_scan_enumerates_nm_access_points",
            "wifi_status_carries_the_active_ssid",
            "wifi_connect_activates_a_remembered_network",
            "wifi_disconnect_disconnects_the_device",
            "the_privilege_probe_is_a_measured_verdict",
            "get_routes_serves_the_kernel_table",
            "set_routes_is_refused_with_a_reason",
            "host_proxy_reads_the_environment_block",
            "get_proxy_config_reads_the_documented_host_file",
            "set_proxy_config_is_refused_with_a_reason",
            "the_route_pass_verbs_carry_a_seam_fault_over_all_20_codes",
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

    // ── Test-local wire builders (the DAEMON's convention, byte for byte) ────

    fn w_pad(buf: &mut Vec<u8>, align: usize) {
        buf.resize(pad_to(buf.len(), align), 0);
    }
    fn w_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn w_str(buf: &mut Vec<u8>, s: &str) {
        w_pad(buf, 4);
        w_u32(buf, s.len() as u32);
        buf.extend_from_slice(s.as_bytes());
        buf.push(0);
    }
    /// One `{sv}` entry in the daemon's convention.
    fn w_entry_sv(buf: &mut Vec<u8>, key: &str, inner_sig: &[u8], value: &Val) {
        w_pad(buf, 8);
        w_str(buf, key);
        buf.push(inner_sig.len() as u8);
        buf.extend_from_slice(inner_sig);
        buf.push(0);
        marshal(buf, inner_sig, value).unwrap();
    }

    /// The @78 wire fix, pinned hermetically: a dbus-daemon-shaped
    /// `a{sa{sv}}` settings reply built byte-for-byte in the DAEMON's
    /// convention — the length field never counts the pre-element padding,
    /// the payload is tail-padded to the element alignment, struct elements
    /// are 8-aligned, and the last setting's value array is empty. Every
    /// shape here desynchronised the hand-rolled reader live.
    #[test]
    fn dbus_settings_reply_in_the_daemons_convention_parses() {
        // Build the body in ONE buffer so every padding is the absolute
        // offset the reader will compute (the daemon aligns from the message
        // start, never from the container's start).
        let mut body: Vec<u8> = Vec::new();
        // Outer `a{sa{sv}}`: a 4-aligned length field, pre-element padding
        // (NOT counted in n), then the entries.
        w_pad(&mut body, 4);
        let outer_len_at = body.len();
        w_u32(&mut body, 0); // placeholder
        w_pad(&mut body, 8); // the first dict entry is 8-aligned
        let outer_start = body.len();

        // Setting "connection" → its `a{sv}` property array.
        w_pad(&mut body, 8);
        w_str(&mut body, "connection");
        w_pad(&mut body, 4);
        let props_len_at = body.len();
        w_u32(&mut body, 0); // placeholder
        // n counts the ELEMENT bytes only: the count starts at the element
        // alignment boundary the daemon pads to after the length field.
        let props_start = pad_to(body.len(), 8);
        w_entry_sv(&mut body, "id", b"s", &Val::Text("Matrix".to_string()));
        // A property whose value is a struct array `a(yu)`: the struct
        // elements are 8-aligned, each an 8-byte payload (y + pad + u).
        w_entry_sv(
            &mut body,
            "slots",
            b"a(yu)",
            &Val::Array(vec![
                Val::Struct(vec![Val::Byte(7), Val::UInt32(1500)]),
                Val::Struct(vec![Val::Byte(9), Val::UInt32(9000)]),
            ]),
        );
        // A property whose value is an array of arrays of uint32 (`aau`).
        w_entry_sv(
            &mut body,
            "addresses",
            b"aau",
            &Val::Array(vec![Val::Array(vec![
                Val::UInt32(0x0a00_0001),
                Val::UInt32(24),
                Val::UInt32(0x0a00_0001),
            ])]),
        );
        // Tail-pad the property array's payload to the dict-entry alignment
        // (8) — the daemon counts that padding inside n.
        w_pad(&mut body, 8);
        let props_len = (body.len() - props_start) as u32;
        body[props_len_at..props_len_at + 4].copy_from_slice(&props_len.to_le_bytes());

        // Setting "proxy" → an EMPTY property array (the live reply's tail:
        // the last entry whose value is `a{sv}` with n == 0).
        w_pad(&mut body, 8);
        w_str(&mut body, "proxy");
        w_pad(&mut body, 4);
        w_u32(&mut body, 0);
        w_pad(&mut body, 8); // the empty payload is still padded to the element alignment

        // The outer payload's tail padding (a no-op here — the last element
        // already ends 8-aligned).
        w_pad(&mut body, 8);
        let outer_len = (body.len() - outer_start) as u32;
        body[outer_len_at..outer_len_at + 4].copy_from_slice(&outer_len.to_le_bytes());

        let mut off = 0usize;
        let value = unmarshal(&body, &mut off, b"a{sa{sv}}").expect("the daemon's convention parses");
        assert_eq!(off, body.len(), "the whole reply is consumed, no desync");
        let settings = flatten_settings(&value).expect("a settings map");
        let names: Vec<&str> = settings.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["connection", "proxy"], "both settings survive");
        let conn = &settings[0].1;
        assert_eq!(conn.len(), 3, "id, slots and addresses survive");
        // The property VALUE is the variant's INNER value — `flatten_settings`
        // unwraps the `{sv}` wrapper (the wrapper is the wire's `v`, and the
        // consumer wants the value inside it, the fix that stops the
        // @78 "0 remembered wifi" gap).
        assert_eq!(conn[0].1, Val::Text("Matrix".to_string()));
        // The struct array keeps its two 8-aligned elements in order.
        match &conn[1].1 {
            Val::Array(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(
                    items[0],
                    Val::Struct(vec![Val::Byte(7), Val::UInt32(1500)])
                );
                assert_eq!(
                    items[1],
                    Val::Struct(vec![Val::Byte(9), Val::UInt32(9000)])
                );
            }
            other => panic!("expected an array, got {other:?}"),
        }
        // The empty setting keeps an empty property list (the profile exists).
        assert!(settings[1].1.is_empty(), "proxy survives as an empty setting");
    }

    /// The marshal/unmarshal codec agrees with itself in the daemon's
    /// convention: a settings-shaped value survives a full round trip.
    #[test]
    fn dbus_settings_value_round_trips_through_the_codec() {
        let value = Val::Array(vec![
            Val::Entry(
                Box::new(Val::Text("connection".to_string())),
                Box::new(Val::Array(vec![Val::Entry(
                    Box::new(Val::Text("id".to_string())),
                    Box::new(Val::Variant(
                        b"s".to_vec(),
                        Box::new(Val::Text("Matrix".to_string())),
                    )),
                )])),
            ),
            Val::Entry(
                Box::new(Val::Text("proxy".to_string())),
                Box::new(Val::Array(vec![])),
            ),
        ]);
        let mut body = Vec::new();
        marshal(&mut body, b"a{sa{sv}}", &value).unwrap();
        let mut off = 0usize;
        let back = unmarshal(&body, &mut off, b"a{sa{sv}}").unwrap();
        assert_eq!(back, value, "the value survives the wire byte for byte");
        assert_eq!(off, body.len());
    }

    /// The polkit `CheckAuthorization` subject `(sa{sv})` — a struct whose
    /// second member is an `a{sv}` of `{sv}` entries — marshals and unmarshals
    /// byte for byte. This is the nested-struct SEND path the privilege probe
    /// exercises, proven here so a marshalling bug cannot masquerade as a
    /// remote refusal.
    #[test]
    fn the_polkit_subject_marshals_as_valid_dbus() {
        let subject = Val::Struct(vec![
            Val::Text("system-bus-name".to_string()),
            Val::Array(vec![Val::Entry(
                Box::new(Val::Text("name".to_string())),
                Box::new(Val::Variant(b"s".to_vec(), Box::new(Val::Text(":1.206".to_string())))),
            )]),
        ]);
        let mut body = Vec::new();
        marshal(&mut body, b"(sa{sv})sa{ss}us", &subject).unwrap();
        let mut off = 0usize;
        let back = unmarshal(&body, &mut off, b"(sa{sv})").unwrap();
        assert_eq!(back, subject);
        assert_eq!(off, body.len(), "the subject's bytes are consumed exactly, no desync");
    }

    /// The FULL `CheckAuthorization` body `(sa{sv})sa{ss}us` marshals and is
    /// consumed byte for byte with no trailing bytes — the exact shape dbus-daemon
    /// must see to forward it, proven so a byte drift cannot masquerade as a
    /// remote refusal.
    #[test]
    fn the_full_check_authorization_body_is_byte_exact() {
        let subject = Val::Struct(vec![
            Val::Text("system-bus-name".to_string()),
            Val::Array(vec![Val::Entry(
                Box::new(Val::Text("name".to_string())),
                Box::new(Val::Variant(b"s".to_vec(), Box::new(Val::Text(":1.206".to_string())))),
            )]),
        ]);
        let args = [
            subject,
            Val::Text("org.freedesktop.NetworkManager.network-control".to_string()),
            Val::Array(Vec::new()),
            Val::UInt32(0),
            Val::Text(String::new()),
        ];
        let mut body = Vec::new();
        let mut rest = b"(sa{sv})sa{ss}us".as_slice();
        for item in &args {
            let (one, r) = split_one_type(rest);
            rest = r;
            marshal(&mut body, one, item).unwrap();
        }
        assert!(rest.is_empty(), "every argument consumed a signature");
        let mut off = 0usize;
        let mut rest = b"(sa{sv})sa{ss}us".as_slice();
        while !rest.is_empty() {
            let (one, r) = split_one_type(rest);
            rest = r;
            unmarshal(&body, &mut off, one).unwrap();
        }
        assert_eq!(off, body.len(), "the whole body is consumed with no trailing bytes");
    }    /// `cargo test -- --ignored` when a bus is present. If NetworkManager is
    /// absent it fails loudly (never silently skipped).
    // ── The seven route-pass methods (`@76`–`@82`) ──────────────────────────

    /// `setInterfaceMtu @76` writes the kernel link attribute for the interface
    /// the caller named — and nothing else. The mapping is the whole claim:
    /// the right ifindex, the right MTU, through the kernel write seam.
    #[test]
    fn set_interface_mtu_writes_the_kernel_link_attribute() {
        let writer = alloc::sync::Arc::new(MockNetlinkWriter::new());
        let b = backend_with_writer(
            kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true), wifi_link("wlp0s20f3", 3)]),
            Mock::default(),
            Box::new(SharedWriter(writer.clone())),
        );
        b.set_interface_mtu(id_of("enp0s31f6"), 9000).unwrap();
        // The write reached the kernel seam with the kernel's own ifindex: the
        // name the caller used is not what crosses.
        assert_eq!(*writer.writes.lock().unwrap(), alloc::vec![(2, 9000)]);
        // An unknown interface is notFound, never a write onto ifindex 0.
        assert_eq!(
            b.set_interface_mtu(id_of("nosuchdev"), 1500).unwrap_err().code,
            ErrorCode::NotFound
        );
        // A zero MTU is invalidArgument, not a kernel round trip.
        assert_eq!(
            b.set_interface_mtu(id_of("enp0s31f6"), 0).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        // Only the two valid calls reached the writer.
        assert_eq!(writer.writes.lock().unwrap().len(), 1);
    }

    /// A fault on the kernel write seam reaches the caller unchanged: error
    /// injection stays total over the write path too, across all 20 codes.
    #[test]
    fn set_interface_mtu_carries_a_writer_fault_over_all_20_codes() {
        for code in ErrorCode::ALL {
            let writer = alloc::sync::Arc::new(MockNetlinkWriter::new());
            writer.fail_with(Error::new(code));
            let b = backend_with_writer(
                kernel(alloc::vec![ethernet_link("enp0s31f6", 2, true)]),
                Mock::default(),
                Box::new(SharedWriter(writer)),
            );
            assert_eq!(
                b.set_interface_mtu(id_of("enp0s31f6"), 9000).unwrap_err().code,
                code,
                "set_interface_mtu carried {code:?}"
            );
        }
    }

    /// `forgetWifiNetwork @77` deletes the NM profile for the SSID the caller
    /// named, and an SSID the host does not remember is `notFound` — never a
    /// vacuous success that tells the caller a network was forgotten.
    #[test]
    fn forget_wifi_network_deletes_the_nm_profile() {
        let b = backend(kernel(alloc::vec![]), Mock {
            profiles: alloc::vec![KnownWifiNetwork {
                ssid: "hem".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });
        b.forget_wifi_network("hem").unwrap();
        assert_eq!(*b.client.deletes.borrow(), alloc::vec!["hem".to_string()]);
        // An empty SSID is a caller error, not a delete of "the first one".
        assert_eq!(
            b.forget_wifi_network("").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        // An unknown SSID is notFound, and no delete was attempted.
        assert_eq!(
            b.forget_wifi_network("nosuchnet").unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(b.client.deletes.borrow().len(), 1);
        // A seam fault propagates unchanged.
        b.client.fault.replace(Some(Error::permission_denied()));
        assert_eq!(
            b.forget_wifi_network("hem").unwrap_err().code,
            ErrorCode::PermissionDenied
        );
    }

    /// `listKnownWifiNetworks @78` is one counted seam call (the profile set is
    /// NM's, so no kernel path owns it), sorted by SSID, paginated under the
    /// contract rule, and carrying no credential.
    #[test]
    fn list_known_wifi_networks_reads_nm_profiles() {
        let b = backend(kernel(alloc::vec![]), Mock {
            profiles: alloc::vec![
                KnownWifiNetwork {
                    ssid: "hem".to_string(),
                    auto_connect: true,
                    last_connected: 1_700_000_000_000_000_000,
                    security: "wpa3".to_string(),
                },
                KnownWifiNetwork {
                    ssid: "aboard".to_string(),
                    auto_connect: false,
                    last_connected: 0,
                    security: "open".to_string(),
                },
            ],
            ..Default::default()
        });
        let page = b.list_known_wifi_networks(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].ssid, "aboard", "sorted by SSID");
        assert_eq!(page.items[1].ssid, "hem");
        assert_eq!(page.items[1].last_connected, 1_700_000_000_000_000_000);
        // limit 0 means one, on this list too.
        let one = b.list_known_wifi_networks(Pagination { offset: 0, limit: 0 }).unwrap();
        assert_eq!(one.items.len(), 1);
        assert_eq!(one.total, 2);
        // Exactly one counted seam call per enumeration, and a second
        // enumeration is a second call (the profile set is NM's, not cached).
        assert_eq!(b.nm_fallback_calls(), 2, "one counted call per enumeration");
        // No credential field exists on the type, so none can be reported.
        let printed = alloc::format!("{:?}", page.items[0]);
        for forbidden in ["credential", "password", "passphrase", "secret", "psk"] {
            assert!(!printed.contains(forbidden), "{forbidden} must not appear");
        }
    }

    /// The regression lock for the @78 gap: NetworkManager's `GetSettings`
    /// reply is `a{sa{sv}}`, and EVERY property value is a VARIANT on the
    /// wire. When `flatten_settings` left the variant wrapper in place, the
    /// wifi-type check (`type == "802-11-wireless"`) matched nothing, so a
    /// host with nine remembered wifi profiles read as ZERO "while connected"
    /// — the founder's exact complaint. This fixture scripts the real wire
    /// shape (variant-wrapped `{sv}` entries) and proves the mapping now keeps
    /// every wifi profile, with ssid/auto-connect/last-connected/security, and
    /// still drops the non-wifi ones.
    #[test]
    fn the_nm_settings_to_known_wifi_mapping_keeps_wifi_profiles() {
        // One property value as the wire spells it: a variant around the inner.
        fn variant(inner: Val) -> Val {
            let sig = match &inner {
                Val::Text(_) | Val::ObjectPath(_) => b's',
                Val::Boolean(_) => b'b',
                Val::UInt32(_) => b'u',
                Val::UInt64(_) => b't',
                Val::Array(_) => b'a',
                _ => b'v',
            };
            Val::Variant(alloc::vec![sig], Box::new(inner))
        }
        // One settings section: `s` name → `a{sv}` properties.
        let section = |props: Vec<(&str, Val)>| -> Val {
            Val::Array(
                props
                    .into_iter()
                    .map(|(k, v)| Val::Entry(Box::new(Val::Text(k.to_string())), Box::new(variant(v))))
                    .collect(),
            )
        };
        let ssid_bytes = |s: &str| -> Val {
            Val::Array(s.bytes().map(Val::Byte).collect())
        };

        // A wifi profile exactly as NM reports it: the connection type in the
        // `connection` section, the SSID bytes in `802-11-wireless`, and the
        // key-management in `802-11-wireless-security`. `connection.timestamp`
        // is SECONDS; the contract's `lastConnected` is nanoseconds.
        let wifi = Val::Array(alloc::vec![
            Val::Entry(
                Box::new(Val::Text("connection".to_string())),
                Box::new(section(alloc::vec![
                    ("type", Val::Text("802-11-wireless".to_string())),
                    ("autoconnect", Val::Boolean(true)),
                    ("timestamp", Val::UInt64(1_700_000_000)),
                ])),
            ),
            Val::Entry(
                Box::new(Val::Text("802-11-wireless".to_string())),
                Box::new(section(alloc::vec![
                    ("ssid", ssid_bytes("VR-3245-0884")),
                ])),
            ),
            Val::Entry(
                Box::new(Val::Text("802-11-wireless-security".to_string())),
                Box::new(section(alloc::vec![
                    ("key-mgmt", Val::Text("wpa-psk".to_string())),
                ])),
            ),
        ]);
        // An ethernet profile (dropped) and an open wifi profile with no
        // security section (kept, security "open").
        let wired = Val::Array(alloc::vec![Val::Entry(
            Box::new(Val::Text("connection".to_string())),
            Box::new(section(alloc::vec![
                ("type", Val::Text("802-3-ethernet".to_string())),
            ])),
        )]);
        let openwifi = Val::Array(alloc::vec![
            Val::Entry(
                Box::new(Val::Text("connection".to_string())),
                Box::new(section(alloc::vec![
                    ("type", Val::Text("802-11-wireless".to_string())),
                    ("autoconnect", Val::Boolean(false)),
                ])),
            ),
            Val::Entry(
                Box::new(Val::Text("802-11-wireless".to_string())),
                Box::new(section(alloc::vec![
                    ("ssid", ssid_bytes("Guest")),
                ])),
            ),
        ]);

        let mut got: Vec<KnownWifiNetwork> = [wifi, wired, openwifi]
            .iter()
            .filter_map(|raw| {
                let settings = flatten_settings(raw).expect("flatten");
                known_wifi_from_settings(&settings)
            })
            .collect();
        got.sort_by(|a, b| a.ssid.cmp(&b.ssid));

        assert_eq!(got.len(), 2, "the two wifi profiles survive, the ethernet one is dropped");
        let guest = &got[0];
        assert_eq!(guest.ssid, "Guest");
        assert!(!guest.auto_connect);
        assert_eq!(guest.security, "open", "no 802-11-wireless-security section means open");
        assert_eq!(guest.last_connected, 0, "no timestamp means never connected, never fabricated");
        let v = &got[1];
        assert_eq!(v.ssid, "VR-3245-0884");
        assert!(v.auto_connect);
        assert_eq!(v.security, "wpa-psk");
        assert_eq!(v.last_connected, 1_700_000_000 * 1_000_000_000, "seconds → nanoseconds, exact");
    }

    /// `wifiScan @6` over the mock seam: the scripted access points become
    /// `WifiNetwork` values (ssid / bssid / strength-as-dBm / frequency /
    /// security), de-duplicated by SSID (a radio reports one AP per SSID across
    /// bands), strongest first, and it is one counted seam call.
    #[test]
    fn wifi_scan_enumerates_nm_access_points() {
        let b = backend(kernel(alloc::vec![wifi_link("wlp0s20f3", 3)]), Mock {
            aps: alloc::vec![
                WifiNetwork {
                    ssid: "VR-3245-0884".to_string(),
                    bssid: MacAddress("AA:BB:CC:DD:EE:FF".to_string()),
                    strength_dbm: -55,
                    frequency_hz: 5_180_000,
                    security: "wpa3".to_string(),
                    hidden: false,
                },
                // The same radio reports this SSID again on 2.4GHz, weaker —
                // de-duplicated away, keeping the strongest.
                WifiNetwork {
                    ssid: "VR-3245-0884".to_string(),
                    bssid: MacAddress("AA:BB:CC:DD:EE:00".to_string()),
                    strength_dbm: -80,
                    frequency_hz: 2_437_000,
                    security: "wpa2".to_string(),
                    hidden: false,
                },
                WifiNetwork {
                    ssid: "Big Boy Soweto".to_string(),
                    bssid: MacAddress("11:22:33:44:55:66".to_string()),
                    strength_dbm: -40,
                    frequency_hz: 2_437_000,
                    security: "open".to_string(),
                    hidden: false,
                },
            ],
            ..Default::default()
        });
        let page = b.wifi_scan(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 2, "the duplicate SSID collapses to one row");
        assert_eq!(page.items[0].ssid, "Big Boy Soweto", "strongest first");
        assert_eq!(page.items[0].strength_dbm, -40);
        assert_eq!(page.items[0].security, "open");
        assert_eq!(page.items[1].ssid, "VR-3245-0884");
        assert_eq!(page.items[1].strength_dbm, -55, "the stronger band survived the dedup");
        assert_eq!(b.nm_fallback_calls(), 1, "one counted seam call for the scan");
    }

    /// `getStatus` now carries the network NAME for a connected wifi link — the
    /// founder's "there is no network name anywhere". One counted seam call
    /// reads NM's active-connection SSID; a non-wifi or disconnected link reads
    /// empty, never invented.
    #[test]
    fn wifi_status_carries_the_active_ssid() {
        let b = backend(
            kernel(alloc::vec![
                ethernet_link("enp0s31f6", 2, true),
                wifi_link("wlp0s20f3", 3),
            ]),
            Mock {
                active_ssid: Some("VR-3245-0884".to_string()),
                ..Default::default()
            },
        );
        let wifi = b.get_status(id_of("wlp0s20f3")).unwrap();
        assert_eq!(wifi.ssid, "VR-3245-0884", "the connected wifi link finally has its name");
        // The wired link never carries an SSID and issues no active-ssid call.
        let wired = b.get_status(id_of("enp0s31f6")).unwrap();
        assert!(wired.ssid.is_empty(), "an ethernet link has no network name");
        // A wifi link with no active connection reads empty — never a guess.
        let idle = backend(kernel(alloc::vec![wifi_link("wlp0s20f3", 3)]), Mock::default());
        assert!(idle.get_status(id_of("wlp0s20f3")).unwrap().ssid.is_empty());
    }

    /// `wifiConnect @7` activates a remembered network over NM (the secret is
    /// already in the profile), an empty SSID is a caller error, and a network
    /// the host does not remember is a measured `no-privilege` — never a fake
    /// success. Host-mutating: proven against the seam, founder-run live.
    #[test]
    fn wifi_connect_activates_a_remembered_network() {
        use crate::core::ids::SecretKeyId;
        let b = backend(kernel(alloc::vec![]), Mock {
            profiles: alloc::vec![KnownWifiNetwork {
                ssid: "hem".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });
        b.wifi_connect("hem", SecretKeyId::new(1)).unwrap();
        assert_eq!(*b.client.connects.borrow(), alloc::vec!["hem".to_string()]);
        assert_eq!(
            *b.client.writes.borrow(),
            alloc::vec!["ActivateConnection(hem)".to_string()],
            "the verb is NM's own ActivateConnection — no cleartext crosses"
        );
        assert_eq!(
            b.wifi_connect("", SecretKeyId::new(1)).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        let err = b.wifi_connect("nosuchnet", SecretKeyId::new(1)).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.message.starts_with("no-privilege:"), "{}", err.message);
        assert!(err.message.contains("remembered"), "{}", err.message);
    }

    /// `wifiDisconnect @8` disconnects the wifi device over NM.
    #[test]
    fn wifi_disconnect_disconnects_the_device() {
        let b = happy_backend();
        b.wifi_disconnect().unwrap();
        assert_eq!(*b.client.writes.borrow(), alloc::vec!["wifi_disconnect_active".to_string()]);
    }

    /// The read-shaped privilege probe's hermetic shape: the measured verdict
    /// is carried verbatim (the three answers the authority can give, and the
    /// `measured` flag), a seam fault propagates unchanged, and the pure
    /// reply-parser (`polkit_verdict`) maps the `(b,b,a{ss})` reply's three
    /// shapes — bare struct, one-element-array-wrapped struct, and none.
    #[test]
    fn the_privilege_probe_is_a_measured_verdict() {
        // Granted shape.
        let granted = Mock {
            privilege: WifiWritePrivilege {
                network_control: "yes".to_string(),
                modify_connection: "no".to_string(),
                measured: true,
                uid: 1000,
                detail: "uid 1000 → network-control=yes".to_string(),
            },
            ..Default::default()
        };
        assert_eq!(granted.privilege.network_control, "yes");
        assert!(granted.privilege.measured);
        // A fault on the probe is carried unchanged (it is a seam call).
        let faulted = Mock {
            fault: RefCell::new(Some(Error::permission_denied())),
            ..Default::default()
        };
        assert_eq!(
            faulted.measure_wifi_write_privilege().unwrap_err().code,
            ErrorCode::PermissionDenied
        );
        // The pure reply parser over the three wire shapes:
        let bare_yes = alloc::vec![Val::Struct(alloc::vec![Val::Boolean(true), Val::Boolean(false)])];
        assert_eq!(polkit_verdict(bare_yes), "yes");
        let wrapped_challenge =
            alloc::vec![Val::Array(alloc::vec![Val::Struct(alloc::vec![
                Val::Boolean(false),
                Val::Boolean(true),
            ])])];
        assert_eq!(polkit_verdict(wrapped_challenge), "challenge");
        let bare_no =
            alloc::vec![Val::Struct(alloc::vec![Val::Boolean(false), Val::Boolean(false)])];
        assert_eq!(polkit_verdict(bare_no), "no");
        assert_eq!(polkit_verdict(alloc::vec![Val::Text("garbage".to_string())]), "unknown");
    }

    /// The probe's fallback resolves an action's polkit DEFAULT against the
    /// uid's session qualifier — the measurement that answers when the live
    /// authority declines a no-interaction query. `allow_active=yes` on an
    /// active session is authorised; `auth_admin` is a challenge; `no` is a
    /// refusal; the "any" qualifier takes over on a session-less uid.
    #[test]
    fn polkit_default_verdict_resolves_the_policy_default() {
        let dir = std::env::temp_dir().join(format!("smart-lib-polkit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let policy = dir.join("nm.policy");
        std::fs::write(
            &policy,
            concat!(
                "<policyconfig>\n",
                "  <action id=\"org.freedesktop.NetworkManager.network-control\">\n",
                "    <defaults>\n",
                "      <allow_any>auth_admin</allow_any>\n",
                "      <allow_active>yes</allow_active>\n",
                "    </defaults>\n",
                "  </action>\n",
                "  <action id=\"org.freedesktop.NetworkManager.other\">\n",
                "    <defaults>\n",
                "      <allow_any>no</allow_any>\n",
                "      <allow_active>auth_admin_keep</allow_active>\n",
                "    </defaults>\n",
                "  </action>\n",
                "</policyconfig>\n",
            ),
        )
        .unwrap();
        let path = policy.to_str().unwrap();
        let nc = "org.freedesktop.NetworkManager.network-control";
        let other = "org.freedesktop.NetworkManager.other";
        // Active session resolves allow_active.
        assert_eq!(polkit_default_verdict(path, nc, true).as_deref(), Some("yes"));
        // Session-less resolves allow_any (auth_admin → a challenge).
        assert_eq!(polkit_default_verdict(path, nc, false).as_deref(), Some("challenge"));
        assert_eq!(polkit_default_verdict(path, other, true).as_deref(), Some("challenge"));
        assert_eq!(polkit_default_verdict(path, other, false).as_deref(), Some("no"));
        // An action the file does not carry is None (reported, never guessed).
        assert_eq!(polkit_default_verdict(path, "org.freedesktop.NetworkManager.absent", true), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The active-session fact is the host's own: `/run/user/<uid>` exists for
    /// an active local session and is absent for one that never logged in. Read
    /// from the host, never guessed.
    #[test]
    fn the_active_session_fact_is_the_runtime_dir() {
        // A uid that almost certainly has no runtime dir reads "no active
        // session"; the current uid's answer is whatever the host holds.
        assert!(!uid_has_active_session(u32::MAX));
        // The probe always measures SOMETHING (measured=true) with a real uid.
        let verdict = probe_wifi_write_privilege().unwrap();
        assert!(verdict.measured);
        assert_eq!(verdict.uid, current_uid());
    }


    /// `getRoutes @79` serves the kernel's routing table with no daemon hop:
    /// a fresh `RTM_GETROUTE` dump, CIDR-spelled destinations, and the contract
    /// interface id derived the same way `getStatus(id)` derives it.
    #[test]
    fn get_routes_serves_the_kernel_table() {
        let b = backend(
            kernel_with_routes(alloc::vec![
                ethernet_link("enp0s31f6", 2, true),
                wifi_link("wlp0s20f3", 3),
            ]),
            Mock::default(),
        );
        let page = b.get_routes(Pagination { offset: 0, limit: u32::MAX }).unwrap();
        assert_eq!(page.total, 3);
        let default = page.items.iter().find(|r| r.destination_cidr == "0.0.0.0/0").unwrap();
        assert_eq!(default.gateway, "192.168.18.1");
        assert_eq!(default.metric, 600);
        assert_eq!(default.interface_id, id_of("enp0s31f6"));
        let v6 = page.items.iter().find(|r| r.destination_cidr == "::/0").unwrap();
        assert_eq!(v6.interface_id, id_of("wlp0s20f3"));
        let onlink = page.items.iter().find(|r| r.destination_cidr == "10.0.0.0/24").unwrap();
        assert!(onlink.gateway.is_empty(), "an on-link route has no gateway");
        // Routes are a kernel fact: no NM seam call answered them.
        assert_eq!(b.nm_fallback_calls(), 0, "the routing table is the kernel's own");
        // A kernel that cannot answer leaves the previous error visible rather
        // than an empty table pretending the host has no routes.
        let k = kernel_with_routes(alloc::vec![ethernet_link("enp0s31f6", 2, true)]);
        k.fail_with(Error::unavailable("netlink route dump: operation not permitted"));
        let b = backend(k, Mock::default());
        let err = b.get_routes(Pagination::default()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("netlink"), "{}", err.message);
    }

    /// `setRoutes @80` is refused with a reason, not half-implemented: a
    /// full-table rewrite has no honest partial form, and the read half is the
    /// kernel's own table.
    #[test]
    fn set_routes_is_refused_with_a_reason() {
        let b = happy_backend();
        let err = b
            .set_routes(id_of("enp0s31f6"), &[RouteInfo::default()])
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.message.starts_with("no-privilege:"), "{}", err.message);
        assert!(!err.retryable, "a refusal is not retryable");
    }

    /// `getProxyConfig @81` reads the host's system-wide proxy block from a
    /// pam_env-style `KEY=VAL` file: the URL, the bypass list in the host's own
    /// order and with its duplicates, and an absent file as "no system-wide
    /// proxy" (not an error, and not a fabricated proxy).
    #[test]
    fn host_proxy_reads_the_environment_block() {
        let dir = std::env::temp_dir().join(format!("smart-lib-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("environment");
        let content = concat!(
            "# pam_env syntax\n",
            "http_proxy=\"http://proxy.corp:3128\"\n",
            "no_proxy=localhost,127.0.0.1,localhost\n",
            "https_proxy=http://proxy.corp:3129\n",
            "UNRELATED=ignored\n",
        );
        std::fs::write(&file, content).unwrap();
        let proxy = read_host_proxy(&file);
        assert!(proxy.enabled);
        assert_eq!(proxy.url, "http://proxy.corp:3128", "the first proxy URL wins, verbatim");
        assert_eq!(
            proxy.bypass,
            alloc::vec![
                ("localhost".to_string(), String::new()),
                ("127.0.0.1".to_string(), String::new()),
                ("localhost".to_string(), String::new()),
            ],
            "the bypass list keeps the host's order and its duplicates"
        );
        // An absent file is an honest empty configuration, never an error and
        // never a guessed proxy.
        assert_eq!(read_host_proxy(&dir.join("nosuchfile")), ProxyConfigInfo::default());
        // A file with no proxy keys at all is also just empty.
        let none = dir.join("none");
        std::fs::write(&none, "PATH=/usr/bin\n").unwrap();
        let empty = read_host_proxy(&none);
        assert!(!empty.enabled);
        assert!(empty.url.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default host proxy path is the documented one, and the backend reads
    /// it (this host's `/etc/environment` is readable by construction).
    #[test]
    fn get_proxy_config_reads_the_documented_host_file() {
        let b = happy_backend();
        let config = b.get_proxy_config().unwrap();
        assert_eq!(DEFAULT_PROXY_PATH, "/etc/environment");
        // Whatever this host carries, the answer is a real read of a real file:
        // enabled ⇔ a URL is present, and never a fabricated one.
        assert_eq!(config.enabled, !config.url.is_empty());
    }

    /// `setProxyConfig @82` is refused: writing the host proxy means writing
    /// `/etc` or driving a settings daemon, and charter law #8 forbids the
    /// first while this library is not the second's privilege broker.
    #[test]
    fn set_proxy_config_is_refused_with_a_reason() {
        let b = happy_backend();
        let err = b
            .set_proxy_config(&ProxyConfigInfo {
                enabled: true,
                url: "http://proxy.corp:3128".to_string(),
                bypass: Vec::new(),
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.message.starts_with("no-privilege:"), "{}", err.message);
        assert!(!err.retryable);
    }

    /// Error injection over the two new seam-backed verbs, across all 20 codes:
    /// a fault on the profile read or the profile delete is carried unchanged.
    #[test]
    fn the_route_pass_verbs_carry_a_seam_fault_over_all_20_codes() {
        for code in ErrorCode::ALL {
            let mut profiles = alloc::vec![KnownWifiNetwork {
                ssid: "hem".to_string(),
                ..Default::default()
            }];
            profiles.push(KnownWifiNetwork {
                ssid: "aboard".to_string(),
                ..Default::default()
            });
            let b = backend(kernel(alloc::vec![]), Mock {
                profiles,
                fault: RefCell::new(Some(Error::new(code))),
                ..Default::default()
            });
            assert_eq!(
                b.list_known_wifi_networks(Pagination::default()).unwrap_err().code,
                code
            );
            assert_eq!(b.forget_wifi_network("hem").unwrap_err().code, code);
        }
    }

    /// A read-only live probe of the NetworkManager settings surface: the
    /// remembered-network set (`listKnownWifiNetworks`) and the host proxy file
    /// (`getProxyConfig`). Law #8: reads only (`GetSettings` returns no secrets
    /// and `ListConnections` changes nothing). `#[ignore]`d so the default
    /// hermetic suite never touches the bus; run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "read-only live probe against the real NetworkManager settings bus; run with `cargo test -- --ignored`"]
    fn live_reads_known_networks_and_proxy() {
        // The remembered-network set is NM's own: this is the read that proves
        // the `a{sa{sv}}` reply is marshalled correctly by the hand-rolled
        // client (the same class of bug the link-table probe caught).
        let client = match RealNmClient::new(None, None) {
            Ok(c) => c,
            Err(e) => panic!(
                "cannot reach NetworkManager on the system bus: {e} \
                 (reads only — nothing changed)"
            ),
        };
        let known = client.known_wifi_networks().expect("listKnownWifiNetworks read");
        std::println!(
            "live known networks: {:?}",
            known
                .iter()
                .map(|n| alloc::format!("{}({},auto={})", n.ssid, n.security, n.auto_connect))
                .collect::<Vec<_>>()
        );
        for network in &known {
            assert!(!network.ssid.is_empty(), "a remembered network has a name");
            assert!(!network.security.is_empty());
            // The critical assertion: no credential crossed.
            let printed = alloc::format!("{network:?}");
            for forbidden in ["psk", "password", "passphrase", "secret", "credential"] {
                assert!(!printed.contains(forbidden), "{forbidden} must not appear");
            }
        }
        // Every profile this host remembers, enumerated so the `ao` reply is
        // proven against the real bus (the empty-list bug this probe caught).
        let paths = client.list_connections().expect("ListConnections");
        std::println!("live profiles enumerated: {}", paths.len());
        assert!(
            paths.iter().all(|p| p.starts_with("/org/freedesktop/NetworkManager/Settings/")),
            "every enumerated path is an NM settings path"
        );
        // A profile the host does not remember is notFound, never a vacuous ok.
        match client.forget_wifi_network("smart-lib-probe-network-that-does-not-exist") {
            Err(e) => assert_eq!(e.code, ErrorCode::NotFound, "an unknown SSID is notFound"),
            Ok(()) => panic!("forgetting an unknown network must not succeed"),
        }

        // The host proxy is a file fact; read it the way the backend does.
        let proxy = read_host_proxy(Path::new(DEFAULT_PROXY_PATH));
        std::println!(
            "live proxy: enabled={} url={:?} bypass={:?}",
            proxy.enabled,
            proxy.url,
            proxy.bypass
        );
        assert_eq!(proxy.enabled, !proxy.url.is_empty());

        // A backend over the live kernel + live NM answers the whole new
        // surface read-only, which is exactly what the daemon will serve.
        let backend = LinuxNetworkBackend::connect().expect("live backend");
        let page = backend
            .get_routes(Pagination { offset: 0, limit: u32::MAX })
            .expect("getRoutes read");
        assert!(page.total >= 1, "the kernel always has at least the loopback route");
        assert!(
            page.items
                .iter()
                .any(|r| r.destination_cidr == "0.0.0.0/0" || r.destination_cidr == "::/0"),
            "the default route must be in the kernel's table"
        );
        let known_page = backend
            .list_known_wifi_networks(Pagination { offset: 0, limit: u32::MAX })
            .expect("listKnownWifiNetworks read");
        assert_eq!(known_page.total, known.len() as u64);
        std::println!(
            "live: {} routes, {} remembered networks, NM seam calls = {}",
            page.total,
            known_page.total,
            backend.nm_fallback_calls()
        );
    }

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
                .any(|r| r.destination == "0.0.0.0/0" || r.destination == "::/0"),
            "the kernel's default route must be in the store, in the contract's CIDR spelling"
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
        let mut live_kinds: Vec<String> = Vec::new();
        for iface in &page.items {
            let s = backend.get_status(iface.id).expect("getStatus read");
            // BUG 1's live lock: the status's type and the enumeration's type
            // come from the same kernel link record. The live verification
            // once saw getStatus type wlp0s20f3 as `ethernet` while
            // getInterfaces typed it `wifi`; this assertion is the
            // regression detector for that divergence.
            assert_eq!(
                s.kind, iface.kind,
                "getStatus typed {} as {:?} but getInterfaces typed it {:?} — \
                 the two paths disagree on the type's source",
                iface.name, s.kind, iface.kind
            );
            live_kinds.push(format!("{}({})", iface.name, s.kind));
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
            "live: netlink links = {:?}, status kinds = {:?}, NM seam calls = {}, store revision = {}",
            backend.store().links().iter().map(|l| format!("{}({})", l.name, l.kind_token())).collect::<Vec<_>>(),
            live_kinds,
            backend.nm_fallback_calls(),
            backend.store_revision()
        );
    }

    /// A read-only live probe of the two paths that must agree on a link's
    /// type: the SNAPSHOT (which `getInterfaces` enumerates from) and an
    /// RTM_NEWLINK EVENT (which the store applies in place). Read-only
    /// (`RTM_GETLINK` dump + a non-blocking event drain; the host is never
    /// mutated). `#[ignore]`d so the default hermetic suite never touches the
    /// host; run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "read-only live netlink probe: snapshot kind vs event-applied kind; run with `cargo test -- --ignored`"]
    fn live_snapshot_and_event_agree_on_a_links_kind() {
        use crate::platforms::linux::netlink::{KernelEvent, NetlinkReader, RealNetlinkReader};
        let reader = RealNetlinkReader::new();
        let snap = reader.snapshot().expect("the kernel answered RTM_GETLINK");
        std::println!(
            "live snapshot links: {:?}",
            snap.links
                .iter()
                .map(|l| alloc::format!("{}({})", l.name, l.kind_token()))
                .collect::<Vec<_>>()
        );
        // A read drains what the kernel already queued (the same bounded,
        // non-blocking drain the store performs) and applies it in place.
        let events = reader.drain_events(64).expect("a live non-blocking drain");
        std::println!(
            "live drained events: {:?}",
            events
                .iter()
                .map(|e| match e {
                    KernelEvent::Link(l) => alloc::format!("link {}({})", l.name, l.kind_token()),
                    KernelEvent::LinkGone(i) => alloc::format!("gone {i}"),
                    KernelEvent::Address(a) => alloc::format!("addr {} {}", a.index, a.address),
                    KernelEvent::AddressGone(a) => alloc::format!("addr-gone {} {}", a.index, a.address),
                })
                .collect::<Vec<_>>()
        );
        for link in &snap.links {
            let snapshot_kind = link.kind_token();
            for event in &events {
                if let KernelEvent::Link(event_link) = event {
                    if event_link.index == link.index && event_link.kind_token() != snapshot_kind {
                        panic!(
                            "type divergence on {}: the snapshot names it {snapshot_kind:?} but an \
                             RTM_NEWLINK event names it {:?} — the two reads of the same link disagree",
                            link.name,
                            event_link.kind_token()
                        );
                    }
                }
            }
        }
    }
}
