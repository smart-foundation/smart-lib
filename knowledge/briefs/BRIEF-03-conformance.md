# BRIEF-03 — Conformance model design (Wave 1, Agent C)

## Mission
Design the mechanical meaning of "this backend is protocol-compliant" for smart-lib, and how conformance is *generated from the canonical contract* rather than hand-written per domain. You write NO library code. You write the model every later wave is verified against — including the per-platform availability matrix format (the data behind `smart doctor`).

## Your exclusive output
- `/home/lufuno/smart/smart-lib-ng/knowledge/03-CONFORMANCE.md`
- (optional append) `/home/lufuno/smart/smart-lib-ng/knowledge/lessons/03-conformance.md`
Create no other files. Edit nothing else. No git commits.

## Inputs to read (read-only)
- Canonical contract: `/home/lufuno/smart/smart-protocol-official/schema/` — especially `cli.capnp` (the registry law: "help must not list a command that cannot run, and the reason must be nameable"), `types.capnp` (T.Error, T.Pagination, T.DataHandle, ID types), `capability.capnp` (authority model), `ai.capnp` (the gold-standard schema).
- `/home/lufuno/smart/smart-protocol-official/scripts/validate.py` — the existing schema-level validator (E1–E14 rules). Your model sits ON TOP of it: it checks schemas; yours checks *implementers*.
- Charter: `/home/lufuno/smart/smart-lib-ng/knowledge/00-CHARTER.md`.

## The checklist

- [ ] **Conformance levels**: define the tiers precisely. Proposal to evaluate and refine: L0 schema-conformant (generated surface matches the contract — validator + codegen diff), L1 API-conformant (public API total over the domain's methods; `T.Error` mapping total; capability gating on every method that needs it), L2 behavior-conformant (generated behavior tests per method: success paths, error paths with honest `retryable`, pagination/cursor semantics, bounded-pull semantics, DataHandle zero-copy — no inline bulk), L3 platform-conformant (per-OS backend passes the L2 suite for the methods it claims REAL; everything else declared UNAVAILABLE with reason class).
- [ ] **Test generation from schemas**: specify how a per-domain conformance test harness is generated from the schema (method inventory via schema parsing or capnp introspection; which cases each method shape auto-generates: zero-arg query, param'd query, mutation round-trip, error injection via a mock backend that forces each `T.Error` variant, unbounded-list pagination test, streaming pull/termination test). The harness must work against ANY backend implementing the domain trait — that is how a mock backend and a Linux backend prove themselves with the same suite.
- [ ] **The honesty audit**: design the check that finds fabricated values (backend returning 0/empty/invented where the host can't measure). What does a conformance test assert about "declared-N/A" (e.g. a struct's `*-Valid` honesty fields, if the contract has them) — check how display.capnp's `*Valid` fields work and generalize the pattern.
- [ ] **Availability matrix format**: the exact machine-readable format (schema for it: fields per (domain × platform): status REAL/DEGRADED/UNAVAILABLE, reason class (no-backend / no-privilege / not-applicable), the capability set `caps()`, contract version, transport(s) supported). This is the data `smart doctor` prints and CI publishes. Specify where it lives in the crate (a generated file? a const table per backend?) and how it is verified (no claimed REAL without a passing L3 test).
- [ ] **Forward-compat conformance**: a test template proving a backend tolerates unknown union variants (the `unknown @N :UnknownEvent` law) and that new enum-free open values don't break old backends.
- [ ] **What L2/L3 do NOT test** (explicit non-goals — keep the suite ruthless and small).

## Report
`03-CONFORMANCE.md`: the level definitions, the generation design (with a concrete example for ONE method — e.g. `files.capnp`'s `readFile` or `statFile` — showing the generated test cases), the honesty-audit design, the availability-matrix schema, the forward-compat template, and the non-goals. Mark each design decision with its rationale in one line.

## Return to orchestrator (final message, ≤25 lines)
The three conformance levels in one line each, the generation mechanism verdict (schema-parse vs introspect), the availability-schema field list, and the top-3 risks in the model.