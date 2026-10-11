# 03 — CONFORMANCE: the mechanical meaning of "protocol-compliant"

Wave 1, Agent C. This document is the model every later wave is verified against.
It defines what a backend must do, how the tests that check it are *generated from
the contract*, and what a backend is allowed to say "I cannot" about.

`scripts/validate.py` checks the **schemas** (E1–E14, currently green: 89 schemas
parsed, exit 0, E1/E2/E4/E6/E9/E12/E13/E14 at zero, E7=715 and E8=267 REPORT
residuals enumerated in `audit-revamp/INTEGRATION-REPORT.md`). This document checks
**implementers**. The two are stacked: L0 cannot be attempted while the gate is red.

Scope: charter law 6 is *honest degradation*, and this model is its enforcement. A
fabricated value is not a bug in a backend; it is a bug in the system, because the
consumer has no way to tell a measurement from a guess. Everything below exists to
make that distinction checkable by a machine.

---

## 1. The four levels

Each level is a binary gate with a mechanical pass criterion. A backend advertises
the highest level it holds; nothing higher is claimed.

### L0 — schema-conformant

> The generated surface is exactly the contract's surface, for exactly the enabled features.

Pass criteria (all mechanical):

1. `python3 scripts/validate.py` exits 0 (the schema gate is green).
2. `capnp compile` over the exact feature-gated schema set succeeds — `build.rs`
   already does this, and writes `OUT_DIR/triad.txt` (binary 1.4.0 / capnpc 0.27.0 /
   capnp runtime 0.27.2).
3. **Determinism + identity.** The build is run twice; `OUT_DIR` regenerates
   byte-identically (a nondeterministic generator makes every later level
   unverifiable). And every schema file the build compiled is recorded — the triad
   file and the generated module header already name them — so the compiled set can be
   diffed against the *contract's* `ls schema/` to prove the crate compiled the
   contract it claims, not a stale copy.
4. **Feature-law proof** (charter law 2): disable a feature, rebuild, and `target/`
   must contain no module for it. `build.rs:343` already emits the warning for a
   feature with no hand-written module; L0 turns the codegen half into an assertion:
   the schema module tree in `OUT_DIR/smart_lib_modules.rs` is read and must list
   exactly the enabled domains — never all 89, and never a domain whose feature is off.

Why L0 is first: every later level tests methods that exist. A surface that does not
compile cannot be tested, and a surface that compiles the wrong schema silently
tests the wrong contract.

### L1 — API-conformant

> The public API is **total** over the domain's methods; error mapping is **total**;
> capability gating is **exactly** where the schema puts it.

Three assertions, each generated from the compiler's method inventory:

1. **Method totality.** For every method the compiler reports on the domain's
   interfaces, the generated conformance harness has a callable entry point. A method
   with no harness entry is a FAIL — not a skip. Rationale: 3,143 methods across 303
   interfaces, so a missing method is invisible without a machine.
2. **`T.Error` mapping totality.** For every `ErrorCode` variant the schema declares
   (20 in `types.capnp:324`), the backend has a mapping into its own error type, and
   every one of its own error variants maps back to exactly one `ErrorCode`. The
   mapping is total in both directions; `unmapped` is not a variant.
3. **Capability gating parity.** For every method whose parameter list carries a
   `Capability` (40 tree-wide), the generated harness requires a capability from the
   caller. For every method that does not, the harness must be able to call it
   without one. A gate in the wrong place is as much a defect as a missing gate —
   `ai.capnp` is the precedent: discovery ungated, operations gated once at
   acquisition, and nothing between.

L1 is a **compile-time plus reflection** level: the generated harness is compiled
against the crate, so a deleted or renamed method fails to build. That is the point:
L1 cannot pass by ignoring a method.

### L2 — behavior-conformant

> Generated behaviour tests per method, run against **any** backend implementing the
> domain trait — the mock backend and the Linux backend prove themselves with the
> same suite.

Per-method, the harness auto-generates the cases the method's **shape** implies
(rule table in §2.4). Generated case taxonomy:

| Case | Triggered by | Asserts |
|---|---|---|
| `zero-arg query` | no parameters | succeeds or returns a total `T.Error`; never a zeroed struct |
| `param'd query` | ≥1 scalar/Text param | a spec-shaped request is accepted; counters flow |
| `mutation round-trip` | a param that is also writable on the same domain | write → read → the read equals the write, or a total error |
| `error injection` | result is a union with a `T.Error` arm (495 methods) | a mock backend forcing **each** arm is answered with that arm, `retryable` honest, `message` non-empty |
| `unbounded-list pagination` | `T.Pagination` param (301 methods) | `limit` is honoured; `offset`+`limit` never exceeds `total`; a limit of 0 means "one", not "all" |
| `bounded pull` | returns `List` (567 methods) or a stream | the caller drives the bound; the producer never outruns it |
| `DataHandle zero-copy` | param or result typed `T.DataHandle` (56 in / 29 out) | bulk never appears inline as a copy; the inline arm is used only below the contract's threshold |
| `capability gating` | a `Capability` param (40 methods) | ungated call is refused; gated call proceeds |

**Mock backend is a first-class conformance citizen.** The suite is generic over the
domain trait, so `MockFilesBackend` (a scripted backend that can force every
`T.Error` variant) and `LinuxFilesBackend` are the same `#[test]` function. The mock
proves L2 for the whole contract; the real backend proves L3 for the subset it claims.

**L2 pass = 100% of generated cases for every method in the domain, zero skips.**
One skip turns into a permanent green failure three waves later.

### L3 — platform-conformant

> The per-OS backend passes the **L2** suite for every method it claims `REAL`, and
> declares every other method `UNAVAILABLE` with a nameable reason class.

1. The availability matrix (§3) is emitted as data, and every `REAL` row has a
   corresponding passing L2 test id recorded in the row. **A `REAL` claim without a
   passing test is a FAIL** — this is the only direction that matters, because it is
   the only one that can fabricate.
2. Every non-`REAL` row is answered at runtime by the method returning
   `T.Error { code: notSupported, retryable: false, message: "<reasonClass>: <what is missing>" }`
   — never a panic, never a zeroed success struct, never `Ok` with invented fields.
3. `cli.capnp` registry law: the CLI's `listUnavailable` must enumerate exactly the
   rows that are not `REAL`, and `unavailableReason` must be `available` for nothing in
   that list — a command that cannot run must never be described as available. The
   reason class must be nameable; `unknown` is the one value `cli.capnp:188-204` keeps
   so a gap is distinguishable from every real case, so it is permitted here only in
   the flagged state §3.4 defines.
4. `caps()` is published and checked against the matrix's `caps` list; `partial` and
   `note` carry a declared-N/A honestly (see §3).

L3 is per-platform and additive: a new OS passes L3 for the methods it can do and
`UNAVAILABLE` for the rest, and that is a **conformant backend**, not a partial one.

---

## 2. Test generation from the contract

### 2.1 Verdict: **capnp introspection, not schema-parse**

I evaluated both and built a working proof of the introspection path. **The harness
is generated from the Cap'n Proto `CodeGeneratorRequest` — the same artifact `build.rs`
already feeds `capnpc`.**

Three verified reasons, in order of weight:

1. **The code under test is generated from the CodeGeneratorRequest.** `build.rs:212`
   runs `capnpc::CompilerCommand`, which drives the system binary (`capnp compile -o-`),
   parses the request it writes to stdout, and emits Rust from that. A conformance
    generator that instead parsed schema *source text* would be reading a different
    artifact from the one that produced the surface under test — so L1/L2 would be
    verifying a surface the crate never had. That is a silent-drift failure mode, and
    it is invisible to review because both readers agree on the happy path.
2. **The mechanism already exists in the dependency tree — zero new tooling.**
   `capnp` 0.27.2 is pinned at `Cargo.toml:57`; that crate ships `schema_capnp` (the
   generated Rust for the compiler's own `schema.capnp`) and `schema::StructSchema`,
   `schema::Field`, `schema::FieldSubset::get_union_fields`, `schema::EnumSchema`,
   and `introspect::Type::which() -> TypeVariant { Void, Bool, …, Text, Data,
   Struct(RawBrandedStructSchema), Capability, Enum(RawEnumSchema), List(Type) }`.
   Every shape decision below reads those, in-process, with the pinned dependency.
   Cross-file types resolve through the compiler's own import closure — which is
   exactly the L1 information (a method's param is `types.DataHandle`).
3. **A text parser of schema source is a second parser of the same source, and it
   can disagree with the compiler.** `validate.py`'s own header records its grep-based
   predecessor passing almost everything because it "cannot see inline enums,
   comments, or a `custom` variant that is not last". We are not repeating that
   mistake one layer up.

I proved the pipeline works with the installed binary (1.4.0), on one schema and on
all 89:

```bash
# the request build.rs already feeds to capnpc
capnp compile -o - schema/files.capnp --no-standard-import -I schema/  > cgr.bin   # 163,520 bytes
# decode it with capnp's own schema, no third-party binding needed
capnp convert binary:text <capnp-0.27.2>/schema.capnp CodeGeneratorRequest < cgr.bin
```

(Verified: exit 0. `capnp convert binary:json` crashes on `defaultValue`'s AnyPointer
at `capnp/compat/json.c++:363` — a capnp 1.4.0 limitation, not a blocker; the text
format is deterministic and parseable, and an in-process Rust generator needs no
decoder at all because `schema_capnp` gives typed readers.)

### 2.2 What the compiler hands us, exactly

For `files.capnp`: the request contains **241 nodes in total** — the domain's own
**209** (195 structs, 10 enums, 4 interfaces) plus **32 more from the import closure**
(`types.capnp`, `capability.capnp`): 222 structs, 12 enums and 4 interfaces across
both sets. Filtering to nodes whose display name starts `schema/files.capnp:` yields
the domain's own surface, and this filter matters, because the request carries the
*transitive closure* — an unfiltered reader counts `KernelService`, `FrameStream` and
`ManifestSubscription` as belonging to `boot`/`camera`/`init`. Tree-wide:

| Measured | Count |
|---|---|
| schemas (domains) | 89 |
| interfaces | **303** (matches `rg -c "^interface "` exactly, 0 mismatches) |
| methods | **3,143** |
| enums | 461 |
| methods taking `T.Pagination` | 301 |
| methods returning `List` | 567 |
| methods returning a `T.Error`-carrying result union | 495 |
| methods taking a `Capability` | **40** |
| methods taking `T.DataHandle` | 56 |
| methods returning `T.DataHandle` | 29 |
| methods with no parameters | 615 + 51 (no-param result-returning) |
| methods with no shape-specific case | 1,471 (smoke only) |

`README.md:65` claims 303 interfaces — this measurement confirms it. The `Files`
interface alone is **74 methods** (`@0`–`@73`), 83 including its three sub-interfaces.

### 2.3 Worked example: `Files.readFile` and `Files.statFile`

The compiler's inventory (real output from the pipeline above):

```
Files.readFile   @2   params:  (handle :files.FileHandle, offset :UInt64, size :UInt32)
                      results: (data :types.DataHandle)

Files.statFile   @7   params:  (path :Text)
                      results: (result :files.FileInfoResult)

Files.listDirectory @5 params:  (path :Text, filter :files.DirectoryFilter)
                       results: (entries :List(DirectoryEntry), total :UInt64)
```

`FileInfoResult` expands (from the same request) to the union
`{ info @0 :FileInfo; error @1 :T.Error; unknown @2 :T.UnknownEvent }`, and `FileInfo`
carries `contentHash @7 :T.HashOutput` and `metadata @8 :T.StringMap`.

**Generated test cases for `Files.readFile` (@2)** — four cases, all generated:

| # | Case | Assertion |
|---|---|---|
| 1 | **DataHandle result (zero-copy out)** | The result is `T.DataHandle`. A read at or below the inline threshold may be the `inline` arm; a larger read must be `buffer`. The harness asserts the *bytes* round-trip and that a 4 MiB read never arrives as an inline `Data` arm — an inline 4 MiB is a copy, and a copy violates doctrine rule 4 ("zero-cost is a rule, not a slogan"). |
| 2 | **Bounded read, honest at EOF** | `size` is the caller's bound. A read of `size = 4096` at an `offset` past EOF returns either an empty `DataHandle` or `T.Error{notFound}` — never a short read silently padded, never a length exceeding `size`, and never bytes invented to fill the requested window. |
| 3 | **Error injection through the handle** | A mock backend whose `openFile` handed back a handle the backend then closed must make `readFile` answer `T.Error{invalidArgument, retryable:false}` with a `message` that names the handle as closed. `T.Error.retryable` is asserted against the *injected* fault class: `temporary` → `retryable:true`; `invalidArgument` / `notFound` / `notSupported` → `retryable:false`. |
| 4 | **Ordinal stability (regression lock)** | The test is emitted for `@2`. If a future contract adds a method at `@2` or reorders, the emitted test no longer compiles and L0/L1 fail loudly. This is the cheapest possible guard on the one rule that keeps the wire format eternal. |

**Generated test cases for `Files.statFile` (@7)** — three cases:

| # | Case | Assertion |
|---|---|---|
| 1 | **Error injection over the result union's arms** | The mock backend is scripted to produce each of `info`, `error`, `unknown` in turn. The harness asserts the caller can distinguish all three *and* that the `error` arm carries a `T.Error` whose `code` is the one the mock was asked to inject — a backend that flattens all three into "empty `FileInfo`" fails. This is the single most important generated case, because `FileInfoResult` exists precisely so a missing file is an error rather than a zeroed struct (`files.capnp:29`). |
| 2 | **Honesty audit on `FileInfo`** | Applied automatically because `FileInfo` is a describing struct (see §4): every reported field must be measured or accompanied by a declared-N/A. `contentHash` is the sharp case — a backend that cannot hash must leave `HashOutput.bytes` empty and say so in `metadata`, not compute a placeholder. `T.HashOutput` has no `*Valid` arm, so the rule falls through to "empty `Data` + a `note`-bearing metadata entry" for this field. |
| 3 | **Zero-value is not a measurement** | A path the backend cannot stat yields `T.Error`, so `size = 0` on a *successful* stat must mean a genuinely empty file. The harness generates a round-trip: create a known 0-byte file, stat it, assert `size == 0` **and** that the same backend reports `size == 0` never for a file the harness wrote 7 bytes into. A backend that fabricates `0` for an unmeasurable size fails case 2 and passes only by also fabricating case 3's input — the two together are what make the fabrication detectable. |

`Files.listDirectory` (@5) additionally generates the **pagination case**: a
`T.Pagination` with `limit=0` must be read as "one entry", never "everything";
`offset+limit` may never exceed `total`; and `total` must be stable across two
identical calls (a `total` that changes while the directory does not is a fabricated
count).

### 2.4 The shape → case rule table

One rule per shape, applied uniformly. Every rule is derived from the compiler's
`TypeVariant`, so no rule is a hand-maintained list:

| Compiler signal | Generated case | Count tree-wide |
|---|---|---|
| `params == []` | `zero-arg query` — plus the result's own case if it carries a union or a `DataHandle` | 615 pure / 51 with result cases |
| a param whose type is a result-union struct carrying a `T.Error` arm | `error injection` — one sub-case per union arm | 495 |
| a param typed `T.Pagination` (or a field of that type) | `pagination/cursor` | 301 |
| a param or result typed `T.DataHandle` | `DataHandle zero-copy` | 56 in / 29 out |
| any param typed `Capability` | `capability gating` | 40 |
| any result typed `List(_)` | `bounded pull` | 567 |
| any result typed `Capability` (an accessor returning a service) | `accessor gating` + the sub-interface's own suite | — |
| a struct containing a capital-`Valid` `Bool` field | `honesty audit` (§4) | 9 |
| a union with an `unknown` / `custom` arm | `forward-compat` (§5) | — |
| anything else | `smoke only` — one assert: it does not panic, and it answers a total result | 1,471 |

The table is exhaustive by construction: every method matches at least one row, and
the last row catches the rest. That is what stops the suite from silently shrinking
as the contract grows.

---

## 3. The availability matrix

### 3.1 Where it lives

**One row per (domain × platform), carrying a per-method status map.** Emitted as a
machine-readable file per backend cell, and checked in:

```
src/platforms/<os>/<domain>.rs      # the backend (that cell's agent owns it)
knowledge/availability/<os>/<domain>.toml   # the claim, generated then committed
```

Rationale for two files rather than one: the matrix is *data* `smart doctor` prints
and CI verifies, so it must be diffable text; the backend is *code*. Generating the
TOML from a call to the backend's `caps()` and freezing it means CI can diff the
frozen claim against a live re-generation and catch a backend whose published matrix
drifted from what it actually does. (Ownership note: `knowledge/availability/**` is
not in the charter's table — the orchestrator should assign it to me or to the
platform cell agents. I have not created it.)

A row is invalid if it claims a status the tests cannot back.

### 3.2 The schema

```toml
# one row per (domain, platform); the per-method map is law-3 granularity,
# because "the backend implements what its platform can do" is per-method.
[[row]]
domain        = "files"                    # schema stem, matches the cargo feature
platform      = "linux"                    # open token: linux, macos, windows, ...
arch          = "any"                      # open token; not a closed enum
contractVersion = "files.capnp@0xa1b2c3d4e5f60025"   # schema id, no drift possible
transports    = ["InProcess", "Shmem", "UnixSocket"]  # supported, not merely linked
caps          = ["files.read", "files.write", "files.notify"]   # open tokens
capsPartial   = false                       # system.capnp Capabilities.partial
capsNote      = ""                          # why partial; never a placeholder

[[row.method]]
name = "readFile"
status = "REAL"                             # REAL | DEGRADED | UNAVAILABLE
l3Test = "l3_linux_files::readFile"         # required exactly when status != UNAVAILABLE
detail = ""                                 # which capability is degraded; else empty

[[row.method]]
name = "lockFile"
status = "UNAVAILABLE"
reasonClass = "not-applicable"          # one of: no-backend | no-privilege |
                                         # not-applicable | disabled-by-policy | unknown
reason = "no advisory-lock primitive on this filesystem; advisory locks unused by design"
l3Test = ""
```

### 3.3 Field list (the answer to "what does a row carry")

`domain`, `platform`, `arch`, `contractVersion`, `transports`, `caps`, `capsPartial`,
`capsNote`, and per method: `name`, `status`, `reasonClass`, `reason`, `l3Test`, `detail`.

### 3.4 Status and reason definitions, exactly

- **`REAL`** — the method passes its full generated L2 suite on this platform. Requires
  an `l3Test` id that passed. No other evidence is accepted.
- **`DEGRADED`** — the method answers, but not completely. Mechanically, one or more
  of: (a) it answers `T.Error{code: temporary}` for some inputs the contract allows;
  (b) it answers with at least one `*Valid = false` honesty flag; (c) its `caps`
  reports `partial = true`. A `DEGRADED` row **must** name the degraded capability in
  `detail` — "degraded" without a named capability is a fabricated claim.
- **`UNAVAILABLE`** — the method answers `T.Error{code: notSupported, retryable:false}`
  and says why. `reasonClass` is one of:
  - `no-backend` — no implementation exists for this (domain, platform).
  - `no-privilege` — the host primitive exists but the caller's credentials do not
    reach it (maps to `cli.capnp AvailabilityReason.capabilityDenied`).
  - `not-applicable` — the concept does not exist on this platform (maps to
    `AvailabilityReason.unsupportedPlatform`). A fold-state sensor on a desktop display.
  - `disabled-by-policy` — exists, held back deliberately (maps to `disabledByPolicy`).
  - `unknown` — **reserved and flagged**. `cli.capnp:200` keeps `unknown` explicitly so
    a gap is distinguishable from every real case. A row that uses it during CI is a
    yellow build: it says "we do not know", which is honest, but it must not become a
    hiding place. Any `unknown` row in a release build is a defect.

Rationale for reusing `cli.capnp`'s `AvailabilityReason` rather than inventing a
vocabulary: a second reason grammar is the anti-pattern the revamp deleted everywhere
else (per-method capability tokens, authority-as-text). The matrix maps 1:1 onto the
contract's own enum, so `smart doctor` and `CliService.listUnavailable` can never
disagree.

### 3.5 Verification (CI's half of the contract)

1. Every `REAL`/`DEGRADED` row's `l3Test` ran and passed. Missing id or failing test
   ⇒ **FAIL**, and specifically a *fabrication* flag, not a test failure.
2. Every `UNAVAILABLE` row's live behaviour is asserted: calling it returns
   `notSupported` with a `message` whose reason class matches the row. A backend that
   claims `UNAVAILABLE` but answers `ok` is lying in the other direction, and is
   caught here.
3. `caps()` (the live backend publication) matches the row's `caps` list exactly. A
   capability the backend publishes but does not hold is as dishonest as a `REAL`
   claim it cannot back.
4. The frozen matrix re-generates identically from the live backend's `caps()` call. A
   diff is a FAIL — that is the drift check, and it is the whole reason the matrix is a
   checked-in file rather than a printed value.
5. `contractVersion` matches the compiled schema's file ID, read from the same
   CodeGeneratorRequest — so the matrix can never describe a contract the build did
   not compile.

---

## 4. The honesty audit

The audit catches a fabricated value: a `0`, an empty list, an invented string, or a
defaulted enum standing in for "the host could not tell me".

### 4.1 The pattern, generalised from `display.capnp`

`display.capnp:290-311` states the rule in prose — *physical size, refresh rate and
EDID come from hardware that frequently does not exist. So every such field is paired
with an explicit validity flag, and the meaning of "unset" is always "not reported by
the hardware"* — and implements it as `physicalSizeValid`, `refreshRateValid`,
`refreshRateRangeValid`, `edidValid` on `Monitor`. Its own words name the defect:
`refreshRate 0 with refreshRateValid=false means the mode table did not state a rate.
This is the field that produced "60 Hz" on hardware that never said 60.`

`discovery.capnp ServiceHealth.sinceValid` states the same law for a measurement:
"`sinceValid` false means no transition has been observed, **NOT** 'it changed zero
microseconds ago'". That is the invariant the audit enforces.

### 4.2 What the audit detects mechanically

Measured across all 89 schemas from the CodeGeneratorRequest, the contract has
**already encoded the distinction in its naming convention**, and it encodes it
consistently:

| Signal | Count | Meaning |
|---|---|---|
| `Bool` field whose name ends in the **capitalised** suffix `Valid` | **9** | an honesty flag — does NOT mean the value is absent by default |
| `Bool` field whose name ends in lowercase `valid` / `invalid` | 12 | a **verdict** — "is this signature valid"; may be false with every measurement real |

The nine honesty flags, all of them:

| Field | Struct | Qualified value |
|---|---|---|
| `physicalSizeValid` | `display.Monitor` | `physicalSizeMm :T.Vec2` |
| `refreshRateValid` | `display.Monitor` | `refreshRate :Float32` |
| `refreshRateRangeValid` | `display.Monitor` | `refreshRateMin` / `refreshRateMax` |
| `edidValid` | `display.Monitor` | `edid :Data` |
| `sinceValid` | `discovery.ServiceHealth` | `sinceUs :UInt64` |
| `sinceValid` | `service.ServiceInstanceState` | `since` |
| `currentValueValid` | `shell.SelectorCompletion` | `currentValue :Text` |
| `pidValid` | `shell.Job` | `pid` |
| `noArgsValid` | `cli.CommandPresentation` | a *specification* question, not a measurement — see below |

The twelve lowercase verdicts are `ChecksumResult.valid`, `VerifyChainResult.valid`,
`PackageVerification.valid`, `IntegrityOutcome.valid`, `AttestationVerification.valid`,
`AttestationVerificationResult.valid`, `AttestationItemResult.valid`,
`introspectToken$Results.valid`, `validateClaims$Results.valid`,
`evaluateCondition$Results.valid`, `EvaluationResult.valid` and
`AccessibilityState.invalid`.

**Why the discrimination is the whole point.** A verdict may be false while every
measurement is real: a checksum that does not match is a real measurement of a real
mismatch. An honesty flag being false is the *only* permitted way for a measurement to
be absent. An audit that conflated them would demand a measurement where the contract
asks for a judgement — and a backend would then have to fabricate to pass. The capital
`Valid` suffix is therefore not a naming accident; it is the contract's honest flag and
should be treated as load-bearing by implementers and documented as such in the
`display.capnp` header it already is.

**Name-resolution of the partner field.** Seven of the nine resolve mechanically —
five by exact name (`refreshRateValid`→`refreshRate`, `edidValid`→`edid`,
`service.sinceValid`→`since`, `currentValueValid`→`currentValue`, `pidValid`→`pid`) and
two by unique prefix (`physicalSizeValid`→`physicalSizeMm`,
`discovery.sinceValid`→`sinceUs`). The remaining two are honest cases:

- `cli.CommandPresentation.noArgsValid` is correctly **not** name-resolvable, because
  its question is "is a bare invocation meaningful" — a documentation fact. It is
  excluded from the audit by rule, and the generated case for it is a *registry* case
  (the flag must agree with `CommandInfo` actually being answerable), not a
  measurement case.
- `display.Monitor.refreshRateRangeValid` qualifies `refreshRateMin` *and*
  `refreshRateMax`, neither of which is name-prefixed by `refreshRateRange`. It is the
  one honesty flag in the tree that name-shape detection cannot resolve. Mitigation in
  §8.3, and the audit falls through to the struct-wide rule below for it.

The companion caps-honesty pattern: **4 structs** carry `partial` + `note` for a
declared-N/A on an *enumeration* (`system.Capabilities`, `service.*`, `ime.*`,
`update.*`). `system.capnp:100-105` defines the semantics — *"True when the backend
could not enumerate its own capability set and is reporting a partial answer"* — which
is the exact same law applied to a capability set instead of to a measurement.

### 4.3 What the audit asserts, per honesty flag

For every honesty flag `F` qualifying value `V`, the generated test:

1. Runs the backend in a mode where the host genuinely cannot measure the qualified
   value (the mock backend forces this; a real backend is run against a known-
   unmeasurable configuration — a headless framebuffer, a filesystem that does not
   report `contentHash`, a job id the OS will not disclose).
2. Asserts `F == false`.
3. Asserts `V` is at its neutral value (`0`, `0.0`, empty `Data`, empty `Text`).
4. **Asserts the consumer never sees the neutral value as a measurement.** Where the
   struct has a `note`, `reason` or `metadata` slot, it must be non-empty when `F` is
   false — "not reported" is itself reported data.
5. Asserts the inverse on the REAL path: when the host *can* measure, `F == true` and
   `V` is not the neutral value. A `0 Hz` refresh rate on a panel that reported 60 is
   the fabrication this case exists to catch.

For the two honesty flags whose partner is not name-resolvable, the same five
assertions run against the struct's **entire measurable sibling set** at once: the
struct-wide fallthrough. It is weaker than an exact pairing but strictly stronger than
nothing, and it is what `display.Monitor.refreshRateRangeValid` gets.

### 4.4 Declared-N/A vs measured, the four states

A value's provenance is one of exactly four things, and only four:

- **Measured** — the host told us; the value is real, the honesty flag is `true`.
- **Declared-N/A** — we can answer the question and the answer is "not applicable on
  this platform"; the method is `UNAVAILABLE` with `not-applicable`.
- **Unknown-N/A** — we cannot enumerate at all; `partial = true`, `note` non-empty, and
  the consumer must not read an absent token as "not supported" (`system.capnp:100-105`).
- **Fabricated** — a zero, a default or an invented string standing in for any of the
  first three. **This is the one unforgivable defect**, and §4.3 is the test that finds
  it. Everything else in this document is scaffolding for that single sentence.

### 4.5 The E8 connection (why this audit also guards forward compatibility)

The schema gate currently reports **267 E8 findings**: enum `@0` values that are
paradigm values rather than neutral sentinels (`ColorSpace.srgb`, `UsbTransferType.control`,
`UsbDataRole.host`, `EndpointDirection.in`). A backend that answers an unrecognised
enum value by defaulting to `@0` therefore fabricates a *specific, plausible-looking*
answer — `srgb`, `control`, `host` — rather than reporting ignorance. That is the
fabrication class the forward-compat template must also close (§5): unknown-tolerance
must mean `NotInSchema`, never a default.

---

## 5. Forward-compatibility tolerance template

The contract's rule (README §"How the contract stays forever", item 4): every event
union ends in `unknown @N :T.UnknownEvent`, so a producer can forward a kind it did
not foresee and a consumer routes it without breaking.

The generated template, one instance per union that has an `unknown`/`custom` arm:

```text
FORWARD-COMPAT   <domain>.<Struct>
  producer case:
    1. Construct the message with the union's highest declared discriminant set to an
       arm that does NOT exist in the compiled schema (discriminant value = N+1).
    2. Round-trip it through the transport under test.
    3. ASSERT: reader_which() == Err(capnp::NotInSchema(N+1))  — the discriminant
       survives the round trip and no arm was invented.
  carrier case:
    4. Build T.UnknownEvent { typeId = <an unpublished uint64>, payload = <bytes> }.
    5. ASSERT: it round-trips; typeId and payload are byte-equal; none of the known
       arms is set; the payload is NOT widened into a T.DataHandle and NOT re-encoded.
  consumer case:
    6. A backend receiving (3) or (4) must route it — T.Error{cancelled} with the
       original discriminant in the message, or the consumer's own unknown-arm
       handler. It must NOT reject the message, panic, or silently fall into a
       known arm.
```

Three facts make this honest rather than aspirational:

1. `capnp::NotInSchema(pub u16)` exists in the pinned runtime (`capnp-0.27.2/src/lib.rs:170`)
   and generated union readers return `Result<Which, NotInSchema>`
   (`capnpc-0.27.0/src/codegen.rs:1502`). An unknown discriminant is a **typed
   error carrying the unknown value** — never a panic, never a default. The template
   asserts the type-level witness, which a hand-written test could only assert by
   convention.
2. `types.capnp:297-300` names `T.UnknownEvent` as the **single** canonical carrier
   with one declared exemption from the bulk rule (`validate.py`'s `R2_ESCAPE_HATCH`),
   so the template tests one type, not a private copy per domain. `R7` is checkable
   against one type.
3. Open tokens need no schema edit: an unrecognised `T.MimeType` subtype,
   `CapabilityDescriptor.name`, `Usage.unit` or analysis-kind token must be ignored,
   never rejected (`ai.capnp:42-47`). The template's "enum-free open values" case
   feeds a nonsense token and asserts passage.

**Companion assertion — additive growth must not move an existing arm.** For every
union, the template also asserts that adding a *future* discriminant leaves the wire
layout of every existing arm untouched: `discriminantCount` may grow, but the offset,
type and ordinal of each existing arm are pinned by the test against the compiled
schema. This is the additive-only rule of README §"How the contract stays forever"
item 2, made executable — and it is the assertion that catches a backend which
"extends" a union by repurposing an existing ordinal.

---

## 6. What L2/L3 do NOT test (explicit non-goals)

The suite is ruthless and small. Every test below would cost more than it earns.

1. **No performance or latency assertions.** No "readFile < 1 ms". Timing is a
   benchmark suite's job, and a timing assertion in a conformance suite flakes on
   loaded CI and hides real defects. `bounded-pull` is a *semantics* assertion (the
   producer never outruns the consumer's stated bound), never a throughput one.
2. **No internal-representation assertions.** No test that a `T.DataHandle.buffer` is
   a specific shared-memory object, or that a handle is a specific number. Opacity is
   doctrine rule 5; a backend's backing is its business.
3. **No cross-domain integration.** L2 is per-domain. A files+audio workflow is an
   integration test and belongs to the integration wave.
4. **No real-hardware or privileged tests in CI.** A test that needs an actual audio
   device, a machine-integrity check, or root is `ignored` locally and run in a
   hardware lane. CI runs the mock plus the unprivileged real paths.
5. **No assertion on values the contract leaves open.** No test that a `status` token
   is one of today's strings, that a `metadata` key exists, or that a `note` is
   non-empty on a REAL row. Unknown tokens are ignored, never required.
6. **No test of the CLI's rendering.** `cli.capnp`'s registry *data* is asserted
   (`listUnavailable` ↔ matrix, reason nameable); the help text's typography is not.
7. **No test of transport internals.** Capability-transfer *semantics* per transport
   are asserted (a handle survives the hop); socket framing, scheduling and zero-copy
   *mechanism* are the transport wave's own tests.
8. **No golden-file conformance.** The generated suite is checked in so it is
   reviewable and diffable, but no test compares a whole message against a frozen
   dump — those break on any legitimate additive change and stop being reviewed.
9. **No coverage percentage target.** 1,471 methods generate a smoke case; chasing a
   number would push hand-written filler. The rule is total, not graduated: every
   method has the cases its shape implies, and that is the whole objective.
10. **No re-implementation of `validate.py`.** The schema gate is upstream of this
    model and stays in its own repo. L0 *runs* it; it does not copy it.

---

## 7. Evidence: what was actually run

| Claim | Command | Result |
|---|---|---|
| capnp binary | `capnp --version` | 1.4.0 |
| CGR dump works | `capnp compile -o - schema/files.capnp --no-standard-import -I schema/` | exit 0, 163,520 bytes |
| whole tree dumps | same over `schema/*.capnp` | exit 0, 6,123,728 bytes |
| CGR is decodable by capnp itself | `capnp convert binary:text <capnp-0.27.2>/schema.capnp CodeGeneratorRequest` | exit 0, 10,096 lines for files.capnp |
| JSON decode | `capnp convert binary:json …` | exit 1 — `capnp/compat/json.c++:363` AnyPointer in `defaultValue`; documented, not blocking |
| introspection API available in the pinned dep | `rg "pub mod schema" capnp-0.27.2/src/lib.rs`; `rg "pub struct StructSchema" …/src/schema.rs`; `rg "pub enum TypeVariant" …/src/introspect.rs` | all present (StructSchema, Field, FieldSubset::get_union_fields, EnumSchema, TypeVariant incl. `Struct`/`Capability`/`Enum`/`List`) |
| unknown-discriminant behaviour is typed | `rg "pub struct NotInSchema" capnp-0.27.2/src/lib.rs`; `rg "pub fn which" capnpc-0.27.0/src/codegen.rs` | `NotInSchema(pub u16)`; `fn which() -> Result<Which, capnp::NotInSchema>` |
| method/interface counts | inventory built from the CGR, own-file filter, all 89 schemas | 303 interfaces (grep-verified, 0 mismatches), 3,143 methods, 461 enums |
| shape counts | same | 301 Pagination, 567 list, 495 error-union, 40 Capability, 56 DataHandle-in, 29 DataHandle-out |
| honesty flags | same | 9 capital-`Valid` honesty flags (7 name-resolvable), 12 lowercase `valid`/`invalid` verdicts, 4 caps-`partial` structs |
| schema gate green | `python3 scripts/validate.py` | exit 0; E7=715, E8=267 REPORT |

---

## 8. Risks in this model

1. **`unknown` reason-class becomes the hiding place.** It exists in the contract
   precisely so a gap is visible; a backend that reaches for it under deadline
   converts a visible gap into an invisible one. Mitigation: an `unknown` row in a
   release matrix is a CI failure (§3.5), and the taxonomy is short enough that
   `unknown` is never the right answer — one of the other four always is.
2. **The CGR is load-bearing in a second place.** The generator now reads the same
   artifact `build.rs` does, so the two are coupled by construction — which is the
   point, but it means a change to either moves the other. Mitigation: the generator
   uses the *pinned crate's* `schema_capnp` in-process (no external decoder), so its
   only version coupling is the `capnp` dependency that already exists, and a system
   binary change surfaces as `build.rs`'s triad failure, which already refuses to
   silently downgrade.
3. **Honesty-flag detection is name-shaped, and one flag in the tree defeats it.** The
   capital-`Valid` suffix is a convention, not a schema-level annotation, so a future
   schema could express honesty without it and the audit would miss it; and
   `display.Monitor.refreshRateRangeValid` qualifies `refreshRateMin`/`refreshRateMax`
   without being name-derivable from either. Mitigation: the struct-wide fallthrough
   (any capital-`Valid` Bool gates the struct's measurable siblings), the
   `partial`/`note` pattern enumerations, and a recommendation to the schema owner to
   add a real `$Honest` annotation — which is a contract change and therefore not mine
   to make.
4. **`DEGRADED` is the easy status.** It is the one status that requires naming *which*
   capability is degraded, so it is the one a rushed backend will over-use to avoid
   either finishing or admitting. Mitigation: §3.4 requires `detail`, and `l3Test` is
   required for `DEGRADED` rows, so a degraded claim is still backed by a passing test.
5. **The model is only as total as the shape table.** If a method matches no row, the
   `smoke only` row catches it — so the suite cannot silently shrink, but it also
   cannot grow into a real test. Mitigation: a new *shape* in a future contract (a new
   `TypeVariant` arm, say) must add a rule-table row, and that is the one place this
   document needs a human. Stated here so it is not discovered at `L2 == green`.

## 9. What I need from other owners

- **Orchestrator:** assign `knowledge/availability/**` (used by §3.1) — it is in no
  one's row, and two agents creating it independently is a conflict.
- **Wave-1 skeleton agent (owns `build.rs`, `Cargo.toml`):** expose the compiled
  `CodeGeneratorRequest` (or a typed inventory derived from it) to the conformance
  crate via `OUT_DIR`, so the generator and the codegen share one artifact. Today
  `build.rs` consumes the request and drops it.
- **Conformance agent (later wave):** `src/conformance/**` per the ownership table;
  `tests/` is currently unowned — decide which holds the generated suite.
- **Domain/platform agents:** the availability rows are yours to author; the schema in
  §3.2 is the only thing I am asking you to conform to.
