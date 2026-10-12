//! Feature-gated codegen driver — the mechanical proof of charter law #2.
//!
//! Capnp triad (verified at build time, recorded in `OUT_DIR/triad.txt`):
//!
//! | slot     | version | where                                    |
//! |----------|---------|------------------------------------------|
//! | runtime  | 0.27.x  | `capnp` dependency (contract's line)     |
//! | codegen  | 0.27.0  | `capnpc` build-dependency                |
//! | binary   | 1.4.0   | system `capnp` (override with `$CAPNP`)  |
//!
//! `capnpc` drives the system binary (`capnp compile -o-`) and parses the code
//! generator request it writes to stdout, then generates Rust in-process. The
//! binary is therefore version-coupled through the request format; if a system
//! binary is incompatible the build fails here with the compiler's own error,
//! and we do NOT silently downgrade the pin.
//!
//! Schema source (read-only input, never modified):
//!
//!   1. `$SMART_SCHEMA_DIR` if set (hermetic / vendored builds);
//!   2. otherwise `<manifest dir>/../smart-protocol-official/schema` (this
//!      workspace's layout), falling back to the documented absolute path
//!      `/home/lufuno/smart/smart-protocol-official/schema`.
//!
//! Every schema file read is declared with `cargo::rerun-if-changed`, and the
//! post-processed output plus the generated module tree are written to
//! `OUT_DIR`, so `target/` is the acceptance-test evidence for the feature law.
//!
//! What is compiled: exactly the union of
//!
//!   * foundation (types, capability, abi + their imports),
//!   * the enabled `domain-*` features' schemas,
//!   * plus those schemas' transitive imports.
//!
//! Nothing else: `cargo build --no-default-features` compiles the three foundation
//! schemas and nothing more.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "src/codegen/mod.rs"]
mod codegen;

use codegen::modules::{DomainModule, SCHEMA_MODULE};

/// Foundation schemas: never feature-gated, always compiled. Their own imports
/// (`types.capnp` only) are pulled in by `schema_set`.
const FOUNDATION: [&str; 3] = ["abi.capnp", "capability.capnp", "types.capnp"];

/// The triad as declared in `Cargo.toml`. One pair of constants so the versions
/// cannot drift between the dependency line, the failure message and
/// `triad.txt`.
const CAPNP_VERSION: &str = "0.27.2";
const CAPNPC_VERSION: &str = "0.27.0";

/// Name of the build.rs-generated module tree included by `src/lib.rs`.
const MODULES_FILE: &str = "smart_lib_modules.rs";

/// The raw CodeGeneratorRequest capnpc consumes, kept in OUT_DIR: the single
/// artifact both codegen and conformance read (see
/// `src/codegen/inventory.rs`). Exposed at compile time as
/// `SMART_LIB_CODEGEN_REQUEST`.
const CODEGEN_REQUEST_FILE: &str = "codegeneratorrequest.bin";

/// The text index derived from that request (domain -> module, interface,
/// method, ordinal, shape flags). Exposed at compile time as
/// `SMART_LIB_CODEGEN_INVENTORY`.
const CODEGEN_INVENTORY_FILE: &str = "codegen-inventory.txt";

fn main() {
    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env("OUT_DIR"));
    let schema_dir = schema_dir(&manifest_dir);

    // ── the feature law, mechanically ────────────────────────────────────
    let enabled_domains = enabled_domains();
    let files = schema_set(&schema_dir, &enabled_domains);

    for file in &files {
        println!(
            "cargo::rerun-if-changed={}",
            schema_dir.join(file).display()
        );
    }
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/codegen/mod.rs");
    // Directories whose contents change module wiring. Only declared when they
    // exist: a rerun-if-changed on a missing path makes cargo consider the
    // build script dirty on every build.
    for dir in ["src/domains", "src/core", "src/transport", "src/platforms", "src/conformance"] {
        if manifest_dir.join(dir).is_dir() {
            println!("cargo::rerun-if-changed={dir}");
        }
    }
    println!("cargo::rerun-if-env-changed=SMART_SCHEMA_DIR");
    println!("cargo::rerun-if-env-changed=CAPNP");

    // ── codegen: compile exactly that union, then fix the output up ──────
    let binary = std::env::var("CAPNP").unwrap_or_else(|_| "capnp".to_string());
    let triad = compile(&binary, &schema_dir, &out_dir, &files);
    codegen::post_process_dir(&out_dir);
    write_modules(&out_dir, &manifest_dir, &files, &enabled_domains, &triad);
    emit_inventory(&out_dir, &files, &enabled_domains, &triad);
}

/// Read back the raw CodeGeneratorRequest capnpc just consumed, turn it into the
/// text inventory, and expose both artifacts to the crate at compile time.
fn emit_inventory(
    out_dir: &Path,
    files: &[String],
    enabled_domains: &[String],
    triad: &Triad,
) {
    let request_path = out_dir.join(CODEGEN_REQUEST_FILE);
    let bytes = std::fs::read(&request_path).unwrap_or_else(|e| {
        panic!(
            "build.rs: cannot read the CodeGeneratorRequest at {}: {e}",
            request_path.display()
        )
    });
    let message = capnp::serialize::read_message(
        &mut &bytes[..],
        capnp::message::ReaderOptions::new(),
    )
    .unwrap_or_else(|e| panic!("build.rs: cannot decode the CodeGeneratorRequest: {e}"));
    let request: capnp::schema_capnp::code_generator_request::Reader<'_> = message
        .get_root()
        .unwrap_or_else(|e| panic!("build.rs: no root in the CodeGeneratorRequest: {e}"));

    // L0-grade identity: the request must name exactly the files we compiled —
    // never more (a stale import closure), never fewer (a silently dropped
    // domain).
    assert_requested_files_match(&request, files);

    let schemas: Vec<codegen::inventory::SchemaFile> = files
        .iter()
        .map(|file| {
            let stem = codegen::modules::schema_file_to_stem(file);
            let (kind, feature) = if file_is_foundation(file) {
                (codegen::inventory::FileKind::Foundation, None)
            } else {
                (
                    codegen::inventory::FileKind::Domain,
                    Some(codegen::modules::domain_feature_name(stem)),
                )
            };
            codegen::inventory::SchemaFile {
                file: file.clone(),
                module: codegen::modules::schema_module_name(file),
                kind,
                feature,
            }
        })
        .collect();

    let inventory = codegen::inventory::render(&request, &schemas, &triad.one_line());
    let inventory_path = out_dir.join(CODEGEN_INVENTORY_FILE);
    std::fs::write(&inventory_path, inventory).unwrap_or_else(|e| {
        panic!(
            "build.rs: cannot write {}: {e}",
            inventory_path.display()
        )
    });

    // Both artifacts are reachable from the crate at compile time, for the
    // conformance wave and for tests.
    println!(
        "cargo::rustc-env=SMART_LIB_CODEGEN_REQUEST={}",
        request_path.display()
    );
    println!(
        "cargo::rustc-env=SMART_LIB_CODEGEN_INVENTORY={}",
        inventory_path.display()
    );
    // `enabled_domains` carries file names (`files.capnp`); the warning and the
    // inventory both want feature names.
    let enabled = if enabled_domains.is_empty() {
        "<none>".to_string()
    } else {
        enabled_domains
            .iter()
            .map(|file| {
                codegen::modules::domain_feature_name(codegen::modules::schema_file_to_stem(file))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!(
        "cargo::warning=smart-lib: {} inventory written for features [{}]",
        CODEGEN_INVENTORY_FILE, enabled
    );
}

/// True for the always-compiled foundation files.
fn file_is_foundation(file: &str) -> bool {
    FOUNDATION.contains(&file)
}

/// The request's requested-file set must equal the compiled set, as multisets.
fn assert_requested_files_match(
    request: &capnp::schema_capnp::code_generator_request::Reader<'_>,
    files: &[String],
) {
    let mut requested: Vec<String> = Vec::new();
    if let Ok(list) = request.get_requested_files() {
        for file in list.iter() {
            let name = file
                .get_filename()
                .map(|t| String::from_utf8_lossy(t.as_bytes()).into_owned())
                .unwrap_or_default();
            requested.push(name);
        }
    }
    requested.sort();
    let mut compiled: Vec<String> = files.to_vec();
    compiled.sort();
    if requested != compiled {
        panic!(
            "build.rs: the CodeGeneratorRequest does not describe the compiled set.\n\
             compiled ({}): {}\n\
             requested by the compiler ({}): {}\n\
             This is an L0 identity failure: codegen and conformance would be \
             reading different artifacts.",
            compiled.len(),
            compiled.join(" "),
            requested.len(),
            requested.join(" ")
        );
    }
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|e| panic!("build.rs: {key} not set: {e}"))
}

/// Resolve the schema directory. Candidate order:
///
/// 1. `$SMART_SCHEMA_DIR` (hermetic / vendored build);
/// 2. `../smart-protocol-official/schema` next to this workspace — the layout
///    this workspace lives in, which keeps the build portable;
/// 3. the documented absolute path of the canonical tree.
fn schema_dir(manifest_dir: &Path) -> PathBuf {
    let candidates = match std::env::var_os("SMART_SCHEMA_DIR") {
        Some(dir) => vec![PathBuf::from(dir)],
        None => vec![
            manifest_dir.join("../smart-protocol-official/schema"),
            PathBuf::from("/home/lufuno/smart/smart-protocol-official/schema"),
        ],
    };
    for candidate in candidates {
        if let Ok(dir) = candidate.canonicalize() {
            if dir.is_dir() {
                return dir;
            }
        }
    }
    panic!(
        "build.rs: cannot find the schema tree (tried: \
         $SMART_SCHEMA_DIR, <manifest>/../smart-protocol-official/schema, \
         /home/lufuno/smart/smart-protocol-official/schema)."
    );
}

/// The schema files of the enabled domain features, derived from
/// `CARGO_FEATURE_*`.
///
/// `domain-files` -> `files.capnp`; dashes become underscores, which
/// is unambiguous over the contract because no schema filename contains a
/// dash. This is what lets a new domain be added with a single manifest line.
fn enabled_domains() -> Vec<String> {
    let mut stems: Vec<String> = Vec::new();
    for (key, value) in std::env::vars() {
        if value != "1" {
            continue;
        }
        let Some(flag) = key.strip_prefix("CARGO_FEATURE_") else {
            continue;
        };
        // cargo uppercases feature names and maps `-` -> `_`; undo both.
        let feature = flag.to_lowercase().replace('_', "-");
        if let Some(stem) = codegen::modules::domain_feature_to_schema_file(&feature) {
            stems.push(stem);
        }
    }
    stems.sort();
    stems.dedup();
    stems
}

/// The exact set of schema files to compile: foundation + enabled domains +
/// transitive imports. Cycles (kernel.capnp <-> boot.capnp) and diamonds are
/// handled; a missing file is a hard error naming the requester.
fn schema_set(schema_dir: &Path, enabled_domains: &[String]) -> Vec<String> {
    let mut queue: Vec<String> = FOUNDATION.iter().map(|f| f.to_string()).collect();
    queue.extend(enabled_domains.iter().cloned());
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut missing: Vec<String> = Vec::new();

    while let Some(file) = queue.pop() {
        if !found.insert(file.clone()) {
            continue;
        }
        assert!(
            !file.contains('/') && !file.contains("..") && file.ends_with(".capnp"),
            "build.rs: refusing to compile schema path `{file}` outside the schema dir"
        );
        let path = schema_dir.join(&file);
        if !path.is_file() {
            missing.push(file);
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("build.rs: cannot read {}: {e}", path.display()));
        for import in codegen::imported_schema_files(&source) {
            assert!(
                !import.contains('/') && !import.contains(".."),
                "build.rs: {} imports `{import}`, which escapes the schema dir",
                path.display()
            );
            if !found.contains(&import) {
                queue.push(import);
            }
        }
    }

    if !missing.is_empty() {
        panic!(
            "build.rs: schema file(s) not found in {}: {}\n\
             A schema imports a file that does not exist (deleted during the \
             revamp), or a `domain-*` feature has no matching schema.",
            schema_dir.display(),
            missing.join(", ")
        );
    }
    found.into_iter().collect()
}

/// The capnp triad, as strings, for the report and the generated header.
struct Triad {
    binary: String,
    binary_version: String,
    codegen: String,
    runtime: String,
}

impl Triad {
    /// One deterministic line for the inventory header.
    fn one_line(&self) -> String {
        format!(
            "binary {} | codegen {} | runtime {}",
            self.binary_version, self.codegen, self.runtime
        )
    }
}

/// Run the system `capnp` binary over exactly `files`, generating Rust into
/// `out_dir`, and return the recorded triad.
fn compile(binary: &str, schema_dir: &Path, out_dir: &Path, files: &[String]) -> Triad {
    let binary_version = capnp_version(binary);

    let mut command = capnpc::CompilerCommand::new();
    command
        .capnp_executable(binary)
        .src_prefix(schema_dir)
        .import_path(schema_dir)
        .default_parent_module([SCHEMA_MODULE.to_string()].to_vec())
        .output_path(out_dir)
        // Keep the request itself: the inventory and the conformance wave read
        // the one artifact capnpc consumed, never a second parse.
        .raw_code_generator_request_path(out_dir.join(CODEGEN_REQUEST_FILE));
    for file in files {
        command.file(schema_dir.join(file));
    }
    command.run().unwrap_or_else(|e| {
        panic!(
            "build.rs: capnp compile failed (binary: {binary} {binary_version}; \
             codegen: capnpc {CAPNPC_VERSION}; runtime: capnp {CAPNP_VERSION}) \
             over {} schema file(s) in {}:\n{e:?}",
            files.len(),
            schema_dir.display()
        )
    });

    let triad = Triad {
        binary: binary.to_string(),
        binary_version,
        codegen: format!("capnpc {CAPNPC_VERSION}"),
        runtime: format!("capnp {CAPNP_VERSION}"),
    };
    std::fs::write(
        out_dir.join("triad.txt"),
        format!(
            "binary     : {} {}\ncodegen    : {}\nruntime    : {}\n\
             schema dir : {}\nschemas    : {}\n",
            triad.binary, triad.binary_version, triad.codegen, triad.runtime,
            schema_dir.display(),
            files.join(" ")
        ),
    )
    .unwrap_or_else(|e| panic!("build.rs: cannot write triad.txt: {e}"));
    triad
}

/// Ask the binary for its version (capnpc does the same check internally).
fn capnp_version(binary: &str) -> String {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "build.rs: failed to execute `{binary} --version`: {e}\n\
                 capnp 1.4.0 is the installed binary; set $CAPNP to override."
            )
        });
    if !output.status.success() {
        panic!(
            "build.rs: `{binary} --version` exited {:?}",
            output.status
        );
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Emit `OUT_DIR/smart_lib_modules.rs`: the module tree `src/lib.rs` includes.
///
/// This file is the whole reason a domain agent never edits a shared file:
/// enabled domains produce (a) the generated schema module and (b) the
/// declaration of the hand-written `src/domains/<stem>.rs`, and both derive
/// from the same feature name. The tree itself is rendered by
/// `codegen::modules::render_module_tree`, which is unit-tested — see
/// `tests/codegen.rs`.
fn write_modules(
    out_dir: &Path,
    manifest_dir: &Path,
    files: &[String],
    enabled_domains: &[String],
    triad: &Triad,
) {
    // `enabled_domains` holds file names (`files.capnp`); the hand-written
    // half is looked up under the stem.
    let enabled: Vec<DomainModule> = enabled_domains
        .iter()
        .map(|file| {
            let module = DomainModule::from_file(file);
            let stem = codegen::modules::schema_file_to_stem(file);
            match codegen::modules::hand_written_domain_path(manifest_dir, stem) {
                Some(path) => module.with_hand_written(path),
                None => module,
            }
        })
        .collect();

    let features = || -> String {
        if enabled.is_empty() {
            "<none>".to_string()
        } else {
            enabled
                .iter()
                .map(DomainModule::feature)
                .collect::<Vec<_>>()
                .join(", ")
        }
    };

    let mut out = String::new();
    out.push_str(&format!(
        "// @generated by build.rs \u{2014} DO NOT EDIT (regenerated on every build).\n\
         //\n\
         // Triad: {} {} | {} | {}\n\
         // Schemas compiled ({}): {}\n\
         // Enabled domain features: {}\n\
         //\n\
         // To add a domain: `domain-<stem> = []` in Cargo.toml plus your code in\n\
         // src/domains/<stem>.rs. This file regenerates; never edit it.\n",
        triad.binary,
        triad.binary_version,
        triad.codegen,
        triad.runtime,
        files.len(),
        files.join(" "),
        features(),
    ));
    out.push_str(&codegen::modules::render_module_tree(
        files,
        &enabled,
        &wave_modules(manifest_dir),
    ));

    // One warning carries both facts a reader of the build log needs: how much
    // schema was compiled (the feature-law evidence) and which enabled domains
    // still have no hand-written module (the wiring-incomplete signal).
    let mut notes = vec![format!(
        "compiled {} schema file(s) for features [{}]",
        files.len(),
        features()
    )];
    let unwritten: Vec<String> = enabled
        .iter()
        .filter(|m| m.hand_written.is_none())
        .map(|m| format!("src/domains/{}.rs", m.stem))
        .collect();
    if !unwritten.is_empty() {
        notes.push(format!(
            "no hand-written domain module for enabled feature(s): {}",
            unwritten.join(", ")
        ));
    }
    println!("cargo::warning=smart-lib: {}", notes.join("; "));

    let path = out_dir.join(MODULES_FILE);
    std::fs::write(&path, out)
        .unwrap_or_else(|e| panic!("build.rs: cannot write {}: {e}", path.display()));
}

/// The wave-2/3 top-level modules, pre-declared in the generated tree so those
/// waves never edit `src/lib.rs`. Each module flips from a documented empty
/// placeholder to a `#[path]` declaration the moment its owner writes the file
/// (or, for `platforms`, the moment an `src/platforms/<os>.rs` appears).
fn wave_modules(manifest_dir: &Path) -> Vec<codegen::modules::WaveModule> {
    let specs: [(&str, &'static [&'static str]); 5] = [
        (
            "serve",
            &[
                "Placeholder: `src/serve/**` is the service-harness wave's file.",
                "Write `src/serve/mod.rs` (or `src/serve.rs`) and it becomes this module",
                "automatically. The harness turns any domain backend into a capnp",
                "service over the four `Channel` transports; network is the first",
                "served domain, not a special case.",
            ],
        ),
        (
            "core",
            &[
                "Placeholder: `src/core/**` is the wave-2 core agent's file.",
                "Write `src/core/mod.rs` (or `src/core.rs`) and it becomes this module",
                "automatically; no manifest edit and no `src/lib.rs` edit. Charter laws 3",
                "and 6 live here: one object-safe backend trait per domain, `Unavailable`",
                "+ reason as a first-class result, and `caps()` on every backend.",
            ],
        ),
        (
            "transport",
            &[
                "Placeholder: `src/transport/**` is the wave-3 transport agent's file.",
                "Write `src/transport/mod.rs` (or `src/transport.rs`) and it becomes this",
                "module automatically. Charter law 4 lives here: the four `Channel`",
                "transports share one API shape, and `dyn` belongs at that seam only.",
            ],
        ),
        (
            "platforms",
            &[
                "Placeholder: the per-OS backends live here.",
                "`src/platforms/<os>.rs` files are auto-wired by build.rs, and each OS file",
                "declares its own domain cells, so `src/platforms/<os>/<domain>.rs` needs no",
                "build.rs change. Charter ownership: every (os, domain) cell is that cell",
                "agent's file; the per-OS aggregator file is unassigned in the charter table",
                "and is requested in knowledge/reports/06-all-features.md.",
            ],
        ),
        (
            "conformance",
            &[
                "Placeholder: the generated conformance suite (L0-L3) lives here.",
                "`src/conformance/**` is the conformance-implementation wave's file: write",
                "`src/conformance/mod.rs` (or `src/conformance.rs`) and it becomes this",
                "module automatically. L0 reads two OUT_DIR artifacts, both derived from the",
                "same CodeGeneratorRequest as the code under test:",
                "* `SMART_LIB_CODEGEN_INVENTORY` - the text index (domain, module,",
                "  interface, method, ordinal, shape flags);",
                "* `SMART_LIB_CODEGEN_REQUEST` - the raw request, for per-field detail.",
            ],
        ),
    ];

    specs
        .iter()
        .map(|(name, placeholder_doc)| {
            let mut module = match codegen::modules::whole_module_path(manifest_dir, name) {
                Some(file) => codegen::modules::WaveModule::wired(name, file),
                None => codegen::modules::WaveModule::placeholder(name, placeholder_doc),
            };
            if *name == "platforms" {
                module.children = codegen::modules::platform_layers(manifest_dir);
            }
            module
        })
        .collect()
}
