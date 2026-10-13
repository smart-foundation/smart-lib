# 00 — CHARTER: what smart-lib-ng is and the laws of working here

You are an agent building **smart-lib** (next generation) in `/home/lufuno/smart/smart-lib-ng/`. Read this fully before touching anything. Your brief (`briefs/BRIEF-NN-*.md`) says what YOU build; this charter says what everything is.

## What this is

smart-lib is the library of **protocol-compliant backends for common operating systems**, so any application on any OS speaks smart-protocol natively. It is built on the frozen canonical contract at `/home/lufuno/smart/smart-protocol-official/` (89 schemas, validator green — never modify it). The old implementation (`smart-protocol/crates/smart-lib`) is frozen reference material: study it, never edit it. The old `smart-protocol/crates/smart-protocol/src/backend.rs` (727-line god-trait importing three deleted schemas) is the anti-pattern we replace.

## The seven laws (non-negotiable)

1. **One library, every altitude.** Core is `no_std` + `alloc`. Same API in a kernel module, a service, an app.
2. **Feature = domain.** Each domain is a cargo feature; `build.rs` compiles only enabled features' schemas + foundation. Prove it: disabling a feature must remove its codegen (check `target/` output). Never compile all 88 domains at once — that is the failure mode this law exists to prevent.
3. **Per-domain backend traits.** `FilesBackend`, `NetworkBackend`, ... — one object-safe trait per domain. No god-trait. A backend implements what its platform can do; the rest is honest `Unavailable` with a reason.
4. **Transports are first-class.** A `Channel` abstraction: InProcess (direct), Shmem (zero-copy frames), UnixSocket, Tcp. One API shape across all four. Capability-transfer semantics documented per transport.
5. **Authority is a handle.** No permission strings anywhere. Calls carry capability references; host credentials (uid/caps/entitlements) map into handles at acquisition. Security mechanisms are backend detail, not API.
6. **Honest degradation.** `Unavailable` + reason is a first-class result; every backend publishes `caps()`. Measured or declared-N/A — a fabricated value is the one unforgivable defect.
7. **Generated glue, hand-written core.** State structs and conversions generated from schemas; platform logic hand-written and small; conformance tests generated from the contract.

8. **The host is read-only.** You never change the state of the machine you run on. Reading is free (`/proc`, `/sys`, `/etc`, D-Bus reads, `--print-reply` queries, opening devices read-only). Changing is banned: no state-changing calls to host services or daemons (a D-Bus method that writes, `nmcli`-style commands that act), no `sudo`, no killing or starting processes, no writing `/etc` or `/var`, no enabling/disabling anything. The incident that birthed this law: an agent called NetworkManager's `Enable(false)` over D-Bus to "probe" an unprivileged call; it succeeded, wrote `NetworkingEnabled=false` into `/var/lib/NetworkManager/NetworkManager.state` — persistent state that survived reboot and killed the host's wifi. A "what happens if" question is answered from documentation, reasoning, or a mock — never live fire on someone's machine. **The live proof of any host-mutating capability is run by the founder, not by you.**

9. **The transport hierarchy: kernel first, daemon for policy, contract for consumers.** A host read that the kernel is authoritative for (interfaces, addresses, routes, counters, carrier) is read from the kernel — on Linux that is netlink, event-fed into a local state store, so a read costs no daemon hop. A daemon (D-Bus and friends) is used only for what the kernel does not own: connection profiles, credentials, vendor policy surfaces — and for the control (mutating) operations only that daemon exposes. The daemon path is a FALLBACK for reads, never the primary. Between our own components, the contract's transports carry the traffic (zero-copy where the contract says DataHandle). Hand-rolling a host wire protocol is a last resort, never a first: use maintained crate ecosystems behind a mockable seam. The proven failure of the alternative is recorded: a per-call D-Bus read path made every status query a marshalled daemon round trip and needed three wire-bug fixes in its own marshaller.

## File ownership (conflicts are fatal — respect absolutely)

| Path | Owner |
|---|---|
| `build.rs`, `Cargo.toml`, `src/lib.rs` | Wave-1 skeleton agent only |
| `src/core/**` | Wave-2 core agent |
| `src/transport/**` | Wave-3 transport agent |
| `src/domains/<domain>.rs`, `src/domains/<domain>/**` | that domain's agent only |
| `src/platforms/<os>/<domain>.rs` | that (os, domain) cell's agent only |
| `src/conformance/**` | conformance agent |
| `knowledge/briefs/**` | orchestrator (read-only to you) |
| `knowledge/reports/NN-*.md` | YOU write ONLY your own report file |
| `knowledge/lessons/` | you may APPEND your own `<NN>-lessons.md` |
| `knowledge/availability/**` | the conformance-implementation wave's agent (assigned 2026-10-11; nobody else writes here) |

If you need a file you don't own: do not edit it. Note the requirement in your report; the orchestrator dispatches the owner or a wire agent.

## The capnp triad (verified fact)

System `capnp` binary: **1.4.0**. The old workspace pins 0.25.6/0.25.3; the canonical contract's current line is **0.27.x**. The new workspace must declare one consistent triad and PROVE codegen from the canonical tree works with the installed binary; if the binary is incompatible with the pinned capnpc, report the exact error — do not silently downgrade the pin, and do not install system packages without reporting first.

## Build discipline

- Build only in this workspace, one build at a time, `-j 6`, never `cargo clean`, never `--workspace` builds of other projects.
- Touch NOTHING under `/home/lufuno/smart/naledzi`, `/home/lufuno/smart/smart-protocol`, `/home/lufuno/smart/smart-protocol-official` (the contract is read-only input).
- No git commits.

## What "done" means for every task

Your brief carries a checklist. A box is checkable only when it is mechanically verified (build output, test output, or a generated artifact cited in your report). Your report must state, for each checklist item: DONE (with evidence), PARTIAL (what remains and why), or BLOCKED (what you need from whom). Never mark by opinion. If you discover the brief is wrong, say so in the report with evidence — following a wrong brief faithfully is a failure mode here.

## The product standard (what we are judged by)

Blazing (no allocations on hot paths, bounded pulls, no polling, static dispatch by default — `dyn` only at the transport seam), zero-cost (you pay only for the features you enable), timeless (additive-only public API; breaking only via new canonical schema IDs), agnostic (no platform noun in required positions; no vendor lock), simple (fewer concepts beat more features — if a design needs a paragraph to justify, it is probably wrong), robust (errors total, states exhaustive, fuzz-able seams).

10. **Machine safety (the build box is small: 15 GB RAM, no swap, `/tmp` is tmpfs).** Never copy a workspace containing `target/` (it copies gigabytes into RAM and has frozen this machine once — 2026-10-13, a 6.7 GB `target` copied into the 7.7 GB tmpfs; the parallelism was NOT the problem, the copy was); `/tmp` is not scratch for anything big; `-j 6` is the approved build bound — do not slow work down further; verify with `cargo check` before `cargo build` when iterating; no command runs without a stated reason it answers, and no command that copies, moves, or deletes at scale runs at all unless the task requires it — focus on the work, not on clever diagnostics.

11. **Never build the old repo's bindings crate whole, and never two heavy crates in one command.** The old `smart-protocol` crate compiles the ENTIRE schema tree (~94 files) in one codegen and OOMs this 15 GB no-swap box; `cargo check -p smart-protocol -p smart-lib -j 6` crashed it directly on 2026-10-13. In the old repo: one crate at a time, `--lib -p` for heavy ones, never `-p smart-protocol`, never workspace-wide checks with every schema. `--all-features` stays banned everywhere.
