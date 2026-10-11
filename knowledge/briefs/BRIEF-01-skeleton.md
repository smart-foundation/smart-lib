# BRIEF-01 — Crate skeleton + feature-gated codegen (Wave 1, Agent A)

## Mission
Create the `smart-lib` (next generation) crate skeleton in `/home/lufuno/smart/smart-lib-ng/`: a self-contained workspace whose `build.rs` compiles schemas from the canonical contract **per enabled feature**, on a verified capnp 0.27 triad, with a module pattern that lets domain agents add domains without ever conflicting on shared files. This skeleton is load-bearing: Waves 2+ build on it.

## Your exclusive files (create; you own them)
- `Cargo.toml`, `Cargo.lock` (generated), `build.rs`, `src/lib.rs`, `.gitignore`, `src/codegen/` (your own post-processing helper), `src/prelude.rs` (re-export point only)
- DO NOT create or edit: `src/core/**`, `src/transport/**`, `src/domains/**`, `src/platforms/**`, anything in `/home/lufuno/smart/` outside this directory. DO NOT git commit.

## Requirements (the checklist)

- [ ] **Workspace + crate manifest**: edition 2021+, `#![no_std]` with `extern crate alloc` in lib.rs; workspace resolver 2; crate name `smart-lib`, version 0.1.0.
- [ ] **Feature = domain law**: one cargo feature per domain (`domain-files`, `domain-network`, `domain-audio`, `domain-system` for the pilot four; the pattern must extend to all 88 without manifest rewrites beyond a feature line each). Foundation schemas (`types.capnp`, `capability.capnp`, `abi.capnp` + their imports) ALWAYS compile regardless of features.
- [ ] **build.rs compiles from the canonical tree only**: schema source = `/home/lufuno/smart/smart-protocol-official/schema` (read-only, absolute or relative path — your choice, document it). Compile exactly the union of: foundation + enabled features' schemas (+ their transitive imports).
- [ ] **PROVE the feature law**: build with default features, then with `--no-default-features --features domain-files`; show in your report that generated code for network/audio/system is absent in the second build (e.g. `find target -name "*_capnp.rs" | sort` diff, or equivalent evidence). This proof is the brief's primary acceptance test.
- [ ] **Codegen post-processing**: the old workspace's `build-helpers` injects `use alloc::boxed::Box; use alloc::string::ToString; use alloc::vec::Vec;` into generated files containing interface code (capnp 0.27 generates those calls). Port that logic into your own `src/codegen/` helper — do NOT path-depend on the old workspace (it pins capnp 0.25.6).
- [ ] **capnp triad**: declare one consistent triad (`capnp` / `capnpc` / C++ binary). The installed binary is **1.4.0**; the canonical contract's line is 0.27.x. Make codegen work with what is installed; if `capnpc` and the binary are incompatible, report the EXACT error verbatim and stop — do not downgrade pins silently, do not apt-install anything.
- [ ] **Module registration pattern**: design `src/lib.rs` so domain agents add a domain by creating ONLY `src/domains/<domain>.rs` (+ its dir) — the manifest gets a one-line feature (orchestrator's wire agent does that), never a shared-file edit by the domain agent. Document the pattern in a comment block at the top of `src/lib.rs`.
- [ ] **Smoke test**: `tests/smoke.rs` (or a unit test) that constructs a message with the generated types (e.g. a `types.capnp` type) and round-trips it — proof the codegen+post-processing works.
- [ ] **no_std proof**: `cargo build --no-default-features` (or the minimal feature set) compiles for a no_std-ish target profile to the extent the dependency graph allows; document what (if anything) still requires std and why.
- [ ] Report the exact `capnp --version`, the triad versions, and the full build output of the feature-law proof.

## Report
Write `/home/lufuno/smart/smart-lib-ng/knowledge/reports/01-skeleton.md`: checklist status per item (DONE/PARTIAL/BLOCKED with evidence), the triad verification, the feature-law proof output, the module pattern documentation, anything you had to decide that a later wave must know (API sketches for core/transport to build on), and blockers. You may append `/home/lufuno/smart/smart-lib-ng/knowledge/lessons/01-skeleton.md` with lessons learned.

## Return to orchestrator (final message, ≤25 lines)
Checklist status summary, triad verdict, feature-law proof one-liner (generated-file counts per feature set), blockers.