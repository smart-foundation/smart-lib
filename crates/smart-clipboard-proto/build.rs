//! Compile the contract's own schemas for the client half.
//!
//! The app compiles the contract itself rather than importing the engine's
//! generated types. That is the decoupling rule in one line: a product speaks
//! schemas, not a library's internals. If `smart-lib` changed its private
//! types tomorrow, this crate's build would not notice — because it never
//! reads them.
//!
//! Six files and their closure: the domain this product exists for
//! (`clipboard`), the two foundation surfaces it must speak to reach it
//! (`discovery`, `capability`), and the three files those import
//! (`types`, `diagnostics`, `logging`). Everything else in the 89-schema contract is
//! deliberately not compiled here; a clipboard app that compiled the whole
//! tree would be paying for domains it will never call (charter law #2, and
//! law #11 for the build box).
//!
//! ## Schema resolution
//!
//! Same candidate order as the engine's `build.rs`, so one checkout works for
//! both halves:
//!
//! 1. `$SMART_SCHEMA_DIR` — hermetic/vendored builds, and CI;
//! 2. `<this crate>/../../../smart-protocol-official/schema` — the layout the
//!    workspace lives in;
//! 3. `/home/lufuno/smart/smart-protocol-official/schema` — the canonical
//!    tree on the founder's box.
//!
//! The resolution is *reported*: the generated module carries the resolved
//! path, and the app's About panel shows the schema triple it was built
//! against. A client that cannot say which contract it speaks has no business
//! claiming to speak it.

use std::path::{Path, PathBuf};

/// The schemas this product speaks. Order matters only for readability.
const SCHEMAS: [&str; 6] = [
    "types.capnp",
    "logging.capnp",
    "diagnostics.capnp",
    "capability.capnp",
    "discovery.capnp",
    "clipboard.capnp",
];

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let schema_dir = schema_dir(&manifest_dir);

    let mut command = capnpc::CompilerCommand::new();
    command.src_prefix(&schema_dir);
    for schema in SCHEMAS {
        let path = schema_dir.join(schema);
        assert!(
            path.is_file(),
            "build.rs: the contract schema {} is missing (resolved schema dir: {})",
            schema,
            schema_dir.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
        command.file(path);
    }
    command
        .run()
        .expect("build.rs: the contract's schemas must compile with the pinned capnpc");

    // capnpc emits `crate::<stem>_capnp` paths, assuming the generated modules
    // sit at the crate root. This crate keeps them under `schema` (so a reader
    // can tell generated code from hand-written code at a glance), which is the
    // same arrangement the engine uses — so the same rewrite applies, and it is
    // done *here* rather than by moving the modules, because a product should not
    // reshape the contract's own output.
    rewrite_paths(&std::env::var("OUT_DIR").expect("OUT_DIR"), &[
        "types_capnp",
        "logging_capnp",
        "diagnostics_capnp",
        "capability_capnp",
        "discovery_capnp",
        "clipboard_capnp",
    ]);

    // The identity of the contract this binary was built against. Shown in
    // the app's About panel; logged when a service publishes a different
    // schema id (see `contract::SCHEMA_DRIFT`).
    println!(
        "cargo:rustc-env=SMART_CLIPBOARD_SCHEMA_DIR={}",
        schema_dir.display()
    );
    println!("cargo:rerun-if-env-changed=SMART_SCHEMA_DIR");
    println!("cargo:rerun-if-env-changed=CAPNP");
}

/// Point the generated modules' `crate::<stem>_capnp` paths at `crate::schema`.
///
/// A plain string replacement over the crate's own four stems: the set is
/// closed, the stems are known, and a regex would be a dependency for no gain.
/// Each replacement is skip-safe because the search text is the unprefixed form
/// and the replacement inserts the prefix exactly once.
fn rewrite_paths(out_dir: &str, stems: &[&str]) {
    let entries = match std::fs::read_dir(out_dir) {
        Ok(entries) => entries,
        Err(error) => panic!("build.rs: cannot read {out_dir}: {error}"),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        let mut rewritten = source.clone();
        for stem in stems {
            let from = format!("crate::{stem}");
            let to = format!("crate::schema::{stem}");
            // `crate::schema::types_capnp` must not become
            // `crate::schema::schema::types_capnp`, so the search text is
            // anchored on the *unprefixed* form and only unprefixed occurrences
            // are replaced.
            rewritten = rewritten.replace(&from, &to).replace("crate::schema::schema::", "crate::schema::");
        }
        if rewritten != source {
            std::fs::write(&path, rewritten)
                .unwrap_or_else(|error| panic!("build.rs: cannot rewrite {}: {error}", path.display()));
        }
    }
}

/// Resolve the canonical schema tree, in the documented order.
fn schema_dir(manifest_dir: &Path) -> PathBuf {
    let candidates = match std::env::var_os("SMART_SCHEMA_DIR") {
        Some(dir) => vec![PathBuf::from(dir)],
        None => vec![
            manifest_dir.join("../../../smart-protocol-official/schema"),
            manifest_dir.join("../smart-protocol-official/schema"),
            PathBuf::from("/home/lufuno/smart/smart-protocol-official/schema"),
        ],
    };
    for candidate in &candidates {
        if let Ok(dir) = candidate.canonicalize() {
            if dir.is_dir() {
                return dir;
            }
        }
    }
    let tried: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect();
    panic!(
        "build.rs: cannot find the smart-protocol schema tree. Tried:\n  {}\n\
         Set SMART_SCHEMA_DIR to the contract's `schema/` directory.",
        tried.join("\n  ")
    );
}
