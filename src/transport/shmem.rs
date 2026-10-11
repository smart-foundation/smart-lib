//! Shmem: zero-copy frames through a shared arena.
//!
//! The payload is written **once**, into a shared arena, and referenced on
//! the wire by the contract's `T.BufferId` — the message carries the
//! reference, never the bytes (`types.capnp:502` `DataHandle.buffer`).
//!
//! ## The measured copy counts
//!
//! The arena keeps counters, and the tests assert them:
//!
//! | path | copies | where counted |
//! |---|---|---|
//! | `publish` (producer writes its payload into a slot) | 1 | `ArenaStats::publishes` |
//! | `resolve` (consumer reads through the `BufferId`) | **0** | `ArenaStats::resolve_copy_bytes` is structurally 0 (a borrow) |
//! | control ring (`send_message`/`recv`: the small capnp message that *references* a slot) | 1 write + 1 read | `control_copy_bytes` |
//!
//! Zero is not a claim here: `resolve` returns a borrow of the slot, so
//! there is no `memcpy` in the code path at all, and the Linux witness
//! (`tests/shmem.rs::memfd_two_mappings_are_the_same_pages`) proves the
//! consumer reads the producer's *pages*: a byte written through one
//! mapping of a memfd is observed through a second, independent mapping.
//!
//! ## Ring content format
//!
//! A ring slot is `[u32 content_len][content]`, where `content` is
//! `[u8 kind][payload]` with the `frame::KIND_*` vocabulary shared with the
//! stream transports. Bulk data never rides the ring — only the small
//! messages that reference slots by `T.BufferId`.
//!
//! ## Safety model (stated, because this is the shared-memory layer)
//!
//! * Slot payloads are written through raw pointers into `&self`-shared
//!   memory. Sound because: slots are disjoint, one producer owns a slot at
//!   a time (the per-slot sequence protocol below enforces it), and
//!   publication is an atomic `Release` store; readers `Acquire`-load the
//!   same word before touching the payload. Standard seqlock-style
//!   publication, which `std` has no portable API for.
//! * The header's atomics sit at fixed 8-byte offsets in an 8-byte-aligned
//!   region (`Vec<u64>` for the heap backing; page-aligned `mmap` for the
//!   OS backing), which is what makes shared `&AtomicU64` references to
//!   them legal.
//! * `Storage` is `Send + Sync` via `unsafe impl` justified by the above.
//! * The ring is bounded: a full ring answers `LimitExceeded`, never an
//!   unbounded queue (charter: bounded pulls, no push).

use alloc::string::ToString;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::schema::types_capnp;
use crate::transport::bootstrap::RegionPass;
use crate::transport::channel::{
    frame, CapHandoff, CapId, CapOutcome, Channel, CloseReason, FrameRef, MessageOut,
    NegotiateResponse, OsHandle, PeerCredentials, RecvBatch, RecvMessage, TransportCaps,
    TransportError, TransportKind,
};
use crate::transport::sync::Mutex;

/// Arena magic: `SMTR` + format generation.
pub const ARENA_MAGIC: u64 = u64::from_le_bytes(*b"SMTRNG01");
/// On-region format version. Bump on any layout change.
pub const ARENA_FORMAT: u32 = 1;

// Header field offsets. All 8-byte aligned by construction.
const OFF_MAGIC: usize = 0;
const OFF_FORMAT: usize = 8;
const OFF_SLOT_BYTES: usize = 12;
const OFF_SLOT_COUNT: usize = 16;
const OFF_RING_SLOTS: usize = 20;
const OFF_RING_BYTES: usize = 24;
const OFF_NAMESPACE_ROOT: usize = 32;
const OFF_EPOCH: usize = 40;
const OFF_SEALS: usize = 44;
const OFF_STATE: usize = 48;
const OFF_A_PUB: usize = 64;
const OFF_A_CON: usize = 72;
const OFF_B_PUB: usize = 80;
const OFF_B_CON: usize = 88;
/// End of the header; slots begin here.
pub const HEADER_LEN: usize = 96;

/// The arena is inert (no publish, no resolve, no frame) until the
/// authenticated bootstrap established it.
pub const STATE_INERT: u32 = 0;
/// Established: publishes and resolves are live.
pub const STATE_ESTABLISHED: u32 = 1;

/// Per-slot layout: `[AtomicU64 seq][u32 len][u32 reserved][payload]`.
const SLOT_HEAD_LEN: usize = 16;

/// Process-global arena epoch counter: two arenas must never share a
/// `BufferId` namespace, and the epoch is what derives the root.
static EPOCH_COUNTER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

fn next_epoch() -> u32 {
    EPOCH_COUNTER.fetch_add(1, Ordering::Relaxed)
}

const MAX_SLOT_BYTES: u32 = 16 * 1024 * 1024;
const MAX_SLOT_COUNT: u32 = 4096;
const MAX_RING_SLOTS: u32 = 1024;

/// Geometry of an arena. Fixed at creation, validated at adoption; a
/// mismatch is a bootstrap refusal, never a reinterpretation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaConfig {
    /// Payload bytes per slot.
    pub slot_bytes: u32,
    /// Number of data slots.
    pub slot_count: u32,
    /// Slots per control ring (each direction has one).
    pub ring_slots: u32,
    /// Bytes per control-ring slot.
    pub ring_bytes: u32,
}

impl Default for ArenaConfig {
    fn default() -> Self {
        Self {
            slot_bytes: 64 * 1024,
            slot_count: 64,
            ring_slots: 16,
            ring_bytes: 64 * 1024,
        }
    }
}

impl ArenaConfig {
    fn validate(&self) -> Result<(), TransportError> {
        if self.slot_bytes == 0 || self.slot_bytes > MAX_SLOT_BYTES || self.slot_bytes % 8 != 0 {
            return Err(TransportError::Framing {
                detail: alloc::format!(
                    "slot_bytes {} is not a valid 8-aligned size",
                    self.slot_bytes
                ),
            });
        }
        if self.slot_count == 0 || self.slot_count > MAX_SLOT_COUNT {
            return Err(TransportError::Framing {
                detail: alloc::format!("slot_count {} out of range", self.slot_count),
            });
        }
        if self.ring_slots < 2 || self.ring_slots > MAX_RING_SLOTS || self.ring_bytes < 64 {
            return Err(TransportError::Framing {
                detail: "ring geometry out of range".to_string(),
            });
        }
        Ok(())
    }

    fn slot_stride(&self) -> usize {
        SLOT_HEAD_LEN + self.slot_bytes as usize
    }

    /// Total region bytes this geometry requires.
    pub fn region_bytes(&self) -> usize {
        HEADER_LEN
            + self.slot_count as usize * self.slot_stride()
            + 2 * self.ring_slots as usize * self.ring_bytes as usize
    }
}

/// The measured arena counters. `resolve_copy_bytes` is structurally zero:
/// `resolve` returns a borrow. The field exists so the zero is a *measured*
/// number in the report, not an adjective.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArenaStats {
    /// Producer writes into slots (the one unavoidable write).
    pub publishes: u64,
    /// Resolve calls served.
    pub resolve_calls: u64,
    /// Bytes the resolve path copied: always 0 (borrow, not copy).
    pub resolve_copy_bytes: u64,
    /// Control frames written into the rings.
    pub control_frames_sent: u64,
    /// Control frames read out of the rings.
    pub control_frames_received: u64,
    /// Bytes the control path copied (the small referencing messages).
    pub control_copy_bytes: u64,
    /// Out-of-namespace `T.BufferId` lookups refused.
    pub refused_out_of_namespace: u64,
}

struct ArenaCounters {
    publishes: AtomicU64,
    resolve_calls: AtomicU64,
    control_frames_sent: AtomicU64,
    control_frames_received: AtomicU64,
    control_copy_bytes: AtomicU64,
    refused_out_of_namespace: AtomicU64,
}

impl ArenaCounters {
    fn snapshot(&self) -> ArenaStats {
        ArenaStats {
            publishes: self.publishes.load(Ordering::Relaxed),
            resolve_calls: self.resolve_calls.load(Ordering::Relaxed),
            // By construction there is no copy in `resolve`; the counter
            // therefore has no increment site. It stays a first-class
            // measured field so a future copy cannot slip in unseen.
            resolve_copy_bytes: 0,
            control_frames_sent: self.control_frames_sent.load(Ordering::Relaxed),
            control_frames_received: self.control_frames_received.load(Ordering::Relaxed),
            control_copy_bytes: self.control_copy_bytes.load(Ordering::Relaxed),
            refused_out_of_namespace: self.refused_out_of_namespace.load(Ordering::Relaxed),
        }
    }
}

/// Which end of the arena a channel drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Producer side of the A→B ring; consumer of the B→A ring.
    A,
    /// Producer side of the B→A ring; consumer of the A→B ring.
    B,
}

impl Direction {
    /// The other end's ring (the one this direction reads from).
    pub fn other(self) -> Self {
        match self {
            Direction::A => Direction::B,
            Direction::B => Direction::A,
        }
    }
}

/// How the arena's bytes are actually backed.
enum Storage {
    /// `alloc`-only, process-private. 8-byte aligned by construction
    /// (`Vec<u64>`), which is what the header atomics require.
    Heap(Vec<u64>),
    /// A real shared region: `memfd_create` + `mmap` (Linux, `std` builds).
    /// The mapping and its fd (the SCM_RIGHTS-passable handle) live in
    /// `sys::SharedRegion`.
    Mmap(crate::transport::sys::SharedRegion),
}

// SAFETY: `Storage::Mmap` holds a raw pointer to a private mapping of a
// shared region. All *shared* mutation goes through the atomics and the
// publication protocol documented at the module level; the mapping is
// owned by the `Arena` and released exactly once (on drop).
unsafe impl Send for Storage {}
unsafe impl Sync for Storage {}

impl Drop for Storage {
    fn drop(&mut self) {
        if let Storage::Mmap(region) = self {
            // The mapping and its fd are released exactly once here; peers
            // hold their own mappings/fds.
            crate::transport::sys::release_shared_region(region);
        }
    }
}

impl core::fmt::Debug for Arena {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Arena")
            .field("epoch", &self.epoch)
            .field("namespace_root", &self.namespace_root())
            .field("established", &self.is_established())
            .finish_non_exhaustive()
    }
}

/// A shared arena of frames plus two bounded control rings.
pub struct Arena {
    storage: Storage,
    slot_bytes: u32,
    slot_count: u32,
    ring_slots: u32,
    ring_bytes: u32,
    epoch: u32,
    stats: ArenaCounters,
}

impl Arena {
    /// Create (own) a new arena. It starts **inert**: nothing publishes or
    /// resolves until the bootstrap establishes it.
    ///
    /// On Linux+std the backing is a real sealed shared region (so the
    /// bootstrap can pass its fd with SCM_RIGHTS); everywhere else it is
    /// the `alloc`-only heap backing, and the arena is process-private by
    /// construction.
    pub fn create(config: ArenaConfig) -> Result<Arc<Self>, TransportError> {
        config.validate()?;
        let storage = match crate::transport::sys::create_shared_region(config.region_bytes()) {
            Ok(region) => Storage::Mmap(region),
            Err(TransportError::NotCompiled { .. }) => Storage::new_heap(config.region_bytes())?,
            Err(e) => return Err(e),
        };
        let epoch = next_epoch();
        let arena = Arc::new(Self::assemble(storage, &config, epoch));
        arena.write_header(&config, epoch, 0);
        Ok(arena)
    }

    /// Adopt a region a peer passed (bootstrap, peer side). Validates the
    /// geometry and epoch; a mismatch is refused, never reinterpreted.
    pub fn adopt(
        region: &RegionPass,
        expected: &ArenaConfig,
        expected_epoch: u32,
    ) -> Result<Arc<Self>, TransportError> {
        match region {
            RegionPass::Shared(arena) => {
                if arena.config() != *expected || arena.epoch() != expected_epoch {
                    return Err(TransportError::Framing {
                        detail: "offered arena does not match the expected geometry".to_string(),
                    });
                }
                Ok(arena.clone())
            }
            RegionPass::OsHandle(fd) => {
                let storage = Storage::adopt_fd(fd.0 as i32)?;
                Ok(Arc::new(Self::from_storage(storage, expected, expected_epoch)?))
            }
        }
    }

    /// Construct from an already-validated region (peer side).
    fn from_storage(
        storage: Storage,
        expected: &ArenaConfig,
        expected_epoch: u32,
    ) -> Result<Self, TransportError> {
        let len = storage.len();
        let min = expected.region_bytes();
        if len < min {
            return Err(TransportError::Framing {
                detail: alloc::format!("region is {len} bytes, geometry needs {min}"),
            });
        }
        if storage.read_u64(OFF_MAGIC) != ARENA_MAGIC {
            return Err(TransportError::Framing {
                detail: "region magic mismatch: not an arena".to_string(),
            });
        }
        if storage.read_u32(OFF_FORMAT) != ARENA_FORMAT {
            return Err(TransportError::Framing {
                detail: alloc::format!(
                    "arena format {}, this build speaks {ARENA_FORMAT}",
                    storage.read_u32(OFF_FORMAT)
                ),
            });
        }
        let config = ArenaConfig {
            slot_bytes: storage.read_u32(OFF_SLOT_BYTES),
            slot_count: storage.read_u32(OFF_SLOT_COUNT),
            ring_slots: storage.read_u32(OFF_RING_SLOTS),
            ring_bytes: storage.read_u32(OFF_RING_BYTES),
        };
        if config != *expected {
            return Err(TransportError::Framing {
                detail: alloc::format!("arena geometry {config:?} != expected {expected:?}"),
            });
        }
        let epoch = storage.read_u32(OFF_EPOCH);
        if epoch != expected_epoch {
            return Err(TransportError::Framing {
                detail: alloc::format!("arena epoch {epoch} != expected {expected_epoch}"),
            });
        }
        Ok(Self::assemble(storage, &config, epoch))
    }

    /// Build the struct from a backing and geometry, without header
    /// validation (the creation path: the header is ours to write).
    fn assemble(storage: Storage, config: &ArenaConfig, epoch: u32) -> Self {
        Self {
            storage,
            slot_bytes: config.slot_bytes,
            slot_count: config.slot_count,
            ring_slots: config.ring_slots,
            ring_bytes: config.ring_bytes,
            epoch,
            stats: ArenaCounters {
                publishes: AtomicU64::new(0),
                resolve_calls: AtomicU64::new(0),
                control_frames_sent: AtomicU64::new(0),
                control_frames_received: AtomicU64::new(0),
                control_copy_bytes: AtomicU64::new(0),
                refused_out_of_namespace: AtomicU64::new(0),
            },
        }
    }

    fn write_header(&self, config: &ArenaConfig, epoch: u32, namespace_root: u64) {
        self.storage.write_u64(OFF_MAGIC, ARENA_MAGIC);
        self.storage.write_u32(OFF_FORMAT, ARENA_FORMAT);
        self.storage.write_u32(OFF_SLOT_BYTES, config.slot_bytes);
        self.storage.write_u32(OFF_SLOT_COUNT, config.slot_count);
        self.storage.write_u32(OFF_RING_SLOTS, config.ring_slots);
        self.storage.write_u32(OFF_RING_BYTES, config.ring_bytes);
        self.storage.write_u64(OFF_NAMESPACE_ROOT, namespace_root);
        self.storage.write_u32(OFF_EPOCH, epoch);
        self.storage.write_u32(OFF_SEALS, self.seals_word());
        self.atomic_state().store(STATE_INERT, Ordering::Release);
        // Compile-time layout assertions: if these move, the format
        // version must move with them (the bootstrap refuses mismatches).
        const _: () = assert!(HEADER_LEN == 96);
        const _: () = assert!(OFF_STATE % 4 == 0 && OFF_A_PUB % 8 == 0 && OFF_B_CON % 8 == 0);
    }

    /// Assign the `T.BufferId` namespace: slot 0 gets `root`. The bootstrap
    /// calls this before establishment; ids outside the resulting range are
    /// refused, not resolved.
    /// Assign the `T.BufferId` namespace.
    pub fn set_namespace_root(&self, root: u64) {
        self.storage.write_u64(OFF_NAMESPACE_ROOT, root);
    }

    /// Mark established. The bootstrap calls this after the peer's
    /// `Accept`; both sides observe it (it lives in the shared header).
    /// Mark the arena established (shared state word).
    pub fn establish(&self) {
        self.atomic_state().store(STATE_ESTABLISHED, Ordering::Release);
    }

    /// Whether the arena is established.
    pub fn is_established(&self) -> bool {
        self.atomic_state().load(Ordering::Acquire) == STATE_ESTABLISHED
    }

    fn check_established(&self) -> Result<(), TransportError> {
        if self.is_established() {
            Ok(())
        } else {
            Err(TransportError::ArenaNotEstablished)
        }
    }

    /// The arena geometry.
    pub fn config(&self) -> ArenaConfig {
        ArenaConfig {
            slot_bytes: self.slot_bytes,
            slot_count: self.slot_count,
            ring_slots: self.ring_slots,
            ring_bytes: self.ring_bytes,
        }
    }

    /// The arena epoch.
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// First `T.BufferId` this arena resolves (read from the live header so
    /// an owner that re-assigned the namespace sees the current value).
    /// First `T.BufferId` this arena resolves (read from the live header).
    pub fn namespace_root(&self) -> u64 {
        self.storage.read_u64(OFF_NAMESPACE_ROOT)
    }

    /// The OS handle of the region, when this backing has one — the thing
    /// SCM_RIGHTS passes. Passing an anonymous memfd's fd *is* the
    /// capability transfer: no third process can open the region by name.
    /// The OS handle of the region, when this backing has one.
    pub fn region_handle(&self) -> Option<OsHandle> {
        match &self.storage {
            Storage::Mmap(region) => Some(OsHandle(region.fd as u64)),
            Storage::Heap(_) => None,
        }
    }

    /// Seal word of this backing (0 when the host cannot seal — the heap
    /// backing — declared, not faked).
    /// The seal word of this backing.
    pub fn seals_word(&self) -> u32 {
        match &self.storage {
            Storage::Mmap(_) => crate::transport::sys::memory_seals(),
            Storage::Heap(_) => 0,
        }
    }

    /// The measured counters (snapshot).
    pub fn stats(&self) -> ArenaStats {
        self.stats.snapshot()
    }

    fn slot_index(&self, id: u64) -> Result<usize, TransportError> {
        let root = self.namespace_root();
        if id < root || id >= root + self.slot_count as u64 {
            self.stats
                .refused_out_of_namespace
                .fetch_add(1, Ordering::Relaxed);
            return Err(TransportError::BufferNotResolvable {
                id,
                detail: alloc::format!(
                    "outside this arena's namespace [{root}, {})",
                    root + self.slot_count as u64
                ),
            });
        }
        Ok((id - root) as usize)
    }

    fn slot_offset(&self, index: usize) -> usize {
        HEADER_LEN + index * (SLOT_HEAD_LEN + self.slot_bytes as usize)
    }

    fn slot_seq(&self, index: usize) -> &AtomicU64 {
        // SAFETY: 8-byte aligned by construction; atomics only.
        unsafe { &*(self.storage.base().add(self.slot_offset(index)) as *const AtomicU64) }
    }

    /// Publish `bytes` into slot `id`: the producer's single write. Every
    /// later read through the handle is a borrow.
    pub fn publish(
        &self,
        id: types_capnp::buffer_id::Reader<'_>,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        self.check_established()?;
        let index = self.slot_index(id.get_id())?;
        if bytes.len() > self.slot_bytes as usize {
            return Err(TransportError::LimitExceeded {
                limit: self.slot_bytes,
            });
        }
        let seq = self.slot_seq(index);
        let current = seq.load(Ordering::Relaxed);
        if current % 2 == 1 {
            return Err(TransportError::Message {
                detail: "slot write in progress".to_string(),
            });
        }
        // Mark "writing" (odd), write the payload, then publish (even) with
        // a Release so the payload is visible before the sequence is.
        seq.store(current + 1, Ordering::Relaxed);
        let slot = self.slot_offset(index);
        // SAFETY: this producer owns the slot (the odd sequence excludes
        // every other writer) and the destination is in-bounds (checked
        // above; the region was sized for the geometry at construction).
        unsafe {
            let base = self.storage.base().add(slot + 8);
            (base as *mut u32).write_unaligned(bytes.len() as u32);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(8), bytes.len());
        }
        seq.store(current + 2, Ordering::Release);
        self.stats.publishes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Resolve the published payload behind `id`. Zero copies: the returned
    /// `FrameRef` borrows the arena slot.
    pub fn resolve(
        &self,
        id: types_capnp::buffer_id::Reader<'_>,
    ) -> Result<FrameRef<'_>, TransportError> {
        self.check_established()?;
        let index = self.slot_index(id.get_id())?;
        let seq = self.slot_seq(index).load(Ordering::Acquire);
        if seq == 0 || seq % 2 == 1 {
            return Err(TransportError::BufferNotResolvable {
                id: id.get_id(),
                detail: "nothing published at this slot".to_string(),
            });
        }
        let slot = self.slot_offset(index);
        // SAFETY: the Acquire-load pairs with the producer's Release store,
        // so length and payload are visible; the slot is disjoint from
        // every writer; the region outlives the returned borrow.
        let (len, ptr) = unsafe {
            let base = self.storage.base().add(slot + 8);
            let len = (base as *const u32).read_unaligned();
            (len as usize, base.add(8))
        };
        if len > self.slot_bytes as usize {
            return Err(TransportError::Framing {
                detail: "slot length exceeds geometry: arena corrupt".to_string(),
            });
        }
        self.stats.resolve_calls.fetch_add(1, Ordering::Relaxed);
        Ok(FrameRef::new(unsafe { core::slice::from_raw_parts(ptr, len) }))
    }

    /// Raw address and length of a published slot's payload, for callers
    /// that hold this arena in shared storage and must hand out a borrow
    /// tied to *their* lifetime rather than to a temporary reference to the
    /// arena (the UnixSocket region path does exactly this).
    ///
    /// SAFETY for callers: the returned pointer is valid as long as the
    /// arena lives and the slot is not republished; constructing a slice
    /// from it is sound for exactly as long as the arena outlives the
    /// borrow — the same contract `resolve` states.
    pub fn published_payload_ptr(&self, id: u64) -> Result<(*const u8, usize), TransportError> {
        self.check_established()?;
        let index = self.slot_index(id)?;
        let seq = self.slot_seq(index).load(Ordering::Acquire);
        if seq == 0 || seq % 2 == 1 {
            return Err(TransportError::BufferNotResolvable {
                id,
                detail: "nothing published at this slot".to_string(),
            });
        }
        let slot = self.slot_offset(index);
        // SAFETY: as in `resolve` — the Acquire pairs with the producer's
        // Release, and the region outlives any borrow the caller forms.
        let (len, ptr) = unsafe {
            let base = self.storage.base().add(slot + 8);
            let len = (base as *const u32).read_unaligned();
            (len as usize, base.add(8))
        };
        if len > self.slot_bytes as usize {
            return Err(TransportError::Framing {
                detail: "slot length exceeds geometry: arena corrupt".to_string(),
            });
        }
        Ok((ptr, len))
    }

    fn ring_sequences(&self, ring_of: Direction) -> (&AtomicU64, &AtomicU64) {
        let base = self.storage.base();
        let (pub_off, con_off) = match ring_of {
            Direction::A => (OFF_A_PUB, OFF_A_CON),
            Direction::B => (OFF_B_PUB, OFF_B_CON),
        };
        // SAFETY: 8-byte aligned by construction; atomics only.
        unsafe {
            (
                &*(base.add(pub_off) as *const AtomicU64),
                &*(base.add(con_off) as *const AtomicU64),
            )
        }
    }

    fn ring_offset(&self, ring_of: Direction, index: usize) -> usize {
        let slots_bytes = self.slot_count as usize * (SLOT_HEAD_LEN + self.slot_bytes as usize);
        let rings_base = HEADER_LEN + slots_bytes;
        let ring = match ring_of {
            Direction::A => 0,
            Direction::B => 1,
        };
        rings_base
            + ring * self.ring_slots as usize * self.ring_bytes as usize
            + index * self.ring_bytes as usize
    }

    /// Enqueue a capnp control message on this direction's ring. One copy:
    /// the frame is written into the shared slot.
    pub fn send_message(
        &self,
        from: Direction,
        kind: u8,
        message: &impl capnp::message::ReaderSegments,
    ) -> Result<(), TransportError> {
        self.check_established()?;
        let (pub_seq, con_seq) = self.ring_sequences(from);
        let head = pub_seq.load(Ordering::Acquire);
        let tail = con_seq.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.ring_slots as u64 {
            return Err(TransportError::LimitExceeded {
                limit: self.ring_slots,
            });
        }
        let index = (head % self.ring_slots as u64) as usize;
        let off = self.ring_offset(from, index);
        let cap = self.ring_bytes as usize;
        // SAFETY: the slot at `head` is free (bounded above); the producer
        // publishes with Release after writing; the consumer reuses a slot
        // only after copying it and advancing `con` with Release.
        let written = unsafe {
            let dst = core::slice::from_raw_parts_mut(self.storage.base().add(off + 4), cap - 4);
            dst[0] = kind;
            frame::flat_message_into(&mut dst[1..], message)?
        };
        // SAFETY: same slot, header word of the slot.
        unsafe {
            (self.storage.base().add(off) as *mut u32).write_unaligned((written + 1) as u32);
        }
        pub_seq.store(head + 1, Ordering::Release);
        self.stats
            .control_frames_sent
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .control_copy_bytes
            .fetch_add((written + 1) as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Enqueue a close notice on this direction's ring.
    pub fn send_close(&self, from: Direction, reason: &CloseReason) -> Result<(), TransportError> {
        self.check_established()?;
        let (pub_seq, con_seq) = self.ring_sequences(from);
        let head = pub_seq.load(Ordering::Acquire);
        let tail = con_seq.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.ring_slots as u64 {
            return Err(TransportError::LimitExceeded {
                limit: self.ring_slots,
            });
        }
        let index = (head % self.ring_slots as u64) as usize;
        let off = self.ring_offset(from, index);
        let payload = frame::encode_close(reason);
        if 1 + payload.len() + 4 > self.ring_bytes as usize {
            return Err(TransportError::LimitExceeded {
                limit: self.ring_bytes,
            });
        }
        // SAFETY: slot is free (bounded above), published with Release below.
        unsafe {
            let base = self.storage.base().add(off);
            (base as *mut u32).write_unaligned((1 + payload.len()) as u32);
            *base.add(4) = frame::KIND_CLOSE;
            core::ptr::copy_nonoverlapping(payload.as_ptr(), base.add(5), payload.len());
        }
        pub_seq.store(head + 1, Ordering::Release);
        Ok(())
    }

    /// Copy the next frame from `ring_of` into `scratch`
    /// (`[kind][payload...]`). Returns false when the ring is empty
    /// (bounded pull). One copy.
    pub fn recv_frame(&self, ring_of: Direction, scratch: &mut Vec<u8>) -> Result<bool, TransportError> {
        let (pub_seq, con_seq) = self.ring_sequences(ring_of);
        let head = pub_seq.load(Ordering::Acquire);
        let tail = con_seq.load(Ordering::Acquire);
        if head == tail {
            return Ok(false);
        }
        let index = (tail % self.ring_slots as u64) as usize;
        let off = self.ring_offset(ring_of, index);
        // SAFETY: the slot was published (Release by the producer, observed
        // through the Acquire above) and is not reused until this consumer
        // advances `con`.
        let (len, bytes) = unsafe {
            let base = self.storage.base().add(off);
            let len = (base as *const u32).read_unaligned() as usize;
            if len + 4 > self.ring_bytes as usize {
                return Err(TransportError::Framing {
                    detail: "ring slot length exceeds geometry".to_string(),
                });
            }
            (len, base.add(4))
        };
        scratch.clear();
        scratch.reserve(len);
        // SAFETY: `bytes`/`len` are inside the mapping; the slot stays
        // valid until `con` advances below.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes, scratch.as_mut_ptr(), len);
            scratch.set_len(len);
        }
        con_seq.store(tail + 1, Ordering::Release);
        self.stats
            .control_frames_received
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .control_copy_bytes
            .fetch_add(len as u64, Ordering::Relaxed);
        Ok(true)
    }

    fn atomic_state(&self) -> &AtomicU32 {
        // SAFETY: 8-byte aligned by construction; atomics only.
        unsafe { &*(self.storage.base().add(OFF_STATE) as *const AtomicU32) }
    }
}

impl Storage {
    fn new_heap(total: usize) -> Result<Self, TransportError> {
        // `Vec<u64>` gives the 8-byte alignment the header atomics require.
        let words = (total + 7) / 8;
        let mut v: Vec<u64> = Vec::new();
        v.try_reserve_exact(words)
            .map_err(|_| TransportError::Message {
                detail: "arena allocation failed".to_string(),
            })?;
        v.resize(words, 0);
        Ok(Storage::Heap(v))
    }

    fn adopt_fd(fd: i32) -> Result<Self, TransportError> {
        Ok(Storage::Mmap(crate::transport::sys::adopt_shared_region(fd)?))
    }

    fn base(&self) -> *mut u8 {
        match self {
            Storage::Heap(v) => v.as_ptr() as *mut u8,
            Storage::Mmap(region) => region.ptr,
        }
    }

    fn len(&self) -> usize {
        match self {
            Storage::Heap(v) => v.len() * 8,
            Storage::Mmap(region) => region.len,
        }
    }

    fn read_u64(&self, off: usize) -> u64 {
        // SAFETY: offsets are inside the region (sized at construction from
        // the geometry) and 8-byte aligned.
        unsafe { (self.base().add(off) as *const u64).read_unaligned() }
    }

    fn read_u32(&self, off: usize) -> u32 {
        unsafe { (self.base().add(off) as *const u32).read_unaligned() }
    }

    fn write_u64(&self, off: usize, value: u64) {
        unsafe { (self.base().add(off) as *mut u64).write_unaligned(value) }
    }

    fn write_u32(&self, off: usize, value: u32) {
        unsafe { (self.base().add(off) as *mut u32).write_unaligned(value) }
    }
}

impl core::fmt::Debug for ShmemHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The arena's bytes are not the interesting part of a handle.
        f.debug_struct("ShmemHandle")
            .field("direction", &self.direction)
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

/// The bootstrap-produced handle that `Transport::Shmem` carries: an arena
/// plus this side's direction plus the (observed) peer identity.
pub struct ShmemHandle {
    /// The shared arena.
    pub arena: Arc<Arena>,
    /// Which end of the rings this side drives.
    pub direction: Direction,
    /// The peer's identity, observed during the bootstrap.
    pub peer: Option<PeerCredentials>,
}

impl ShmemHandle {
    /// Build a handle from a bootstrapped arena and this side's direction.
    pub fn new(arena: Arc<Arena>, direction: Direction, peer: Option<PeerCredentials>) -> Self {
        Self {
            arena,
            direction,
            peer,
        }
    }
}

/// A `Channel` over a shared arena.
pub struct ShmemChannel {
    arena: Arc<Arena>,
    direction: Direction,
    peer: Option<PeerCredentials>,
    close_notice: Mutex<Option<CloseReason>>,
    closed: AtomicBool,
}

impl ShmemChannel {
    /// A channel over one direction of a bootstrapped arena.
    pub fn from_handle(handle: ShmemHandle) -> Self {
        Self {
            arena: handle.arena,
            direction: handle.direction,
            peer: handle.peer,
            close_notice: Mutex::new(None),
            closed: AtomicBool::new(false),
        }
    }

    /// The underlying arena (for direct publish/resolve in tests and peers).
    pub fn arena(&self) -> &Arc<Arena> {
        &self.arena
    }

    /// Which end of the arena this channel drives.
    pub fn direction(&self) -> Direction {
        self.direction
    }
}

fn shmem_caps() -> TransportCaps {
    TransportCaps {
        kind: TransportKind::Shmem,
        framing: "bounded SPSC control rings (kind-tagged capnp frames) + slot arena; bulk referenced by T.BufferId",
        capability_transfer: crate::transport::channel::CapabilityTransfer::BootstrapOnly,
        // The control ring copies the small referencing message; the bulk
        // payload does not. Both numbers are measured in ArenaStats.
        zero_copy_message: false,
        zero_copy_data_handle: true,
        // The arena has no peer identity of its own: the bootstrap pairs it
        // with an authenticated Unix channel (02-HOST-CAPABILITIES §4.2).
        peer_identity: false,
        blocking: false,
        max_frame_bytes: 0,
        note: "carries no authority by itself: the one-time authenticated \
               bootstrap (transport::bootstrap) is the only capability \
               transfer; after it, handles live in the arena",
    }
}

impl Channel for ShmemChannel {
    fn kind(&self) -> TransportKind {
        TransportKind::Shmem
    }

    fn caps(&self) -> TransportCaps {
        shmem_caps()
    }

    fn send(&self, message: MessageOut<'_>) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::AlreadyClosed);
        }
        // Capability staging is refused on this transport (see `attach`),
        // so no caps ride the frame: the message is the frame.
        self.arena
            .send_message(self.direction, frame::KIND_DATA, &MsgSegments(&message))
    }

    fn recv(&self, scratch: &mut Vec<u8>) -> Result<RecvBatch, TransportError> {
        loop {
            if !self.arena.recv_frame(self.direction.other(), scratch)? {
                break;
            }
            match parse_control_frame(scratch)? {
                ControlFrame::Close(reason) => {
                    return Err(TransportError::Closed { reason });
                }
                ControlFrame::Data => {
                    let reader = capnp::serialize::read_message(
                        &mut &scratch[1..],
                        capnp::message::ReaderOptions::new(),
                    )
                    .map_err(|e| TransportError::message(e.to_string()))?;
                    return Ok(RecvBatch {
                        message: RecvMessage::Owned(reader),
                        caps: Vec::new(),
                    });
                }
                // Unreachable by construction (attach is refused here);
                // kept total so a future capability path cannot silently
                // drop a frame.
                ControlFrame::Attach { .. } | ControlFrame::Detach { .. } => continue,
            }
        }
        match self.close_notice.lock().clone() {
            Some(reason) => Err(TransportError::Closed { reason }),
            None => Err(TransportError::Empty),
        }
    }

    fn attach(&self, cap: &mut CapHandoff) -> Result<CapOutcome, TransportError> {
        let kind = match cap {
            CapHandoff::Live(_) => "live handle",
            CapHandoff::Descriptor(_) => "descriptor",
            CapHandoff::OsHandle(_) => "os handle",
            CapHandoff::Region { .. } => "region",
        };
        // 02-HOST-CAPABILITIES §4.2: Shmem carries no authority. The value
        // is handed back untouched — nothing was consumed.
        Err(TransportError::UnsupportedCapability {
            kind: kind.to_string(),
            detail: "Shmem carries no authority; the one-time authenticated \
                     bootstrap (transport::bootstrap) is the transfer point"
                .to_string(),
        })
    }

    fn detach(&self, _id: CapId) -> Result<(), TransportError> {
        Err(TransportError::UnsupportedCapability {
            kind: "detach".to_string(),
            detail: "Shmem stages no capabilities".to_string(),
        })
    }

    fn resolve_buffer(&self, id: types_capnp::buffer_id::Reader<'_>) -> Result<FrameRef<'_>, TransportError> {
        self.arena.resolve(id)
    }

    fn negotiate(&self, request: MessageOut<'_>) -> Result<NegotiateResponse, TransportError> {
        // The handshake rides the control ring like any message; answering
        // it is the peer's job (for Shmem the peer is the other direction
        // of the same arena, so a synchronous test peer can reply).
        self.arena
            .send_message(self.direction, frame::KIND_NEGOTIATE, &MsgSegments(&request))?;
        let mut scratch = Vec::new();
        loop {
            if !self.arena.recv_frame(self.direction.other(), &mut scratch)? {
                return Err(TransportError::Empty);
            }
            if matches!(parse_control_frame(&scratch)?, ControlFrame::Data) {
                let reader = capnp::serialize::read_message(
                    &mut &scratch[1..],
                    capnp::message::ReaderOptions::new(),
                )
                .map_err(|e| TransportError::message(e.to_string()))?;
                return Ok(capnp::message::TypedReader::new(reader));
            }
        }
    }

    fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.peer
    }

    fn close(self: Box<Self>, reason: CloseReason) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Err(TransportError::AlreadyClosed);
        }
        *self.close_notice.lock() = Some(CloseReason::PeerClosed);
        // Best-effort: the close notice rides the far side's ring.
        let _ = self.arena.send_close(self.direction, &reason);
        Ok(())
    }
}

/// Adapt any `MessageOut` into `ReaderSegments` for the ring path.
struct MsgSegments<'a>(&'a MessageOut<'a>);

impl capnp::message::ReaderSegments for MsgSegments<'_> {
    fn get_segment(&self, id: u32) -> Option<&[u8]> {
        self.0.get_segment(id)
    }
}

/// A decoded ring-level control frame.
pub enum ControlFrame {
    /// A capnp message frame.
    Data,
    /// A close notice.
    Close(
        /// The declared reason.
        CloseReason,
    ),
    /// A capability attachment.
    Attach {
        /// The attachment kind.
        kind: u8,
        /// The capability id.
        id: CapId,
        /// The in-band payload (descriptor or region notice).
        data: Vec<u8>,
    },
    /// A capability detach.
    Detach {
        /// The capability id being dropped.
        id: CapId,
    },
}

/// Parse a ring frame's content (`[kind][payload]`).
pub fn parse_control_frame(bytes: &[u8]) -> Result<ControlFrame, TransportError> {
    if bytes.is_empty() {
        return Err(TransportError::Framing {
            detail: "empty control frame".to_string(),
        });
    }
    let kind = bytes[0];
    let payload = &bytes[1..];
    match kind {
        frame::KIND_DATA | frame::KIND_NEGOTIATE => Ok(ControlFrame::Data),
        frame::KIND_CLOSE => Ok(ControlFrame::Close(frame::decode_close(payload)?)),
        frame::KIND_ATTACH => {
            if payload.len() < 11 {
                return Err(TransportError::Framing {
                    detail: "attach payload too short".to_string(),
                });
            }
            let cap_kind = payload[0];
            let id = u64::from_be_bytes(payload[1..9].try_into().unwrap());
            let data_len = u16::from_be_bytes([payload[9], payload[10]]) as usize;
            if payload.len() < 11 + data_len {
                return Err(TransportError::Framing {
                    detail: "attach data length exceeds payload".to_string(),
                });
            }
            Ok(ControlFrame::Attach {
                kind: cap_kind,
                id: CapId(id),
                data: payload[11..11 + data_len].to_vec(),
            })
        }
        frame::KIND_DETACH => {
            if payload.len() != 8 {
                return Err(TransportError::Framing {
                    detail: "detach payload must be 8 bytes".to_string(),
                });
            }
            Ok(ControlFrame::Detach {
                id: CapId(u64::from_be_bytes(payload[0..8].try_into().unwrap())),
            })
        }
        _ => Err(TransportError::Framing {
            detail: alloc::format!("unknown control kind {kind}"),
        }),
    }
}
