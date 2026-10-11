# 01 — lessons from the skeleton wave

Appended by the Wave-1 skeleton agent. Nothing here is normative; the charter and the briefs
are. Each item cost a debugging cycle.

## capnp 0.27 facts worth keeping

1. **`capnpc` 0.27 is pure Rust; the C++ binary is only the schema front-end.** `CompilerCommand`
   runs `capnp compile -o-` and parses the request itself. So "the triad" is really
   `binary (schema compiler + request writer)` + `capnpc (request reader + Rust emitter)` +
   `capnp (runtime)`. Binary 1.4.0 was accepted without a murmur.
2. **capnp needs its `alloc` feature for interface code.** `FromClientHook` / `DynClientHook` /
   `DispatchCallResult` are behind `feature = "alloc"`. With `default-features = false` and no
   `features = ["alloc"]`, codegen compiles but *our* crate fails with
   `error[E0433]: cannot find FromClientHook in capability`. The dependency line is
   `capnp = { version = "0.27.2", default-features = false, features = ["alloc"] }`.
3. **Generated files are a flat list of `pub mod <type> { … }` blocks — there is no
   `<file>_capnp` wrapper module inside the file.** Our `schema::<stem>_capnp` wrapper is ours
   to provide, which is why `default_parent_module(["schema"])` is mandatory: capnp emits
   cross-file paths as `crate::schema::<stem>_capnp::…`.
4. **0.25 → 0.27 API drift**: `init_root::<T::Builder<'_>>()` / `get_root::<T::Reader<'_>>()`
   (not `::Owned`); per-type modules (`capability_capnp::capability::Reader`); enums at module
   scope with CamelCase variants (`capability_capnp::Operation::Read`); generated `Server`
   traits are **not** dyn-compatible (RPITIT), so `dyn …::Server` is a compile error.
5. **The alloc-import injection must carry `#[allow(unused_imports)]`.** The old build-helpers
   injected bare `use alloc::…` and left 2,281 warnings across the default build; the import
   block is injected at every module boundary of a file that contains interface code anywhere.
   Keep the allows, or CI with `-D warnings` dies.

## Schema-tree pitfalls

6. **Strip comments before parsing imports.** `usb.capnp`'s prose names the *deleted*
   `filesystem.capnp`; a naive `import "` regex makes build.rs demand a file that does not
   exist. Comment stripping is the difference between a green build and a bogus blocker.
7. **`kernel.capnp` imports `boot.capnp` and `boot.capnp` imports `kernel.capnp`** — the tree
   has cycles. Any resolver needs a `visited` set, not a depth counter.
8. **Domains leak into each other.** `shell` alone pulls 8 more schemas two levels deep
   (`shell → compositor → theme`, `shell → appindex → manifest → abi`), so
   `domain-shell` = 13 compiled files. That is the contract's design, not a bug: a domain's
   schema cannot compile without its imports.

## Registration / build-script practice

9. **Absolute `#[path]` beats relative.** Relative `#[path]` inside an inline `mod { … }`
   resolves against the *module's* directory, not the file's — a subtle failure when the
   declaration lives in a build-generated file. Emit absolute paths and the class of bug
   disappears.
10. **Build scripts cannot use `#[path = "../src/…"]`.** build.rs sits at the manifest root, so
    the path from build.rs into `src/` is `src/codegen/mod.rs`, not `../src/codegen/mod.rs`.
11. **A post-processing helper in `src/`, included from both `build.rs` (`#[path]`) and a test
    target, gets unit tests for free.** The old workspace's version was untested; ours is
    covered by 17 tests including the real `usb.capnp`.
12. **Distinct feature sets get distinct `OUT_DIR`s** (cargo hashes the unit), so multiple
    feature-set builds coexist in `target/debug/build/smart-lib-*/out`. That is what makes the
    "generated code absent when the feature is off" proof inspectable without a clean build
    (charter forbids `cargo clean`). Directory mtimes only change on add/remove, so identify a
    dir by its `smart_lib_modules.rs` header, not by `ls -dt`.
