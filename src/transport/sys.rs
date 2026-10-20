//! Host primitives behind one narrow, audited seam.
//!
//! Everything in this module is a *declaration* of a host call plus the
//! smallest amount of logic needed to be honest about failure. Real
//! implementations are compiled only for Linux with the `std` feature;
//! every other configuration keeps the same signatures and answers
//! `TransportError::NotCompiled`, which the transports surface in their
//! `caps()` (a fabricated "available" here would be the unforgivable
//! defect).
//!
//! ## What is verified and what is not
//!
//! **Verified on this box** (Void Linux 6.18, glibc 2.41): the struct
//! layouts (`const` asserts below), `SCM_RIGHTS` fd passing, `SO_PEERCRED`,
//! `memfd_create` + `mmap` + `F_ADD_SEALS` — all exercised by the tests in
//! `src/transport/tests/unix.rs` and `src/transport/tests/shmem.rs`, run on
//! this machine.
//!
//! **Not verified here** (marked [U] in the report): macOS. XNU has
//! `SCM_RIGHTS` (POSIX ancillary data, per `02-HOST-CAPABILITIES.md` §4.1's
//! correction) but its `cmsghdr` alignment differs (4-byte vs Linux's
//! 8-byte `CMSG_ALIGN`), so this file deliberately does **not** compile an
//! unverifiable macOS path: shipping code that cannot be compiled on any
//! available target would be the dishonest option. Windows AF_UNIX +
//! SCM_RIGHTS support is likewise [U] and unimplemented.

#[cfg(all(target_os = "linux", feature = "std"))]
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ffi::{c_char, c_int};

#[cfg(all(target_os = "linux", feature = "std"))]
use core::ffi::{c_uint, c_void};

use crate::transport::channel::TransportError;

// This module compiles in every configuration (the stubs are the honest
// answer where the host calls are not compiled), but only its `std` blocks
// touch `std`, and the crate root is `no_std` (the skeleton agent's file),
// so the link is made here.
#[cfg(feature = "std")]
extern crate std;

/// Maximum file descriptors in one ancillary message. The host accepts 253
/// (`02-HOST-CAPABILITIES.md` §0, verified); 8 is this layer's declared
/// batch, sized for the control buffer below.
/// Maximum file descriptors in one ancillary message.
pub const ANCILLARY_FDS: usize = 8;

/// `AF_UNIX` address family.
pub const AF_UNIX: c_int = 1;
/// `SOCK_STREAM` socket type (framed by this layer).
pub const SOCK_STREAM: c_int = 1;
/// `SOL_SOCKET` option level.
pub const SOL_SOCKET: c_int = 1;
/// `SO_PEER_CRED`: the peer identity option (Linux).
pub const SO_PEERCRED: c_int = 17;
/// `SCM_RIGHTS`: the fd-passing ancillary type.
pub const SCM_RIGHTS: c_int = 1;
/// Do not raise SIGPIPE when the peer vanished.
/// Do not raise `SIGPIPE` when the peer vanished.
pub const MSG_NOSIGNAL: c_int = 0x4000;

/// `mmap` read protection.
pub const PROT_READ: c_int = 1;
/// `mmap` write protection.
pub const PROT_WRITE: c_int = 2;
/// `mmap` shared mapping.
pub const MAP_SHARED: c_int = 1;
/// `lseek` from the end (region size discovery).
pub const SEEK_END: c_int = 2;
/// `memfd_create` close-on-exec flag.
pub const MFD_CLOEXEC: c_int = 1;
/// `memfd_create` sealing-enabled flag.
pub const MFD_ALLOW_SEALING: c_int = 2;
/// `fcntl` add-seals command.
pub const F_ADD_SEALS: c_int = 1033;
/// `fcntl` get-seals command.
pub const F_GET_SEALS: c_int = 1034;
// Seal bits from the kernel's fcntl.h. These are load-bearing: getting
// F_SEAL_SEAL wrong (0x0010 is F_SEAL_FUTURE_WRITE, not F_SEAL_SEAL) blocks
// every *future writable mapping* of the region — which is precisely the
// peer transfer this layer exists to perform. Caught mechanically by
/// `tests/shmem.rs::memfd_two_mappings_are_the_same_pages`.
/// Seal bit: prevent further seals from being set.
pub const F_SEAL_SEAL: c_int = 0x0001;
/// Seal bit: prevent the file from shrinking.
pub const F_SEAL_SHRINK: c_int = 0x0002;
/// Seal bit: prevent the file from growing.
pub const F_SEAL_GROW: c_int = 0x0004;
/// The seal set every arena region is created with: the geometry cannot
/// change under a peer (SHRINK|GROW), the seal set itself is locked (SEAL),
/// and the region stays writable so peers can map it read-write.
/// The seal set every arena region is created with.
pub const ARENA_SEALS: c_int = F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL;

/// Linux `CMSG_SPACE(ANCILLARY_FDS * sizeof(i32))` = 16 + 32 = 48.
const CMSG_BUF_BYTES: usize = 48;
const _: () = assert!(CMSG_BUF_BYTES == 48, "ancillary buffer size");

/// The ancillary buffer, 8-byte aligned because a `Cmsghdr` is written
/// through a pointer into it (a misaligned write would be UB in Rust even
/// though the kernel accepts any alignment).
#[cfg(all(target_os = "linux", feature = "std"))]
#[repr(align(8))]
struct CmsgBuffer([u8; CMSG_BUF_BYTES]);

#[cfg(all(target_os = "linux", feature = "std"))]
impl CmsgBuffer {
    fn zeroed() -> Self {
        Self([0u8; CMSG_BUF_BYTES])
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

/// `struct sockaddr_un` (Linux).
#[repr(C)]
pub struct SockaddrUn {
    /// Address family (`AF_UNIX`).
    pub sun_family: u16,
    /// The path (or abstract name).
    pub sun_path: [u8; 108],
}

/// `struct iovec`.
#[repr(C)]
pub struct Iovec {
    /// The buffer.
    pub iov_base: *mut u8,
    /// The buffer length.
    pub iov_len: usize,
}

/// `struct msghdr` (Linux x86_64 ABI).
#[repr(C)]
pub struct Msghdr {
    /// Optional peer address.
    pub msg_name: *mut u8,
    /// Address length.
    pub msg_namelen: u32,
    /// Data buffers.
    pub msg_iov: *mut Iovec,
    /// Number of data buffers.
    pub msg_iovlen: usize,
    /// Ancillary data buffer.
    pub msg_control: *mut u8,
    /// Ancillary buffer length.
    pub msg_controllen: usize,
    /// Received message flags.
    pub msg_flags: c_int,
}

/// `struct cmsghdr` (Linux: `size_t cmsg_len`, `int level`, `int type`).
#[repr(C)]
pub struct Cmsghdr {
    /// Control message length (header + data).
    pub cmsg_len: usize,
    /// Originating protocol level.
    pub cmsg_level: c_int,
    /// Protocol-specific type (`SCM_RIGHTS` here).
    pub cmsg_type: c_int,
}

/// `struct ucred` (SO_PEERCRED value).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Ucred {
    /// Peer process id.
    pub pid: i32,
    /// Peer user id.
    pub uid: u32,
    /// Peer group id.
    pub gid: u32,
}

/// Compile-time layout guards. If any of these fail, the FFI layer is
/// wrong and must not be used — they are the mechanical half of trusting
/// hand-written declarations.
// Verified against the glibc header (/usr/include/bits/socket.h): msg_name
// 0..8, msg_namelen 8..12, msg_iov 16..24, msg_iovlen 24..32, msg_control
// 32..40, msg_controllen 40..48, msg_flags 48..52 → 56 bytes.
const _: () = assert!(core::mem::size_of::<Msghdr>() == 56, "msghdr layout");
const _: () = assert!(core::mem::size_of::<Cmsghdr>() == 16, "cmsghdr layout");
const _: () = assert!(core::mem::size_of::<Ucred>() == 12, "ucred layout");
const _: () = assert!(core::mem::align_of::<Msghdr>() == 8, "msghdr alignment");
const _: () = assert!(core::mem::size_of::<SockaddrUn>() == 110, "sockaddr_un layout");

#[cfg(all(target_os = "linux", feature = "std"))]
mod imp {
    use super::*;

extern "C" {
    pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    pub fn socketpair(domain: c_int, ty: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
    pub fn bind(fd: c_int, addr: *const SockaddrUn, len: u32) -> c_int;
    pub fn listen(fd: c_int, backlog: c_int) -> c_int;
    pub fn accept(fd: c_int, addr: *mut SockaddrUn, len: *mut u32) -> c_int;
    pub fn connect(fd: c_int, addr: *const SockaddrUn, len: u32) -> c_int;
    pub fn sendmsg(fd: c_int, msg: *const Msghdr, flags: c_int) -> isize;
    pub fn recvmsg(fd: c_int, msg: *mut Msghdr, flags: c_int) -> isize;
    pub fn close(fd: c_int) -> c_int;
    pub fn unlink(path: *const c_char) -> c_int;
    pub fn getsockopt(
        fd: c_int,
        level: c_int,
        optname: c_int,
        optval: *mut c_void,
        optlen: *mut u32,
    ) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn lseek(fd: c_int, off: i64, whence: c_int) -> i64;
    pub fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
    pub fn ftruncate(fd: c_int, len: i64) -> c_int;
    pub fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        off: i64,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
}
}

#[cfg(all(target_os = "linux", feature = "std"))]
use imp::*;

/// The current errno (std-gated: `std::io::Error::last_os_error`).
#[cfg(all(target_os = "linux", feature = "std"))]
pub fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

#[cfg(all(target_os = "linux", feature = "std"))]
fn host(errno: i32, detail: impl Into<String>) -> TransportError {
    TransportError::Host {
        errno,
        detail: detail.into(),
    }
}

// Only reachable in the non-Linux / non-std configurations; on this box's
// build the real implementations take every call path.
#[allow(dead_code)]
fn not_compiled(scheme: &'static str) -> TransportError {
    TransportError::NotCompiled { scheme }
}

// ─── sockets ────────────────────────────────────────────────────────────

/// `socket(AF_UNIX, SOCK_STREAM, 0)`.
/// `socket(AF_UNIX, SOCK_STREAM, 0)`.
pub fn unix_socket() -> Result<i32, TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        // SAFETY: constant arguments; no pointers involved.
        let fd = unsafe { socket(AF_UNIX, SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(host(errno(), "socket(AF_UNIX)"));
        }
        Ok(fd)
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// `socketpair(AF_UNIX, SOCK_STREAM, 0)` — the connected pair used by the
/// tests and by the bootstrap link.
/// `socketpair(AF_UNIX, SOCK_STREAM, 0)` — a connected pair.
pub fn unix_socketpair() -> Result<(i32, i32), TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let mut fds = [0 as c_int; 2];
        // SAFETY: `fds` is a valid two-element output buffer.
        let rc = unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, fds.as_mut_ptr()) };
        if rc != 0 {
            return Err(host(errno(), "socketpair(AF_UNIX)"));
        }
        Ok((fds[0], fds[1]))
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// Build a `sockaddr_un` from a path plus the exact name length the kernel
/// wants. A leading NUL byte denotes the Linux abstract namespace; the
/// 107-byte limit is enforced here with a typed error rather than an
/// EINVAL from the kernel (`02-HOST-CAPABILITIES.md` §0).
pub fn sockaddr(path: &str) -> Result<(SockaddrUn, usize), TransportError> {
    let bytes = path.as_bytes();
    let abstract_ns = bytes.first() == Some(&0);
    let name: &[u8] = if abstract_ns { &bytes[1..] } else { bytes };
    if name.len() > 107 {
        return Err(TransportError::Connect {
            detail: alloc::format!(
                "unix path {name_len} bytes exceeds the 107-byte sun_path limit",
                name_len = name.len()
            ),
        });
    }
    let mut addr = SockaddrUn {
        sun_family: AF_UNIX as u16,
        sun_path: [0; 108],
    };
    addr.sun_path[..name.len()].copy_from_slice(name);
    // Abstract names keep their leading NUL: the length is the name length,
    // and the family field is 2 bytes, so `2 + name.len()`.
    Ok((addr, 2 + name.len()))
}

/// `bind` + `listen`; unlinks a stale *path* socket first.
/// `bind` + `listen`; unlinks a stale *path* socket first.
pub fn unix_bind_listen(fd: i32, path: &str) -> Result<(), TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let (addr, name_len) = sockaddr(path)?;
        let abstract_ns = path.as_bytes().first() == Some(&0);
        if !abstract_ns {
            if let Ok(cpath) = alloc::ffi::CString::new(path) {
                // Best effort: a stale path socket must not block a restart.
                // SAFETY: NUL-terminated borrow of a live allocation.
                unsafe { unlink(cpath.as_ptr()) };
            }
        }
        // SAFETY: `addr` outlives the call; the length covers family + name.
        let rc = unsafe { bind(fd, &addr, name_len as u32) };
        if rc != 0 {
            return Err(host(errno(), alloc::format!("bind({path})")));
        }
        let rc = unsafe { listen(fd, 64) };
        if rc != 0 {
            return Err(host(errno(), alloc::format!("listen({path})")));
        }
        Ok(())
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = (fd, path);
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// `connect` a stream socket to a path (or abstract name).
pub fn unix_connect(fd: i32, path: &str) -> Result<(), TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let (addr, name_len) = sockaddr(path)?;
        // SAFETY: `addr` outlives the call.
        let rc = unsafe { connect(fd, &addr, name_len as u32) };
        if rc != 0 {
            return Err(host(errno(), alloc::format!("connect({path})")));
        }
        Ok(())
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = (fd, path);
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// `accept` one connection from a listening socket.
pub fn unix_accept(fd: i32) -> Result<i32, TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        // SAFETY: a null address is allowed for `accept`.
        let new = unsafe { accept(fd, core::ptr::null_mut(), core::ptr::null_mut()) };
        if new < 0 {
            return Err(host(errno(), "accept"));
        }
        Ok(new)
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = fd;
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// Remove a path socket file (listener teardown). No-op elsewhere.
///
/// # Safety
/// `path` must be a valid NUL-terminated C string.
pub unsafe fn unlink_path(path: *const c_char) {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let _ = unlink(path);
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = path;
    }
}

/// Close a descriptor this layer owns.
pub fn close_fd(fd: i32) {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        // SAFETY: `fd` is owned by the caller.
        unsafe {
            let _ = close(fd);
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = fd;
    }
}

/// `sendmsg` one buffer, optionally with `fds` in the ancillary channel.
pub fn send_with_fds(fd: i32, bytes: &[u8], fds: &[i32]) -> Result<usize, TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let mut iov = Iovec {
            iov_base: bytes.as_ptr() as *mut u8,
            iov_len: bytes.len(),
        };
        let mut cmsg_buf = CmsgBuffer::zeroed();
        let (control, controllen) = if fds.is_empty() {
            (core::ptr::null_mut(), 0usize)
        } else {
            if fds.len() > ANCILLARY_FDS {
                return Err(TransportError::LimitExceeded {
                    limit: ANCILLARY_FDS as u32,
                });
            }
            let space = cmsg_space(fds.len());
            fill_cmsg_rights(cmsg_buf.as_bytes_mut(), fds);
            (cmsg_buf.0.as_mut_ptr(), space)
        };
        let msg = Msghdr {
            msg_name: core::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            msg_control: control,
            msg_controllen: controllen,
            msg_flags: 0,
        };
        // SAFETY: all pointers are live for the call; `MSG_NOSIGNAL` keeps a
        // vanished peer from killing the process.
        let sent = unsafe { sendmsg(fd, &msg, MSG_NOSIGNAL) };
        if sent < 0 {
            return Err(host(errno(), "sendmsg"));
        }
        Ok(sent as usize)
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = (fd, bytes, fds);
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// `recvmsg` up to `buf.len()` bytes plus up to `ANCILLARY_FDS` passed fds.
pub fn recv_with_fds(fd: i32, buf: &mut [u8]) -> Result<(usize, Vec<i32>), TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let mut iov = Iovec {
            iov_base: buf.as_mut_ptr(),
            iov_len: buf.len(),
        };
        let mut cmsg_buf = CmsgBuffer::zeroed();
        let mut msg = Msghdr {
            msg_name: core::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            msg_control: cmsg_buf.0.as_mut_ptr(),
            msg_controllen: CMSG_BUF_BYTES,
            msg_flags: 0,
        };
        // SAFETY: `buf` and the control buffer outlive the call.
        let got = unsafe { recvmsg(fd, &mut msg, 0) };
        if got < 0 {
            return Err(host(errno(), "recvmsg"));
        }
        let mut fds = Vec::new();
        if msg.msg_controllen >= core::mem::size_of::<Cmsghdr>() {
            // SAFETY: the kernel wrote a well-formed control buffer.
            unsafe { collect_cmsg_rights(&msg, &mut fds) };
        }
        Ok((got as usize, fds))
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = (fd, buf);
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

/// `SO_PEERCRED`: the peer's `(pid, uid, gid)`. This is the peer identity
/// the bootstrap authenticates against and that backends map to authority
/// (02-HOST-CAPABILITIES §3.1 step 1).
pub fn peer_credential_triple(fd: i32) -> Result<(i32, u32, u32), TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let mut cred = Ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = core::mem::size_of::<Ucred>() as u32;
        // SAFETY: `cred` is a valid output buffer of exactly `len` bytes.
        let rc = unsafe {
            getsockopt(
                fd,
                SOL_SOCKET,
                SO_PEERCRED,
                &mut cred as *mut Ucred as *mut c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(host(errno(), "getsockopt(SO_PEERCRED)"));
        }
        Ok((cred.pid, cred.uid, cred.gid))
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = fd;
        Err(not_compiled(
            crate::transport::registry::UNIX_SCHEME,
        ))
    }
}

// ─── control-message helpers (Linux cmsghdr arithmetic) ─────────────────

/// `CMSG_ALIGN` for Linux: align to `size_t`.
#[cfg(all(target_os = "linux", feature = "std"))]
#[cfg(all(target_os = "linux", feature = "std"))]
fn cmsg_align(len: usize) -> usize {
    let a = core::mem::size_of::<usize>();
    (len + a - 1) & !(a - 1)
}

/// `CMSG_SPACE(n_fds)` for Linux.
#[cfg(all(target_os = "linux", feature = "std"))]
fn cmsg_space(n_fds: usize) -> usize {
    cmsg_align(core::mem::size_of::<Cmsghdr>()) + cmsg_align(n_fds * core::mem::size_of::<i32>())
}

/// Write an `SCM_RIGHTS` control message carrying `fds`.
#[cfg(all(target_os = "linux", feature = "std"))]
fn fill_cmsg_rights(buf: &mut [u8], fds: &[i32]) {
    let space = cmsg_space(fds.len());
    debug_assert!(buf.len() >= space);
    let hdr = buf.as_mut_ptr() as *mut Cmsghdr;
    // SAFETY: `buf` is the 8-byte-aligned `CmsgBuffer`, zeroed and large
    // enough for the computed space.
    unsafe {
        (*hdr).cmsg_len = core::mem::size_of::<Cmsghdr>() + fds.len() * core::mem::size_of::<i32>();
        (*hdr).cmsg_level = SOL_SOCKET;
        (*hdr).cmsg_type = SCM_RIGHTS;
        let data = cmsg_data_ptr(hdr);
        for (i, fd) in fds.iter().enumerate() {
            data.add(i).write(*fd);
        }
    }
}

/// `CMSG_DATA`: header address + aligned header size.
#[cfg(all(target_os = "linux", feature = "std"))]
unsafe fn cmsg_data_ptr(hdr: *mut Cmsghdr) -> *mut i32 {
    (hdr as *mut u8).add(cmsg_align(core::mem::size_of::<Cmsghdr>())) as *mut i32
}

/// Extract every `SCM_RIGHTS` fd from a received `msghdr`'s control buffer.
#[cfg(all(target_os = "linux", feature = "std"))]
unsafe fn collect_cmsg_rights(msg: &Msghdr, out: &mut Vec<i32>) {
    let mut cmsg = msg.msg_control as *mut Cmsghdr;
    let end = msg.msg_control.add(msg.msg_controllen) as *mut Cmsghdr;
    while !cmsg.is_null() && (cmsg as usize) < (end as usize) {
        if (*cmsg).cmsg_level == SOL_SOCKET && (*cmsg).cmsg_type == SCM_RIGHTS {
            let data = cmsg_data_ptr(cmsg);
            let bytes = (*cmsg).cmsg_len - core::mem::size_of::<Cmsghdr>();
            let count = bytes / core::mem::size_of::<i32>();
            for i in 0..count {
                out.push(*data.add(i));
            }
        }
        let next = (cmsg as *mut u8).add(cmsg_align((*cmsg).cmsg_len)) as *mut Cmsghdr;
        if next <= cmsg {
            break;
        }
        cmsg = next;
    }
}

// ─── shared regions (memfd + mmap) ──────────────────────────────────────

/// A mapped shared region plus the fd that backs it.
pub struct SharedRegion {
    /// The region's descriptor.
    pub fd: i32,
    /// The mapping's base address.
    pub ptr: *mut u8,
    /// The mapping's length in bytes.
    pub len: usize,
}

// SAFETY: the raw pointer describes a mapping owned solely by this value.
unsafe impl Send for SharedRegion {}
unsafe impl Sync for SharedRegion {}

/// Create a sealed, sized, mapped region (`memfd_create` + `ftruncate` +
/// `mmap` + `F_ADD_SEALS`). This is the capability a bootstrap passes: an
/// anonymous memfd cannot be opened by name, so the fd *is* the authority
/// over the arena.
pub fn create_shared_region(len: usize) -> Result<SharedRegion, TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        if len == 0 || len % 8 != 0 {
            return Err(TransportError::Framing {
                detail: alloc::format!("region size {len} is not a valid 8-aligned size"),
            });
        }
        let name = b"smart-lib-arena\0";
        // SAFETY: NUL-terminated constant name.
        let fd = unsafe {
            memfd_create(
                name.as_ptr() as *const c_char,
                (MFD_CLOEXEC | MFD_ALLOW_SEALING) as c_uint,
            )
        };
        if fd < 0 {
            return Err(host(errno(), "memfd_create"));
        }
        // SAFETY: `fd` is fresh and owned here.
        if unsafe { ftruncate(fd, len as i64) } != 0 {
            let e = errno();
            // SAFETY: owned fd.
            unsafe { close(fd) };
            return Err(host(e, "ftruncate"));
        }
        let ptr = unsafe {
            mmap(
                core::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr as isize == -1 {
            let e = errno();
            // SAFETY: owned fd.
            unsafe { close(fd) };
            return Err(host(e, "mmap"));
        }
        // Sealing failure is not fatal for the mapping, but it must be
        // declared: `seals_word()` reports the truth either way.
        // SAFETY: `fd` is owned; variadic call with one integer argument.
        unsafe { fcntl(fd, F_ADD_SEALS, ARENA_SEALS) };
        Ok(SharedRegion {
            fd,
            ptr: ptr as *mut u8,
            len,
        })
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = len;
        Err(not_compiled(
            crate::transport::registry::SHMEM_SCHEME,
        ))
    }
}

/// Map a region another process passed (bootstrap, peer side).
pub fn adopt_shared_region(fd: i32) -> Result<SharedRegion, TransportError> {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        if fd < 0 {
            return Err(TransportError::Framing {
                detail: "region fd is not valid".to_string(),
            });
        }
        // SAFETY: `fd` was received over SCM_RIGHTS and is owned here.
        let len = unsafe { lseek(fd, 0, SEEK_END) };
        if len <= 0 || len % 8 != 0 {
            return Err(host(errno(), "lseek(SEEK_END) on region fd"));
        }
        let ptr = unsafe {
            mmap(
                core::ptr::null_mut(),
                len as usize,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr as isize == -1 {
            return Err(host(errno(), "mmap of adopted region"));
        }
        Ok(SharedRegion {
            fd,
            ptr: ptr as *mut u8,
            len: len as usize,
        })
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = fd;
        Err(not_compiled(
            crate::transport::registry::SHMEM_SCHEME,
        ))
    }
}

/// Release a region: unmap, and close the fd we own.
pub fn release_shared_region(region: &SharedRegion) {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        // SAFETY: `ptr`/`len` came from `mmap`; this runs once, on drop.
        unsafe {
            let _ = munmap(region.ptr as *mut c_void, region.len);
            let _ = close(region.fd);
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        let _ = region;
    }
}

/// The seal set this host can actually enforce (0 when sealing is
/// unavailable — declared, never fabricated). The probe adds the arena's
/// seal set to a throwaway memfd and reads it back, so the number is the
/// *enforced* set, not an aspiration.
pub fn memory_seals() -> u32 {
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        let name = b"smart-lib-seals\0";
        // SAFETY: NUL-terminated constant name.
        let fd = unsafe {
            memfd_create(
                name.as_ptr() as *const c_char,
                (MFD_CLOEXEC | MFD_ALLOW_SEALING) as c_uint,
            )
        };
        if fd < 0 {
            return 0;
        }
        // SAFETY: `fd` is fresh and owned here; variadic call with one
        // integer argument.
        let added = unsafe { fcntl(fd, F_ADD_SEALS, ARENA_SEALS) };
        // SAFETY: `fd` is fresh and owned here; `F_GET_SEALS` returns the
        // seal set directly.
        let seals = if added == 0 {
            unsafe { fcntl(fd, F_GET_SEALS) }
        } else {
            -1
        };
        // SAFETY: owned fd.
        unsafe { close(fd) };
        if seals < 0 {
            0
        } else {
            seals as u32
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "std")))]
    {
        0
    }
}
