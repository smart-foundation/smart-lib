//! Rendering of the module tree that `build.rs` writes into `OUT_DIR`.
//!
//! Kept as a pure function of its inputs so it can be mechanically tested (see
//! `tests/codegen.rs`) without building the crate: the emitted text is the
//! contract between `build.rs` and `src/lib.rs`, and a mistake in it is exactly
//! the kind of thing that silently stops a domain agent's file from compiling.

use std::path::{Path, PathBuf};

/// Module holding the capnp-generated code (see `crate::schema`).
pub const SCHEMA_MODULE: &str = "schema";

/// Module holding the hand-written per-domain glue written by domain agents.
pub const DOMAINS_MODULE: &str = "domains";

/// Feature prefix that marks a domain feature (`domain-files`).
pub const DOMAIN_PREFIX: &str = "domain-";

/// One of the wave-2/3 top-level modules `src/lib.rs` must never need editing to
/// accommodate. `core`, `transport` and `conformance` are whole modules whose
/// body is one file; `platforms` additionally auto-wires whatever
/// `src/platforms/<os>.rs` files exist (each OS file declares its own domain
/// cells, so `src/platforms/linux/files.rs` needs no build.rs change).
pub struct WaveModule {
    /// Crate-level module name: `core`, `transport`, `platforms`, `conformance`.
    pub name: &'static str,
    /// Doc comment emitted on the placeholder while the owner has not written
    /// the file yet: one entry per line, so the renderer emits `//!` lines
    /// without inheriting source-line indentation.
    pub placeholder_doc: &'static [&'static str],
    /// The hand-written file that becomes this module's body, if it exists yet.
    pub file: Option<PathBuf>,
    /// Auto-wired children (`platforms`), in sorted order.
    pub children: Vec<(String, PathBuf)>,
}

impl WaveModule {
    /// A module that is not written yet: the placeholder branch.
    pub fn placeholder(name: &'static str, placeholder_doc: &'static [&'static str]) -> Self {
        WaveModule {
            name,
            placeholder_doc,
            file: None,
            children: Vec::new(),
        }
    }

    /// A module whose file exists: build.rs emits a `#[path]` declaration, so
    /// writing the file is the whole wiring step.
    pub fn wired(name: &'static str, file: PathBuf) -> Self {
        WaveModule {
            name,
            placeholder_doc: &[],
            file: Some(file),
            children: Vec::new(),
        }
    }

    /// Attach auto-wired children (used by `platforms`).
    pub fn with_children(mut self, children: Vec<(String, PathBuf)>) -> Self {
        self.children = children;
        self
    }
}

/// Where a whole-module body may live: `src/<name>/mod.rs` or `src/<name>.rs`,
/// either layout, first match wins (`mod.rs` so a directory of files can grow).
pub fn whole_module_path(manifest_dir: &Path, name: &str) -> Option<PathBuf> {
    let nested = manifest_dir.join(format!("src/{name}/mod.rs"));
    if nested.is_file() {
        return Some(nested);
    }
    let flat = manifest_dir.join(format!("src/{name}.rs"));
    if flat.is_file() {
        return Some(flat);
    }
    None
}

/// Every `src/platforms/<os>.rs` that exists, sorted by OS token. Directory
/// entries that are not `.rs` files are ignored, so a platform wave may create
/// `src/platforms/linux/` subdirectories without confusing this scan.
pub fn platform_layers(manifest_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut layers: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(manifest_dir.join("src/platforms")) {
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem == "mod" {
                continue; // declared by the module body, not wired as a layer
            }
            layers.push((stem.to_string(), path));
        }
    }
    layers.sort();
    layers
}

/// Feature name for a schema stem: `files` -> `domain-files`.
pub fn domain_feature_name(stem: &str) -> String {
    format!("{DOMAIN_PREFIX}{}", stem.replace('_', "-"))
}

/// Schema file for a feature name: `domain-files` -> `files.capnp`.
///
/// Dashes in the feature name become underscores in the file name, which is
/// unambiguous over the contract because no schema filename contains a dash.
pub fn domain_feature_to_schema_file(feature: &str) -> Option<String> {
    let stem = feature.strip_prefix(DOMAIN_PREFIX)?;
    if stem.is_empty() {
        return None;
    }
    Some(format!("{}.capnp", stem.replace('-', "_")))
}

/// Schema stem for a compiled file: `files.capnp` -> `files`. This is the name
/// of the hand-written module (`src/domains/files.rs`) and of the domain
/// feature modulo dashes (`domain-files`).
pub fn schema_file_to_stem(file: &str) -> &str {
    file.strip_suffix(".capnp").unwrap_or(file)
}

/// Module name capnp codegen uses for `<stem>.capnp`: `<stem>_capnp`.
/// Must stay in lockstep with capnpc's `path_to_stem_string` + `_capnp`, because
/// capnp emits cross-file references as `crate::<parent>::<stem>_capnp::…`.
pub fn schema_module_name(file: &str) -> String {
    format!("{}_capnp", schema_file_to_stem(file).replace('-', "_"))
}

/// Render the module tree `src/lib.rs` includes at its root.
///
/// * `files` — every schema file compiled (already the transitive closure).
/// * `enabled` — one entry per enabled domain feature, carrying where the
///   domain agent's hand-written file lives, if it has been written.
/// * `waves` — the wave-2/3 top-level modules (`core`, `transport`,
///   `platforms`, `conformance`), pre-declared so later waves never edit
///   `src/lib.rs`. Each is either a `#[path]` declaration for the file the
///   owner wrote, or a documented empty placeholder.
pub fn render_module_tree(files: &[String], enabled: &[DomainModule], waves: &[WaveModule]) -> String {
    let mut out = String::with_capacity(files.len() * 80 + enabled.len() * 160);

    out.push_str(&format!("pub mod {SCHEMA_MODULE} {{\n"));
    out.push_str("    #![allow(missing_docs)]\n");
    out.push_str("    #![allow(clippy::all)]\n");
    for file in files {
        let module = schema_module_name(file);
        out.push_str(&format!(
            "    pub mod {module} {{ include!(concat!(env!(\"OUT_DIR\"), \"/{module}.rs\")); }}\n"
        ));
    }
    out.push_str("}\n");

    out.push_str(&format!("\npub mod {DOMAINS_MODULE} {{\n"));
    out.push_str("    #![allow(missing_docs)]\n");
    for module in enabled {
        match &module.hand_written {
            Some(path) => {
                out.push_str(&format!(
                    "    #[path = {:?}]\n    pub mod {};\n",
                    path.display().to_string(),
                    module.stem
                ));
            }
            None => {
                out.push_str(&format!(
                    "    /// `src/domains/{}.rs` not written yet.\n",
                    module.stem
                ));
                out.push_str(&format!("    pub mod {} {{}}\n", module.stem));
            }
        }
    }
    out.push_str("}\n");

    for wave in waves {
        out.push('\n');
        match (&wave.file, wave.children.is_empty()) {
            // File written by the wave's owner: the declaration is the wiring.
            (Some(file), _) => {
                out.push_str(&format!("#[path = {:?}]\n", file.display().to_string()));
                out.push_str(&format!("pub mod {};\n", wave.name));
            }
            // Nothing written, nothing to auto-wire: a documented empty module.
            (None, true) => {
                out.push_str(&format!("pub mod {} {{\n", wave.name));
                for line in wave.placeholder_doc {
                    out.push_str("    //! ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str("    #![allow(missing_docs)]\n");
                out.push_str("}\n");
            }
            // Nothing written, but children exist (platforms with OS layers but
            // no module body): keep the children under a documented parent.
            (None, false) => {
                out.push_str(&format!("pub mod {} {{\n", wave.name));
                for line in wave.placeholder_doc {
                    out.push_str("    //! ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str("    #![allow(missing_docs)]\n");
                for (os, path) in &wave.children {
                    out.push_str(&format!(
                        "    #[path = {:?}]\n    pub mod {};\n",
                        path.display().to_string(),
                        os
                    ));
                }
                out.push_str("}\n");
            }
        }
    }
    out
}

/// One enabled domain feature: the schema stem and where its hand-written glue
/// was expected. `hand_written` is `None` until the domain agent writes the
/// file; build.rs then emits an empty placeholder so the build stays green.
pub struct DomainModule {
    pub stem: String,
    pub hand_written: Option<PathBuf>,
}

impl DomainModule {
    /// From a compiled schema file name (`files.capnp`).
    pub fn from_file(file: &str) -> Self {
        Self::new(schema_file_to_stem(file))
    }

    pub fn new(stem: &str) -> Self {
        DomainModule {
            stem: stem.to_string(),
            hand_written: None,
        }
    }

    pub fn with_hand_written(mut self, path: PathBuf) -> Self {
        self.hand_written = Some(path);
        self
    }

    /// The feature that turns this domain on.
    pub fn feature(&self) -> String {
        domain_feature_name(&self.stem)
    }
}

/// Where a domain agent's hand-written glue may live for `stem`, if anywhere.
/// Two layouts are accepted: `src/domains/<stem>.rs` (the brief's shape) or
/// `src/domains/<stem>/mod.rs` (the same thing once the domain grows a
/// directory of its own).
pub fn hand_written_domain_path(manifest_dir: &Path, stem: &str) -> Option<PathBuf> {
    let flat = manifest_dir.join(format!("src/domains/{stem}.rs"));
    if flat.is_file() {
        return Some(flat);
    }
    let nested = manifest_dir.join(format!("src/domains/{stem}/mod.rs"));
    if nested.is_file() {
        return Some(nested);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> {
        vec![
            "abi.capnp".to_string(),
            "capability.capnp".to_string(),
            "files.capnp".to_string(),
            "types.capnp".to_string(),
        ]
    }

    #[test]
    fn feature_to_schema_round_trips() {
        assert_eq!(
            domain_feature_to_schema_file("domain-files").as_deref(),
            Some("files.capnp")
        );
        assert_eq!(
            domain_feature_to_schema_file("domain-key-management").as_deref(),
            Some("key_management.capnp")
        );
        assert_eq!(
            domain_feature_to_schema_file("domain-system-search").as_deref(),
            Some("system_search.capnp")
        );
        assert!(domain_feature_to_schema_file("std").is_none());
        assert!(domain_feature_to_schema_file("domain-").is_none());
    }

    #[test]
    fn schema_module_names_match_capnp_convention() {
        assert_eq!(schema_module_name("types.capnp"), "types_capnp");
        assert_eq!(schema_module_name("key_management.capnp"), "key_management_capnp");
    }

    #[test]
    fn tree_emits_path_attribute_for_written_domains() {
        let enabled = vec![DomainModule::new("files")
            .with_hand_written(PathBuf::from("/abs/src/domains/files.rs"))];
        let tree = render_module_tree(&files(), &enabled, &[]);
        assert!(tree.contains(
            "    #[path = \"/abs/src/domains/files.rs\"]\n    pub mod files;\n"
        ));
        // The domain module sits inside `pub mod domains`, not at the crate root.
        let domains_start = tree.find("pub mod domains {").unwrap();
        let path_attr = tree.find("#[path =").unwrap();
        assert!(path_attr > domains_start);
    }

    #[test]
    fn hand_written_path_accepts_both_layouts() {
        let root = std::env::temp_dir().join("smart-lib-domain-path-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/domains/gpu")).unwrap();
        std::fs::create_dir_all(root.join("src/domains/shell")).unwrap();
        std::fs::write(root.join("src/domains/files.rs"), "// domain agent's file\n").unwrap();
        std::fs::write(root.join("src/domains/gpu/mod.rs"), "// domain agent's file\n").unwrap();

        assert!(hand_written_domain_path(&root, "files").unwrap().ends_with("src/domains/files.rs"));
        assert!(
            hand_written_domain_path(&root, "gpu")
                .unwrap()
                .ends_with("src/domains/gpu/mod.rs")
        );
        assert!(hand_written_domain_path(&root, "network").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tree_falls_back_to_placeholder_for_unwritten_domains() {
        let tree = render_module_tree(&files(), &[DomainModule::new("network")], &[]);
        assert!(tree.contains("/// `src/domains/network.rs` not written yet.\n"));
        assert!(tree.contains("    pub mod network {}\n"));
        assert!(!tree.contains("#[path"));
    }

    #[test]
    fn tree_emits_schema_modules_only_for_compiled_files() {
        let tree = render_module_tree(&files(), &[], &[]);
        for module in ["abi_capnp", "capability_capnp", "files_capnp", "types_capnp"] {
            assert!(
                tree.contains(&format!("pub mod {module} {{ include!")),
                "missing {module} in:\n{tree}"
            );
        }
        assert!(!tree.contains("network_capnp"));
        // Inner attributes must precede any item in a module block.
        let head = tree.find("pub mod schema {").unwrap();
        let allow = tree[head..].find("#![allow(missing_docs)]").unwrap();
        let first_mod = tree[head..].find("pub mod abi_capnp").unwrap();
        assert!(allow < first_mod);
    }

    #[test]
    fn domain_modules_derive_from_file_names() {
        let mut files = files();
        files.push("key_management.capnp".to_string());
        let enabled = vec![
            DomainModule::from_file("files.capnp").with_hand_written(PathBuf::from("/x/files.rs")),
            DomainModule::from_file("key_management.capnp"),
        ];
        assert_eq!(
            enabled.iter().map(|m| m.feature()).collect::<Vec<_>>(),
            vec!["domain-files".to_string(), "domain-key-management".to_string()]
        );
        let tree = render_module_tree(&files, &enabled, &[]);
        assert!(tree.contains("    pub mod key_management {}\n"));
        assert!(tree.ends_with("}\n"));
    }

    /// The wave-2/3 skeleton: an unwritten module becomes a documented empty
    /// module (compiles with nothing in it), and writing the file flips it to a
    /// `#[path]` declaration with no shared-file edit.
    #[test]
    fn wave_modules_fall_back_to_documented_placeholders() {
        let tree = render_module_tree(&files(), &[], &[
            WaveModule::placeholder("core", &["Placeholder line one.", "Placeholder line two."]),
            WaveModule::placeholder("conformance", &["Owned by the conformance wave."]),
        ]);
        assert!(!tree.contains("#[path"));
        for module in ["core", "conformance"] {
            assert!(tree.contains(&format!("pub mod {module} {{\n")));
        }
        // Each placeholder line is an inner doc comment, indented by one level.
        assert!(tree.contains("    //! Placeholder line one.\n    //! Placeholder line two.\n"));
        assert!(tree.contains("    #![allow(missing_docs)]\n"));
        assert!(tree.ends_with("}\n"));
    }

    #[test]
    fn wave_modules_flip_to_a_path_attribute_when_the_file_exists() {
        let tree = render_module_tree(&files(), &[], &[
            WaveModule::wired("transport", PathBuf::from("/abs/src/transport/mod.rs")),
        ]);
        assert!(tree.contains(
            "#[path = \"/abs/src/transport/mod.rs\"]\npub mod transport;\n"
        ));
        // A wired module replaces its placeholder entirely: no empty block left over.
        assert!(!tree.contains("pub mod transport {"));
        assert!(!tree.contains("#![allow(missing_docs)]\npub mod transport"));
    }

    #[test]
    fn platforms_auto_wires_os_layers_under_documented_parent() {
        let platforms = WaveModule::placeholder("platforms", &["Per-OS backends live here."])
            .with_children(vec![
                ("linux".to_string(), PathBuf::from("/abs/src/platforms/linux.rs")),
                ("macos".to_string(), PathBuf::from("/abs/src/platforms/macos.rs")),
            ]);
        let tree = render_module_tree(&files(), &[], &[platforms]);
        assert!(tree.contains("pub mod platforms {\n"));
        assert!(tree.contains("    //! Per-OS backends live here.\n"));
        assert!(tree.contains(
            "    #[path = \"/abs/src/platforms/linux.rs\"]\n    pub mod linux;\n"
        ));
        assert!(tree.contains(
            "    #[path = \"/abs/src/platforms/macos.rs\"]\n    pub mod macos;\n"
        ));
    }

    #[test]
    fn whole_module_and_platform_paths_resolve_according_to_the_filesystem() {
        let root = std::env::temp_dir().join("smart-lib-wave-path-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/core")).unwrap();
        std::fs::create_dir_all(root.join("src/platforms")).unwrap();
        std::fs::write(root.join("src/core/mod.rs"), "// wave-2 core\n").unwrap();
        std::fs::write(root.join("src/platforms/linux.rs"), "// linux layer\n").unwrap();

        // mod.rs wins over the flat layout, and the platform scan ignores
        // directories and a `mod.rs` that belongs to the module body itself.
        assert!(whole_module_path(&root, "core").unwrap().ends_with("src/core/mod.rs"));
        assert!(whole_module_path(&root, "transport").is_none());
        assert_eq!(
            platform_layers(&root),
            vec![("linux".to_string(), root.join("src/platforms/linux.rs"))]
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
