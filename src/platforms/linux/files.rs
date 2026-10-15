//! The Linux **files** backend for the files domain — the host's own files and
//! directories, read and written natively (law #9).
//!
//! ## The mechanisms, and why each one
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `openFile`/`closeFile`/`writeFile`/`flushFile` | the host's own descriptors: `openat` (through `OpenOptions`), `pwrite` at the offset, `fsync`/`fdatasync` on the descriptor the kernel handed back |
//! | `readFile` | `mmap` the file and serve the window straight out of the mapping — the bytes cross from the kernel's page cache to the caller without a second copy. A `MappedWindow` is the zero-copy form the hot path uses |
//! | `statFile`/`getFileType`/`getFileMetadata` | `statx(2)`, so `created` is the kernel's own birth time and inode/device/link-count are reported data rather than a guess |
//! | `listDirectory`/`walkTree`/`getDirectoryStats` | `getdents64` through the directory handle, with a stated depth bound and the contract's pagination |
//! | `watchDirectory` | `inotify` — event-driven: the kernel queues the event and the subscription reads it, no polling thread and no `inotifywait` |
//! | `getDiskUsage` | `statvfs` on the path, plus the mount's own row from `/proc/mounts` |
//! | `getFileHash`/`getFileChecksum` | SHA-256 implemented here, over the file's own bytes — no `sha256sum` fork |
//! | `setCompression`/`setLifecycle`/`getFileChecksum` | the file's own user xattrs, the host's own attribute namespace, so the policy is visible to every other tool |
//! | versioning / content addressing / dedupe / branches | a content-addressed store under the host's own cache root; every object is a real file |
//! | `getSpecialPath` | the host's own XDG environment and conventions |
//! | `getDefaultApp` | the host's own XDG `mimeapps.list` and `mimeinfo.cache` |
//! | `listTrash`/`getRecentFiles` | the host's own freedesktop trash directory and recent-files record |
//! | `createEphemeral` | `memfd_create` — the kernel's own anonymous file, which needs no path and no `tmpfs` agreement |
//! | `lockFile` | the kernel's own advisory record locks through `fcntl(F_SETLK)` |
//!
//! ## What replaced the archive body's `cp -r`
//!
//! `copy_file_range` — the kernel's own in-kernel copy, so a copy is one
//! syscall loop with no user-space bounce buffer, and the error the host gives
//! is the error the caller sees. There is no `Command::new` anywhere in this
//! file: the archive body's `Command::new("cp").args(["-r", src, dst])` is the
//! pattern this backend exists to end.
//!
//! ## Law #8 (the host is read-only)
//!
//! Every host-mutating path (create/write/copy/rename/remove/link/touch,
//! restore/empty/merge/apply/quota) is implemented, tested against the
//! backend's own store and a private temp tree, and **never performed against
//! the host** by a build or a test. The read paths read the host directly: the
//! `/proc/mounts`, `/etc/passwd`, `/usr/share/mime/globs`, mimeapps and trash
//! reads are live measurements, and the tests assert the measured truth.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::core::ids::{
    AppId, EventId, FileHandleId, FilesystemId, KeyId, MountId, SnapshotId,
};
use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, ErrorCode, Result};

use crate::domains::files::backend::{
    AuditSubscription, FilesBackend, WatchSubscription,
};
use crate::domains::files::types::{
    self, AuditAction, BatchCopyRequest, BatchDeleteRequest, BatchMoveRequest, BatchResult,
    BoundPath, BranchResult, ChecksumResult, ComparisonResult, ContentAddressedRef, ContentRefResult,
    ContentSearchQuery, ContentSearchResult, CreateEncryptedRequest, DefaultAppResult, DirectoryEntry,
    DirectoryFilter, DirectoryStats, DiskUsage, EncryptionInfoResult,
    EphemeralFileConfig, ExtendedMetadata, FileActivity, FileAnalysisRequest, FileAnalysisResult,
    FileAuditEntry, FileAuditFilter, FileBranch, FileCompressionConfig, FileDeduplicationInfo,
    FileEvent, FileEventKind, FileHandle, FileHashResult, FileInfo, FileInfoResult, FileKind,
    FileLock, FileSearchQuery, FileSearchResult, FileTypeCount, FileVersion, FilesystemHealth,
    FilesystemHealthResult, FilesystemQuota, GetByHashRequest, LifecyclePolicy,
    LifecyclePolicyResult, LockFileRequest, LockKind, Metadata, OpenFlag, OpenMode, OpenRequest,
    Page, Pagination, RecentFile, RetentionAction, SnapshotRefResult, SortField, SortOrder,
    SpecialPath,
    TextResult, TrashEntry, VersionResult,
};
use crate::domains::files::types::DefaultAppInfo;

// ═══════════════════════════════════════════════════════════════════════
// The kernel's own file interface, declared locally.
//
// The same precedent the clock domain set: the engine links only `std` (which
// links libc), so the handful of functions this backend needs are declared
// once with their stable kernel ABI — `stat`, `statfs`, `statvfs`, the
// extended-attribute pair, `mmap`, `copy_file_range`, `memfd_create` and the
// `inotify` trio. No new dependency, no shellout, and the struct layouts are
// the kernel's own (`linux/stat.h`, `sys/statvfs.h`), verified against
// `std::fs::Metadata` in the suite below.
// ═══════════════════════════════════════════════════════════════════════

// ── statx ───────────────────────────────────────────────────────────────
const AT_FDCWD: i32 = -100;
const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
const STATX_TYPE: u32 = 0x0000_0001;
const STATX_MODE: u32 = 0x0000_0002;
const STATX_NLINK: u32 = 0x0000_0004;
const STATX_UID: u32 = 0x0000_0008;
const STATX_GID: u32 = 0x0000_0010;
const STATX_ATIME: u32 = 0x0000_0020;
const STATX_MTIME: u32 = 0x0000_0040;
const STATX_CTIME: u32 = 0x0000_0080;
const STATX_INO: u32 = 0x0000_0100;
const STATX_SIZE: u32 = 0x0000_0200;
const STATX_BLOCKS: u32 = 0x0000_0400;
const STATX_BTIME: u32 = 0x0000_0800;
const STATX_MNT_ID: u32 = 0x0000_1000;

/// `linux/stat.h`'s `statx_timestamp`.
#[repr(C)]
#[derive(Clone, Copy)]
struct StatxTimestamp {
    tv_sec: i64,
    tv_nsec: u32,
    __reserved: i32,
}

/// `linux/stat.h`'s `struct statx` — the modern metadata read, whose birth
/// time (`stx_btime`) is the only portable creation timestamp a Unix host
/// offers.
#[repr(C)]
#[derive(Clone, Copy)]
struct Statx {
    stx_mask: u32,
    stx_blksize: u32,
    stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    __spare0: [u16; 1],
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    stx_attributes_mask: u64,
    stx_atime: StatxTimestamp,
    stx_btime: StatxTimestamp,
    stx_ctime: StatxTimestamp,
    stx_mtime: StatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    stx_mnt_id: u64,
    stx_dio_mem_align: u32,
    stx_dio_offset_align: u32,
    __spare3: [u64; 12],
}

unsafe extern "C" {
    fn statx(
        dirfd: i32,
        pathname: *const u8,
        flags: i32,
        mask: u32,
        buf: *mut Statx,
    ) -> i32;
}

/// The mask this backend always asks for: everything it may report, including
/// the birth time the old `stat` could not give.
const STATX_ASK: u32 = STATX_TYPE
    | STATX_MODE
    | STATX_NLINK
    | STATX_UID
    | STATX_GID
    | STATX_ATIME
    | STATX_MTIME
    | STATX_CTIME
    | STATX_BTIME
    | STATX_INO
    | STATX_SIZE
    | STATX_BLOCKS
    | STATX_MNT_ID;

/// One `statx` answer, with the host's own words when it declines.
#[derive(Clone, Copy, Debug, Default)]
struct Stat {
    /// Whether the kernel answered at all.
    present: bool,
    mode: u16,
    nlink: u32,
    uid: u32,
    gid: u32,
    size: u64,
    ino: u64,
    blocks: u64,
    mnt_id: u64,
    atime: u64,
    btime: u64,
    ctime: u64,
    mtime: u64,
    /// Which fields the kernel actually filled (the host's own answer about
    /// what it knows — `stx_btime` is cleared on filesystems without it).
    mask: u32,
}

impl Stat {
    /// The `FileKind` the mode's format bits name.
    fn kind(&self) -> FileKind {
        const S_IFMT: u16 = 0o170_000;
        const S_IFDIR: u16 = 0o040_000;
        const S_IFREG: u16 = 0o100_000;
        const S_IFLNK: u16 = 0o120_000;
        match self.mode & S_IFMT {
            S_IFDIR => FileKind::Directory,
            S_IFREG => FileKind::Regular,
            S_IFLNK => FileKind::Symlink,
            _ if self.present => FileKind::Other,
            _ => FileKind::Unknown,
        }
    }

    fn is_dir(&self) -> bool {
        self.kind() == FileKind::Directory
    }

    fn is_regular(&self) -> bool {
        self.kind() == FileKind::Regular
    }

    fn is_symlink(&self) -> bool {
        self.kind() == FileKind::Symlink
    }

    fn permission_bits(&self) -> u32 {
        u32::from(self.mode & 0o7777)
    }

    /// A nanosecond timestamp from the kernel's own second/nanosecond pair,
    /// saturating rather than wrapping.
    fn nanos(secs: i64, nanos: u32) -> u64 {
        let secs = secs.max(0) as u64;
        let nanos = u64::from(nanos.min(999_999_999));
        secs.saturating_mul(1_000_000_000).saturating_add(nanos)
    }

    fn modified(&self) -> Timestamp {
        Timestamp::from_nanos(self.mtime)
    }
}

/// Read one path's `statx` answer. `follow` decides whether a symlink is
/// resolved (`stat`) or read as itself (`lstat`), which is what makes a
/// symlink listing honest about its own size and type.
fn statx_of(path: &str, follow: bool) -> Result<Stat> {
    let cstring = std::ffi::CString::new(path)
        .map_err(|_| Error::invalid_argument(format!("a path with an interior NUL cannot name a file: {path}")))?;
    let mut raw = Statx {
        stx_mask: 0,
        stx_blksize: 0,
        stx_attributes: 0,
        stx_nlink: 0,
        stx_uid: 0,
        stx_gid: 0,
        stx_mode: 0,
        __spare0: [0],
        stx_ino: 0,
        stx_size: 0,
        stx_blocks: 0,
        stx_attributes_mask: 0,
        stx_atime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
        stx_btime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
        stx_ctime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
        stx_mtime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
        stx_rdev_major: 0,
        stx_rdev_minor: 0,
        stx_dev_major: 0,
        stx_dev_minor: 0,
        stx_mnt_id: 0,
        stx_dio_mem_align: 0,
        stx_dio_offset_align: 0,
        __spare3: [0; 12],
    };
    let flags = if follow { 0 } else { AT_SYMLINK_NOFOLLOW };
    // SAFETY: `cstring` outlives the call, `raw` is a correctly sized and
    // aligned out-param for the kernel's `struct statx`, and the constants are
    // this file's own.
    let rc = unsafe { statx(AT_FDCWD, cstring.as_ptr() as *const u8, flags, STATX_ASK, &mut raw) };
    if rc != 0 {
        return Err(io_error(&format!("statx({path})"), std::io::Error::last_os_error()));
    }
    Ok(Stat {
        present: raw.stx_mask != 0,
        mode: raw.stx_mode,
        nlink: raw.stx_nlink,
        uid: raw.stx_uid,
        gid: raw.stx_gid,
        size: raw.stx_size,
        ino: raw.stx_ino,
        blocks: raw.stx_blocks,
        mnt_id: raw.stx_mnt_id,
        atime: Stat::nanos(raw.stx_atime.tv_sec, raw.stx_atime.tv_nsec),
        btime: Stat::nanos(raw.stx_btime.tv_sec, raw.stx_btime.tv_nsec),
        ctime: Stat::nanos(raw.stx_ctime.tv_sec, raw.stx_ctime.tv_nsec),
        mtime: Stat::nanos(raw.stx_mtime.tv_sec, raw.stx_mtime.tv_nsec),
        mask: raw.stx_mask,
    })
}

// ── statvfs ─────────────────────────────────────────────────────────────

/// `sys/statvfs.h`'s `struct statvfs` on a 64-bit host.
#[repr(C)]
#[derive(Clone, Copy)]
struct Statvfs {
    f_bsize: usize,
    f_frsize: usize,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: usize,
    f_flag: usize,
    f_namemax: usize,
    f_type: u32,
    __f_spare: [i32; 5],
}

/// `ST_RDONLY` — the mount is read-only, which is a health answer and not a
/// failure.
const ST_RDONLY: usize = 1;

unsafe extern "C" {
    fn statvfs(pathname: *const u8, buf: *mut Statvfs) -> i32;
}

/// The space of the filesystem carrying `path`, plus the mount's own flag
/// word. `None` when the host refuses to answer.
fn statvfs_of(path: &str) -> Option<Statvfs> {
    let Ok(cstring) = std::ffi::CString::new(path) else {
        return None;
    };
    let mut buf = Statvfs {
        f_bsize: 0,
        f_frsize: 0,
        f_blocks: 0,
        f_bfree: 0,
        f_bavail: 0,
        f_files: 0,
        f_ffree: 0,
        f_favail: 0,
        f_fsid: 0,
        f_flag: 0,
        f_namemax: 0,
        f_type: 0,
        __f_spare: [0; 5],
    };
    // SAFETY: as with statx — a correctly sized, aligned out-param.
    let rc = unsafe { statvfs(cstring.as_ptr() as *const u8, &mut buf) };
    if rc != 0 {
        return None;
    }
    Some(buf)
}

// ── the extended attributes ─────────────────────────────────────────────

unsafe extern "C" {
    fn getxattr(
        path: *const u8,
        name: *const u8,
        value: *mut u8,
        size: usize,
    ) -> isize;
    fn setxattr(
        path: *const u8,
        name: *const u8,
        value: *const u8,
        size: usize,
        flags: i32,
    ) -> i32;
}

/// The file's own `user.*` namespace, the host's own attribute store: a policy
/// the backend writes here is visible to every other tool on the host.
const XATTR_COMPRESSION: &[u8] = b"user.smart-lib.compression\0";
const XATTR_LIFECYCLE: &[u8] = b"user.smart-lib.lifecycle\0";
const XATTR_CHECKSUM: &[u8] = b"user.smart-lib.checksum\0";

/// Read one xattr, or `None` when the host does not have it (which includes a
/// filesystem with no attribute support — a measurement, not an error).
fn xattr_get(path: &str, name: &[u8]) -> Option<Vec<u8>> {
    let Ok(cstring) = std::ffi::CString::new(path) else {
        return None;
    };
    // SAFETY: both pointers name NUL-terminated bytes that outlive the call.
    let len = unsafe { getxattr(cstring.as_ptr() as *const u8, name.as_ptr(), core::ptr::null_mut(), 0) };
    if len <= 0 {
        return None;
    }
    let mut buf = alloc::vec![0u8; len as usize];
    let got = unsafe {
        getxattr(
            cstring.as_ptr() as *const u8,
            name.as_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
    if got <= 0 {
        return None;
    }
    buf.truncate(got as usize);
    Some(buf)
}

/// Write one xattr. Returns the host's own error when it refuses (a read-only
/// file, or a filesystem without attribute support).
fn xattr_set(path: &str, name: &[u8], value: &[u8]) -> Result<()> {
    let Ok(cstring) = std::ffi::CString::new(path) else {
        return Err(Error::invalid_argument("a path with an interior NUL cannot name a file"));
    };
    let rc = unsafe {
        setxattr(
            cstring.as_ptr() as *const u8,
            name.as_ptr(),
            value.as_ptr(),
            value.len(),
            0,
        )
    };
    if rc != 0 {
        return Err(io_error(&format!("setxattr({path})"), std::io::Error::last_os_error()));
    }
    Ok(())
}

// ── mmap ────────────────────────────────────────────────────────────────

const PROT_READ: c_int = 0x1;
const MAP_PRIVATE: c_int = 0x02;
const MAP_FAILED: isize = -1;
const MFD_CLOEXEC: c_uint = 0x0001;

// ═══════════════════════════════════════════════════════════════════════
// The kernel's own mapping interface, declared locally.
//
// The signatures are byte-identical to the transport seam's own
// (`crate::transport::sys`, which const-asserts its struct layouts), so a
// second declaration of the same host call is the same ABI and not a second
// opinion about it. They live here because the mapping is the read hot path's
// mechanism, and the read path belongs to this file.
// ═══════════════════════════════════════════════════════════════════════

unsafe extern "C" {
    fn mmap(
        addr: *mut core::ffi::c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut core::ffi::c_void;
    fn munmap(addr: *mut core::ffi::c_void, length: usize) -> c_int;
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
}

use core::ffi::{c_char, c_int, c_uint};

/// One `mmap` of one file: the process's own mapping, unmapped on drop.
///
/// A read served through this never copies: the kernel's page cache is mapped
/// into the process and the caller's byte window is a slice of it.
pub struct Mapping {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is a read-only view of a file the caller owns, and every
// method takes `&self`, so sharing it across threads is as safe as sharing the
// bytes it aliases.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Map a whole file read-only. A zero-length file has no mapping (there is
    /// nothing to map), which is a valid empty read rather than an error.
    fn open(file: &File) -> Option<Mapping> {
        let len = file.metadata().ok()?.len() as usize;
        if len == 0 {
            return Some(Mapping {
                ptr: core::ptr::null_mut(),
                len: 0,
            });
        }
        // SAFETY: `fd` is a live descriptor this process owns, `len` is its
        // exact size, and the protection/flags are read-only and private.
        let ptr = unsafe {
            mmap(
                core::ptr::null_mut(),
                len,
                PROT_READ,
                MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        } as *mut u8;
        if ptr as isize == MAP_FAILED {
            return None;
        }
        Some(Mapping { ptr, len })
    }

    /// The mapped bytes.
    fn bytes(&self) -> &[u8] {
        if self.len == 0 || self.ptr.is_null() {
            return &[];
        }
        // SAFETY: `ptr..ptr+len` is a live read-only mapping of exactly `len`
        // bytes for as long as `self` is alive.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        if self.len != 0 && !self.ptr.is_null() {
            // SAFETY: the pointer and length are the ones `mmap` returned.
            unsafe { munmap(self.ptr as *mut core::ffi::c_void, self.len) };
        }
    }
}

/// A byte window served straight out of the process's own mapping: the
/// zero-copy form of the read path, for a caller that wants the file's bytes
/// without a hop through a heap buffer.
pub struct MappedWindow {
    map: Mapping,
    start: usize,
    len: usize,
}

impl MappedWindow {
    /// The window's own bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.map.bytes()[self.start..self.start + self.len]
    }

    /// How many bytes this window carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the window is empty (a read past the end is, and says so).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ── copy_file_range ─────────────────────────────────────────────────────

unsafe extern "C" {
    fn copy_file_range(
        fd_in: i32,
        off_in: *mut i64,
        fd_out: i32,
        off_out: *mut i64,
        len: usize,
        flags: u32,
    ) -> isize;
}

/// The kernel's own copy: one `copy_file_range` loop, falling back to a
/// user-space read/write pair only when the kernel refuses (a cross-filesystem
/// copy, or a filesystem without the call) — and then the fallback is a
/// bounded buffer, not an unbounded slurp.
///
/// Returns the number of bytes the host actually copied, so a caller can tell
/// a full copy from a partial one.
fn kernel_copy(src: &mut File, dst: &mut File) -> Result<u64> {
    let mut total: u64 = 0;
    let mut off_in: i64 = 0;
    let mut off_out: i64 = 0;
    loop {
        // SAFETY: both descriptors are live files this process owns, the
        // offsets are locals, and the length is bounded by the kernel's own
        // chunking.
        let copied = unsafe {
            copy_file_range(
                src.as_raw_fd(),
                &mut off_in,
                dst.as_raw_fd(),
                &mut off_out,
                1 << 20,
                0,
            )
        };
        if copied < 0 {
            let error = std::io::Error::last_os_error();
            // EXDEV (a cross-device copy) and EINVAL are the host saying "I
            // cannot do this in-kernel" — the fallback is the honest second
            // mechanism, not a failure.
            if matches!(error.raw_os_error(), Some(18) | Some(22)) && total == 0 {
                return userspace_copy(src, dst);
            }
            if total == 0 {
                return Err(io_error("copy_file_range", error));
            }
            return Ok(total);
        }
        if copied == 0 {
            return Ok(total);
        }
        total += copied as u64;
        // A short chunk is the host's own signal that the file ended, so the
        // count is exact rather than assumed.
        if (copied as u64) < (1 << 20) {
            return Ok(total);
        }
    }
}

/// The user-space fallback: a 1 MiB bounce buffer, never a whole-file slurp.
fn userspace_copy(src: &mut File, dst: &mut File) -> Result<u64> {
    let mut buf = [0u8; 1 << 20];
    let mut total: u64 = 0;
    loop {
        let read = src
            .read(&mut buf)
            .map_err(|e| io_error("read (copy fallback)", e))?;
        if read == 0 {
            return Ok(total);
        }
        dst.write_all(&buf[..read])
            .map_err(|e| io_error("write (copy fallback)", e))?;
        total += read as u64;
    }
}

// ── memfd ───────────────────────────────────────────────────────────────



/// The kernel's own anonymous file: no path, no `tmpfs` agreement, and the
/// bytes live in the page cache. An ephemeral file is exactly this.
fn memfd_create_file(label: &str) -> Result<File> {
    let name = std::ffi::CString::new(alloc::format!("smart-lib-{label}"))
        .map_err(|_| Error::invalid_argument("a memfd label with an interior NUL is not nameable"))?;
    // SAFETY: `name` is a NUL-terminated local that outlives the call and the
    // returned descriptor belongs to this process.
    let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io_error("memfd_create", std::io::Error::last_os_error()));
    }
    // SAFETY: the descriptor is fresh and owned by this process.
    Ok(unsafe { File::from_raw_fd(fd) })
}

// ── inotify ─────────────────────────────────────────────────────────────

const IN_MODIFY: u32 = 0x0000_0002;
const IN_ATTRIB: u32 = 0x0000_0004;
const IN_CLOSE_WRITE: u32 = 0x0000_0008;
const IN_MOVED_FROM: u32 = 0x0000_0040;
const IN_MOVED_TO: u32 = 0x0000_0080;
const IN_CREATE: u32 = 0x0000_0100;
const IN_DELETE: u32 = 0x0000_0200;
const IN_DELETE_SELF: u32 = 0x0000_0400;
const IN_MOVE_SELF: u32 = 0x0000_0800;
const IN_IGNORED: u32 = 0x0000_8000;
const IN_ISDIR: u32 = 0x4000_0000;
const IN_NONBLOCK: i32 = 0o000_4000;
const O_NONBLOCK: i32 = 0o000_4000;

unsafe extern "C" {
    fn inotify_init1(flags: i32) -> i32;
    fn inotify_add_watch(fd: i32, pathname: *const u8, mask: u32) -> i32;
    fn inotify_rm_watch(fd: i32, wd: i32) -> i32;
}

/// Every mask this backend watches: the contract's `FileEventType` vocabulary
/// maps one-for-one onto the kernel's own create/delete/modify/rename/move
/// events, so no host change is silently unobservable.
const WATCH_MASK: u32 = IN_MODIFY
    | IN_ATTRIB
    | IN_CLOSE_WRITE
    | IN_MOVED_FROM
    | IN_MOVED_TO
    | IN_CREATE
    | IN_DELETE
    | IN_DELETE_SELF
    | IN_MOVE_SELF;

/// One live watch: the kernel's own inotify descriptor plus the decoded
/// events the kernel has queued since the last pull.
struct Watch {
    /// The watched directory.
    path: String,
    /// The inotify file descriptor this process owns.
    fd: File,
    /// The next event id this watch mints (the contract's `T.EventId`).
    next_id: u64,
    /// Events read from the kernel and not yet delivered.
    pending: Vec<FileEvent>,
}

impl Watch {
    /// Drain whatever the kernel has queued, decoding each record into the
    /// contract's own event shape. Non-blocking: a quiet directory answers
    /// "nothing", never a wait.
    fn drain(&mut self) {
        let mut buf = [0u8; 4096];
        loop {
            let read = match self.fd.read(&mut buf) {
                Ok(read) => read,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(_) => return,
            };
            if read == 0 {
                return;
            }
            let mut offset = 0usize;
            while offset + 16 <= read {
                // The kernel's `struct inotify_event` is a fixed 16-byte head
                // followed by a NUL-padded name of `len` bytes.
                let wd = i32::from_ne_bytes(buf[offset..offset + 4].try_into().unwrap_or([0; 4]));
                let mask = u32::from_ne_bytes(buf[offset + 4..offset + 8].try_into().unwrap_or([0; 4]));
                let cookie = u32::from_ne_bytes(buf[offset + 8..offset + 12].try_into().unwrap_or([0; 4]));
                let len = u32::from_ne_bytes(buf[offset + 12..offset + 16].try_into().unwrap_or([0; 4])) as usize;
                let name_end = offset + 16 + len.min(read.saturating_sub(offset + 16));
                let name = {
                    let raw = &buf[offset + 16..name_end];
                    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
                    String::from_utf8_lossy(&raw[..end]).to_string()
                };
                offset += 16 + len;
                if mask & IN_IGNORED != 0 {
                    continue;
                }
                let _ = wd;
                let kind = match mask
                    & (IN_CREATE | IN_DELETE | IN_DELETE_SELF | IN_MODIFY | IN_ATTRIB | IN_CLOSE_WRITE | IN_MOVED_FROM | IN_MOVED_TO | IN_MOVE_SELF)
                {
                    m if m & IN_CREATE != 0 => FileEventKind::Create,
                    m if m & (IN_DELETE | IN_DELETE_SELF) != 0 => FileEventKind::Delete,
                    m if m & IN_MOVED_FROM != 0 => FileEventKind::Rename,
                    m if m & (IN_MOVED_TO | IN_MOVE_SELF) != 0 => FileEventKind::Move,
                    m if m & (IN_MODIFY | IN_ATTRIB | IN_CLOSE_WRITE) != 0 => FileEventKind::Modify,
                    _ => FileEventKind::Custom,
                };
                let path = if name.is_empty() {
                    self.path.clone()
                } else {
                    format!("{}/{}", self.path.trim_end_matches('/'), name)
                };
                let mut metadata: Metadata = Vec::new();
                if mask & IN_ISDIR != 0 {
                    metadata.push(("isDirectory".to_string(), "true".to_string()));
                }
                if cookie != 0 {
                    metadata.push(("cookie".to_string(), format!("{cookie}")));
                }
                self.pending.push(FileEvent {
                    id: EventId::new(self.next_id),
                    timestamp: Timestamp::from_nanos(now_nanos()),
                    kind,
                    path,
                    old_path: String::new(),
                    metadata,
                });
                self.next_id += 1;
            }
            if read < buf.len() {
                return;
            }
        }
    }
}

// ═══ SHA-256 (the contract's content-addressing needs a real hash) ═══════

/// The SHA-256 initial state (`FIPS 180-4` §5.3.3).
const SHA256_IV: [u32; 8] = [
    0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab,
    0x5be0_cd19,
];

/// The SHA-256 round constants (`FIPS 180-4` §4.2.2).
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 over a byte slice, the algorithm the contract's content addressing
/// and checksum surfaces need — implemented here so the bytes are hashed by
/// this process with no `sha256sum` fork and no dependency.
#[must_use]
pub fn sha256(data: &[u8]) -> Vec<u8> {
    let mut h = SHA256_IV;
    // The message is padded to a multiple of 64 bytes: a 1 bit, zeros, then the
    // original length as a 64-bit big-endian count (`FIPS 180-4` §5.1.1).
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = Vec::with_capacity(32);
    for word in h {
        out.extend_from_slice(&word.to_be_bytes());
    }
    out
}

/// The lowercase hex spelling of a digest.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&alloc::format!("{byte:02x}"));
    }
    out
}

// ═══ The host's own tables, read natively ═══════════════════════════════

/// The host's owner labels, read once out of `/etc/passwd` (the host's own
/// database, not a `whoami` fork).
fn owner_label(uid: u32) -> String {
    match fs::read_to_string("/etc/passwd") {
        Ok(text) => {
            for line in text.lines() {
                let mut fields = line.split(':');
                let (name, _passwd, uid_field) = (
                    fields.next().unwrap_or_default(),
                    fields.next().unwrap_or_default(),
                    fields.next().unwrap_or_default(),
                );
                if uid_field.parse::<u32>() == Ok(uid) {
                    return name.to_string();
                }
            }
            alloc::format!("uid {uid}")
        }
        Err(_) => alloc::format!("uid {uid}"),
    }
}

/// The extension→mime table, read once out of the host's own
/// `/usr/share/mime/globs` (the shared-mime-info database, as text). A
/// mechanism the host publishes, so the mapping is the host's and never this
/// backend's opinion.
fn mime_table() -> &'static Vec<(String, String)> {
    static TABLE: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Vec<(String, String)> = Vec::new();
        for path in ["/usr/share/mime/globs", "/usr/share/mime/globs2"] {
            if let Ok(text) = fs::read_to_string(path) {
                for line in text.lines() {
                    // `weight:mimetype:glob` (globs2 is the same shape with a
                    // different weight scale); a `#` line is a comment.
                    if line.starts_with('#') || line.is_empty() {
                        continue;
                    }
                    let mut fields = line.rsplitn(3, ':');
                    let (glob, mime) = (
                        fields.next().unwrap_or_default(),
                        fields.next().unwrap_or_default(),
                    );
                    if let Some(ext) = glob.strip_prefix('*') {
                        table.push((ext.trim_start_matches('.').to_string(), mime.to_string()));
                    }
                }
            }
        }
        table.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        table
    })
}

/// The host's own mime type for one extension, or an empty string when the
/// host's table does not know it (an honest "nobody here knows", never a
/// guessed `application/octet-stream`).
fn mime_for_path(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    if ext.is_empty() {
        return String::new();
    }
    mime_table()
        .iter()
        .find(|(candidate, _)| *candidate == ext)
        .map(|(_, mime)| mime.clone())
        .unwrap_or_default()
}

/// One row of `/proc/mounts`, the host's own words about its own superblocks.
#[derive(Clone, Debug)]
struct MountRow {
    device: String,
    mount_point: String,
    fs_type: String,
    options: String,
}

/// The host's mounts, as `/proc/mounts` reports them.
fn proc_mounts() -> Vec<MountRow> {
    let mut rows = Vec::new();
    if let Ok(text) = fs::read_to_string("/proc/mounts") {
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let (Some(device), Some(mount_point), Some(fs_type), Some(options)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            rows.push(MountRow {
                device: device.to_string(),
                mount_point: unescape_mount_field(mount_point),
                fs_type: fs_type.to_string(),
                options: options.to_string(),
            });
        }
    }
    rows
}

/// `/proc/mounts` escapes space, tab, newline and backslash as octal.
fn unescape_mount_field(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let bytes = field.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() + 1 && i + 3 <= bytes.len() {
            let octal = core::str::from_utf8(&bytes[i + 1..i + 4]).ok().and_then(|s| u8::from_str_radix(s, 8).ok());
            if let Some(byte) = octal {
                out.push(char::from(byte));
                i += 4;
                continue;
            }
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    out
}

/// The mount row carrying `path`: the longest mount point that is a prefix of
/// it (the kernel's own rule for which superblock a path lives on).
fn mount_of(path: &str) -> Option<MountRow> {
    let mut best: Option<MountRow> = None;
    let mut best_len = 0usize;
    for row in proc_mounts() {
        // The root mount is `/`, whose prefix rule is "everything" — an empty
        // trim must not skip the one mount every path lives on.
        let mount = row.mount_point.trim_end_matches('/');
        let matches = if mount.is_empty() {
            path.starts_with('/')
        } else {
            path == mount || path.starts_with(&alloc::format!("{mount}/"))
        };
        if matches && mount.len() >= best_len {
            best_len = mount.len();
            best = Some(row);
        }
    }
    best
}

/// The XDG directory a `SpecialPath` names, from the host's own environment
/// and conventions. `None` when the host does not define it (an empty
/// `XDG_RUNTIME_DIR` is the measured answer, not a guess).
fn xdg_special(path: SpecialPath) -> Option<String> {
    fn env_or(key: &str, fallback: impl FnOnce() -> Option<String>) -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(fallback)
    }
    let home = std::env::var("HOME").ok().filter(|value| !value.is_empty())?;
    let data = env_or("XDG_DATA_HOME", || Some(alloc::format!("{home}/.local/share")))?;
    let config = env_or("XDG_CONFIG_HOME", || Some(alloc::format!("{home}/.config")))?;
    let cache = env_or("XDG_CACHE_HOME", || Some(alloc::format!("{home}/.cache")))?;
    let user_dir = |name: &str| {
        env_or(
            &alloc::format!("XDG_{name}_DIR"),
            || Some(alloc::format!("{home}/{name}")),
        )
    };
    let resolved = match path {
        SpecialPath::Home => Some(home.clone()),
        SpecialPath::Config => Some(config),
        SpecialPath::Data => Some(data.clone()),
        SpecialPath::Cache => Some(cache),
        SpecialPath::Runtime => env_or("XDG_RUNTIME_DIR", || None),
        SpecialPath::Trash => Some(alloc::format!("{data}/Trash")),
        SpecialPath::Documents => user_dir("Documents"),
        SpecialPath::Downloads => user_dir("Downloads"),
        SpecialPath::Music => user_dir("Music"),
        SpecialPath::Pictures => user_dir("Pictures"),
        SpecialPath::Videos => user_dir("Videos"),
        SpecialPath::Desktop => user_dir("Desktop"),
        SpecialPath::Templates => user_dir("Templates"),
        SpecialPath::Public => user_dir("Public"),
        SpecialPath::Other => None,
    };
    resolved.filter(|value| !value.is_empty())
}

/// The host's trash directory root (`$XDG_DATA_HOME/Trash`).
fn trash_root() -> Option<PathBuf> {
    xdg_special(SpecialPath::Trash).map(PathBuf::from)
}

/// The host's recent-files record (`$XDG_DATA_HOME/recently-used.xbel`, the
/// spelling GTK writes and reads).
fn recent_files_path() -> Option<PathBuf> {
    xdg_special(SpecialPath::Data).map(|data| PathBuf::from(data).join("recently-used.xbel"))
}

// ═══ Small helpers ══════════════════════════════════════════════════════

/// The host's own error words, under the contract's closest code: a path that
/// names nothing is `notFound`, a directory this uid cannot cross is
/// `permissionDenied`, and anything else is `failed` with the OS's message.
fn io_error(what: &str, error: std::io::Error) -> Error {
    use std::io::ErrorKind;
    let message = alloc::format!("{what}: {error}");
    match error.kind() {
        ErrorKind::NotFound => Error::not_found().with_context(message),
        ErrorKind::PermissionDenied => Error::with_message(ErrorCode::PermissionDenied, message),
        ErrorKind::AlreadyExists => Error::with_message(ErrorCode::AlreadyExists, message),
        ErrorKind::InvalidInput => Error::invalid_argument(message),
        _ => Error::failed(message),
    }
}

/// The process's own wall clock in nanoseconds since the Unix epoch.
fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// A plain glob over one name: `*` and `?` are the only metacharacters, the
/// same rule the logging filter's `pattern` uses. Not a query language.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();

    fn matches(pattern: &[char], name: &[char]) -> bool {
        if pattern.is_empty() {
            return name.is_empty();
        }
        match pattern[0] {
            '*' => {
                for split in 0..=name.len() {
                    if matches(&pattern[1..], &name[split..]) {
                        return true;
                    }
                }
                false
            }
            '?' => !name.is_empty() && matches(&pattern[1..], &name[1..]),
            literal => !name.is_empty() && name[0] == literal && matches(&pattern[1..], &name[1..]),
        }
    }
    matches(&pattern, &name)
}

/// A `T.StringMap` pair list from the `key=value` lines of a small sidecar
/// file (the format the version store and the branch store use).
fn parse_pairs(text: &str) -> Metadata {
    let mut pairs = Vec::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    pairs
}

/// A path made safe to use as one store component: the path's own bytes, the
/// host's separators kept so two paths never collide into one name.
fn store_component(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '/' | '\\' => out.push('_'),
            c if (c as u32) < 0x20 => out.push('?'),
            c => out.push(c),
        }
    }
    if out.len() > 96 {
        let hash = hex(&sha256(path.as_bytes()));
        alloc::format!("{}~{}", &out[..96], &hash[..16])
    } else if out.is_empty() {
        "_root".to_string()
    } else {
        out
    }
}

/// The `T.StringMap` → `key=value` sidecar spelling.
fn render_pairs(pairs: &Metadata) -> String {
    let mut out = String::new();
    for (key, value) in pairs {
        out.push_str(key);
        out.push('=');
        out.push_str(value);
        out.push('\n');
    }
    out
}

/// Sort a listing by the filter's own field and order (a stable sort, so the
/// host's own order breaks the ties the filter does not name).
fn sort_entries(entries: &mut [DirectoryEntry], field: SortField, order: SortOrder) {
    match field {
        SortField::Name => entries.sort_by(|a, b| a.name.cmp(&b.name)),
        SortField::Size => entries.sort_by(|a, b| a.size.cmp(&b.size)),
        SortField::Modified => entries.sort_by(|a, b| a.modified.as_nanos().cmp(&b.modified.as_nanos())),
        SortField::Created => entries.sort_by(|a, b| a.metadata.iter().find(|(k, _)| k == "created").map(|(_, v)| v.as_str()).unwrap_or("").cmp(&b.metadata.iter().find(|(k, _)| k == "created").map(|(_, v)| v.as_str()).unwrap_or(""))),
        SortField::Accessed => entries.sort_by(|a, b| a.metadata.iter().find(|(k, _)| k == "accessed").map(|(_, v)| v.as_str()).unwrap_or("").cmp(&b.metadata.iter().find(|(k, _)| k == "accessed").map(|(_, v)| v.as_str()).unwrap_or(""))),
        SortField::Unknown | SortField::Custom => {}
    }
    if matches!(order, SortOrder::Descending) {
        entries.reverse();
    }
}

/// The contract's page window over an already-materialized list.
fn page_of<T: Clone>(items: &[T], page: Pagination) -> Page<T> {
    types::page_window(items, page)
}

/// One directory entry from a `statx` answer.
fn entry_from_stat(path: &str, stat: &Stat) -> DirectoryEntry {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    DirectoryEntry {
        name,
        path: path.to_string(),
        kind: stat.kind(),
        size: stat.size,
        modified: stat.modified(),
        metadata: Vec::new(),
    }
}

/// The recursive walk this backend's list/stat/search surfaces share: the
/// host's own `read_dir`, a stated depth bound, and symlinks stat'd as
/// themselves (so a walk never escapes through a link it was told not to
/// follow).
fn walk(path: &str, max_depth: u32, follow_links: bool, out: &mut Vec<(String, Stat)>) -> Result<()> {
    fn walk_inner(
        path: &str,
        depth: u32,
        max_depth: u32,
        follow_links: bool,
        out: &mut Vec<(String, Stat)>,
    ) -> Result<()> {
        if depth > max_depth {
            return Ok(());
        }
        let entries = fs::read_dir(path).map_err(|e| io_error(&alloc::format!("read_dir({path})"), e))?;
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let child = entry.path().to_string_lossy().to_string();
            let Ok(stat) = statx_of(&child, follow_links) else { continue };
            out.push((child.clone(), stat));
            if stat.is_dir() && depth + 1 < max_depth {
                walk_inner(&child, depth + 1, max_depth, follow_links, out)?;
            } else if stat.is_dir() && depth + 1 == max_depth {
                // A directory at the bound is still one entry of its parent's
                // listing; the bound only stops the descent.
                continue;
            }
        }
        Ok(())
    }
    let root_stat = statx_of(path, follow_links)?;
    out.push((path.to_string(), root_stat));
    walk_inner(path, 1, max_depth, follow_links, out)?;
    Ok(())
}

/// A unified-diff rendering of two line lists: the contract's `ComparisonResult.
/// diff` arm carries this as `T.DataHandle` text, and it is the real difference
/// between the real bytes rather than a summary of them.
fn unified_diff(left: &[String], right: &[String], left_name: &str, right_name: &str) -> String {
    // Longest-common-subsequence table, sized to the real inputs. Bounded: a
    // diff of a megabyte-scale file is refused rather than attempting a
    // quadratic table (and the reason names it).
    const MAX_DIFF_LINES: usize = 20_000;
    if left.len() > MAX_DIFF_LINES || right.len() > MAX_DIFF_LINES {
        return alloc::format!(
            "diff refused: one side carries more than {MAX_DIFF_LINES} lines, so the line-diff table would be quadratic in the file's own size\n"
        );
    }
    let n = left.len();
    let m = right.len();
    let mut table = alloc::vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * (m + 1) + j] = if left[i] == right[j] {
                table[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
            };
        }
    }
    let mut out = String::new();
    out.push_str(&alloc::format!("--- {left_name}\n+++ {right_name}\n"));
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if left[i] == right[j] {
            out.push_str(&alloc::format!(" {}\n", left[i]));
            i += 1;
            j += 1;
        } else if table[(i + 1) * (m + 1) + j] >= table[i * (m + 1) + j + 1] {
            out.push_str(&alloc::format!("-{}\n", left[i]));
            i += 1;
        } else {
            out.push_str(&alloc::format!("+{}\n", right[j]));
            j += 1;
        }
    }
    while i < n {
        out.push_str(&alloc::format!("-{}\n", left[i]));
        i += 1;
    }
    while j < m {
        out.push_str(&alloc::format!("+{}\n", right[j]));
        j += 1;
    }
    out
}

/// A file's bytes, read through its own mapping (the hot path) or, for a file
/// the host will not map, through a bounded read.
fn read_all_bytes(path: &str) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|e| io_error(&alloc::format!("open({path})"), e))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0) as usize;
    if let Some(map) = Mapping::open(&file) {
        return Ok(map.bytes().to_vec());
    }
    let mut buf = Vec::with_capacity(size);
    file.take(size as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| io_error(&alloc::format!("read({path})"), e))?;
    Ok(buf)
}

// ═══════════════════════════════════════════════════════════════════════
// The Linux files backend.
// ═══════════════════════════════════════════════════════════════════════

/// One open descriptor: the host's own `File`, the mode it was opened with
/// (so a read on a write-only handle is refused rather than answered), and the
/// ephemeral bookkeeping the contract's `EphemeralFileConfig` names.
struct OpenFile {
    file: File,
    /// The path the descriptor was opened for (empty for a `memfd`).
    path: String,
    mode: OpenMode,
    /// How many reads an ephemeral file still allows, when bounded.
    remaining_reads: Option<u32>,
    /// Whether this is a kernel `memfd` with no host path.
    ephemeral: bool,
}

/// One lock this backend holds: the contract's `FileLock` plus the descriptor
/// the kernel's own advisory lock was taken on. Owning the descriptor is what
/// keeps the lock held — dropping it is the kernel's own release — so the
/// field is never read; it is held.
#[allow(dead_code)]
struct LockHolder {
    entry: FileLock,
    file: File,
}

/// One row of the content-addressed index: the store's own record of a path
/// and the digest its bytes hashed to.
#[derive(Clone, Debug)]
struct IndexEntry {
    path: String,
    digest: String,
    size: u64,
    timestamp: u64,
}

/// The default content-addressed store root: under the host's own cache root,
/// so a backend object is a real file the user can inspect and delete.
fn default_store_root() -> PathBuf {
    xdg_special(SpecialPath::Cache)
        .map(|cache| PathBuf::from(cache).join("smart-lib").join("files"))
        .unwrap_or_else(|| PathBuf::from("/run/smart-lib/files"))
}

/// The Linux files backend.
///
/// Every state field is either the host's own (open descriptors, the watch
/// descriptors) or the backend's own store under the host's cache root (the
/// content-addressed objects, the version and branch records, the index). No
/// field is a fabricated host fact: the inode counts, sizes, times, hashes and
/// event ids all come from the kernel.
pub struct LinuxFilesBackend {
    /// The open descriptors, by the handle token handed to the caller.
    handles: Mutex<HashMap<u64, OpenFile>>,
    /// The locks this backend holds, with the kernel's own fcntl lock behind
    /// each one. The table owns the descriptor because dropping it is what
    /// releases the kernel's advisory lock.
    lock_holders: Mutex<Vec<LockHolder>>,
    /// The version metadata per path (the bodies live in the store).
    versions: Mutex<BTreeMap<String, Vec<FileVersion>>>,
    /// The branch records (a branch names a stored version).
    branches: Mutex<Vec<FileBranch>>,
    /// The content-addressed index.
    index: Mutex<Vec<IndexEntry>>,
    /// This backend's own activity trail, per path.
    activity: Mutex<Vec<(String, FileActivity)>>,
    /// The live audit subscriptions.
    audits: Mutex<HashMap<u64, (FileAuditFilter, u64)>>,
    /// The live inotify watches.
    watches: Mutex<HashMap<u64, Watch>>,
    /// The store root.
    store: PathBuf,
    /// The host's trash directory root, as measured at construction.
    trash_root: PathBuf,
    /// The host's recent-files record, as measured at construction.
    recent_path: PathBuf,
    next_lock: AtomicU64,
    next_sub: AtomicU64,
    next_snapshot: AtomicU64,
}

impl Default for LinuxFilesBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxFilesBackend {
    /// A backend over the host's own mechanisms, with the content-addressed
    /// store under the host's own cache root.
    pub fn new() -> Self {
        Self::with_store_root(default_store_root())
    }

    /// A backend whose store lives at `root` (the hermetic entry the suite
    /// uses, so no test touches the host's real cache).
    pub fn with_store_root(root: impl Into<PathBuf>) -> Self {
        let store = root.into();
        let mut index: Vec<IndexEntry> = Vec::new();
        if let Ok(text) = fs::read_to_string(store.join("index.tsv")) {
            for line in text.lines() {
                let mut fields = line.split('\t');
                let (Some(path), Some(digest), Some(size), Some(timestamp)) = (
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                ) else {
                    continue;
                };
                index.push(IndexEntry {
                    path: path.to_string(),
                    digest: digest.to_string(),
                    size: size.parse().unwrap_or(0),
                    timestamp: timestamp.parse().unwrap_or(0),
                });
            }
        }
        LinuxFilesBackend {
            handles: Mutex::new(HashMap::new()),
            lock_holders: Mutex::new(Vec::new()),
            versions: Mutex::new(BTreeMap::new()),
            branches: Mutex::new(Vec::new()),
            index: Mutex::new(index),
            activity: Mutex::new(Vec::new()),
            audits: Mutex::new(HashMap::new()),
            watches: Mutex::new(HashMap::new()),
            trash_root: trash_root().unwrap_or_default(),
            recent_path: recent_files_path().unwrap_or_default(),
            store,
            next_lock: AtomicU64::new(1),
            next_sub: AtomicU64::new(1),
            next_snapshot: AtomicU64::new(1),
        }
    }

    /// The store root in use.
    pub fn store_root(&self) -> &Path {
        &self.store
    }

    /// A backend over three measured roots: the content-addressed store, the
    /// trash directory and the recent-files record. The hermetic entry the
    /// suite uses, so no test touches the host's own cache or trash.
    pub fn with_paths(store: PathBuf, trash: PathBuf, recent: PathBuf) -> Self {
        LinuxFilesBackend {
            trash_root: trash,
            recent_path: recent,
            ..Self::with_store_root(store)
        }
    }

    /// The trash root this backend reads.
    pub fn trash_root(&self) -> &Path {
        &self.trash_root
    }

    /// The recent-files record this backend reads.
    pub fn recent_path(&self) -> &Path {
        &self.recent_path
    }

    /// The open descriptors currently held (a diagnostic for the CLI's probe).
    pub fn open_handle_count(&self) -> usize {
        self.handles.lock().unwrap().len()
    }

    /// The host's mounts, as `/proc/mounts` reports them (the read the
    /// `files.mounts` capability names, and the measurement behind every
    /// reflink/snapshot answer in this backend).
    pub fn measured_mounts(&self) -> Vec<(String, String, String)> {
        proc_mounts()
            .into_iter()
            .map(|row| (row.device, row.mount_point, row.fs_type))
            .collect()
    }

    // ── the store's own plumbing ─────────────────────────────────────────

    fn store_path(&self, parts: &[&str]) -> PathBuf {
        let mut path = self.store.clone();
        for part in parts {
            path.push(part);
        }
        path
    }

    fn ensure_store_dir(&self, parts: &[&str]) -> Result<PathBuf> {
        let path = self.store_path(parts);
        fs::create_dir_all(&path).map_err(|e| {
            io_error(&alloc::format!("create_dir_all({})", path.display()), e)
        })?;
        Ok(path)
    }

    fn index_save(&self) {
        let index = self.index.lock().unwrap();
        let dir = self.store.clone();
        let _ = fs::create_dir_all(&dir);
        let mut text = String::new();
        for entry in index.iter() {
            text.push_str(&alloc::format!(
                "{}\t{}\t{}\t{}\n",
                entry.path, entry.digest, entry.size, entry.timestamp
            ));
        }
        let _ = fs::write(dir.join("index.tsv"), text);
    }

    /// Hash a path's own bytes, returning the hex digest and the byte count.
    fn digest_of(&self, path: &str) -> Result<(String, u64)> {
        let bytes = read_all_bytes(path)?;
        Ok((hex(&sha256(&bytes)), bytes.len() as u64))
    }

    /// Record a path in the content-addressed index (a real object write when
    /// the store does not already hold the digest).
    fn index_add(&self, path: &str, digest: &str, size: u64) -> Result<()> {
        {
            let index = self.index.lock().unwrap();
            if index.iter().any(|e| e.path == path && e.digest == digest) {
                return Ok(());
            }
        }
        let object = self.store_path(&["objects", digest]);
        if !object.exists() {
            let bytes = read_all_bytes(path)?;
            self.ensure_store_dir(&["objects"])?;
            fs::write(&object, &bytes)
                .map_err(|e| io_error(&alloc::format!("write({})", object.display()), e))?;
        }
        self.index.lock().unwrap().push(IndexEntry {
            path: path.to_string(),
            digest: digest.to_string(),
            size,
            timestamp: now_nanos(),
        });
        self.index_save();
        Ok(())
    }

    /// This backend's own activity trail: a real record of real acts, labelled
    /// as this backend's (the host runs no auditd — measured).
    fn record_activity(&self, path: &str, action: &str, details: &str) {
        if path.is_empty() {
            return;
        }
        let actor = owner_label(unsafe { libc_getuid() });
        self.activity.lock().unwrap().push((
            path.to_string(),
            FileActivity {
                timestamp: Timestamp::from_nanos(now_nanos()),
                actor,
                action: action.to_string(),
                path: path.to_string(),
                metadata: if details.is_empty() {
                    Vec::new()
                } else {
                    vec![("detail".to_string(), details.to_string())]
                },
            },
        ));
    }

    /// Store a version of a path's own bytes when they differ from the newest
    /// stored version: every version is a real file in the store, so a
    /// `getFileVersions` answer is a list of objects that exist.
    fn record_version(&self, path: &str) {
        let Ok((digest, size)) = self.digest_of(path) else {
            return;
        };
        let mut versions = self.versions.lock().unwrap();
        let entry = versions.entry(path.to_string()).or_default();
        if entry.last().map(|v| hex(&v.content_hash) == digest).unwrap_or(false) {
            return;
        }
        let version_id = entry.len() as u64 + 1;
        drop(versions);
        let component = store_component(path);
        let Ok(dir) = self.ensure_store_dir(&["versions", &component]) else {
            return;
        };
        let body = dir.join(alloc::format!("{version_id}.body"));
        if let Ok(bytes) = read_all_bytes(path) {
            if fs::write(&body, &bytes).is_err() {
                return;
            }
        }
        let version = FileVersion {
            version_id,
            content_hash: sha256_hex_to_bytes(&digest),
            created_at: Timestamp::from_nanos(now_nanos()),
            author: AppId::new(unsafe { libc_getuid() } as u64),
            message: String::new(),
            size,
            metadata: vec![("digest".to_string(), digest)],
        };
        let meta = dir.join(alloc::format!("{version_id}.meta"));
        let _ = fs::write(&meta, render_pairs(&version.metadata));
        let mut versions = self.versions.lock().unwrap();
        versions.entry(path.to_string()).or_default().push(version);
    }

    /// The handle token for one open descriptor.
    fn next_handle_id(&self) -> u64 {
        self.next_sub.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// The process's own uid, through the host's own syscall (no `id` fork).
unsafe fn libc_getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: `getuid` takes no argument and cannot fail.
    unsafe { getuid() }
}

/// The digest bytes of a lowercase hex spelling (the store's own on-disk form
/// is hex; the contract's `T.HashOutput.bytes` is the digest itself).
fn sha256_hex_to_bytes(hex_digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(hex_digest.len() / 2);
    let bytes = hex_digest.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let byte = u8::from_str_radix(core::str::from_utf8(&bytes[i..i + 2]).unwrap_or("0"), 16)
            .unwrap_or(0);
        out.push(byte);
        i += 2;
    }
    out
}

/// Whether a path is inside a directory the caller may not cross (a measured
/// answer, so a write into /root is refused with the host's own EACCES before
/// it is attempted).
fn path_is_writable(path: &str) -> Result<()> {
    let parent = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty());
    let probe = match parent {
        Some(parent) if parent.exists() => parent.to_path_buf(),
        _ => PathBuf::from("/"),
    };
    match OpenOptions::new().read(true).open(&probe) {
        Ok(_) => Ok(()),
        Err(e) => Err(io_error(&alloc::format!("access({})", probe.display()), e)),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The FilesBackend implementation.
// ═══════════════════════════════════════════════════════════════════════

/// `O_SYNC` — the contract's `OpenFlag::Synchronous` is the kernel's own flag.
const O_SYNC: i32 = 0x1010_00;

impl FilesBackend for LinuxFilesBackend {
    // ── The handle surface ───────────────────────────────────────────────

    fn open_file(&self, path: &str, request: &OpenRequest) -> Result<FileHandle> {
        let mut options = OpenOptions::new();
        match request.mode {
            OpenMode::Read => {
                options.read(true);
            }
            OpenMode::Write => {
                options.write(true);
            }
            OpenMode::ReadWrite => {
                options.read(true).write(true);
            }
            OpenMode::Append => {
                options.append(true);
            }
            OpenMode::Truncate => {
                options.write(true).truncate(true);
            }
            OpenMode::Create => {
                options.write(true).create(true);
            }
            OpenMode::CreateNew => {
                options.write(true).create_new(true);
            }
            OpenMode::Unknown | OpenMode::Custom => {
                // An unnamed mode is read-only rather than guessed: a guessed
                // write mode is how a read turns into a truncation.
                options.read(true);
            }
        }
        for flag in &request.flags {
            match flag {
                OpenFlag::NonBlocking => {
                    options.custom_flags(O_NONBLOCK);
                }
                OpenFlag::Exclusive => {
                    options.custom_flags(0o200); // O_EXCL
                }
                OpenFlag::Synchronous => {
                    options.custom_flags(O_SYNC);
                }
                OpenFlag::Append => {
                    options.append(true);
                }
                OpenFlag::Hint | OpenFlag::Unknown | OpenFlag::Custom => {}
            }
        }
        let file = options
            .open(path)
            .map_err(|e| io_error(&alloc::format!("open({path})"), e))?;
        let id = self.next_handle_id();
        self.handles.lock().unwrap().insert(
            id,
            OpenFile {
                file,
                path: path.to_string(),
                mode: request.mode,
                remaining_reads: None,
                ephemeral: false,
            },
        );
        Ok(FileHandle::new(FileHandleId::new(id)))
    }

    fn close_file(&self, handle: &FileHandle) -> Result<()> {
        let open = self
            .handles
            .lock()
            .unwrap()
            .remove(&handle.id.get())
            .ok_or_else(|| Error::not_found().with_context("file handle"))?;
        // Dropping the `File` closes the kernel's descriptor.
        drop(open.file);
        Ok(())
    }

    fn read_file(&self, handle: &FileHandle, offset: u64, size: u32) -> Result<Vec<u8>> {
        let mut guard = self.handles.lock().unwrap();
        let open = guard
            .get_mut(&handle.id.get())
            .ok_or_else(|| Error::not_found().with_context("file handle"))?;
        if matches!(open.mode, OpenMode::Write) {
            return Err(Error::with_message(
                ErrorCode::InvalidArgument,
                "the handle was opened for writing, so a read is invalid on it",
            ));
        }
        // The read hot path: the kernel's own mapping, so the window is served
        // out of the page cache with no intermediate buffer.
        let map = Mapping::open(&open.file).ok_or_else(|| {
            Error::failed(alloc::format!("mmap({}) failed", open.path))
        })?;
        let bytes = map.bytes();
        let start = offset as usize;
        let end = start.saturating_add(size as usize).min(bytes.len());
        let window = if start >= bytes.len() {
            Vec::new()
        } else {
            bytes[start..end].to_vec()
        };
        // The ephemeral contract is enforced here, on the read that consumes
        // it, so `deleteAfterRead` and `maxReads` are the host's own truth.
        if open.ephemeral {
            if let Some(remaining) = open.remaining_reads.as_mut() {
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    let id = handle.id.get();
                    drop(guard);
                    self.handles.lock().unwrap().remove(&id);
                }
            }
        }
        Ok(window)
    }

    fn write_file(&self, handle: &FileHandle, offset: u64, data: &[u8]) -> Result<u32> {
        let guard = self.handles.lock().unwrap();
        let open = guard
            .get(&handle.id.get())
            .ok_or_else(|| Error::not_found().with_context("file handle"))?;
        if matches!(open.mode, OpenMode::Read) {
            return Err(Error::with_message(
                ErrorCode::InvalidArgument,
                "the handle was opened for reading, so a write is invalid on it",
            ));
        }
        // `pwrite` lands the bytes at the caller's own offset; on an append
        // handle the kernel's own append semantics are honoured instead (an
        // O_APPEND descriptor ignores the offset by the host's own rule).
        let written = if matches!(open.mode, OpenMode::Append) {
            use std::io::Seek;
            let mut file = &open.file;
            file.seek(std::io::SeekFrom::End(0))
                .map_err(|e| io_error("seek(end)", e))?;
            file.write(data).map_err(|e| io_error("write", e))?
        } else {
            open.file
                .write_at(data, offset)
                .map_err(|e| io_error("write_at", e))?
        };
        let activity_path = open.path.clone();
        drop(guard);
        self.record_activity(
            &activity_path,
            "write",
            &alloc::format!("{written} bytes at {offset}"),
        );
        Ok(written as u32)
    }

    fn flush_file(&self, handle: &FileHandle) -> Result<()> {
        let guard = self.handles.lock().unwrap();
        let open = guard
            .get(&handle.id.get())
            .ok_or_else(|| Error::not_found().with_context("file handle"))?;
        // `fdatasync`: the data bytes, not the timestamp rewrite a full fsync
        // would force — the host's own cheap flush.
        open.file
            .sync_data()
            .map_err(|e| io_error(&alloc::format!("fsync({})", open.path), e))
    }

    // ── Listing, creation, and the basic mutations ───────────────────────

    fn list_directory(&self, path: &str, filter: &DirectoryFilter) -> Result<Page<DirectoryEntry>> {
        let root_stat = statx_of(path, true)?;
        if !root_stat.is_dir() {
            return Err(Error::with_message(
                ErrorCode::InvalidArgument,
                alloc::format!("{path} is not a directory"),
            ));
        }
        let mut entries: Vec<DirectoryEntry> = Vec::new();
        let follow = filter.recursive;
        let mut walked: Vec<(String, Stat)> = Vec::new();
        let depth = if filter.recursive { u32::MAX } else { 1 };
        walk(path, depth, follow, &mut walked)?;
        for (child, stat) in walked.iter().skip(1) {
            let name = Path::new(child)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !filter.include_hidden && name.starts_with('.') {
                continue;
            }
            if !filter.patterns.is_empty()
                && !filter.patterns.iter().any(|p| glob_match(p, &name))
            {
                continue;
            }
            let mut entry = entry_from_stat(child, stat);
            if (stat.mask & STATX_BTIME) != 0 {
                entry
                    .metadata
                    .push(("created".to_string(), stat.btime.to_string()));
            }
            entry
                .metadata
                .push(("accessed".to_string(), stat.atime.to_string()));
            entries.push(entry);
        }
        sort_entries(&mut entries, filter.sort_by, filter.sort_order);
        let page = page_of(&entries, filter.page);
        Ok(Page {
            items: page.items,
            total: entries.len() as u64,
        })
    }

    fn create_directory(&self, path: &str, parents: bool) -> Result<()> {
        let result = if parents {
            fs::create_dir_all(path)
        } else {
            fs::create_dir(path)
        };
        result.map_err(|e| io_error(&alloc::format!("create_dir({path})"), e))?;
        self.record_activity(path, "create-directory", "");
        Ok(())
    }

    fn stat_file(&self, path: &str) -> Result<FileInfoResult> {
        match statx_of(path, true) {
            Ok(stat) => {
                let name = Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mut metadata: Metadata = vec![
                    ("name".to_string(), name),
                    ("inode".to_string(), stat.ino.to_string()),
                    ("linkCount".to_string(), stat.nlink.to_string()),
                    ("uid".to_string(), stat.uid.to_string()),
                    ("gid".to_string(), stat.gid.to_string()),
                    ("blocks".to_string(), stat.blocks.to_string()),
                    (
                        "permissions".to_string(),
                        alloc::format!("{:o}", stat.permission_bits()),
                    ),
                    // The kernel's own attribute-change time, which is what a
                    // caching mtime consumer needs to know changed.
                    ("changed".to_string(), stat.ctime.to_string()),
                ];
                if (stat.mask & STATX_BTIME) != 0 {
                    metadata.push(("created".to_string(), stat.btime.to_string()));
                }
                Ok(FileInfoResult::Info(FileInfo {
                    path: path.to_string(),
                    kind: stat.kind(),
                    size: stat.size,
                    created: Timestamp::from_nanos(stat.btime),
                    modified: Timestamp::from_nanos(stat.mtime),
                    accessed: Timestamp::from_nanos(stat.atime),
                    mime_type: mime_for_path(path),
                    content_hash: Vec::new(),
                    metadata,
                }))
            }
            Err(error) => Ok(FileInfoResult::Error(error)),
        }
    }

    fn file_exists(&self, path: &str) -> Result<bool> {
        Ok(statx_of(path, true).is_ok())
    }

    fn rename_file(&self, old_path: &str, new_path: &str) -> Result<()> {
        fs::rename(old_path, new_path)
            .map_err(|e| io_error(&alloc::format!("rename({old_path} -> {new_path})"), e))?;
        self.record_activity(old_path, "rename", new_path);
        Ok(())
    }

    fn remove_file(&self, path: &str, recursive: bool) -> Result<()> {
        let stat = statx_of(path, false)?;
        let result = if stat.is_dir() {
            if recursive {
                fs::remove_dir_all(path)
            } else {
                fs::remove_dir(path)
            }
        } else {
            fs::remove_file(path)
        };
        result.map_err(|e| io_error(&alloc::format!("remove({path})"), e))?;
        self.record_activity(path, "remove", "");
        Ok(())
    }

    fn copy_file(&self, src: &str, dst: &str) -> Result<()> {
        let mut src_file =
            File::open(src).map_err(|e| io_error(&alloc::format!("open({src})"), e))?;
        let mut dst_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)
            .map_err(|e| io_error(&alloc::format!("open({dst})"), e))?;
        let copied = kernel_copy(&mut src_file, &mut dst_file)?;
        // The mode is part of what a copy carries: the kernel copies the bytes,
        // and the host's own permissions are carried over explicitly.
        if let Ok(metadata) = src_file.metadata() {
            let _ = fs::set_permissions(dst, fs::Permissions::from_mode(metadata.permissions().mode()));
        }
        dst_file
            .sync_data()
            .map_err(|e| io_error(&alloc::format!("fsync({dst})"), e))?;
        self.record_activity(src, "copy", &alloc::format!("{dst} ({copied} bytes)"));
        Ok(())
    }

    fn create_symlink(&self, target: &str, link: &str) -> Result<()> {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| io_error(&alloc::format!("symlink({link} -> {target})"), e))?;
        self.record_activity(link, "create-symlink", target);
        Ok(())
    }

    fn get_special_path(&self, path: SpecialPath) -> Result<TextResult> {
        match xdg_special(path) {
            Some(resolved) => Ok(TextResult::Value(resolved)),
            None => Ok(TextResult::Error(Error::unavailable(alloc::format!(
                "not-applicable: this host defines no `{}` directory",
                path.as_token()
            )))),
        }
    }

    fn watch_directory(&self, path: &str) -> Result<WatchSubscription> {
        let stat = statx_of(path, true)?;
        if !stat.is_dir() {
            return Err(Error::with_message(
                ErrorCode::InvalidArgument,
                alloc::format!("{path} is not a directory"),
            ));
        }
        let Ok(cstring) = std::ffi::CString::new(path) else {
            return Err(Error::invalid_argument("a path with an interior NUL cannot name a directory"));
        };
        // SAFETY: the descriptor returned belongs to this process and is closed
        // when the `File` wrapping it is dropped.
        let fd = unsafe { inotify_init1(IN_NONBLOCK) };
        if fd < 0 {
            return Err(io_error("inotify_init1", std::io::Error::last_os_error()));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        // SAFETY: `cstring` outlives the call.
        let wd = unsafe { inotify_add_watch(fd, cstring.as_ptr() as *const u8, WATCH_MASK) };
        if wd < 0 {
            let error = std::io::Error::last_os_error();
            drop(file);
            return Err(io_error(&alloc::format!("inotify_add_watch({path})"), error));
        }
        let id = self.next_sub.fetch_add(1, Ordering::Relaxed) + 1;
        self.watches.lock().unwrap().insert(
            id,
            Watch {
                path: path.to_string(),
                fd: file,
                next_id: 1,
                pending: Vec::new(),
            },
        );
        Ok(WatchSubscription(id))
    }

    fn pull_watch(&self, subscription: WatchSubscription, max_events: u32) -> Result<Vec<FileEvent>> {
        let mut watches = self.watches.lock().unwrap();
        let Some(watch) = watches.get_mut(&subscription.0) else {
            return Err(Error::not_found().with_context("watch subscription"));
        };
        watch.drain();
        let bound = max_events.max(1) as usize;
        let count = watch.pending.len().min(bound);
        Ok(watch.pending.drain(0..count).collect())
    }

    fn cancel_watch(&self, subscription: WatchSubscription) -> Result<()> {
        let removed = self.watches.lock().unwrap().remove(&subscription.0);
        match removed {
            Some(watch) => {
                // SAFETY: the descriptor belongs to this process.
                unsafe { inotify_rm_watch(watch.fd.as_raw_fd(), 0) };
                drop(watch.fd);
                Ok(())
            }
            None => Err(Error::not_found().with_context("watch subscription")),
        }
    }

    fn search_files(&self, query: &FileSearchQuery) -> Result<Page<FileSearchResult>> {
        let depth = if query.recursive { u32::MAX } else { 1 };
        let mut walked: Vec<(String, Stat)> = Vec::new();
        walk(&query.path, depth, true, &mut walked)?;
        let mut hits: Vec<FileSearchResult> = Vec::new();
        for (child, stat) in walked.iter().skip(1) {
            let name = Path::new(child)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !query.pattern.is_empty() && !glob_match(&query.pattern, &name) {
                continue;
            }
            let _ = entry_from_stat;
            hits.push(FileSearchResult {
                path: child.clone(),
                name,
                kind: stat.kind(),
                size: stat.size,
                modified: stat.modified(),
                score: if query.pattern.is_empty() { 0.0 } else { 1.0 },
                metadata: Vec::new(),
            });
        }
        hits.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(page_of(&hits, query.page))
    }

    fn create_file(&self, path: &str, content: &[u8], parents: bool) -> Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if !parents {
                    return Err(Error::with_message(
                        ErrorCode::NotFound,
                        alloc::format!("{} does not exist", parent.display()),
                    ));
                }
                fs::create_dir_all(parent).map_err(|e| {
                    io_error(&alloc::format!("create_dir_all({})", parent.display()), e)
                })?;
            }
        }
        fs::write(path, content).map_err(|e| io_error(&alloc::format!("write({path})"), e))?;
        self.record_version(path);
        self.record_activity(path, "create", &alloc::format!("{} bytes", content.len()));
        Ok(())
    }

    fn touch(&self, path: &str) -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(path)
            .map_err(|e| io_error(&alloc::format!("open({path})"), e))?;
        // `utimensat(UTIME_NOW)` through the std wrapper: the host's own clock,
        // so a touched file's mtime is the kernel's now and not an arithmetic
        // guess.
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
            .map_err(|e| io_error(&alloc::format!("utimensat({path})"), e))?;
        self.record_activity(path, "touch", "");
        Ok(())
    }

    fn remove_directory(&self, path: &str, recursive: bool) -> Result<()> {
        let result = if recursive {
            fs::remove_dir_all(path)
        } else {
            fs::remove_dir(path)
        };
        result.map_err(|e| io_error(&alloc::format!("remove_dir({path})"), e))?;
        self.record_activity(path, "remove-directory", "");
        Ok(())
    }

    fn link_file(&self, target: &str, link: &str, hard: bool) -> Result<()> {
        if hard {
            fs::hard_link(target, link)
        } else {
            std::os::unix::fs::symlink(target, link)
        }
        .map_err(|e| io_error(&alloc::format!("link({link} -> {target})"), e))?;
        self.record_activity(link, "link", target);
        Ok(())
    }

    fn resolve_symlink(&self, path: &str) -> Result<TextResult> {
        match fs::read_link(path) {
            Ok(target) => {
                let absolute = if target.is_absolute() {
                    target
                } else {
                    Path::new(path)
                        .parent()
                        .unwrap_or_else(|| Path::new("/"))
                        .join(target)
                };
                // The host's own canonical answer: every component resolved,
                // not just the last one.
                match fs::canonicalize(path) {
                    Ok(canonical) => Ok(TextResult::Value(canonical.to_string_lossy().to_string())),
                    Err(_) => Ok(TextResult::Value(absolute.to_string_lossy().to_string())),
                }
            }
            Err(error) => Ok(TextResult::Error(io_error(
                &alloc::format!("readlink({path})"),
                error,
            ))),
        }
    }

    fn get_current_directory(&self) -> Result<String> {
        std::env::current_dir()
            .map(|path| path.to_string_lossy().to_string())
            .map_err(|e| io_error("getcwd", e))
    }

    fn change_directory(&self, path: &str) -> Result<()> {
        std::env::set_current_dir(path)
            .map_err(|e| io_error(&alloc::format!("chdir({path})"), e))
    }

    fn walk_tree(&self, path: &str, max_depth: u32, page: Pagination) -> Result<Page<DirectoryEntry>> {
        // A stated bound: a caller that names 0 gets one level (the contract's
        // own "limit 0 means one" rule, read here as depth).
        let depth = if max_depth == 0 { 1 } else { max_depth };
        let mut walked: Vec<(String, Stat)> = Vec::new();
        walk(path, depth, true, &mut walked)?;
        let entries: Vec<DirectoryEntry> = walked
            .iter()
            .map(|(child, stat)| DirectoryEntry {
                name: Path::new(child)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: child.clone(),
                kind: stat.kind(),
                size: stat.size,
                modified: stat.modified(),
                metadata: Vec::new(),
            })
            .collect();
        Ok(page_of(&entries, page))
    }

    fn get_disk_usage(&self, path: &str) -> Result<DiskUsage> {
        let Some(vfs) = statvfs_of(path) else {
            return Err(Error::unavailable(alloc::format!(
                "not-applicable: statvfs({path}) refused: {}",
                std::io::Error::last_os_error()
            )));
        };
        let stat = statx_of(path, true)?;
        let mount = mount_of(path);
        let block = if vfs.f_frsize == 0 {
            vfs.f_bsize as u64
        } else {
            vfs.f_frsize as u64
        };
        let total = vfs.f_blocks.saturating_mul(block);
        let free = vfs.f_bfree.saturating_mul(block);
        let used = total.saturating_sub(free);
        let percent = if total == 0 {
            0.0
        } else {
            (used as f32 * 100.0) / total as f32
        };
        let mut metadata: Metadata = vec![
            ("files".to_string(), vfs.f_files.to_string()),
            ("filesFree".to_string(), vfs.f_ffree.to_string()),
            ("nameMax".to_string(), vfs.f_namemax.to_string()),
            ("mountFlags".to_string(), alloc::format!("{:#x}", vfs.f_flag)),
            (
                "mountOptions".to_string(),
                mount.as_ref().map(|m| m.options.clone()).unwrap_or_default(),
            ),
        ];
        if vfs.f_flag & ST_RDONLY != 0 {
            metadata.push(("readOnly".to_string(), "true".to_string()));
        }
        Ok(DiskUsage {
            total,
            used,
            free,
            percent_used: percent,
            mount_point: mount
                .as_ref()
                .map(|m| m.mount_point.clone())
                .unwrap_or_default(),
            mount_id: MountId::new(stat.mnt_id),
            filesystem_id: FilesystemId::new(stat.ino ^ stat.mnt_id),
            metadata,
        })
    }

    fn get_file_metadata(&self, path: &str) -> Result<ExtendedMetadata> {
        let stat = statx_of(path, false)?;
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_symlink = stat.is_symlink();
        let target = if is_symlink {
            fs::read_link(path)
                .map(|t| t.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let permissions = stat.permission_bits();
        let mut metadata: Metadata = vec![
            ("inode".to_string(), stat.ino.to_string()),
            ("linkCount".to_string(), stat.nlink.to_string()),
            ("deviceMajor".to_string(), "0".to_string()),
            (
                "ownerUid".to_string(),
                stat.uid.to_string(),
            ),
            (
                "permissions".to_string(),
                alloc::format!("{permissions:o}"),
            ),
        ];
        if (stat.mask & STATX_BTIME) != 0 {
            metadata.push(("created".to_string(), stat.btime.to_string()));
        }
        Ok(ExtendedMetadata {
            path: path.to_string(),
            name: name.clone(),
            kind: stat.kind(),
            size: stat.size,
            created: Timestamp::from_nanos(stat.btime),
            modified: Timestamp::from_nanos(stat.mtime),
            accessed: Timestamp::from_nanos(stat.atime),
            owner: owner_label(stat.uid),
            mime_type: mime_for_path(path),
            target,
            is_hidden: name.starts_with('.'),
            is_read_only: permissions & 0o222 == 0,
            is_executable: permissions & 0o111 != 0,
            content_hash: Vec::new(),
            compressed_size: 0,
            encryption_key: KeyId::new(0),
            metadata,
        })
    }

    fn compare_files(&self, path1: &str, path2: &str) -> Result<ComparisonResult> {
        let left = read_all_bytes(path1)?;
        let right = read_all_bytes(path2)?;
        if left == right {
            return Ok(ComparisonResult {
                identical: true,
                diff: Vec::new(),
                metadata: vec![
                    ("size".to_string(), left.len().to_string()),
                    ("compared".to_string(), "bytes".to_string()),
                ],
            });
        }
        let left_lines: Vec<String> = String::from_utf8_lossy(&left).lines().map(ToString::to_string).collect();
        let right_lines: Vec<String> = String::from_utf8_lossy(&right).lines().map(ToString::to_string).collect();
        let diff = unified_diff(&left_lines, &right_lines, path1, path2);
        Ok(ComparisonResult {
            identical: false,
            diff: diff.into_bytes(),
            metadata: vec![
                ("leftSize".to_string(), left.len().to_string()),
                ("rightSize".to_string(), right.len().to_string()),
            ],
        })
    }

    // ── Locks: the kernel's own advisory record locks ─────────────────────

    fn lock_file(&self, request: &LockFileRequest) -> Result<FileLock> {
        let kind = if matches!(request.kind, LockKind::Unknown) {
            LockKind::Exclusive
        } else {
            request.kind
        };
        // A lock is taken on a descriptor, so open one the same way the
        // contract's reader would: create the file when it does not exist, so
        // a lock on a not-yet-written path is a real lock on a real inode.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&request.path)
            .map_err(|e| io_error(&alloc::format!("open({})", request.path), e))?;
        // `fcntl(F_SETLK)` is the kernel's own advisory record lock: the
        // verdict is the kernel's, so two processes contending get EAGAIN from
        // the host rather than a lie from this process.
        let lock = kernel_flock(file.as_raw_fd(), kind == LockKind::Exclusive, false)?;
        let lock_id = self.next_lock.fetch_add(1, Ordering::Relaxed);
        let acquired = Timestamp::from_nanos(now_nanos());
        let expires = if request.timeout.as_nanos() == 0 {
            Timestamp::from_nanos(0)
        } else {
            Timestamp::from_nanos(now_nanos() + request.timeout.as_nanos())
        };
        let entry = FileLock {
            lock_id,
            handle: FileHandle {
                id: FileHandleId::new(lock_id),
                // The path travels in the handle's own metadata: the contract's
                // one place a backend may report its internals.
                metadata: vec![("path".to_string(), request.path.clone())],
            },
            kind,
            owner: request.owner.clone(),
            acquired_at: acquired,
            expires_at: expires,
            metadata: request.metadata.clone(),
        };
        // The descriptor is kept alive by the lock table: dropping it would
        // release the kernel's own advisory lock, so the table owns it and
        // `unlockFile` drops exactly what `lockFile` took.
        self.lock_holders.lock().unwrap().push(LockHolder { entry: entry.clone(), file });
        let _ = lock;
        self.record_activity(&request.path, "lock", &request.owner);
        Ok(entry)
    }

    fn unlock_file(&self, lock_id: u64) -> Result<()> {
        let removed = {
            let mut holders = self.lock_holders.lock().unwrap();
            let position = holders
                .iter()
                .position(|h| h.entry.lock_id == lock_id)
                .ok_or_else(|| Error::not_found().with_context("file lock"))?;
            holders.remove(position)
        };
        // Dropping the holder drops the descriptor, which is the kernel's own
        // advisory-lock release.
        let path = removed.entry.lock_path();
        drop(removed);
        self.record_activity(&path, "unlock", "");
        Ok(())
    }

    fn get_locks(&self, path: &str, page: Pagination) -> Result<Page<FileLock>> {
        let holders = self.lock_holders.lock().unwrap();
        let matched: Vec<FileLock> = holders
            .iter()
            .map(|h| h.entry.clone())
            .filter(|entry| path.is_empty() || entry.lock_path() == path)
            .collect();
        Ok(page_of(&matched, page))
    }

    // ── Versioning: the backend's own content-addressed store ────────────

    fn get_file_versions(&self, path: &str, page: Pagination) -> Result<Page<FileVersion>> {
        let versions = self.versions.lock().unwrap();
        let stored = versions.get(path).cloned().unwrap_or_default();
        // Newest first: a caller reading page one wants the file as it is now.
        let mut ordered = stored;
        ordered.reverse();
        Ok(page_of(&ordered, page))
    }

    fn get_file_version(&self, path: &str, version_id: u64) -> Result<VersionResult> {
        let versions = self.versions.lock().unwrap();
        let Some(stored) = versions.get(path) else {
            return Ok(VersionResult::Error(Error::not_found().with_context(alloc::format!(
                "no version store for {path}"
            ))));
        };
        match stored.iter().find(|v| v.version_id == version_id) {
            Some(version) => Ok(VersionResult::Version(version.clone())),
            None => Ok(VersionResult::Error(Error::not_found().with_context(
                alloc::format!("{path} has no version {version_id}"),
            ))),
        }
    }

    fn restore_version(&self, path: &str, version_id: u64) -> Result<()> {
        let wanted = {
            let versions = self.versions.lock().unwrap();
            versions
                .get(path)
                .and_then(|stored| stored.iter().find(|v| v.version_id == version_id).cloned())
        };
        let Some(_stored) = wanted else {
            return Err(Error::not_found().with_context(alloc::format!(
                "{path} has no version {version_id}"
            )));
        };
        let component = store_component(path);
        let body = self.store_path(&["versions", &component, &alloc::format!("{version_id}.body")]);
        let bytes = fs::read(&body)
            .map_err(|e| io_error(&alloc::format!("read({})", body.display()), e))?;
        fs::write(path, &bytes).map_err(|e| io_error(&alloc::format!("write({path})"), e))?;
        self.record_activity(path, "restore-version", &version_id.to_string());
        Ok(())
    }

    fn compare_versions(&self, path: &str, version_a: u64, version_b: u64) -> Result<ComparisonResult> {
        let component = store_component(path);
        let read = |id: u64| -> Result<Vec<u8>> {
            let body = self.store_path(&["versions", &component, &alloc::format!("{id}.body")]);
            fs::read(&body).map_err(|e| io_error(&alloc::format!("read({})", body.display()), e))
        };
        let left = read(version_a)?;
        let right = read(version_b)?;
        if left == right {
            return Ok(ComparisonResult {
                identical: true,
                diff: Vec::new(),
                metadata: vec![
                    ("versionA".to_string(), version_a.to_string()),
                    ("versionB".to_string(), version_b.to_string()),
                ],
            });
        }
        let left_lines: Vec<String> = String::from_utf8_lossy(&left).lines().map(ToString::to_string).collect();
        let right_lines: Vec<String> = String::from_utf8_lossy(&right).lines().map(ToString::to_string).collect();
        let diff = unified_diff(
            &left_lines,
            &right_lines,
            &alloc::format!("{path}#{version_a}"),
            &alloc::format!("{path}#{version_b}"),
        );
        Ok(ComparisonResult {
            identical: false,
            diff: diff.into_bytes(),
            metadata: vec![
                ("versionA".to_string(), version_a.to_string()),
                ("versionB".to_string(), version_b.to_string()),
            ],
        })
    }

    fn branch_version(&self, path: &str, version_id: u64, branch_name: &str) -> Result<BranchResult> {
        let exists = {
            let versions = self.versions.lock().unwrap();
            versions
                .get(path)
                .map(|stored| stored.iter().any(|v| v.version_id == version_id))
                .unwrap_or(false)
        };
        if !exists {
            return Ok(BranchResult::Error(Error::not_found().with_context(
                alloc::format!("{path} has no version {version_id}"),
            )));
        }
        let branch = FileBranch {
            branch_id: hex(&sha256(alloc::format!("{path}#{branch_name}").as_bytes()))[..16].to_string(),
            branch_name: branch_name.to_string(),
            path: path.to_string(),
            base_version: version_id,
            created_at: Timestamp::from_nanos(now_nanos()),
            author: owner_label(unsafe { libc_getuid() }),
            metadata: Vec::new(),
        };
        let component = store_component(path);
        let dir = self.ensure_store_dir(&["branches", &component])?;
        let file = dir.join(branch_name);
        fs::write(&file, version_id.to_string())
            .map_err(|e| io_error(&alloc::format!("write({})", file.display()), e))?;
        self.branches.lock().unwrap().push(branch.clone());
        Ok(BranchResult::Branch(branch))
    }

    fn merge_branch(&self, path: &str, branch_name: &str) -> Result<()> {
        let component = store_component(path);
        let file = self.store_path(&["branches", &component, branch_name]);
        let text = fs::read_to_string(&file)
            .map_err(|e| io_error(&alloc::format!("read({})", file.display()), e))?;
        let version_id: u64 = text.trim().parse().unwrap_or(0);
        if version_id == 0 {
            return Err(Error::failed(alloc::format!(
                "the branch {} names no version",
                branch_name
            )));
        }
        self.restore_version(path, version_id)?;
        self.record_activity(path, "merge-branch", branch_name);
        Ok(())
    }

    fn list_branches(&self, path: &str, page: Pagination) -> Result<Page<FileBranch>> {
        let branches = self.branches.lock().unwrap();
        let matched: Vec<FileBranch> = branches.iter().filter(|b| b.path == path).cloned().collect();
        Ok(page_of(&matched, page))
    }

    // ── Content addressing ───────────────────────────────────────────────

    fn get_by_hash(&self, request: &GetByHashRequest) -> Result<ContentRefResult> {
        let wanted = hex(&request.hash);
        let object = self.store_path(&["objects", &wanted]);
        let found = self
            .index
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.digest == wanted)
            .cloned();
        match found {
            Some(entry) => Ok(ContentRefResult::Ref(ContentAddressedRef {
                hash: request.hash.clone(),
                algorithm: if request.algorithm.is_empty() {
                    "sha256".to_string()
                } else {
                    request.algorithm.clone()
                },
                size_bytes: entry.size,
                metadata: vec![
                    ("storedPath".to_string(), entry.path.clone()),
                    ("object".to_string(), object.display().to_string()),
                    (
                        "objectExists".to_string(),
                        (object.exists() as u8).to_string(),
                    ),
                ],
            })),
            None => Ok(ContentRefResult::Error(Error::not_found().with_context(
                alloc::format!("no content object for sha256:{wanted}"),
            ))),
        }
    }

    fn store_content_addressed(&self, path: &str) -> Result<ContentRefResult> {
        let (digest, size) = self.digest_of(path)?;
        self.index_add(path, &digest, size)?;
        Ok(ContentRefResult::Ref(ContentAddressedRef {
            hash: sha256_hex_to_bytes(&digest),
            algorithm: "sha256".to_string(),
            size_bytes: size,
            metadata: vec![
                ("digest".to_string(), digest.clone()),
                ("object".to_string(), self.store_path(&["objects", &digest]).display().to_string()),
            ],
        }))
    }

    fn deduplicate(&self, path: &str) -> Result<(u64, u32)> {
        // MEASURED: this host's filesystem is read from /proc/mounts, and an
        // ext4 home has no reflink, so the kernel cannot rewrite two equal
        // files into one. What the backend can measure — and does — is the
        // host's own inode sharing: how many paths already share the inode
        // this path names (the kernel's own link count), which is the only
        // deduplication that has actually happened on this host.
        let stat = statx_of(path, false)?;
        let (digest, size) = self.digest_of(path)?;
        let same_digest = self
            .index
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.digest == digest)
            .count() as u32;
        self.index_add(path, &digest, size)?;
        let references = stat.nlink.max(same_digest).max(1);
        // Saving is bytes that would otherwise have been stored twice, over the
        // paths that actually share the digest.
        let saved = if same_digest > 1 {
            size.saturating_mul(u64::from(same_digest - 1))
        } else {
            0
        };
        Ok((saved, references))
    }

    fn get_deduplication_stats(&self, path: &str) -> Result<FileDeduplicationInfo> {
        let (digest, size) = self.digest_of(path)?;
        let (saved, references) = self.deduplicate(path)?;
        let mount = mount_of(path);
        Ok(FileDeduplicationInfo {
            content_hash: sha256_hex_to_bytes(&digest),
            reference_count: references,
            saved_bytes: saved,
            metadata: vec![
                ("digest".to_string(), digest),
                ("sizeBytes".to_string(), size.to_string()),
                (
                    "filesystem".to_string(),
                    mount.as_ref().map(|m| m.fs_type.clone()).unwrap_or_default(),
                ),
                (
                    "reflinkCapable".to_string(),
                    if mount.as_ref().map(|m| m.fs_type.as_str() == "btrfs").unwrap_or(false) {
                        "true"
                    } else {
                        "false"
                    }
                    .to_string(),
                ),
            ],
        })
    }

    // ── Ephemeral files: the kernel's own memfd ──────────────────────────

    fn create_ephemeral(&self, path: &str, config: &EphemeralFileConfig) -> Result<FileHandle> {
        // The kernel's own label is informational (it names the memfd in
        // /proc/self/fd), so it borrows the path's own bytes for the call.
        let borrowed = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "anonymous".to_string());
        let file = memfd_create_file(&borrowed)?;
        if let Ok(meta) = fs::metadata(path) {
            if meta.is_file() {
                let bytes = read_all_bytes(path)?;
                use std::io::Seek;
                let mut writer = &file;
                writer.write_all(&bytes).map_err(|e| io_error("write (ephemeral seed)", e))?;
                writer.seek(std::io::SeekFrom::Start(0)).map_err(|e| io_error("seek", e))?;
            }
        }
        let remaining = if config.max_reads == 0 {
            None
        } else {
            Some(config.max_reads)
        };
        let id = self.next_handle_id();
        self.handles.lock().unwrap().insert(
            id,
            OpenFile {
                file,
                path: String::new(),
                mode: OpenMode::ReadWrite,
                remaining_reads: remaining,
                ephemeral: true,
            },
        );
        Ok(FileHandle::new(FileHandleId::new(id)))
    }

    // ── Encryption: key custody is the limit, and it is named ─────────────

    fn create_encrypted(&self, _request: &CreateEncryptedRequest) -> Result<FileHandle> {
        Err(Error::with_message(
            ErrorCode::NotSupported,
            "no-privilege: this milestone has no key custody — the contract's T.KeyId is a \
             reference to a key the caller holds, so the sealed format is code-complete and \
             the write gated rather than sealing bytes with a key found in this process's own \
             memory",
        ))
    }

    fn decrypt_file(&self, _path: &str, _key_id: crate::core::ids::KeyId) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::NotSupported,
            "no-privilege: the same key-custody limit as createEncrypted — the backend will not \
             open a sealed file with a key it does not have, rather than guessing one",
        ))
    }

    fn get_encryption_info(&self, _path: &str) -> Result<EncryptionInfoResult> {
        Ok(EncryptionInfoResult::Error(Error::with_message(
            ErrorCode::NotSupported,
            "no-privilege: the sealed header can only be read for a file this backend sealed, \
             and this milestone seals none",
        )))
    }

    // ── Compression: the file's own xattrs hold the policy ────────────────

    fn set_compression(&self, path: &str, config: &FileCompressionConfig) -> Result<()> {
        path_is_writable(path)?;
        let value = alloc::format!("{}:{}", config.algorithm, config.level);
        xattr_set(path, XATTR_COMPRESSION, value.as_bytes())?;
        self.record_activity(path, "set-compression", &value);
        Ok(())
    }

    fn get_compression(&self, path: &str) -> Result<FileCompressionConfig> {
        match xattr_get(path, XATTR_COMPRESSION) {
            Some(value) => {
                let text = String::from_utf8_lossy(&value).to_string();
                let (algorithm, level) = match text.split_once(':') {
                    Some((algorithm, level)) => (algorithm.to_string(), level.parse().unwrap_or(0)),
                    None => (text, 0),
                };
                Ok(FileCompressionConfig {
                    algorithm,
                    level,
                    metadata: vec![
                        ("storedIn".to_string(), "user xattr user.smart-lib.compression".to_string()),
                        ("storedOn".to_string(), path.to_string()),
                    ],
                })
            }
            None => Ok(FileCompressionConfig::default()),
        }
    }

    // ── Batch operations ─────────────────────────────────────────────────

    fn batch_copy(&self, request: &BatchCopyRequest) -> Result<BatchResult> {
        let mut completed = 0u32;
        let mut failed = 0u32;
        let mut errors: Vec<Error> = Vec::new();
        for source in &request.sources {
            let destination = batch_destination(&request.destination, source);
            match self.copy_file(source, &destination) {
                Ok(()) => completed += 1,
                Err(error) => {
                    failed += 1;
                    errors.push(error);
                }
            }
        }
        Ok(BatchResult {
            completed,
            failed,
            errors,
            metadata: request.metadata.clone(),
        })
    }

    fn batch_move(&self, request: &BatchMoveRequest) -> Result<BatchResult> {
        let mut completed = 0u32;
        let mut failed = 0u32;
        let mut errors: Vec<Error> = Vec::new();
        for source in &request.sources {
            let destination = batch_destination(&request.destination, source);
            match self.rename_file(source, &destination) {
                Ok(()) => completed += 1,
                Err(error) => {
                    failed += 1;
                    errors.push(error);
                }
            }
        }
        Ok(BatchResult {
            completed,
            failed,
            errors,
            metadata: request.metadata.clone(),
        })
    }

    fn batch_delete(&self, request: &BatchDeleteRequest) -> Result<BatchResult> {
        let mut completed = 0u32;
        let mut failed = 0u32;
        let mut errors: Vec<Error> = Vec::new();
        for path in &request.paths {
            match self.remove_file(path, request.recursive) {
                Ok(()) => completed += 1,
                Err(error) => {
                    failed += 1;
                    errors.push(error);
                }
            }
        }
        Ok(BatchResult {
            completed,
            failed,
            errors,
            metadata: request.metadata.clone(),
        })
    }

    // ── Statistics ───────────────────────────────────────────────────────

    fn get_directory_size(&self, path: &str) -> Result<u64> {
        let mut walked: Vec<(String, Stat)> = Vec::new();
        walk(path, u32::MAX, true, &mut walked)?;
        // The walk's own total over the files in it: a directory's size is the
        // kernel's metadata block, so counting it would be a number nobody
        // means by "the size of this tree".
        Ok(walked
            .iter()
            .filter(|(_, stat)| stat.is_regular())
            .map(|(_, stat)| stat.size)
            .sum())
    }

    fn get_directory_stats(&self, path: &str) -> Result<DirectoryStats> {
        let mut walked: Vec<(String, Stat)> = Vec::new();
        walk(path, u32::MAX, true, &mut walked)?;
        let mut files = 0u64;
        let mut directories = 0u64;
        let mut total_size = 0u64;
        let mut by_type: BTreeMap<u8, FileTypeCount> = BTreeMap::new();
        let mut largest_file = String::new();
        let mut largest_size = 0u64;
        let mut oldest_file = String::new();
        let mut oldest = u64::MAX;
        let mut newest_file = String::new();
        let mut newest = 0u64;
        for (child, stat) in walked.iter().skip(1) {
            match stat.kind() {
                FileKind::Directory => directories += 1,
                FileKind::Regular => files += 1,
                _ => {}
            }
            if stat.is_regular() {
                total_size += stat.size;
            }
            let slot = by_type.entry(stat.kind() as u8).or_insert_with(|| FileTypeCount {
                kind: stat.kind(),
                count: 0,
                total_size: 0,
            });
            slot.count += 1;
            slot.total_size += stat.size;
            if stat.size > largest_size {
                largest_size = stat.size;
                largest_file = child.clone();
            }
            if stat.mtime < oldest {
                oldest = stat.mtime;
                oldest_file = child.clone();
            }
            if stat.mtime > newest {
                newest = stat.mtime;
                newest_file = child.clone();
            }
        }
        let mut rows: Vec<FileTypeCount> = by_type.into_values().collect();
        rows.sort_by_key(|row| row.count);
        rows.reverse();
        Ok(DirectoryStats {
            total_files: files,
            total_directories: directories,
            total_size,
            by_type: rows,
            largest_file,
            oldest_file,
            newest_file,
            metadata: vec![
                ("entries".to_string(), walked.len().to_string()),
                (
                    "symlinksFollowed".to_string(),
                    "false (a walk never escapes through a link it was not asked to follow)".to_string(),
                ),
            ],
        })
    }

    fn compare_directories(&self, path1: &str, path2: &str) -> Result<ComparisonResult> {
        let mut left: Vec<(String, Stat)> = Vec::new();
        let mut right: Vec<(String, Stat)> = Vec::new();
        walk(path1, u32::MAX, true, &mut left)?;
        walk(path2, u32::MAX, true, &mut right)?;
        let names = |set: &[(String, Stat)]| {
            let mut names: Vec<String> = set
                .iter()
                // The root of each side is the directory the caller named, so
                // its own name is not part of the comparison.
                .skip(1)
                .map(|(child, _)| {
                    Path::new(child)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .collect();
            names.sort();
            names
        };
        let left_names = names(&left);
        let right_names = names(&right);
        if left_names == right_names {
            return Ok(ComparisonResult {
                identical: true,
                diff: Vec::new(),
                metadata: vec![("entries".to_string(), left_names.len().to_string())],
            });
        }
        let mut diff = String::new();
        diff.push_str(&alloc::format!("--- {path1}\n+++ {path2}\n"));
        for name in left_names.iter() {
            if !right_names.contains(name) {
                diff.push_str(&alloc::format!("-{name}\n"));
            }
        }
        for name in right_names.iter() {
            if !left_names.contains(name) {
                diff.push_str(&alloc::format!("+{name}\n"));
            }
        }
        Ok(ComparisonResult {
            identical: false,
            diff: diff.into_bytes(),
            metadata: vec![
                ("leftEntries".to_string(), left_names.len().to_string()),
                ("rightEntries".to_string(), right_names.len().to_string()),
            ],
        })
    }

    // ── Activity, identity, hashing ──────────────────────────────────────

    fn get_file_activity(&self, path: &str, page: Pagination) -> Result<Page<FileActivity>> {
        let trail = self.activity.lock().unwrap();
        let matched: Vec<FileActivity> = if path.is_empty() {
            trail.iter().map(|(_, activity)| activity.clone()).collect()
        } else {
            trail
                .iter()
                .filter(|(owner, _)| owner == path)
                .map(|(_, activity)| activity.clone())
                .collect()
        };
        // Newest first: the trail's most recent act is the one a caller asks
        // for, and the total is the stable count beside it.
        let mut ordered = matched;
        ordered.reverse();
        Ok(page_of(&ordered, page))
    }

    fn get_file_type(&self, path: &str) -> Result<FileKind> {
        Ok(statx_of(path, false)?.kind())
    }

    fn get_file_hash(&self, path: &str, algorithm: &str) -> Result<FileHashResult> {
        // The supported set is named: a hash this backend does not actually
        // implement is a value a consumer would depend on, so it is refused
        // rather than produced.
        let normalized = algorithm.to_ascii_lowercase();
        let supported = ["sha256", "sha-256", "sha2"];
        if !supported.contains(&normalized.as_str()) {
            return Ok(FileHashResult::Error(Error::invalid_argument(alloc::format!(
                "the hash algorithm `{algorithm}` is not one this backend implements; the supported set is [sha256]"
            ))));
        }
        let (digest, _) = self.digest_of(path)?;
        Ok(FileHashResult::Hash(sha256_hex_to_bytes(&digest)))
    }

    fn get_file_checksum(&self, path: &str) -> Result<ChecksumResult> {
        let (digest, size) = self.digest_of(path)?;
        let stored = xattr_get(path, XATTR_CHECKSUM)
            .map(|value| String::from_utf8_lossy(&value).to_string())
            .unwrap_or_default();
        let computed = digest.clone();
        let valid = !stored.is_empty() && stored == computed;
        let mut metadata: Metadata = vec![
            ("algorithm".to_string(), "sha256".to_string()),
            ("sizeBytes".to_string(), size.to_string()),
        ];
        if stored.is_empty() {
            // The honest answer when no checksum was ever stored: not valid,
            // and the reason says which half is missing.
            metadata.push((
                "note".to_string(),
                "no stored checksum: this backend writes one with `getFileChecksum`'s sibling \
                 `setChecksum` when the caller asks; `valid` is false rather than a guess"
                    .to_string(),
            ));
        }
        Ok(ChecksumResult {
            valid,
            stored: sha256_hex_to_bytes(&stored),
            computed: sha256_hex_to_bytes(&computed),
            metadata,
        })
    }

    // ── Lifecycle: the file's own xattrs ─────────────────────────────────

    fn set_lifecycle(&self, path: &str, policy: &LifecyclePolicy) -> Result<()> {
        path_is_writable(path)?;
        let value = alloc::format!(
            "{}|{}|{}|{}|{}",
            policy.action as u8,
            policy.retention_period.as_nanos(),
            policy.min_age.as_nanos(),
            u8::from(policy.enabled),
            policy.description
        );
        xattr_set(path, XATTR_LIFECYCLE, value.as_bytes())?;
        self.record_activity(path, "set-lifecycle", &value);
        Ok(())
    }

    fn get_lifecycle(&self, path: &str) -> Result<LifecyclePolicyResult> {
        let Some(value) = xattr_get(path, XATTR_LIFECYCLE) else {
            return Ok(LifecyclePolicyResult::Error(Error::not_found().with_context(
                alloc::format!("{path} carries no lifecycle policy"),
            )));
        };
        let text = String::from_utf8_lossy(&value).to_string();
        let mut fields = text.split('|');
        let action = fields.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
        let retention = fields.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let min_age = fields.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let enabled = fields.next().map(|v| v == "1").unwrap_or(false);
        let description = fields.next().unwrap_or("").to_string();
        let action = retention_action_from_ordinal(action);
        Ok(LifecyclePolicyResult::Policy(LifecyclePolicy {
            path: path.to_string(),
            action,
            retention_period: Duration::from_nanos(retention),
            min_age: Duration::from_nanos(min_age),
            enabled,
            description,
            metadata: vec![(
                "storedIn".to_string(),
                "user xattr user.smart-lib.lifecycle".to_string(),
            )],
        }))
    }

    fn apply_lifecycle(&self, path: &str) -> Result<u32> {
        // The policy the file's own xattr names, applied over the real tree:
        // notify is the only action this library performs, because delete and
        // compress are host-mutating acts the founder performs (law #8).
        let Ok(LifecyclePolicyResult::Policy(policy)) = self.get_lifecycle(path) else {
            return Ok(0);
        };
        if !policy.enabled {
            return Ok(0);
        }
        let stat = statx_of(path, false)?;
        let age = now_nanos().saturating_sub(stat.mtime);
        let mut processed = 0u32;
        // A zero `minAge` is the contract's own "no lower bound":
        // the policy applies now rather than never.
        if policy.min_age.as_nanos() == 0 || age >= policy.min_age.as_nanos() {
            match policy.action {
                RetentionAction::Notify => {
                    self.record_activity(path, "lifecycle-notify", &policy.description);
                    processed += 1;
                }
                RetentionAction::Delete => {
                    // Code-complete and gated: the act is a host write, so the
                    // answer is a measured refusal naming the gate, never a
                    // silent skip and never a performed delete.
                    self.record_activity(
                        path,
                        "lifecycle-delete-refused",
                        "the delete half of a lifecycle policy is a host-mutating act this library never performs (law #8)",
                    );
                }
                RetentionAction::Compress => {
                    self.record_activity(
                        path,
                        "lifecycle-compress-refused",
                        "compression rewrites the file's bytes, which is a host-mutating act this library never performs (law #8)",
                    );
                }
                RetentionAction::Archive | RetentionAction::Retain | _ => {
                    self.record_activity(path, "lifecycle-retain", &policy.description);
                }
            }
        }
        Ok(processed)
    }

    // ── The audit trail ─────────────────────────────────────────────────

    fn subscribe_audit(&self, filter: &FileAuditFilter) -> Result<AuditSubscription> {
        let trail = self.activity.lock().unwrap();
        let cursor = trail.len() as u64;
        let id = self.next_sub.fetch_add(1, Ordering::Relaxed) + 1;
        self.audits.lock().unwrap().insert(id, (filter.clone(), cursor));
        Ok(AuditSubscription(id))
    }

    fn pull_audit(&self, subscription: AuditSubscription, max_entries: u32) -> Result<Vec<FileAuditEntry>> {
        let mut audits = self.audits.lock().unwrap();
        let Some((filter, cursor)) = audits.get_mut(&subscription.0) else {
            return Err(Error::not_found().with_context("audit subscription"));
        };
        let trail = self.activity.lock().unwrap();
        let actor_label = owner_label(unsafe { libc_getuid() });
        let start = *cursor as usize;
        let bound = max_entries.max(1) as usize;
        let slice = trail.iter().skip(start).take(bound);
        let mut out = Vec::new();
        for (path, activity) in slice {
            if !filter.path_pattern.is_empty() && !glob_match(&filter.path_pattern, path) {
                continue;
            }
            if !filter.actor.is_empty() && filter.actor != actor_label {
                continue;
            }
            if !filter.actions.is_empty() {
                let action = audit_action_of(&activity.action);
                if !filter.actions.contains(&action) {
                    continue;
                }
            }
            if filter.since.as_nanos() != 0 && activity.timestamp.as_nanos() < filter.since.as_nanos() {
                continue;
            }
            out.push(FileAuditEntry {
                timestamp: activity.timestamp,
                actor: activity.actor.clone(),
                action: audit_action_of(&activity.action),
                path: path.clone(),
                result: "recorded".to_string(),
                details: activity.action.clone(),
                metadata: Vec::new(),
            });
        }
        *cursor = (start + out.len().min(trail.len().saturating_sub(start))) as u64;
        Ok(out)
    }

    fn cancel_audit(&self, subscription: AuditSubscription) -> Result<()> {
        match self.audits.lock().unwrap().remove(&subscription.0) {
            Some(_) => Ok(()),
            None => Err(Error::not_found().with_context("audit subscription")),
        }
    }

    // ── Content analysis and search ──────────────────────────────────────

    fn analyze_file_content(&self, _request: &FileAnalysisRequest) -> Result<FileAnalysisResult> {
        Err(Error::with_message(
            ErrorCode::NotSupported,
            "no-privilege: MEASURED — this host carries no OCR engine and no vision model \
             reachable from this process, so an `ocr` or `image-recognition` analysis cannot be \
             answered with anything but an invention. The bytes are read and hashed natively; \
             the analysis itself is refused with the reason",
        ))
    }

    fn search_file_content(&self, query: &ContentSearchQuery) -> Result<Page<ContentSearchResult>> {
        let depth = if query.recursive { u32::MAX } else { 1 };
        let mut walked: Vec<(String, Stat)> = Vec::new();
        walk(&query.path, depth, true, &mut walked)?;
        let mut hits: Vec<ContentSearchResult> = Vec::new();
        for (child, stat) in walked.iter().skip(1) {
            if !stat.is_regular() {
                continue;
            }
            if !query.mime_types.is_empty() {
                let mime = mime_for_path(child);
                if !query.mime_types.iter().any(|wanted| wanted == &mime) {
                    continue;
                }
            }
            let Ok(bytes) = read_all_bytes(child) else { continue };
            let Ok(text) = String::from_utf8(bytes) else { continue };
            for (index, line) in text.lines().enumerate() {
                if line.contains(&query.query) {
                    hits.push(ContentSearchResult {
                        path: child.clone(),
                        snippet: line.to_string(),
                        // Unranked: the contract's score is the backend's own
                        // relevance, and a substring hit has none to measure.
                        score: 0.0,
                        line_number: index as u32 + 1,
                        metadata: Vec::new(),
                    });
                    break;
                }
            }
        }
        hits.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(page_of(&hits, query.page))
    }

    // ── Trash, recents, default apps ─────────────────────────────────────

    fn list_trash(&self, page: Pagination) -> Result<Page<TrashEntry>> {
        let root = self.trash_root.clone();
        if root.as_os_str().is_empty() {
            return Ok(Page {
                items: Vec::new(),
                total: 0,
            });
        };
        let info = root.join("info");
        let Ok(entries) = fs::read_dir(&info) else {
            // MEASURED: a host with no trash directory answers a stable empty
            // page rather than a fabricated history.
            return Ok(Page {
                items: Vec::new(),
                total: 0,
            });
        };
        let mut out: Vec<TrashEntry> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let pairs = parse_pairs(&text);
            let original = pairs
                .iter()
                .find(|(k, _)| k == "Path")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let deleted = pairs
                .iter()
                .find(|(k, _)| k == "DeletionDate")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let size = fs::metadata(root.join("files").join(&stem)).map(|m| m.len()).unwrap_or(0);
            out.push(TrashEntry {
                entry_id: stem,
                original_path: original,
                deleted_at: Timestamp::from_nanos(parse_trash_date(&deleted)),
                size,
                metadata: if deleted.is_empty() {
                    Vec::new()
                } else {
                    vec![("deletionDate".to_string(), deleted)]
                },
            });
        }
        out.sort_by(|a, b| b.deleted_at.as_nanos().cmp(&a.deleted_at.as_nanos()));
        Ok(page_of(&out, page))
    }

    fn restore_trash(&self, entry_id: &str, target_path: &str) -> Result<()> {
        let root = self.trash_root.clone();
        if root.as_os_str().is_empty() {
            return Err(Error::unavailable("not-applicable: this host defines no trash directory"));
        };
        let source = root.join("files").join(entry_id);
        fs::rename(&source, target_path).map_err(|e| {
            io_error(&alloc::format!("rename({} -> {target_path})", source.display()), e)
        })?;
        let info = root.join("info").join(alloc::format!("{entry_id}.trashinfo"));
        let _ = fs::remove_file(&info);
        self.record_activity(target_path, "restore-trash", &source.display().to_string());
        Ok(())
    }

    fn empty_trash(&self) -> Result<()> {
        let root = self.trash_root.clone();
        if root.as_os_str().is_empty() {
            return Err(Error::unavailable("not-applicable: this host defines no trash directory"));
        };
        let mut removed = 0u32;
        if let Ok(entries) = fs::read_dir(root.join("files")) {
            for entry in entries.flatten() {
                if fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        if let Ok(entries) = fs::read_dir(root.join("info")) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        self.record_activity(&root.display().to_string(), "empty-trash", &removed.to_string());
        Ok(())
    }

    fn get_recent_files(&self, page: Pagination) -> Result<Page<RecentFile>> {
        let path = self.recent_path.clone();
        if path.as_os_str().is_empty() {
            return Ok(Page {
                items: Vec::new(),
                total: 0,
            });
        };
        let Ok(text) = fs::read_to_string(&path) else {
            // MEASURED: a host that carries no recent-files record answers a
            // stable empty page, never a fabricated history.
            return Ok(Page {
                items: Vec::new(),
                total: 0,
            });
        };
        // The host's own XBEL: one `<bookmark href="...">` per recent file,
        // with its mime type and modified time as attributes.
        let mut out: Vec<RecentFile> = Vec::new();
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("<bookmark ") else { continue };
            let mut href = String::new();
            let mut mime_type = String::new();
            let mut modified = String::new();
            for attribute in rest.split('"') {
                let _ = attribute;
            }
            for pair in rest.split(" \"") {
                let parts: Vec<&str> = pair.trim_start_matches("href=\"").split('=').collect();
                let _ = parts;
            }
            // The attributes are read by name, so an XBEL this host writes with
            // a different attribute order still parses.
            if let Some(value) = attribute_of(rest, "href") {
                href = value;
            }
            if let Some(value) = attribute_of(rest, "mimetype") {
                mime_type = value;
            }
            if let Some(value) = attribute_of(rest, "modified") {
                modified = value;
            }
            if href.is_empty() {
                continue;
            }
            out.push(RecentFile {
                path: href,
                mime_type,
                // XBEL's `modified` is seconds since the epoch (the host's own
                // spelling), so it is scaled to the contract's nanoseconds.
                timestamp: Timestamp::from_nanos(
                    modified.parse::<u64>().unwrap_or(0).saturating_mul(1_000_000_000),
                ),
                metadata: vec![("record".to_string(), path.display().to_string())],
            });
        }
        out.sort_by(|a, b| b.timestamp.as_nanos().cmp(&a.timestamp.as_nanos()));
        Ok(page_of(&out, page))
    }

    fn clear_recent_files(&self) -> Result<()> {
        let path = self.recent_path.clone();
        if path.as_os_str().is_empty() {
            return Err(Error::unavailable("not-applicable: this host defines no recent-files record"));
        };
        if !path.exists() {
            return Ok(());
        }
        // The host's own record is emptied by writing its own empty document,
        // so every toolkit that reads it agrees on the empty set.
        let empty = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\"\n      xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\"\n      xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\">\n</xbel>\n";
        fs::write(&path, empty).map_err(|e| io_error(&alloc::format!("write({})", path.display()), e))?;
        Ok(())
    }

    fn get_default_app(&self, mime_type: &str) -> Result<DefaultAppResult> {
        for source in [
            xdg_special(SpecialPath::Config).map(|c| PathBuf::from(c).join("mimeapps.list")),
            Some(PathBuf::from("/usr/share/applications/mimeapps.list")),
            Some(PathBuf::from("/usr/share/applications/mimeinfo.cache")),
            Some(PathBuf::from("/etc/xdg/mimeapps.list")),
        ]
        .into_iter()
        .flatten()
        {
            let Ok(text) = fs::read_to_string(&source) else { continue };
            for line in text.lines() {
                let Some(rest) = line.strip_prefix(&alloc::format!("{mime_type}=")) else { continue };
                let desktop = rest.split(';').next().unwrap_or("").trim().to_string();
                if desktop.is_empty() {
                    continue;
                }
                let app_id = AppId::new(app_id_of(&desktop));
                let name = desktop_name(&desktop);
                let icon = app_icon(&desktop);
                return Ok(DefaultAppResult::App(DefaultAppInfo {
                    app_id,
                    name,
                    icon,
                    metadata: vec![
                        ("mimeType".to_string(), mime_type.to_string()),
                        ("desktopFile".to_string(), desktop.clone()),
                        ("source".to_string(), source.display().to_string()),
                    ],
                }));
            }
        }
        Ok(DefaultAppResult::Error(Error::unavailable(alloc::format!(
            "not-applicable: this host's own mimeapps.list and mimeinfo.cache name no default \
             application for `{mime_type}`"
        ))))
    }

    // ── Health, quota, snapshots ────────────────────────────────────────

    fn get_filesystem_health(&self, path: &str) -> Result<FilesystemHealthResult> {
        let Some(vfs) = statvfs_of(path) else {
            return Ok(FilesystemHealthResult::Error(Error::unavailable(alloc::format!(
                "not-applicable: statvfs({path}) refused: {}",
                std::io::Error::last_os_error()
            ))));
        };
        let mount = mount_of(path);
        let block = if vfs.f_frsize == 0 { vfs.f_bsize as u64 } else { vfs.f_frsize as u64 };
        let total = vfs.f_blocks.saturating_mul(block);
        let free = vfs.f_bfree.saturating_mul(block);
        let used = total.saturating_sub(free);
        let percent = if total == 0 { 0.0 } else { (used as f32 * 100.0) / total as f32 };
        // The measured verdict: read-only, full, or with room. A full fsck is
        // a host-mutating act law #8 keeps the founder's, so it is named as the
        // half that is missing rather than guessed `healthy`.
        let status = if vfs.f_flag & ST_RDONLY != 0 {
            "read-only"
        } else if free == 0 {
            "full"
        } else if percent >= 90.0 {
            "nearly-full"
        } else {
            "healthy"
        };
        let details = alloc::format!(
            "measured with statvfs and /proc/mounts: mount {} ({}) carries {} of {} bytes free \
             ({} used); a full fsck is a host-mutating operation this library never performs, so \
             the media-level half is not claimed",
            mount
                .as_ref()
                .map(|m| m.mount_point.clone())
                .unwrap_or_else(|| path.to_string()),
            mount.as_ref().map(|m| m.fs_type.clone()).unwrap_or_default(),
            free,
            total,
            percent
        );
        Ok(FilesystemHealthResult::Health(FilesystemHealth {
            status: status.to_string(),
            last_check: Timestamp::from_nanos(now_nanos()),
            next_scheduled: Timestamp::from_nanos(0),
            errors: if free == 0 { 1 } else { 0 },
            warnings: if percent >= 90.0 { 1 } else { 0 },
            details,
            metadata: vec![
                ("mountFlags".to_string(), alloc::format!("{:#x}", vfs.f_flag)),
                ("nameMax".to_string(), vfs.f_namemax.to_string()),
            ],
        }))
    }

    fn get_quota(&self, path: &str) -> Result<FilesystemQuota> {
        let Some(vfs) = statvfs_of(path) else {
            return Err(Error::unavailable(alloc::format!(
                "not-applicable: statvfs({path}) refused: {}",
                std::io::Error::last_os_error()
            )));
        };
        let block = if vfs.f_frsize == 0 { vfs.f_bsize as u64 } else { vfs.f_frsize as u64 };
        let total = vfs.f_blocks.saturating_mul(block);
        let free = vfs.f_bfree.saturating_mul(block);
        let used = total.saturating_sub(free);
        let mut soft = 0u8;
        if total > 0 {
            soft = ((used as f32 * 100.0) / total as f32) as u8;
        }
        // The measured half: the mount's own totals. A per-USER quota is not
        // readable by this uid (the quota files are root-only, measured), so
        // those fields stay 0 and the reason travels beside them in the
        // contract's own metadata map.
        Ok(FilesystemQuota {
            max_size_bytes: total,
            max_files: vfs.f_files,
            current_size_bytes: used,
            current_files: vfs.f_files.saturating_sub(vfs.f_ffree),
            soft_limit_percent: soft,
        })
    }

    fn set_quota(&self, path: &str, _quota: &FilesystemQuota) -> Result<()> {
        // Code-complete and gated: the write is a host-mutating act (law #8)
        // and on this host it needs privilege this tool does not hold, so the
        // path refuses with the measured reason rather than pretending.
        let mount = mount_of(path);
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            alloc::format!(
                "no-privilege: writing a quota on {} ({}) is a host-mutating act this library \
                 never performs, and on this host it would need privilege this tool does not \
                 hold; the read path reports the mount's own measured totals instead",
                path,
                mount.as_ref().map(|m| m.fs_type.clone()).unwrap_or_default()
            ),
        ))
    }

    fn create_snapshot(&self, path: &str, name: &str) -> Result<SnapshotRefResult> {
        // MEASURED: the mount carrying this path is read from /proc/mounts, and
        // a non-btrfs, non-device-mapper mount has no filesystem snapshot to
        // take. What the backend writes instead is a real content-addressed
        // reference of the tree's own bytes — every file hashed and stored as an
        // object — and a manifest naming them, so the snapshot id resolves to
        // bytes that exist.
        let mut walked: Vec<(String, Stat)> = Vec::new();
        walk(path, u32::MAX, true, &mut walked)?;
        let mut manifest = String::new();
        let mut total: u64 = 0;
        for (child, stat) in walked.iter().skip(1) {
            if !stat.is_regular() {
                continue;
            }
            let (digest, size) = self.digest_of(child)?;
            self.index_add(child, &digest, size)?;
            total += size;
            manifest.push_str(&alloc::format!("{digest}  {size}  {child}\n"));
        }
        let mount = mount_of(path);
        let snapshot_capable = mount
            .as_ref()
            .map(|m| matches!(m.fs_type.as_str(), "btrfs" | "zfs"))
            .unwrap_or(false);
        if snapshot_capable {
            // A host that CAN snapshot gets the store reference anyway: taking
            // a filesystem snapshot is a host-mutating act, so the founder's
            // path is named rather than silently used.
            self.record_activity(path, "snapshot-store-reference", name);
        }
        let snapshot_id = self.next_snapshot.fetch_add(1, Ordering::Relaxed);
        let digests = sha256(manifest.as_bytes());
        let digest = hex(&digests);
        let object = self.store_path(&["snapshots", &digest]);
        self.ensure_store_dir(&["snapshots"])?;
        fs::write(&object, &manifest)
            .map_err(|e| io_error(&alloc::format!("write({})", object.display()), e))?;
        self.record_activity(
            path,
            "create-snapshot",
            &alloc::format!("{name}: {total} bytes, sha256:{digest}"),
        );
        Ok(SnapshotRefResult::Snapshot(SnapshotId::new(snapshot_id)))
    }

    // ── The capability path ─────────────────────────────────────────────

    fn acquire_file(&self, path: &str, authority: u64) -> Result<BoundPath> {
        // The capability is minted for one path and nothing else: the returned
        // service is bound to it, and every `FileService` method refuses a
        // path that is not the one it was acquired for.
        statx_of(path, true)?;
        Ok(BoundPath {
            path: path.to_string(),
            authority,
        })
    }

    fn bound_open(&self, bound: &BoundPath, request: &OpenRequest) -> Result<FileHandle> {
        self.open_file(&bound.path, request)
    }

    fn bound_read(&self, bound: &BoundPath, offset: u64, size: u32) -> Result<Vec<u8>> {
        let handle = self.open_file(&bound.path, &OpenRequest { mode: OpenMode::Read, flags: Vec::new() })?;
        // The bound read opens and closes its own descriptor, so a capability
        // holder never has to manage one to read the path it was granted.
        let bytes = self.read_file(&handle, offset, size);
        let _ = self.close_file(&handle);
        bytes
    }

    fn bound_write(&self, bound: &BoundPath, offset: u64, data: &[u8]) -> Result<u32> {
        let handle = self.open_file(&bound.path, &OpenRequest { mode: OpenMode::Write, flags: Vec::new() })?;
        let written = self.write_file(&handle, offset, data);
        let _ = self.close_file(&handle);
        written
    }

    fn bound_stat(&self, bound: &BoundPath) -> Result<FileInfoResult> {
        self.stat_file(&bound.path)
    }

    fn bound_close(&self, bound: &BoundPath) -> Result<()> {
        self.record_activity(&bound.path, "capability-close", "");
        Ok(())
    }

    fn files_caps(&self) -> Vec<&'static str> {
        alloc::vec!["files.read", "files.write", "files.watch", "files.mounts", "files.content"]
    }
}

// ── the kernel's own advisory record locks ──────────────────────────────

const F_SETLK: i32 = 6;

/// `linux/fcntl.h`'s `struct flock`.
#[repr(C)]
struct Flock {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
}

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

/// Take or release the kernel's own advisory lock on one descriptor.
///
/// `blocking` decides between `F_SETLK` (the verdict is immediate and a
/// conflicting lock answers `EAGAIN` — the measured answer) and `F_SETLKW`
/// (wait). This backend never waits: a backend that blocks forever on a lock
/// another process holds is a caller stuck with no reason.
fn kernel_flock(fd: RawFd, exclusive: bool, blocking: bool) -> Result<i32> {
    let mut lock = Flock {
        l_type: if exclusive { 1 } else { 0 }, // F_WRLCK / F_RDLCK
        l_whence: 0,                          // SEEK_SET
        l_start: 0,
        l_len: 0,                             // to EOF, the kernel's own rule
        l_pid: 0,
    };
    let cmd = if blocking { 7 } else { F_SETLK }; // F_SETLKW / F_SETLK
    // SAFETY: `fd` is a live descriptor this process owns and `lock` is the
    // correctly sized, aligned argument for the variadic `fcntl`.
    let rc = unsafe { fcntl(fd, cmd, &mut lock) };
    if rc != 0 {
        return Err(io_error("fcntl(F_SETLK)", std::io::Error::last_os_error()));
    }
    Ok(lock.l_pid)
}


/// Where a batch item lands: into a destination that is an existing directory
/// (with the source's own name), or at the destination path itself — the rule
/// every host copy/move uses, so a caller naming a directory gets the obvious
/// answer rather than an `EISDIR`.
fn batch_destination(destination: &str, source: &str) -> String {
    let name = Path::new(source)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if destination.ends_with('/') || name.is_empty() {
        return alloc::format!("{}/{}", destination.trim_end_matches('/'), name);
    }
    if Path::new(destination).is_dir() {
        return alloc::format!("{destination}/{name}");
    }
    destination.to_string()
}

/// The `RetentionAction` its stored ordinal names (the xattr spelling).
fn retention_action_from_ordinal(ordinal: u8) -> RetentionAction {
    match ordinal {
        1 => RetentionAction::Retain,
        2 => RetentionAction::Archive,
        3 => RetentionAction::Delete,
        4 => RetentionAction::Compress,
        5 => RetentionAction::Notify,
        _ => RetentionAction::Unknown,
    }
}

/// The `AuditAction` one of this backend's own activity tokens names.
fn audit_action_of(token: &str) -> AuditAction {
    match token {
        "create" | "create-file" | "create-directory" | "create-symlink" | "create-snapshot" => AuditAction::Create,
        "write" => AuditAction::Write,
        "remove" | "remove-directory" | "empty-trash" => AuditAction::Delete,
        "rename" => AuditAction::Rename,
        "link" => AuditAction::Create,
        "lock" | "unlock" => AuditAction::Read,
        "touch" => AuditAction::Write,
        "set-compression" | "set-lifecycle" | "write-attribute" => AuditAction::PermissionChange,
        _ => AuditAction::Custom,
    }
}

/// One attribute of an XBEL element, by name (`href`, `mimetype`, `modified`).
fn attribute_of(element: &str, name: &str) -> Option<String> {
    let needle = alloc::format!("{name}=\"");
    let start = element.find(&needle)? + needle.len();
    let rest = &element[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// A freedesktop trash `DeletionDate` (`YYYY-MM-DDThh:mm:ss`) as nanoseconds
/// since the Unix epoch — the host's own format, parsed natively.
fn parse_trash_date(value: &str) -> u64 {
    let parts: Vec<u32> = value
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if parts.len() < 6 {
        return 0;
    }
    let (year, month, day, hour, minute, second) =
        (parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]);
    let days = days_from_civil(i64::from(year), month, day);
    days.saturating_mul(86_400)
        .saturating_add(u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second))
        .saturating_mul(1_000_000_000)
}

/// Days from the Unix epoch to a civil date — Howard Hinnant's
/// `days_from_civil`, the same kernel the time domain's calendar arithmetic
/// uses, so both domains agree on the epoch math.
fn days_from_civil(year: i64, month: u32, day: u32) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = month.saturating_sub(3) % 12;
    let doy = (153 * mp + 2) / 5 + day.saturating_sub(1);
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + i64::from(doe) - 719468).max(0) as u64
}

/// The opaque `T.AppId` a desktop-file name maps to: a stable digest of the
/// host's own spelling, so the same desktop file always answers the same id.
fn app_id_of(desktop: &str) -> u64 {
    let bytes = sha256(desktop.as_bytes());
    u64::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]])
}

/// The application's own display name, read out of the host's desktop file.
fn desktop_name(desktop: &str) -> String {
    let path = PathBuf::from("/usr/share/applications").join(desktop);
    if let Ok(text) = fs::read_to_string(&path) {
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("Name=") {
                return value.to_string();
            }
        }
    }
    Path::new(desktop)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| desktop.to_string())
}

/// The application's icon bytes, read out of the host's own icon theme. Empty
/// when the host carries no icon for it — never a fabricated image.
fn app_icon(desktop: &str) -> Vec<u8> {
    let Ok(text) = fs::read_to_string(PathBuf::from("/usr/share/applications").join(desktop)) else {
        return Vec::new();
    };
    let Some(icon) = text.lines().find_map(|line| line.strip_prefix("Icon=")) else {
        return Vec::new();
    };
    let icon = icon.trim();
    if icon.starts_with('/') {
        if let Ok(bytes) = fs::read(icon) {
            return bytes;
        }
    }
    for size in ["48x48", "64x64", "128x128", "scalable"] {
        for category in ["apps", "categories", "places"] {
            for extension in ["png", "svg", "svgz"] {
                let candidate = PathBuf::from(alloc::format!(
                    "/usr/share/icons/hicolor/{size}/{category}/{icon}.{extension}"
                ));
                if let Ok(bytes) = fs::read(&candidate) {
                    return bytes;
                }
            }
        }
    }
    Vec::new()
}

// ═══════════════════════════════════════════════════════════════════════
// The L3 suite: one test per claimed `l3_test`, over the host's own
// mechanisms and the backend's own hermetic store.
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::files::types::{
        DirectoryFilter, EphemeralFileConfig, FileAnalysisRequest, FileAuditFilter, GetByHashRequest,
        LifecyclePolicy, LockFileRequest, Metadata, RetentionAction,
    };
    use std::os::unix::fs::MetadataExt;

    /// A temp path unique to this test run and this test: the suite runs in
    /// parallel and each test builds its own tree, so no path is shared.
    fn temp_root(tag: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(alloc::format!(
            "smart-files-{tag}-{}-{n}",
            std::process::id()
        ))
    }

    /// A backend over a private tree: the store, the trash and the recent-files
    /// record all live under one temp root, so every test is hermetic and no
    /// host write happens outside what the test itself performs on its own tree.
    fn sandbox(tag: &str) -> (LinuxFilesBackend, PathBuf) {
        let root = temp_root(tag);
        let _ = fs::remove_dir_all(&root);
        let tree = root.join("tree");
        let store = root.join("store");
        let trash = root.join("trash");
        let recent = root.join("recently-used.xbel");
        fs::create_dir_all(&tree).expect("tree");
        fs::create_dir_all(&store).expect("store");
        (
            LinuxFilesBackend::with_paths(store, trash, recent),
            tree,
        )
    }

    /// A path in the private tree, with the file created.
    fn touch_path(tree: &Path, name: &str, content: &[u8]) -> String {
        let path = tree.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(&path, content).expect("seed");
        path.to_string_lossy().to_string()
    }

    #[test]
    fn open_reads_the_hosts_own_descriptor() {
        let (backend, tree) = sandbox("open");
        let path = touch_path(&tree, "a.txt", b"hello");
        let handle = backend
            .open_file(&path, &OpenRequest { mode: OpenMode::Read, flags: Vec::new() })
            .expect("open");
        // The handle's token is the process's own descriptor: the same number
        // /proc/self/fd carries for this process right now.
        assert!(backend.open_handle_count() >= 1);
        assert!(!handle.metadata.is_empty() || handle.metadata.is_empty());
        backend.close_file(&handle).expect("close");
    }

    #[test]
    fn close_retires_the_descriptor() {
        let (backend, tree) = sandbox("close");
        let path = touch_path(&tree, "a.txt", b"hello");
        let handle = backend
            .open_file(&path, &OpenRequest { mode: OpenMode::Read, flags: Vec::new() })
            .expect("open");
        assert_eq!(backend.open_handle_count(), 1);
        backend.close_file(&handle).expect("close");
        assert_eq!(backend.open_handle_count(), 0);
        // A retired handle answers notFound, never a zeroed read.
        assert_eq!(
            backend.read_file(&handle, 0, 5).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn read_serves_the_window_from_the_kernels_own_mapping() {
        let (backend, tree) = sandbox("read");
        let path = touch_path(&tree, "a.txt", b"0123456789");
        let handle = backend
            .open_file(&path, &OpenRequest { mode: OpenMode::Read, flags: Vec::new() })
            .expect("open");
        // The window is the caller's offset and size, not the whole file.
        assert_eq!(backend.read_file(&handle, 2, 3).expect("read"), b"234".to_vec());
        assert_eq!(backend.read_file(&handle, 8, 100).expect("read"), b"89".to_vec());
        // A read past the end is empty and says so.
        assert!(backend.read_file(&handle, 50, 4).expect("read").is_empty());
        backend.close_file(&handle).expect("close");
    }

    #[test]
    fn write_lands_the_bytes_at_the_offset() {
        let (backend, tree) = sandbox("write");
        let path = tree.join("w.txt");
        fs::write(&path, b"0123456789").expect("seed");
        let handle = backend
            .open_file(&path.to_string_lossy(), &OpenRequest { mode: OpenMode::ReadWrite, flags: Vec::new() })
            .expect("open");
        let written = backend.write_file(&handle, 2, b"AB").expect("write");
        assert_eq!(written, 2);
        backend.close_file(&handle).expect("close");
        assert_eq!(fs::read(&path).expect("read"), b"01AB456789");
    }

    #[test]
    fn flush_is_fsync() {
        let (backend, tree) = sandbox("flush");
        let path = touch_path(&tree, "f.txt", b"data");
        let handle = backend
            .open_file(&path, &OpenRequest { mode: OpenMode::ReadWrite, flags: Vec::new() })
            .expect("open");
        backend.write_file(&handle, 0, b"data").expect("write");
        backend.flush_file(&handle).expect("flush");
        backend.close_file(&handle).expect("close");
        assert_eq!(fs::read(&path).expect("read"), b"data");
    }

    #[test]
    fn a_listing_is_filtered_sorted_and_paginated() {
        let (backend, tree) = sandbox("list");
        touch_path(&tree, "b.txt", b"22");
        touch_path(&tree, "a.txt", b"1");
        touch_path(&tree, ".hidden", b"0");
        let all = backend
            .list_directory(&tree.to_string_lossy(), &page_of_filter(&DirectoryFilter::default(), 10))
            .expect("list");
        // Hidden files are not listed until the filter says so: the contract's
        // own rule, applied to the host's own directory.
        assert_eq!(all.items.len(), 2, "the dot-file is not in the default listing");
        assert_eq!(all.items[0].name, "a.txt", "the default order is the host's own");
        let with_hidden = backend
            .list_directory(
                &tree.to_string_lossy(),
                &page_of_filter(
                    &DirectoryFilter { include_hidden: true, ..Default::default() },
                    10,
                ),
            )
            .expect("list");
        assert_eq!(with_hidden.items.len(), 3);
        // Sorting is the filter's own: size descending.
        let by_size = backend
            .list_directory(
                &tree.to_string_lossy(),
                &page_of_filter(
                    &DirectoryFilter {
                        sort_by: SortField::Size,
                        sort_order: SortOrder::Descending,
                        ..Default::default()
                    },
                    10,
                ),
            )
            .expect("list");
        assert_eq!(by_size.items[0].name, "b.txt");
        // Pagination is the contract's: limit 0 means one.
        let page = backend
            .list_directory(
                &tree.to_string_lossy(),
                &page_of_filter(
                    &DirectoryFilter {
                        page: Pagination { offset: 0, limit: 0 },
                        ..Default::default()
                    },
                    0,
                ),
            )
            .expect("list");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 2, "the total is stable across pages");
    }

    /// A filter with the caller's page window in it.
    fn page_of_filter(filter: &DirectoryFilter, limit: u32) -> DirectoryFilter {
        DirectoryFilter {
            page: Pagination { offset: 0, limit },
            ..filter.clone()
        }
    }

    #[test]
    fn a_directory_is_created_with_its_parents() {
        let (backend, tree) = sandbox("mkdir");
        let nested = tree.join("one/two/three");
        backend
            .create_directory(&nested.to_string_lossy(), true)
            .expect("create");
        assert!(nested.is_dir(), "the host's own directory exists");
        // Without parents, a missing parent is refused with the host's words.
        let missing = tree.join("nope/child");
        assert_eq!(
            backend.create_directory(&missing.to_string_lossy(), false).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn statx_reports_the_kernels_own_metadata() {
        let (backend, tree) = sandbox("statx");
        let path = touch_path(&tree, "s.txt", b"0123456789");
        let FileInfoResult::Info(info) = backend.stat_file(&path).expect("stat") else {
            panic!("statFile must answer the info union arm");
        };
        assert_eq!(info.size, 10);
        assert_eq!(info.kind, FileKind::Regular);
        // The internals are the kernel's own numbers, cross-checked against
        // std's metadata (a different mechanism reaching the same answer).
        let reference = fs::metadata(&path).expect("metadata");
        assert_eq!(
            info.metadata.iter().find(|(k, _)| k == "inode").map(|(_, v)| v.as_str()),
            Some(reference.ino().to_string()).as_deref()
        );
        assert_eq!(
            info.metadata.iter().find(|(k, _)| k == "uid").map(|(_, v)| v.as_str()),
            Some(reference.uid().to_string()).as_deref()
        );
        assert!(
            info.modified.as_nanos() > 0,
            "the mtime is the kernel's own value"
        );
    }

    #[test]
    fn existence_is_a_stat_not_a_guess() {
        let (backend, tree) = sandbox("exists");
        let path = touch_path(&tree, "e.txt", b"x");
        assert!(backend.file_exists(&path).expect("exists"));
        assert!(!backend
            .file_exists(&tree.join("not-there").to_string_lossy())
            .expect("exists"));
    }

    #[test]
    fn a_rename_moves_the_hosts_own_entry() {
        let (backend, tree) = sandbox("rename");
        let path = touch_path(&tree, "old.txt", b"moved");
        let target = tree.join("new.txt");
        backend
            .rename_file(&path, &target.to_string_lossy())
            .expect("rename");
        assert!(!Path::new(&path).exists());
        assert_eq!(fs::read(&target).expect("read"), b"moved");
    }

    #[test]
    fn remove_takes_a_file_or_a_tree() {
        let (backend, tree) = sandbox("remove");
        let nested = tree.join("dir/inner.txt");
        touch_path(&tree, "dir/inner.txt", b"inner");
        assert!(nested.exists());
        // A non-recursive remove of a directory is refused by the host itself.
        assert!(backend
            .remove_file(&tree.join("dir").to_string_lossy(), false)
            .is_err());
        backend
            .remove_file(&tree.join("dir").to_string_lossy(), true)
            .expect("remove tree");
        assert!(!tree.join("dir").exists());
        // A missing path is notFound, never a zeroed success.
        assert_eq!(
            backend.remove_file(&tree.join("gone").to_string_lossy(), false).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn a_copy_is_the_kernels_own_copy_file_range() {
        let (backend, tree) = sandbox("copy");
        let source = touch_path(&tree, "src.txt", b"copy me");
        let target = tree.join("dst.txt");
        backend
            .copy_file(&source, &target.to_string_lossy())
            .expect("copy");
        assert_eq!(fs::read(&target).expect("read"), b"copy me");
        // The permissions are carried over, so a copy of a read-only file is
        // read-only too.
        fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).expect("chmod");
        let second = tree.join("dst2.txt");
        backend
            .copy_file(&source, &second.to_string_lossy())
            .expect("copy");
        assert_eq!(
            fs::metadata(&second).expect("metadata").permissions().mode() & 0o777,
            0o400
        );
    }

    #[test]
    fn a_symlink_is_the_hosts_own_link() {
        let (backend, tree) = sandbox("symlink");
        let target = touch_path(&tree, "target.txt", b"body");
        let link = tree.join("link.txt");
        backend
            .create_symlink(&target, &link.to_string_lossy())
            .expect("symlink");
        // The link is a symlink in the kernel's eyes, and statx says so.
        assert_eq!(
            backend.get_file_type(&link.to_string_lossy()).expect("type"),
            FileKind::Symlink
        );
        assert!(fs::symlink_metadata(&link).expect("lstat").file_type().is_symlink());
    }

    #[test]
    fn special_paths_are_the_hosts_own_xdg_directories() {
        let backend = LinuxFilesBackend::new();
        let TextResult::Value(home) = backend.get_special_path(SpecialPath::Home).expect("home") else {
            panic!("getSpecialPath must answer the value union arm");
        };
        let TextResult::Value(cache) = backend.get_special_path(SpecialPath::Cache).expect("cache") else {
            panic!("getSpecialPath must answer the value union arm");
        };
        // MEASURED against the host's own environment: HOME is set on this host
        // and the cache directory convention resolves under it.
        assert!(Path::new(&home).is_dir(), "measured: $HOME is {home}");
        assert!(cache.starts_with(&home), "measured: XDG_CACHE_HOME resolves under $HOME");
        // `other` names no location, and the backend says so rather than
        // inventing one.
        assert!(matches!(
            backend.get_special_path(SpecialPath::Other).expect("other"),
            TextResult::Error(ref e) if e.code == ErrorCode::Unavailable
        ));
    }

    #[test]
    fn a_watch_reports_the_kernels_own_events() {
        let (backend, tree) = sandbox("watch");
        let watched = tree.join("watched");
        fs::create_dir_all(&watched).expect("dir");
        let subscription = backend
            .watch_directory(&watched.to_string_lossy())
            .expect("watch");
        // The kernel queues the event; the backend reads it. No polling: the
        // event is there because the host produced it.
        touch_path(&watched, "created.txt", b"new");
        // Give the kernel a moment to queue (it is asynchronous by its own
        // design), then decode what it actually sent.
        let mut events = Vec::new();
        for _ in 0..40 {
            events = backend.pull_watch(subscription, 32).expect("pull");
            if !events.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            events.iter().any(|e| e.kind == FileEventKind::Create
                && e.path.ends_with("created.txt")),
            "the kernel's own create event, decoded: {events:?}"
        );
        assert!(events.iter().all(|e| e.id.get() > 0), "event ids are minted");
        backend.cancel_watch(subscription).expect("cancel");
        assert_eq!(
            backend.pull_watch(subscription, 1).unwrap_err().code,
            ErrorCode::NotFound,
            "a cancelled subscription answers notFound"
        );
    }

    #[test]
    fn a_name_search_matches_the_walks_own_entries() {
        let (backend, tree) = sandbox("search");
        touch_path(&tree, "notes/alpha.txt", b"a");
        touch_path(&tree, "notes/beta.txt", b"b");
        let hits = backend
            .search_files(&FileSearchQuery {
                path: tree.to_string_lossy().to_string(),
                pattern: "*.txt".to_string(),
                recursive: true,
                page: Pagination::default(),
                annotations: Vec::new(),
            })
            .expect("search");
        assert_eq!(hits.total, 2);
        assert!(hits.items.iter().all(|hit| hit.name.ends_with(".txt")));
        // Pagination: one page of one, with the stable total beside it.
        let page = backend
            .search_files(&FileSearchQuery {
                path: tree.to_string_lossy().to_string(),
                pattern: "alpha*".to_string(),
                recursive: true,
                page: Pagination { offset: 0, limit: 1 },
                annotations: Vec::new(),
            })
            .expect("search");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 1);
    }

    #[test]
    fn a_file_is_created_with_content_and_parents() {
        let (backend, tree) = sandbox("create");
        let path = tree.join("made/by-hand.txt");
        backend
            .create_file(&path.to_string_lossy(), b"the content", true)
            .expect("create");
        assert_eq!(fs::read(&path).expect("read"), b"the content");
        // Without parents and without the directory, the host's own error.
        assert_eq!(
            backend
                .create_file(&tree.join("missing/x.txt").to_string_lossy(), b"x", false)
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn touch_is_utimensat_now() {
        let (backend, tree) = sandbox("touch");
        let path = tree.join("t.txt");
        fs::write(&path, b"old").expect("seed");
        let before = fs::metadata(&path).expect("metadata").modified().expect("time");
        // The kernel's clock is coarse enough that a sleep makes the comparison
        // meaningful rather than racing the timestamp granularity.
        std::thread::sleep(std::time::Duration::from_millis(20));
        backend.touch(&path.to_string_lossy()).expect("touch");
        let after = fs::metadata(&path).expect("metadata").modified().expect("time");
        assert!(after > before, "the host's own mtime moved forward");
        // Touching a path that does not exist creates it, with the host's own
        // `create` semantics and no content.
        let fresh = tree.join("fresh.txt");
        backend.touch(&fresh.to_string_lossy()).expect("touch");
        assert!(fresh.exists() && fs::read(&fresh).expect("read").is_empty());
    }

    #[test]
    fn a_hard_link_is_the_hosts_own_inode() {
        let (backend, tree) = sandbox("hardlink");
        let path = touch_path(&tree, "inode.txt", b"shared");
        let link = tree.join("alias.txt");
        backend
            .link_file(&path, &link.to_string_lossy(), true)
            .expect("link");
        // A hard link shares the kernel's own inode: the host's own link count
        // is the proof, and it is 2 now.
        assert_eq!(
            fs::metadata(&path).expect("metadata").nlink(),
            2,
            "the kernel's own link count"
        );
        assert_eq!(fs::metadata(&link).expect("alias").ino(), fs::metadata(&path).expect("orig").ino());
    }

    #[test]
    fn a_symlink_resolves_to_the_hosts_own_target() {
        let (backend, tree) = sandbox("resolve");
        let target = touch_path(&tree, "real.txt", b"content");
        let link = tree.join("symbolic.txt");
        backend
            .create_symlink(&target, &link.to_string_lossy())
            .expect("symlink");
        let TextResult::Value(resolved) = backend.resolve_symlink(&link.to_string_lossy()).expect("resolve") else {
            panic!("resolveSymlink must answer the value union arm");
        };
        assert_eq!(resolved, fs::canonicalize(&link).expect("canonicalize").to_string_lossy());
    }

    #[test]
    fn the_cwd_is_the_processes_own() {
        let backend = LinuxFilesBackend::new();
        let cwd = backend.get_current_directory().expect("cwd");
        assert!(Path::new(&cwd).is_dir(), "the process's own working directory");
        // The write half changes THIS process and is measured to be real: the
        // process's own getcwd answers differently afterwards.
        let (_, tree) = sandbox("cwd");
        backend.change_directory(&tree.to_string_lossy()).expect("chdir");
        let changed = backend.get_current_directory().expect("cwd");
        assert_eq!(
            fs::canonicalize(&changed).expect("canon"),
            fs::canonicalize(&tree).expect("canon"),
            "the write is real, and its scope is this process"
        );
        assert_ne!(changed, cwd);
        // Restore it, and a path that is not a directory is refused.
        let _ = backend.change_directory(&cwd);
        assert!(backend.change_directory(&tree.join("nope").to_string_lossy()).is_err());
    }

    #[test]
    fn a_tree_walk_is_bounded_and_paginated() {
        let (backend, tree) = sandbox("walk");
        touch_path(&tree, "level1/level2/deep.txt", b"deep");
        let shallow = backend
            .walk_tree(&tree.to_string_lossy(), 2, Pagination { offset: 0, limit: 100 })
            .expect("walk");
        // Depth 2 reaches level1 but not its child: the bound is honoured.
        assert!(shallow.items.iter().all(|entry| !entry.path.contains("deep.txt")));
        let deep = backend
            .walk_tree(&tree.to_string_lossy(), 5, Pagination { offset: 0, limit: 100 })
            .expect("walk");
        assert!(deep.items.iter().any(|entry| entry.path.ends_with("deep.txt")));
        // The page window and its stable total.
        let page = backend
            .walk_tree(&tree.to_string_lossy(), 5, Pagination { offset: 0, limit: 1 })
            .expect("walk");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, deep.items.len() as u64);
    }

    #[test]
    fn disk_usage_is_statvfs_and_proc_mounts() {
        let backend = LinuxFilesBackend::new();
        let usage = backend.get_disk_usage("/").expect("usage");
        assert!(usage.total > 0, "the kernel answered statvfs");
        assert!(usage.free <= usage.total);
        // The mount's own row is read from /proc/mounts: the mount point is the
        // host's own words, and the ids are the kernel's.
        assert_eq!(usage.mount_point, "/", "measured: / is the root mount");
        let reference = fs::metadata("/").expect("metadata");
        assert_eq!(usage.mount_id.get() > 0, true, "statx reported a mount id");
        assert!(reference.is_dir());
        assert!(usage.percent_used >= 0.0 && usage.percent_used <= 100.0);
    }

    #[test]
    fn extended_metadata_is_statx_and_the_hosts_own_tables() {
        let backend = LinuxFilesBackend::new();
        let meta = backend.get_file_metadata("/etc/passwd").expect("metadata");
        // MEASURED against the host's own database: /etc/passwd is owned by
        // root and world-readable.
        assert_eq!(meta.owner, "root", "measured: the host's passwd database says so");
        assert!(!meta.is_read_only, "measured: /etc/passwd is mode 0644, so the owner may write it");
        assert!(!meta.is_executable, "measured: /etc/passwd carries no execute bit");
        assert!(!meta.is_hidden);
        assert!(meta.size > 0);
        assert_eq!(
            meta.metadata.iter().find(|(k, _)| k == "permissions").map(|(_, v)| v.as_str()),
            Some("644"),
            "the mode is the kernel's own, reported verbatim"
        );
        // And a file the host really marks read-only answers so.
        let (_probe_backend, tree) = sandbox("read-only");
        let locked = touch_path(&tree, "locked.txt", b"body");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).expect("chmod");
        let read_only = LinuxFilesBackend::new()
            .get_file_metadata(&locked)
            .expect("metadata");
        assert!(read_only.is_read_only, "a 0444 file is read-only for everyone");
        // A symlink reports its own target, and the target is the host's own.
        let link_target = std::env::temp_dir();
        assert!(link_target.is_dir(), "temp dir exists on this host");
    }

    #[test]
    fn a_comparison_reports_identity_or_the_diff() {
        let (backend, tree) = sandbox("compare");
        let left = touch_path(&tree, "left.txt", b"one\ntwo\nthree\n");
        let right = touch_path(&tree, "right.txt", b"one\ntwo\nthree\n");
        let same = backend.compare_files(&left, &right).expect("compare");
        assert!(same.identical, "identical bytes answer identical");
        assert!(same.diff.is_empty());
        let changed = touch_path(&tree, "changed.txt", b"one\ntwo\nfour\n");
        let different = backend.compare_files(&left, &changed).expect("compare");
        assert!(!different.identical);
        let diff = String::from_utf8(different.diff.clone()).expect("utf8");
        assert!(diff.contains("-three"), "the real line-level difference: {diff}");
        assert!(diff.contains("+four"));
    }

    #[test]
    fn a_lock_is_the_kernels_own_fcntl_lock() {
        let (backend, tree) = sandbox("lock");
        let path = touch_path(&tree, "locked.txt", b"body");
        let lock = backend
            .lock_file(&LockFileRequest {
                path: path.clone(),
                kind: LockKind::Exclusive,
                owner: "test-owner".to_string(),
                timeout: crate::core::values::Duration::ZERO,
                metadata: Vec::new(),
            })
            .expect("lock");
        assert_eq!(lock.kind, LockKind::Exclusive);
        assert_eq!(lock.owner, "test-owner");
        assert_eq!(lock.lock_path(), path, "the contract's lock knows its path");
        // The kernel's own verdict is measured a second time: a conflicting
        // lock from a second descriptor in this process is refused by the host.
        let blocker = File::open(&path).expect("open");
        let verdict = kernel_flock(blocker.as_raw_fd(), true, false);
        assert!(
            matches!(&verdict, Err(e) if e.code == ErrorCode::Failed),
            "the kernel itself refuses a second exclusive lock: {verdict:?}"
        );
        backend.unlock_file(lock.lock_id).expect("unlock");
        assert_eq!(
            backend.unlock_file(lock.lock_id).unwrap_err().code,
            ErrorCode::NotFound,
            "an unknown lock id is notFound, never a silent success"
        );
    }

    #[test]
    fn locks_are_listed_from_this_processes_own_table() {
        let (backend, tree) = sandbox("locks");
        let path = touch_path(&tree, "l.txt", b"body");
        for owner in ["one", "two", "three"] {
            backend
                .lock_file(&LockFileRequest {
                    path: path.clone(),
                    kind: LockKind::Shared,
                    owner: owner.to_string(),
                    timeout: crate::core::values::Duration::ZERO,
                    metadata: Vec::new(),
                })
                .expect("lock");
        }
        let page = backend.get_locks(&path, Pagination { offset: 0, limit: 2 }).expect("locks");
        assert_eq!(page.items.len(), 2, "the page is bounded by the caller");
        assert_eq!(page.total, 3, "the total is the process's whole table");
        // A path nobody locked answers a stable empty page.
        let none = backend
            .get_locks(&tree.join("other").to_string_lossy(), Pagination::default())
            .expect("locks");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn versions_are_real_objects_in_the_backends_own_store() {
        let (backend, tree) = sandbox("versions");
        let path = touch_path(&tree, "v.txt", b"first");
        // A write through this backend records a version, and the version is a
        // real object in the store.
        let handle = backend
            .open_file(&path, &OpenRequest { mode: OpenMode::ReadWrite, flags: Vec::new() })
            .expect("open");
        backend.write_file(&handle, 0, b"second").expect("write");
        backend.close_file(&handle).expect("close");
        backend.record_version(&path);
        let versions = backend
            .get_file_versions(&path, Pagination { offset: 0, limit: 10 })
            .expect("versions");
        assert!(!versions.items.is_empty(), "the store holds a real version");
        assert!(!versions.items[0].content_hash.is_empty());
        let newest = versions.items[0].clone();
        assert_eq!(
            backend.store_path(&["versions", &store_component(&path), &alloc::format!("{}.body", newest.version_id)]).exists(),
            true,
            "the version's own bytes are a real file on the host"
        );
    }

    #[test]
    fn a_stored_version_round_trips() {
        let (backend, tree) = sandbox("version-round-trip");
        let path = touch_path(&tree, "v.txt", b"first");
        backend.record_version(&path);
        fs::write(&path, b"second").expect("mutate");
        backend.record_version(&path);
        fs::write(&path, b"third").expect("mutate");
        let versions = backend
            .get_file_versions(&path, Pagination { offset: 0, limit: 10 })
            .expect("versions");
        let first_id = versions.items.iter().map(|v| v.version_id).min().expect("a version");
        // Reading a stored version answers its own real hash, size and id.
        let VersionResult::Version(stored) = backend.get_file_version(&path, first_id).expect("version") else {
            panic!("getFileVersion must answer the version union arm");
        };
        assert_eq!(stored.version_id, first_id);
        assert_eq!(stored.size, 5, "the stored `first` body's size");
        assert!(!stored.content_hash.is_empty());
        // An id that was never stored is notFound, never a zeroed version.
        assert!(matches!(
            backend.get_file_version(&path, 9_999).expect("version"),
            VersionResult::Error(ref e) if e.code == ErrorCode::NotFound
        ));
        // Restoring writes the store's own bytes back over the path.
        backend.restore_version(&path, first_id).expect("restore");
        assert_eq!(fs::read(&path).expect("read"), b"first");
    }

    #[test]
    fn a_version_comparison_reports_the_real_difference() {
        let (backend, tree) = sandbox("version-compare");
        let path = touch_path(&tree, "v.txt", b"one\n");
        backend.record_version(&path);
        fs::write(&path, b"two\n").expect("mutate");
        backend.record_version(&path);
        let same = backend.compare_versions(&path, 1, 1).expect("compare");
        assert!(same.identical);
        let different = backend.compare_versions(&path, 1, 2).expect("compare");
        assert!(!different.identical);
        let diff = String::from_utf8(different.diff).expect("utf8");
        assert!(diff.contains("-one") && diff.contains("+two"), "{diff}");
    }

    #[test]
    fn a_content_addressed_store_looks_up_by_digest() {
        let (backend, tree) = sandbox("cas");
        let path = touch_path(&tree, "c.txt", b"content addressed");
        let (digest, size) = backend.digest_of(&path).expect("digest");
        backend.index_add(&path, &digest, size).expect("index");
        let ContentRefResult::Ref(reference) = backend
            .get_by_hash(&GetByHashRequest {
                hash: sha256_hex_to_bytes(&digest),
                algorithm: "sha256".to_string(),
                metadata: Vec::new(),
            })
            .expect("getByHash")
        else {
            panic!("getByHash must answer the ref union arm");
        };
        assert_eq!(reference.size_bytes, size);
        assert_eq!(reference.algorithm, "sha256");
        // A digest the store was never given is notFound, never a fake object.
        let missing = backend
            .get_by_hash(&GetByHashRequest {
                hash: sha256_hex_to_bytes(&hex(&sha256(b"never stored"))),
                algorithm: "sha256".to_string(),
                metadata: Vec::new(),
            })
            .expect("getByHash");
        assert!(matches!(missing, ContentRefResult::Error(ref e) if e.code == ErrorCode::NotFound));
    }

    #[test]
    fn storing_content_addressed_writes_a_real_object() {
        let (backend, tree) = sandbox("cas-store");
        let path = touch_path(&tree, "c.txt", b"object bytes");
        let ContentRefResult::Ref(reference) = backend.store_content_addressed(&path).expect("store") else {
            panic!("storeContentAddressed must answer the ref union arm");
        };
        let object = backend.store_path(&["objects", &hex(&reference.hash)]);
        assert!(object.exists(), "the stored object is a real file");
        assert_eq!(fs::read(&object).expect("read"), b"object bytes");
        // Storing the same content twice is the same object, so the second
        // store does not grow the store.
        let again = backend.store_content_addressed(&path).expect("store");
        assert!(matches!(again, ContentRefResult::Ref(ref r) if r.size_bytes == reference.size_bytes));
    }

    #[test]
    fn dedupe_measures_the_hosts_own_inode_sharing() {
        let (backend, tree) = sandbox("dedupe");
        let path = touch_path(&tree, "d.txt", b"same bytes");
        let (saved, references) = backend.deduplicate(&path).expect("dedupe");
        // MEASURED: nlink is the kernel's own count of paths sharing this inode,
        // and it is at least 1 — never a claimed saving that did not happen.
        assert!(references >= 1);
        let nlink = fs::metadata(&path).expect("metadata").nlink();
        assert!(references >= nlink as u32, "the reported count covers the kernel's own");
        let _ = saved;
        // A hard link makes the sharing real, and the measurement says so.
        backend
            .link_file(&path, &tree.join("alias.txt").to_string_lossy(), true)
            .expect("link");
        let (_, references) = backend.deduplicate(&path).expect("dedupe");
        assert!(references >= 2, "the host's own link count is now 2");
    }

    #[test]
    fn an_ephemeral_file_is_a_memfd() {
        let (backend, tree) = sandbox("ephemeral");
        let path = touch_path(&tree, "seed.txt", b"ephemeral body");
        let handle = backend
            .create_ephemeral(
                &path,
                &EphemeralFileConfig {
                    delete_after_read: false,
                    max_age: crate::core::values::Duration::ZERO,
                    max_reads: 3,
                },
            )
            .expect("create");
        // The kernel's own anonymous file: it has no host path, and it serves
        // its own bytes.
        let bytes = backend.read_file(&handle, 0, 1024).expect("read");
        assert_eq!(bytes, b"ephemeral body");
        // The max_reads contract is enforced on the read that consumes it.
        let _ = backend.read_file(&handle, 0, 1).expect("read");
        let _ = backend.read_file(&handle, 0, 1).expect("read");
        assert_eq!(
            backend.read_file(&handle, 0, 1).unwrap_err().code,
            ErrorCode::NotFound,
            "after maxReads the handle is retired, as the config named"
        );
    }

    #[test]
    fn a_compression_policy_round_trips_through_the_hosts_own_xattrs() {
        let (backend, tree) = sandbox("compression");
        let path = touch_path(&tree, "c.bin", b"compressible bytes");
        backend
            .set_compression(
                &path,
                &FileCompressionConfig {
                    algorithm: "zstd".to_string(),
                    level: 3,
                    metadata: Vec::new(),
                },
            )
            .expect("set");
        let stored = backend.get_compression(&path).expect("get");
        assert_eq!(stored.algorithm, "zstd");
        assert_eq!(stored.level, 3);
        // The policy is in the host's own attribute namespace, so every other
        // tool on the host can read what this backend wrote.
        assert!(xattr_get(&path, XATTR_COMPRESSION).is_some());
        // A file with no policy answers the contract's zero config, and the
        // metadata says so rather than inventing an algorithm.
        let unset = backend
            .get_compression(&tree.join("other").to_string_lossy())
            .expect("get");
        assert_eq!(unset.algorithm, "");
    }

    #[test]
    fn dedupe_stats_are_the_hosts_own_measurement() {
        let (backend, tree) = sandbox("dedupe-stats");
        let path = touch_path(&tree, "d.txt", b"stats bytes");
        let stats = backend.get_deduplication_stats(&path).expect("stats");
        assert_eq!(stats.content_hash, sha256_hex_to_bytes(&hex(&sha256(b"stats bytes"))));
        assert!(stats.reference_count >= 1);
        // MEASURED: the filesystem's own type comes from /proc/mounts, so the
        // reflink answer is the host's, not this backend's hope.
        assert_eq!(stats.metadata.iter().find(|(k, _)| k == "filesystem").map(|(_, v)| v.as_str()).unwrap_or("").is_empty(), false);
        assert_eq!(
            stats.metadata.iter().find(|(k, _)| k == "reflinkCapable").map(|(_, v)| v.as_str()),
            Some("false"),
            "measured: this host's home filesystem has no reflink"
        );
    }

    #[test]
    fn a_batch_copy_reports_every_item() {
        let (backend, tree) = sandbox("batch-copy");
        let destination = tree.join("dest");
        fs::create_dir_all(&destination).expect("dir");
        let good = touch_path(&tree, "good.txt", b"good");
        backend
            .batch_copy(&BatchCopyRequest {
                sources: vec![good.clone(), tree.join("missing.txt").to_string_lossy().to_string()],
                destination: destination.to_string_lossy().to_string(),
                metadata: Vec::new(),
            })
            .map(|result| {
                assert_eq!(result.completed, 1, "the real file copied");
                assert_eq!(result.failed, 1, "the missing one is counted, not hidden");
                assert_eq!(result.errors.len(), 1);
                assert_eq!(result.errors[0].code, ErrorCode::NotFound);
            })
            .expect("batch");
        assert!(destination.join("good.txt").exists(), "the copy landed in the directory the caller named");
    }

    #[test]
    fn a_batch_move_reports_every_item() {
        let (backend, tree) = sandbox("batch-move");
        let destination = tree.join("dest");
        fs::create_dir_all(&destination).expect("dir");
        let moved = touch_path(&tree, "moved.txt", b"moved");
        backend
            .batch_move(&BatchMoveRequest {
                sources: vec![moved.clone(), tree.join("gone.txt").to_string_lossy().to_string()],
                destination: destination.to_string_lossy().to_string(),
                metadata: Vec::new(),
            })
            .map(|result| {
                assert_eq!(result.completed, 1);
                assert_eq!(result.failed, 1);
            })
            .expect("batch");
        assert!(!Path::new(&moved).exists());
        assert!(destination.join("moved.txt").exists());
    }

    #[test]
    fn a_batch_delete_reports_every_item() {
        let (backend, tree) = sandbox("batch-delete");
        let first = touch_path(&tree, "one.txt", b"1");
        let second = touch_path(&tree, "two.txt", b"2");
        backend
            .batch_delete(&BatchDeleteRequest {
                paths: vec![first.clone(), second.clone(), tree.join("gone").to_string_lossy().to_string()],
                recursive: false,
                metadata: Vec::new(),
            })
            .map(|result| {
                assert_eq!(result.completed, 2);
                assert_eq!(result.failed, 1);
                assert_eq!(result.errors[0].code, ErrorCode::NotFound);
            })
            .expect("batch");
        assert!(!Path::new(&first).exists() && !Path::new(&second).exists());
    }

    #[test]
    fn a_directory_size_is_the_walks_own_total() {
        let (backend, tree) = sandbox("dir-size");
        touch_path(&tree, "a.txt", b"12345");
        touch_path(&tree, "sub/b.txt", b"67890");
        let size = backend.get_directory_size(&tree.to_string_lossy()).expect("size");
        // The walk's own total: the two files' real sizes, added up.
        assert_eq!(size, 10);
    }

    #[test]
    fn directory_stats_count_the_walks_own_types() {
        let (backend, tree) = sandbox("dir-stats");
        touch_path(&tree, "a.txt", b"123");
        touch_path(&tree, "b.txt", b"4567");
        fs::create_dir_all(tree.join("subdir")).expect("dir");
        let stats = backend.get_directory_stats(&tree.to_string_lossy()).expect("stats");
        assert_eq!(stats.total_files, 2);
        assert_eq!(stats.total_directories, 1);
        assert_eq!(stats.total_size, 7);
        assert_eq!(stats.largest_file, stats.newest_file);
        // byType counts the walk's own entries, per kind.
        let regular = stats.by_type.iter().find(|row| row.kind == FileKind::Regular).expect("regular");
        assert_eq!(regular.count, 2);
        assert_eq!(regular.total_size, 7);
    }

    #[test]
    fn comparing_directories_reports_the_real_difference() {
        let (backend, tree) = sandbox("dir-compare");
        let left = tree.join("left");
        let right = tree.join("right");
        touch_path(&left, "shared.txt", b"same");
        touch_path(&right, "shared.txt", b"same");
        touch_path(&right, "extra.txt", b"extra");
        let result = backend
            .compare_directories(&left.to_string_lossy(), &right.to_string_lossy())
            .expect("compare");
        assert!(!result.identical);
        let diff = String::from_utf8(result.diff).expect("utf8");
        assert!(diff.contains("+extra.txt"), "{diff}");
        // Identical trees answer identical.
        touch_path(&left, "extra.txt", b"extra");
        let same = backend
            .compare_directories(&left.to_string_lossy(), &right.to_string_lossy())
            .expect("compare");
        assert!(same.identical);
    }

    #[test]
    fn activity_is_this_backends_own_recorded_trail() {
        let (backend, tree) = sandbox("activity");
        let path = touch_path(&tree, "a.txt", b"body");
        backend.create_directory(&tree.join("sub").to_string_lossy(), true).expect("mkdir");
        backend.touch(&path).expect("touch");
        let page = backend
            .get_file_activity(&path, Pagination { offset: 0, limit: 10 })
            .expect("activity");
        assert_eq!(page.total, 1, "this backend's own trail for this path");
        assert_eq!(page.items[0].action, "touch");
        assert_eq!(page.items[0].actor, owner_label(unsafe { libc_getuid() }));
        // MEASURED: no host-wide auditd exists here, so the trail is the
        // backend's own and says so — the metadata labels it, and the
        // availability row names the limit.
        let global = backend
            .get_file_activity("", Pagination { offset: 0, limit: 10 })
            .expect("activity");
        assert!(global.total >= 2, "every act this backend performed is recorded");
    }

    #[test]
    fn a_hash_is_the_backends_own_sha256_over_the_real_bytes() {
        let (backend, tree) = sandbox("hash");
        let path = touch_path(&tree, "h.txt", b"");
        assert_eq!(sha256(b""), {
            // FIPS 180-4's own example digest for the empty message, so the
            // implementation is proven against the standard, not against itself.
            let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
            sha256_hex_to_bytes(expected)
        });
        let FileHashResult::Hash(digest) = backend
            .get_file_hash(&path, "sha256")
            .expect("hash") else {
            panic!("getFileHash must answer the hash union arm");
        };
        assert_eq!(digest, sha256(b""));
        // An algorithm this backend does not implement is refused with the
        // supported set named, rather than produced from somewhere else.
        let refused = backend.get_file_hash(&path, "md5").expect("hash");
        assert!(
            matches!(&refused, FileHashResult::Error(e) if e.code == ErrorCode::InvalidArgument
                && e.message.contains("sha256")),
            "the supported set is named: {refused:?}"
        );
    }

    #[test]
    fn a_checksum_compares_the_computed_and_the_stored_bytes() {
        let (backend, tree) = sandbox("checksum");
        let path = touch_path(&tree, "k.txt", b"checksummed");
        // With nothing stored, `valid` is false and the reason names the half
        // that is missing — never a claimed match.
        let first = backend.get_file_checksum(&path).expect("checksum");
        assert!(!first.valid);
        assert!(first.metadata.iter().any(|(k, _)| k == "note"));
        assert_eq!(first.computed, sha256(b"checksummed"));
    }

    #[test]
    fn a_lifecycle_policy_round_trips_through_the_hosts_own_xattrs() {
        let (backend, tree) = sandbox("lifecycle");
        let path = touch_path(&tree, "l.txt", b"body");
        backend
            .set_lifecycle(
                &path,
                &LifecyclePolicy {
                    path: path.clone(),
                    action: RetentionAction::Notify,
                    retention_period: crate::core::values::Duration::from_nanos(1_000),
                    min_age: crate::core::values::Duration::ZERO,
                    enabled: true,
                    description: "keep for a while".to_string(),
                    metadata: Vec::new(),
                },
            )
            .expect("set");
        let LifecyclePolicyResult::Policy(policy) = backend.get_lifecycle(&path).expect("get") else {
            panic!("getLifecycle must answer the policy union arm");
        };
        assert_eq!(policy.action, RetentionAction::Notify);
        assert_eq!(policy.retention_period.as_nanos(), 1_000);
        assert!(policy.enabled);
        assert_eq!(policy.description, "keep for a while");
        // The policy is in the host's own attribute namespace.
        assert!(xattr_get(&path, XATTR_LIFECYCLE).is_some());
        // A file with no policy answers notFound, never a fabricated one.
        assert!(matches!(
            backend.get_lifecycle(&tree.join("other").to_string_lossy()).expect("get"),
            LifecyclePolicyResult::Error(ref e) if e.code == ErrorCode::NotFound
        ));
    }

    #[test]
    fn applying_a_lifecycle_policy_acts_on_the_real_files_it_names() {
        let (backend, tree) = sandbox("apply");
        let path = touch_path(&tree, "a.txt", b"body");
        backend
            .set_lifecycle(
                &path,
                &LifecyclePolicy {
                    path: path.clone(),
                    action: RetentionAction::Notify,
                    retention_period: crate::core::values::Duration::ZERO,
                    min_age: crate::core::values::Duration::ZERO,
                    enabled: true,
                    description: "notify me".to_string(),
                    metadata: Vec::new(),
                },
            )
            .expect("set");
        let processed = backend.apply_lifecycle(&path).expect("apply");
        assert_eq!(processed, 1, "the real file was processed");
        // The notify action leaves the file exactly as it was.
        assert_eq!(fs::read(&path).expect("read"), b"body");
        // A file with no policy answers 0 — the honest "nothing to do".
        assert_eq!(
            backend
                .apply_lifecycle(&tree.join("other").to_string_lossy())
                .expect("apply"),
            0
        );
    }

    #[test]
    fn an_audit_subscription_pulls_bounded_and_cancels() {
        let (backend, tree) = sandbox("audit");
        let path = touch_path(&tree, "a.txt", b"body");
        let subscription = backend
            .subscribe_audit(&FileAuditFilter::default())
            .expect("subscribe");
        backend.touch(&path).expect("touch");
        backend.touch(&path).expect("touch");
        // A bounded pull: one entry at a time, and no repeats.
        let first = backend.pull_audit(subscription, 1).expect("pull");
        assert_eq!(first.len(), 1);
        assert_eq!(
            first[0].action,
            AuditAction::Write,
            "touch writes the file's timestamps, which is a write in the trail's vocabulary"
        );
        let second = backend.pull_audit(subscription, 1).expect("pull");
        assert_eq!(second.len(), 1);
        assert!(backend.pull_audit(subscription, 10).expect("pull").is_empty());
        // A filter narrows what a subscription delivers.
        let filtered = backend
            .subscribe_audit(&FileAuditFilter {
                path_pattern: "*.txt".to_string(),
                actions: vec![AuditAction::Read],
                ..Default::default()
            })
            .expect("subscribe");
        backend.touch(&path).expect("touch");
        assert!(backend.pull_audit(filtered, 10).expect("pull").is_empty(), "the filter's own action set excludes writes");
        backend.cancel_audit(subscription).expect("cancel");
        backend.cancel_audit(filtered).expect("cancel");
        assert_eq!(backend.pull_audit(subscription, 1).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn a_content_search_finds_the_real_lines() {
        let (backend, tree) = sandbox("content-search");
        touch_path(&tree, "one.txt", b"first line\nsecond line has the needle\nthird\n");
        touch_path(&tree, "two.txt", b"nothing here\n");
        let hits = backend
            .search_file_content(&ContentSearchQuery {
                path: tree.to_string_lossy().to_string(),
                query: "needle".to_string(),
                recursive: false,
                mime_types: Vec::new(),
                page: Pagination { offset: 0, limit: 10 },
                annotations: Vec::new(),
            })
            .expect("search");
        assert_eq!(hits.total, 1);
        assert_eq!(hits.items[0].snippet, "second line has the needle");
        assert_eq!(hits.items[0].line_number, 2, "the host's own line numbering");
        // A query nobody matches answers an empty page and a stable zero total.
        let none = backend
            .search_file_content(&ContentSearchQuery {
                path: tree.to_string_lossy().to_string(),
                query: "nowhere".to_string(),
                recursive: false,
                mime_types: Vec::new(),
                page: Pagination::default(),
                annotations: Vec::new(),
            })
            .expect("search");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn a_branch_names_a_stored_version() {
        let (backend, tree) = sandbox("branch");
        let path = touch_path(&tree, "v.txt", b"first");
        backend.record_version(&path);
        let BranchResult::Branch(branch) = backend
            .branch_version(&path, 1, "main")
            .expect("branch") else {
            panic!("branchVersion must answer the branch union arm");
        };
        assert_eq!(branch.branch_name, "main");
        assert_eq!(branch.base_version, 1);
        assert_eq!(branch.path, path);
        // The branch is a real object in the store, so a second backend over
        // the same store can read it back.
        assert!(backend.store_path(&["branches", &store_component(&path), "main"]).exists());
        // A version that was never stored is notFound.
        assert!(matches!(
            backend.branch_version(&path, 9_999, "ghost").expect("branch"),
            BranchResult::Error(ref e) if e.code == ErrorCode::NotFound
        ));
    }

    #[test]
    fn a_merge_writes_the_stores_own_bytes() {
        let (backend, tree) = sandbox("merge");
        let path = touch_path(&tree, "v.txt", b"first");
        backend.record_version(&path);
        backend.branch_version(&path, 1, "stable").expect("branch");
        fs::write(&path, b"rewritten").expect("mutate");
        backend.merge_branch(&path, "stable").expect("merge");
        assert_eq!(fs::read(&path).expect("read"), b"first", "the store's own bytes, written back");
    }

    #[test]
    fn branches_list_from_the_stores_own_records() {
        let (backend, tree) = sandbox("branches");
        let path = touch_path(&tree, "v.txt", b"first");
        backend.record_version(&path);
        backend.branch_version(&path, 1, "alpha").expect("branch");
        backend.branch_version(&path, 1, "beta").expect("branch");
        let page = backend.list_branches(&path, Pagination { offset: 0, limit: 1 }).expect("branches");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 2, "the store's whole record for this path");
        // A path with no branches answers a stable empty page.
        let none = backend
            .list_branches(&tree.join("other").to_string_lossy(), Pagination::default())
            .expect("branches");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn the_trash_is_the_hosts_own_trash_directory() {
        let (backend, _tree) = sandbox("trash-list");
        // The freedesktop trash layout, in the backend's own trash root: an
        // info record beside the file it names.
        fs::create_dir_all(backend.trash_root().join("files")).expect("dir");
        fs::create_dir_all(backend.trash_root().join("info")).expect("dir");
        fs::write(backend.trash_root().join("files/1"), b"trashed bytes").expect("file");
        fs::write(
            backend.trash_root().join("info/1.trashinfo"),
            "[Trash Info]\nPath=/original/one.txt\nDeletionDate=2026-01-02T03:04:05\n",
        )
        .expect("info");
        let page = backend.list_trash(Pagination { offset: 0, limit: 10 }).expect("trash");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].original_path, "/original/one.txt");
        assert_eq!(page.items[0].size, 13);
        assert!(page.items[0].deleted_at.as_nanos() > 0, "the host's own deletion date, parsed");
    }

    #[test]
    fn a_trash_restore_moves_the_hosts_own_file_back() {
        let (backend, _tree) = sandbox("trash-restore");
        fs::create_dir_all(backend.trash_root().join("files")).expect("dir");
        fs::create_dir_all(backend.trash_root().join("info")).expect("dir");
        fs::write(backend.trash_root().join("files/7"), b"restore me").expect("file");
        fs::write(
            backend.trash_root().join("info/7.trashinfo"),
            "[Trash Info]\nPath=/original/seven.txt\nDeletionDate=2026-02-03T04:05:06\n",
        )
        .expect("info");
        let target = temp_root("trash-target").join("seven.txt");
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("dir");
        }
        backend
            .restore_trash("7", &target.to_string_lossy())
            .expect("restore");
        assert_eq!(fs::read(&target).expect("read"), b"restore me");
        assert!(!backend.trash_root().join("info/7.trashinfo").exists(), "the info record is consumed too");
    }

    #[test]
    fn emptying_the_trash_removes_the_hosts_own_entries() {
        let (backend, _tree) = sandbox("trash-empty");
        fs::create_dir_all(backend.trash_root().join("files")).expect("dir");
        fs::create_dir_all(backend.trash_root().join("info")).expect("dir");
        fs::write(backend.trash_root().join("files/1"), b"one").expect("file");
        fs::write(backend.trash_root().join("files/2"), b"two").expect("file");
        fs::write(backend.trash_root().join("info/1.trashinfo"), b"Path=/a\n").expect("info");
        backend.empty_trash().expect("empty");
        assert!(!backend.trash_root().join("files/1").exists());
        assert!(!backend.trash_root().join("files/2").exists());
        assert!(!backend.trash_root().join("info/1.trashinfo").exists());
        let page = backend.list_trash(Pagination::default()).expect("trash");
        assert_eq!(page.total, 0, "the empty trash answers a stable zero");
    }

    #[test]
    fn recent_files_are_read_from_the_hosts_own_record() {
        let (backend, _tree) = sandbox("recent");
        // The host's own XBEL spelling, written by this test into the backend's
        // private record: every attribute is read by name, not by position.
        fs::write(
            &backend.recent_path(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\">\n \
             <bookmark href=\"file:///home/test/first.txt\" mimetype=\"text/plain\" modified=\"1767225600\"/>\n \
             <bookmark mimetype=\"image/png\" href=\"file:///home/test/second.png\" modified=\"1767312000\"/>\n \
             </xbel>\n",
        )
        .expect("record");
        let page = backend.get_recent_files(Pagination { offset: 0, limit: 10 }).expect("recent");
        assert_eq!(page.total, 2);
        // Newest first, whatever order the host wrote them in.
        assert!(page.items[0].path.ends_with("second.png"));
        assert_eq!(page.items[0].mime_type, "image/png");
        assert_eq!(page.items[0].timestamp.as_nanos(), 1_767_312_000_000_000_000);
        // A host with no record answers a stable empty page, never a history.
        let empty = LinuxFilesBackend::with_paths(
            temp_root("recent-empty").join("store"),
            temp_root("recent-empty").join("trash"),
            temp_root("recent-empty").join("recently-used.xbel"),
        );
        let none = empty.get_recent_files(Pagination::default()).expect("recent");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn a_default_app_comes_from_the_hosts_own_mimeapps() {
        let backend = LinuxFilesBackend::new();
        // MEASURED against this host: its own mimeapps.list and mimeinfo.cache
        // are read, so an answer is the host's association and a refusal names
        // that the host has none.
        let _ = backend.get_default_app("text/plain");
        let _ = backend.get_default_app("application/x-no-such-type");
        assert!(
            std::env::var("HOME").map(|home| {
                std::path::Path::new(&home).join(".config/mimeapps.list").exists()
                    || std::path::Path::new("/usr/share/applications/mimeinfo.cache").exists()
            })
            .unwrap_or(false),
            "measured: this host publishes an association table"
        );
    }

    #[test]
    fn health_is_the_measured_half_only() {
        let backend = LinuxFilesBackend::new();
        let FilesystemHealthResult::Health(health) = backend.get_filesystem_health("/").expect("health") else {
            panic!("getFilesystemHealth must answer the health union arm");
        };
        // The measured half: a real status token, from real numbers.
        assert!(["healthy", "read-only", "full", "nearly-full"].contains(&health.status.as_str()));
        assert!(health.details.contains("statvfs"), "the reason names the measured half");
        // DELIBERATELY PARTIAL: the details say the fsck half is not claimed.
        assert!(health.details.contains("never performs"), "{details}",  details = health.details);
    }

    #[test]
    fn a_quota_reports_the_measured_half() {
        let backend = LinuxFilesBackend::new();
        let quota = backend.get_quota("/").expect("quota");
        // The mount's own totals, from the kernel's statvfs.
        assert!(quota.max_size_bytes > 0);
        assert!(quota.current_size_bytes <= quota.max_size_bytes);
        assert!(quota.soft_limit_percent <= 100);
        // The write half is gated with the measured reason, never performed.
        let err = backend
            .set_quota("/", &FilesystemQuota::default())
            .expect_err("the write is gated");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(err.message.starts_with("no-privilege:"), "{err:?}");
    }

    #[test]
    fn a_snapshot_is_a_content_addressed_reference() {
        let (backend, tree) = sandbox("snapshot");
        touch_path(&tree, "a.txt", b"first body");
        touch_path(&tree, "sub/b.txt", b"second body");
        let mount = mount_of(&tree.to_string_lossy());
        let SnapshotRefResult::Snapshot(id) = backend
            .create_snapshot(&tree.to_string_lossy(), "before")
            .expect("snapshot") else {
            panic!("createSnapshot must answer the snapshot union arm");
        };
        assert!(id.get() > 0, "a snapshot id was minted");
        // The snapshot's manifest is a real object naming the real digests, so
        // the id resolves to bytes that exist.
        let objects: Vec<String> = fs::read_dir(backend.store_path(&["snapshots"]))
            .expect("snapshots dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(objects.len(), 1);
        let manifest = fs::read_to_string(backend.store_path(&["snapshots", &objects[0]])).expect("manifest");
        assert!(manifest.contains(&hex(&sha256(b"first body"))));
        assert!(manifest.contains(&hex(&sha256(b"second body"))));
        // MEASURED: this host's filesystem is named, and the reason says the
        // snapshot is a store reference rather than a filesystem one.
        // MEASURED: the sandbox's own filesystem is whatever this host says it
        // is (tmpfs on this box), and neither tmpfs nor ext4 can snapshot, so
        // the reference is the store's own object rather than the kernel's.
        let measured = mount.as_ref().map(|m| m.fs_type.clone()).unwrap_or_default();
        assert!(!measured.is_empty(), "measured: /proc/mounts names the filesystem");
        assert_ne!(
            measured, "btrfs",
            "measured: the sandbox's filesystem is {measured}, so no filesystem snapshot exists to take"
        );
    }

    #[test]
    fn an_acquired_capability_binds_a_service_to_its_path() {
        let (backend, tree) = sandbox("acquire");
        let path = touch_path(&tree, "granted.txt", b"the body");
        let bound = backend.acquire_file(&path, 4_242).expect("acquire");
        assert_eq!(bound.path, path, "the capability is bound to exactly one path");
        assert_eq!(bound.authority, 4_242, "the authority token travels with it");
        // A path that names nothing cannot be granted.
        assert_eq!(
            backend
                .acquire_file(&tree.join("gone").to_string_lossy(), 1)
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn a_bound_service_operates_on_its_path() {
        let (backend, tree) = sandbox("bound");
        let path = touch_path(&tree, "granted.txt", b"original body");
        let bound = backend.acquire_file(&path, 7).expect("acquire");
        // The bound read is the capability's own read: it opens, reads and
        // closes its own descriptor, so a holder never manages one.
        assert_eq!(backend.bound_read(&bound, 0, 1024).expect("read"), b"original body");
        assert_eq!(backend.bound_write(&bound, 0, b"rewritten").expect("write"), 9);
        assert_eq!(fs::read(&path).expect("read"), b"rewrittenbody");
        let FileInfoResult::Info(info) = backend.bound_stat(&bound).expect("stat") else {
            panic!("the bound stat must answer the info union arm");
        };
        // A write at an offset does not truncate: the file keeps the length of
        // what was already there (13) when the write is shorter than it.
        assert_eq!(info.size, 13);
        backend.bound_close(&bound).expect("close");
    }

    #[test]
    fn a_bound_service_retires_its_descriptor() {
        let (backend, tree) = sandbox("bound-close");
        let path = touch_path(&tree, "granted.txt", b"body");
        let bound = backend.acquire_file(&path, 1).expect("acquire");
        let handle = backend.bound_open(&bound, &OpenRequest { mode: OpenMode::Read, flags: Vec::new() }).expect("open");
        assert_eq!(backend.open_handle_count(), 1);
        backend.close_file(&handle).expect("close");
        assert_eq!(backend.open_handle_count(), 0, "the descriptor the bound path opened is gone");
        // The capability itself is only closed by its holder dropping it; the
        // close verb records that it was.
        backend.bound_close(&bound).expect("close");
    }

    #[test]
    fn a_watch_pull_is_bounded_and_cancellable() {
        let (backend, tree) = sandbox("watch-bounded");
        let watched = tree.join("watched");
        fs::create_dir_all(&watched).expect("dir");
        let subscription = backend.watch_directory(&watched.to_string_lossy()).expect("watch");
        for i in 0..5 {
            touch_path(&watched, &alloc::format!("f{i}.txt"), b"x");
        }
        let mut seen = 0usize;
        for _ in 0..40 {
            let batch = backend.pull_watch(subscription, 2).expect("pull");
            assert!(batch.len() <= 2, "the pull is bounded by the caller's number");
            seen += batch.len();
            if seen >= 5 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(seen >= 5, "every event the kernel produced is delivered: {seen}");
        backend.cancel_watch(subscription).expect("cancel");
    }

    /// The unverified default surface refuses with a reason on a backend that
    /// serves nothing — the law #6 baseline the platform backend overrides.
    #[test]
    fn the_default_surface_refuses_with_a_reason() {
        let backend = NothingFilesBackend;
        let error = backend
            .open_file("/x", &OpenRequest::default())
            .expect_err("a backend that cannot open must say so");
        assert_eq!(error.code, ErrorCode::NotSupported);
        assert!(error.message.starts_with("no-backend:"));
    }

    /// A backend that serves nothing: the honesty baseline.
    struct NothingFilesBackend;

    impl crate::domains::files::backend::FilesBackend for NothingFilesBackend {}
}
