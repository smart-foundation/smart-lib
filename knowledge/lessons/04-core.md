# 04 — core lessons (Wave 2, Agent D)

* **capnp 0.27 splits "the runtime error" from "the contract error".** `capnp::Error`
  (`{ kind: ErrorKind, extra: String }`) is the *transport/runtime* failure; `T.Error`
  (`{ code: ErrorCode, message, context, retryable, metadata }`) is the *protocol* failure. They
  are different types and a bridge between them is a classification, not a conversion — write it
  once (`From<capnp::Error> for Error`) or every backend invents its own.
* **`text::Reader::to_string()` returns `Result<String, Utf8Error>`**, unlike `str::to_string`.
  A `.map(|m| m.to_string()).unwrap_or_default()` chain therefore fails to compile with a
  confusing `Result<String, Utf8Error>: Default` error. Read contract `Text` through one helper.
* **Generated module names are not naive snake_case**: `Point3D` → `point3_d` (capnpc puts the
  underscore *after* a digit before a following capital), while `Vec2i` → `vec2i`. Generate the
  name map and assert it against the generated file; do not hand-write it.
* **`core::rc` does not exist** on current toolchains — `Rc` lives in `alloc::rc`. Likewise
  `impl From<Cap<T>> for T` is illegal (E0210, uncovered type parameter in a foreign trait);
  provide an inherent `into_client()` instead.
* **RPITIT server traits have no local client bridge.** Generated `Server` methods take
  `self: Rc<Self>` and the runtime's `FromServer` only yields a `Dispatch` (a `Server`), never a
  `Client` — the server→client adapter is `capnp_rpc::new_client`'s job and capnp-rpc is not in
  our graph. To test capability semantics (revocation, error propagation), implement capnp's
  `ClientHook` seam instead; it is ~40 lines of stubs and needs no executor drama because
  `ClientHook::when_resolved`/`new_call().send()` resolve on first poll (a no-op raw waker drives
  them).
* **A path fragment in `macro_rules!` cannot be followed by `::`.** `$name:path` + `$name::from(…)`
  fails to parse; alias it inside the macro (`type V = $name;`) and call `V::from(…)`.
* **Self-verifying offsets beat hardcoded ones.** For the out-of-schema-discriminant test, the
  byte offset of the code field inside a serialized root message is derived by searching for the
  written code's `u16` little-endian pattern and asserting exactly one hit — the framing (two
  segment-table entries here: an empty first segment plus the data segment) is then proven by the
  test itself, and a future framing change fails loudly with the new offset in the message.
* **Mirroring the wire keeps error mappings total.** A payload-carrying error enum (the old
  `SmartError::Failed(String)`) forces messages onto codes that need none and drops the fields it
  did not model; a struct mirroring the contract plus a code enum that is *exactly* the contract's
  set makes both conversion directions exhaustive with no catch-all, and a contract that grows a
  21st code breaks the build instead of silently mis-mapping.
* **`missing_docs` lints public struct fields, not just items.** Macro-generated structs need a
  `#[doc = concat!(…)]` per field to keep the crate warning-clean.
