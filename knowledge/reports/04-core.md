# 04 — core: errors, IDs, capability handles, values (Wave 2, Agent D)

Status date: 2026-10-11. Workspace: `/home/lufuno/smart/smart-lib-ng/`.
Exclusive files written: `src/core/{mod,error,ids,capability,values,tests}.rs`, this report, and
`knowledge/lessons/04-core.md`. Nothing outside my ownership was created, edited or deleted.

**Verdict: every checklist box is mechanically verified. `cargo test --lib` is green in both the
default feature configuration and `--no-default-features` (187 tests, 0 failures), and the core
compiles for the bare-metal target `x86_64-unknown-none` (witness crate in `/tmp/kilo/corecheck`,
see §7).**

The wire step the brief anticipated is **already done**: the skeleton agent's `build.rs` now emits
`#[path = ".../src/core/mod.rs"] pub mod core;` in the generated module tree (identical mechanism
to the domain modules), so `crate::core` is live and `src/lib.rs` was never edited by anyone.

## 1. Checklist status

### [DONE] `Error`: enum covering the contract's `T.Error` kinds, `From` both directions, total
The contract's error vocabulary is `T.ErrorCode` (`types.capnp:324-346`) — **20 codes**, read from
the schema, not guessed. Core delivers them as `core::error::ErrorCode` (exactly 20 variants, in
schema order) plus `core::error::Error` (the struct mirroring `T.Error`: `code`, `message`,
`context`, `retryable` — `metadata` deliberately not modelled, see §5). Conversions:

* `ErrorCode → types_capnp::ErrorCode` and back: two `match`es with **no catch-all arm** — a 21st
  contract code breaks the build, which is the intended "build-time total".
* `From<&types_capnp::error::Reader> for Error`, `From<&types_capnp::error::Builder> for Error`
  (the inverse direction), and `Error::write_to(&mut types_capnp::error::Builder)` (the forward
  direction; `From<Error> for T.Error::Builder` is not implementable — a Builder must live in a
  message the caller owns, see §5).
* A `T.Error.code` discriminant **not in the compiled schema** (a peer on a newer contract) maps
  to `ErrorCode::Unknown` with the raw discriminant preserved in `message`
  (`03-CONFORMANCE.md` §5: "the original discriminant in the message", never a default).

Witness (verbatim, `cargo test --lib`):

```
test core::tests::error_code_covers_exactly_the_contracts_20_codes ... ok
test core::tests::error_round_trips_every_variant_through_the_wire ... ok
test core::tests::unknown_error_discriminant_is_preserved_not_defaulted ... ok
```

`error_round_trips_every_variant_through_the_wire` builds a real `T.Error` per code (distinct
message, context, both retryable values), serializes with `capnp::serialize`, reads back, converts
core→wire→core, and then asserts the re-serialization is **byte-identical** (wire→core→wire).

### [DONE] `Error` carries the honest fields, plus the backend-writer constructors
`message: String`, `context: String`, `retryable: bool`, `code: ErrorCode`; `retryable` defaults to
`ErrorCode::default_retryable()` (only `temporary` → true). Constructors: `not_found()`,
`permission_denied()`, `failed(reason)`, `temporary(reason)`, `unavailable(reason)`,
`limit_exceeded(reason)`, `out_of_memory()`, `invalid_argument(reason)`, `timeout()`, `cancelled()`,
`already_exists()`, `not_supported(reason)`, `conflict()`, `unknown(reason)`, `hardware_fault(reason)`,
`driver_error(reason)`, `protocol_violation(reason)`, `bus_error(reason)`, `thermal_throttling()`,
`power_state_failure()` — the two law-#6 codes require a reason by signature. Witnesses:
`constructors_carry_the_codes_and_reasons_backends_need`, `retryable_defaults_are_honest`,
`capnp_runtime_errors_classify_without_inventing_facts` (the `From<capnp::Error>` classification:
Unimplemented→notSupported, Overloaded/Disconnected→temporary+retryable, Failed→failed, the runtime
message preserved verbatim), `result_alias_is_the_crate_wide_type`.

### [DONE] `Result<T>` type alias used crate-wide
`core::error::Result<T> = core::result::Result<T, Error>`; re-exported as `core::Result`.

### [DONE] ID types: newtypes over the contract's ID representation for every `T.*Id`
**157** newtypes — the count is enumerated mechanically from the schema (every struct in
`types.capnp` whose entire body is one integer field), not from memory: **155** `id @0 :UInt64`
+ **1** `handle @0 :UInt64` (`CapabilityHandle`) + **1** `id @0 :UInt32` (`CpuId`). Each: `Copy +
Clone + Debug + Default + PartialEq + Eq + PartialOrd + Ord + Hash`, zero allocation, with
`new/get`, `From<u64>`/`From<u32>`, `From<&generated Reader>`, `From<&generated Builder>`, and
`write_to(&mut generated Builder)`. Macro-generated in `src/core/ids.rs`; the invocation list was
produced by parsing the schema and **verified against the generated `types_capnp` module names**
(assertion-guarded generation, §7). 157 tests, one per type: `<name>_round_trips`.

The 157, in schema order (`*` = the two shape exceptions): NodeId, SessionId, SurfaceId, BufferId,
OutputId, WindowId, WorkspaceId, ClientId, CaptureId, ImeContextId, AccessNodeId, DragSessionId,
InhibitHandle, NotificationId, ClipboardId, EventId, PackageId, ServiceId, JobId, TimerId,
PrintJobId, ProcessId, FileId, SearchProviderId, SettingKey, AppId, BackupId, CrashDumpId,
ProcessGroupId, BreakpointId, BiometricId, AuthMethodId, AuthTokenId, PermissionId, BootStageId,
BootEntryId, DeviceAddress, UpgradeId, ContactId, GroupId, ClipboardSourceId, SensorId,
SpeechVoiceId, TrayIconId, SocketId, CertificateId, AccessId, AccessBackendId, AccessCredentialId,
MfaChallengeId, MfaEnrollmentId, SmartCardId, RuntimeInstanceId, FramebufferId, PlaneId, CrtcId,
AudioStreamId, AudioFocusId, AudioDeviceId, CodecId, CalendarId, ReminderId, AttendeeId, CameraId,
RecordingId, ContentId, ContentSubscriptionId, DatabaseId, NamespaceId, DeviceId, ExtensionId,
LogicalMonitorId, ColorProfileId, DisplayModeId, DnsSubscriptionId, HalDeviceId, DmaChannelId,
HookId, InputMethodId, KeyboardLayoutId, KeyBindingId, BiometricTemplateId, GestureId, IntentId,
ActivityId, ServiceHandleId, ServiceName, ManifestName, SchemaId, MediaPlayerId, ModuleId,
NetworkInterfaceId, VpnProfileId, VpnConnectionId, NfcTagId, NfcServiceId, NotificationActionId,
PowerProfileId, PrinterId, UserId, ThreadId, QueueId, QueueMessageId, CapabilityHandle\*, FontId,
FileHandleId, TagId, SubsystemId, MappedRegionId, LayerId, EmailEntryId, PhoneEntryId,
AddressEntryId, SoundId, CallId, ConferenceId, TelephonyMessageId, ThemeId, SecretNamespaceId,
SecretKeyId, MediaItemId, LocationSourceId, CapabilityId, FenceHandle, MmsPartId, TaskGroupId,
SeatId, TaskName, ComponentName, TunnelId, AlarmId, TaskId, ActionId, WindowGroupId, SnapZoneId,
IntentChainId, IntentTraceId, IntentHistoryId, KeyId, EndpointId, MountId, FilesystemId, SandboxId,
StoragePoolId, VolumeId, SnapshotId, SubnetId, PolicyId, FirmwareId, ChannelId, RuleId, TemplateId,
ScopeId, DeviceGroupId, SearchBookmarkId, CpuId\*, DriverId.

### [DONE] Capability handle `Cap<T>` — zero-cost, authority-is-a-handle, acquisition hook
See §4. Witnesses: `cap_is_zero_cost_around_the_client` (size equality with the wrapped client),
`cap_clone_shares_the_underlying_authority`, `cap_revocation_reaches_the_holder_at_the_next_call`,
`acquire_hook_signature_is_implementable_without_host_logic`.

### [DONE] Value types — only what domains actually reference
Measured tree-wide reference counts decide the set (§6 table): `Vec2, Vec2i, Vec3, Vec4,
Quaternion, SpatialPose, Rect, Point, Point3D, Size, CornerRadius, FrameRate, Region, Version,
Duration, Timestamp, MonotonicTimestamp` — 17 wrappers, each with `from_reader`/`write_to` + both
`From` directions, and a wire round-trip test. Types with zero measured references (`Insets`,
`Transform2D`, `Rect3D`) are deliberately absent; `T.DataHandle`/`T.BufferId` are transport-wave
concerns (`BufferId` exists as an ID).

### [DONE] `no_std` + `alloc` clean; `#[must_use]`; no `std::` anywhere
`src/core/**` contains no `std::` path. `#[must_use]` on `Cap<T>` and every `Error` constructor.
Bare-metal proof (§7).

### [DONE] Tests: error totality both ways, ID conversions, capability handle + revoke propagation
187 tests, 0 failures (§7). Revocation is proven with a mock capability at capnp's `ClientHook`
seam: a live call resolves, the backing capability is revoked, the **same held handle** fails the
next call with `ErrorKind::Disconnected`, and the error maps into the model as
`temporary`+retryable. The invariant is also stated loudly in the module docs (§4).

### [DONE] Five-line error-model doc at the top of `src/core/mod.rs`
`src/core/mod.rs:14-24` — numbered 1–5, addressed to domain implementers.

## 2. The Error variant table (20 ↔ 20, total both directions)

| # | `core::ErrorCode` | `T.ErrorCode` (types.capnp:324) | default retryable |
|---|---|---|---|
| 0 | `NotFound` | `notFound @0` | false |
| 1 | `PermissionDenied` | `permissionDenied @1` | false |
| 2 | `Failed` | `failed @2` | false |
| 3 | `Temporary` | `temporary @3` | **true** |
| 4 | `Unavailable` | `unavailable @4` | false |
| 5 | `LimitExceeded` | `limitExceeded @5` | false |
| 6 | `OutOfMemory` | `outOfMemory @6` | false |
| 7 | `InvalidArgument` | `invalidArgument @7` | false |
| 8 | `Timeout` | `timeout @8` | false |
| 9 | `Cancelled` | `cancelled @9` | false |
| 10 | `AlreadyExists` | `alreadyExists @10` | false |
| 11 | `NotSupported` | `notSupported @11` | false |
| 12 | `Conflict` | `conflict @12` | false |
| 13 | `Unknown` | `unknown @13` | false |
| 14 | `HardwareFault` | `hardwareFault @14` | false |
| 15 | `DriverError` | `driverError @15` | false |
| 16 | `ProtocolViolation` | `protocolViolation @16` | false |
| 17 | `BusError` | `busError @17` | false |
| 18 | `ThermalThrottling` | `thermalThrottling @18` | false |
| 19 | `PowerStateFailure` | `powerStateFailure @19` | false |

Plus the out-of-schema case: a `T.Error.code` discriminant ≥ 20 (newer peer) → `Unknown` with the
raw value in `message` (test: `unknown_error_discriminant_is_preserved_not_defaulted`, which
patches a serialized message to discriminant 42 and asserts the 42 survives and no in-schema code
is guessed).

## 3. Cap<T> design and revocation semantics

```rust
pub trait Interface: capnp::capability::FromClientHook + Clone + 'static {}   // blanket impl
pub struct Cap<T: Interface>(T);          // Deref<Target = T>, Clone, From<T>, client(), into_client()
pub trait Acquire<T: Interface> {          // the host-mapping hole platform backends fill
    fn acquire(&self, presented: &capability_capnp::capability::Reader<'_>, target: &str) -> Result<Cap<T>>;
}
```

* **Zero cost**: `Cap<T>` is `size_of::<T>()` (asserted in a test), derefs to the generated client,
  and adds no state — no validity bit, no timestamp, no cache. It is documentation and types
  around the reference, never a second copy of the authority.
* **Holding it IS the authority** (law #5, `capability.capnp:20-27`): no permission string exists
  anywhere in the crate; there is nothing to present on a call.
* **Acquisition is the only OS boundary**: `Acquire::acquire(presented, target)` is deliberately
  host-agnostic — peer identity (`SO_PEERCRED`, shmem pairing token, nothing on InProcess, absent
  on Tcp) is the platform module's own per-session state, never a parameter. The five rules an
  implementation must honour (match → mint; cannot-evaluate → non-permissionDenied error; refuse →
  `permission_denied` with reason; never mint unmeasured authority; never derive handles from uid)
  are in the trait docs, citing `02-HOST-CAPABILITIES.md` §3. The contract's acquisition points:
  `Files.acquireFile @73`, `Network.nicProvider @71`, `Audio.openDriver @99`, plus the host-injected
  root references.
* **Revocation reaches the holder at the next call.** The capnp capability system routes calls on a
  revoked capability to an error (`ErrorKind::Disconnected` in the runtime's vocabulary); because
  `Cap<T>` caches nothing, it cannot go stale — the call is the check. The contract's push channel
  is `CapabilityService.subscribe` → `CapabilitySubscription` → `CapabilityEvent.revocationNotice`
  (`capability.capnp:313-348`); a backend holding long-lived handles should subscribe and drop them
  on notice. A bounded revocation cache is allowed only with declared staleness
  (`02-HOST-CAPABILITIES.md` §3.1). The test proves the mechanism with a mock hook whose revocation
  flag lives behind the client.
* **Clone duplicates authority** (one refcount bump on the hook; `Constraints.shareable` defaults
  true) — proven in `cap_clone_shares_the_underlying_authority` (one revocation seen through both
  handles).

## 4. What domain and platform agents must know

1. Import surface: `use smart_lib::core::{Error, ErrorCode, Result};` plus
   `smart_lib::core::ids::*` and `smart_lib::core::values::*`. The prelude still only re-exports
   the foundation — ask the skeleton agent (or your wire agent) if you want `core::*` added there;
   I do not own `src/prelude.rs`.
2. **Answering honestly is one line**: `Err(Error::unavailable("subsystem not implemented on this
   backend"))` or `Err(Error::not_supported("not-applicable: <reason>"))`. A permanent N/A is
   `notSupported`; a temporary gap is `unavailable`/`temporary`.
3. Never fabricate: a zero, a default or an invented string standing in for "the host could not
   tell me" is the one unforgivable defect (`03-CONFORMANCE.md` §4.4). If the fact is unknown, the
   answer is `Unavailable`+reason or the contract's `*Valid: false` pattern.
4. IDs are opaque: wrap what the backend minted, `write_to` into the generated struct, never
   reinterpret. `CapabilityHandle` in particular is minted per acquisition and never derived from a
   uid.
5. `Cap<T>` wraps the generated interface client (`files_capnp::files::Client`, …); reach methods
   through `Deref` (`cap.read_file_request()`). To implement `Acquire`, pattern-match the presented
   `capability::Reader` (`id`, `target`, `operations`, `constraints`, `expiry`, `chain`) — never a
   permission string.
6. Errors crossing a transport: `Error::write_to` into the `T.Error` field of the result union; on
   receipt, `Error::from(&reader)`. Both are total; do not hand-map codes.
7. Value wrappers are `Copy`/`Clone` and allocation-free except `Region` (a `Vec<Rect>`) and
   `Version`'s two text fields. Conversions are named `from_reader`/`write_to` uniformly.

## 5. Deviations from the brief (and why)

1. **The brief asks for "`Error`: an enum covering the contract's `T.Error` kinds".** `T.Error` is
   itself a *struct* on the wire (`code` + `message` + `context` + `retryable` + `metadata`), so
   core delivers `ErrorCode` (the 20-variant enum — exactly the contract's kind set, exhaustively
   1:1) and `Error` (the struct — exactly the contract's fields). This is what makes the mapping
   total *and lossless*: the old implementation's payload enum (`SmartError::Failed(String)`,
   `Unknown(String)`, `smart-protocol/crates/smart-lib/src/types.rs:12-33`) forced a message onto
   codes that need none, dropped `context` entirely, and carried two codes (`Io`, `MissingTool`)
   the contract does not have. `ErrorCode` is the enum the checklist means; `Error` is the carrier
   the wire demands.
2. **`From<Error> for <generated T.Error reader/builder>`** is not literally implementable for a
   Builder (a capnp builder must live inside a message the caller owns) or a Reader (it borrows a
   message). The forward direction is `Error::write_to(&mut T.Error::Builder)`; the inverse is
   `From<&T.Error::Reader>` / `From<&T.Error::Builder>`. The totality test covers both directions
   through real serialized messages and asserts byte-identical re-serialization, which is the
   property the checklist is protecting.
3. **`T.Error.metadata` is not modelled in core.** The five other fields round-trip exactly;
   `metadata` (a `StringMap`) is backend bookkeeping the generated type still carries, and core
   stays additive — a backend that needs it uses the generated type directly. Same call, stated
   explicitly, for `T.Version.metadata`.
4. **`text::Reader::to_string()` returns `Result<String, Utf8Error>`** in capnp 0.27.2 (not
   `String`), so a malformed `Text` field reads as empty rather than panicking or fabricating; the
   helper `core::error::text_or_empty` documents that a non-UTF-8 `Text` is a peer protocol
   violation that message validation should have caught.

## 6. Value types — the measured set

| Wrapper | Contract type | Tree-wide refs | Pilot-4 refs |
|---|---|---|---|
| `Vec2` | `T.Vec2` | 56 | 0 |
| `Vec2i` | `T.Vec2i` | 30 | 0 |
| `Vec3` | `T.Vec3` | 6 | 0 |
| `Vec4` | `T.Vec4` | 1 | 0 |
| `Quaternion` | `T.Quaternion` | (via `T.SpatialPose`) | 0 |
| `SpatialPose` | `T.SpatialPose` | 2 | 0 |
| `Rect` | `T.Rect` | 26 | 0 |
| `Point` | `T.Point` | 5 | 0 |
| `Point3D` | `T.Point3D` | 1 | 0 |
| `Size` | `T.Size` | 10 | 0 |
| `CornerRadius` | `T.CornerRadius` | 6 | 0 |
| `FrameRate` | `T.FrameRate` | 4 | 0 |
| `Region` | `T.Region` | 3 | 0 |
| `Version` | `T.Version` | 104 | 5 |
| `Duration` | `T.Duration` | 416 | 35 |
| `Timestamp` | `T.Timestamp` | 625 | 44 |
| `MonotonicTimestamp` | `T.MonotonicTimestamp` | 16 | 2 |

(Counted by `grep` over all 89 schemas, 2026-10-11. Geometry types are referenced by non-pilot
domains — compositor, display, windowmanager, location, input — so they belong in core now rather
than being re-invented per domain later.)

## 7. Evidence (verbatim)

**Unit tests, default features** (`cargo test -j 6 --lib`, 7 schemas compiled):

```
test core::tests::acquire_hook_signature_is_implementable_without_host_logic ... ok
test core::tests::cap_clone_shares_the_underlying_authority ... ok
test core::tests::cap_is_zero_cost_around_the_client ... ok
test core::tests::cap_revocation_reaches_the_holder_at_the_next_call ... ok
test core::tests::capnp_runtime_errors_classify_without_inventing_facts ... ok
test core::tests::constructors_carry_the_codes_and_reasons_backends_need ... ok
test core::tests::error_code_covers_exactly_the_contracts_20_codes ... ok
test core::tests::error_round_trips_every_variant_through_the_wire ... ok
test core::tests::result_alias_is_the_crate_wide_type ... ok
test core::tests::retryable_defaults_are_honest ... ok
test core::tests::unknown_error_discriminant_is_preserved_not_defaulted ... ok
test core::tests::version_displays_like_the_contract_names_it ... ok
test core::tests::node_id_round_trips ... ok            (… 157 ID witnesses, one per type …)
test core::tests::vec2_round_trips ... ok               (… 19 value witnesses …)

test result: ok. 187 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Composition: 7 error-model + 157 ID + 19 value + 4 capability = **187**.

**Unit tests, foundation only** (`cargo test -j 6 --lib --no-default-features`, 3 schemas
compiled — the core must stand with zero domain features):

```
warning: smart-lib@0.1.0: smart-lib: compiled 3 schema file(s) for features [<none>]
test result: ok. 187 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

**Bare metal (law #1).** The Wave-3 transport agent's in-flight files transiently break the
whole-lib `--target x86_64-unknown-none` build, so the core was proven independently: a scratch
crate (`/tmp/kilo/corecheck`, outside this workspace) that mirrors the skeleton's module tree from
the *already generated* foundation modules and includes `src/core/mod.rs` by absolute `#[path]`:

```
$ cargo build -j 6 --target x86_64-unknown-none
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.44s
```

The same scratch crate also re-ran the 187 tests green while the workspace tree was mid-refactor
(its only failure is the doctest, which resolves `smart_lib` — the scratch crate is named
`corecheck`; the doctest is correct for the real crate name and its assertions are duplicated as
unit tests).

**Library build is warning-clean from my files**: `cargo build -j 6` emits only the skeleton
agent's informational lines; zero warnings originate in `src/core/**`.

**Mechanical generation checks.** The 157-invocation ID list and the 157-invocation test list were
generated by parsing `types.capnp` (single-integer-field structs), and the generation asserts
every derived module name exists in the generated `types_capnp.rs` before emitting — a schema
rename fails generation, not compilation.

## 8. Blockers / requests to the orchestrator

1. **No blocker for my wave.** Two concurrency notes, neither mine to fix: (a) during my work the
   Wave-3 `src/transport/**` tree was repeatedly mid-edit and transiently broke the whole-lib test
   compile — every error I observed was in their files (verified by attributing each error's
   `-->` path), none in `src/core/**`; (b) `tests/codegen.rs` (skeleton-owned) is currently broken
   by the skeleton's own `render_module_tree` signature change, which also blocks a combined
   `cargo test` (all targets) run right now. The core's own 187 tests pass in both feature
   configurations regardless.
2. **Optional wiring request**: `src/prelude.rs` (skeleton-owned) could add
   `pub use crate::core::{Error, ErrorCode, Result};` for downstream ergonomics. I did not touch it.
3. **Advisory for Wave-3/4**: the `ClientHook` seam (`capnp::private::capability::*`) is what a
   mock capability needs; capnp's runtime provides no local server→client bridge without
   `capnp-rpc` (the generated `Server` traits are RPITIT with `Rc<Self>` receivers), so test
   doubles for capabilities should be built at the hook seam as `src/core/tests.rs` demonstrates.
