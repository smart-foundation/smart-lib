//! Build-time codegen helpers for `smart-lib`.
//!
//! This module is **host-only machinery**: it is compiled into `build.rs`
//! (`#[path = "src/codegen/mod.rs"]`, i.e. relative to `build.rs` at the manifest
//! root) and into `tests/codegen.rs` (`#[path = "../src/codegen/mod.rs"]`), never
//! into the library. That keeps the library core `no_std` + alloc (law #1)
//! while still giving the post-processing logic mechanically verified tests.
//!
//! Three responsibilities, one per file here:
//!
//! 1. **Import resolution** (`imported_schema_files`) — compute the exact set of
//!    schemas to hand to `capnp compile`: foundation + enabled domains + their
//!    transitive imports. Comments are stripped before parsing, because the
//!    contract's prose names *deleted* schemas (`usb.capnp` mentions
//!    `filesystem.capnp` in a comment) and a naive pattern match would resurrect
//!    them and fail the build.
//! 2. **Post-processing** (`post_process_dir`, `inject_alloc_imports`) — capnp
//!    0.27 emits bare `Box<..>`, `Vec` and `.to_string()` in interface code. In a
//!    `no_std` crate those names are not in scope, so we inject
//!    `use alloc::{boxed::Box, string::ToString, vec::Vec};` at the top of every
//!    generated module that contains interface code. `String` is deliberately NOT
//!    imported: capnp can generate enum variants named `String`, which would then
//!    collide.
//! 3. **Module-tree rendering** (`modules.rs`) — the text of
//!    `OUT_DIR/smart_lib_modules.rs`, i.e. the feature-gated module registration
//!    pattern `src/lib.rs` documents.
//! 4. **Codegen inventory** (`inventory.rs`) — the text index of what was
//!    compiled, rendered from the very `CodeGeneratorRequest` that produced the
//!    generated code, so codegen and conformance share one artifact.

use std::path::Path;

pub mod contract;
pub mod inventory;
pub mod modules;

/// Alloc imports injected into capnp generated files that contain interface code.
///
/// - `Box`: required for bare `Box<DynClientHook>` in capability code.
/// - `ToString`: required for `.to_string()` calls in server dispatch stubs.
/// - `Vec`: required for bare `Vec` in generated list code.
///
/// Each import carries `#[allow(unused_imports)]`: the set is injected at every
/// module boundary of a file that contains interface code anywhere, and only a
/// few of those scopes need all three names. The port from the old
/// `build-helpers` left those warnings on (2281 of them across the contract);
/// silencing them keeps `-D warnings` builds possible. `String` is deliberately
/// NOT imported: capnp can generate enum variants named `String`, which would
/// then collide.
pub const ALLOC_IMPORTS: &str = "#[allow(unused_imports)]\nuse alloc::boxed::Box;\n\
                                 #[allow(unused_imports)]\nuse alloc::string::ToString;\n\
                                 #[allow(unused_imports)]\nuse alloc::vec::Vec;\n";

/// Post-process every generated `.rs` file in `dir` (idempotent).
pub fn post_process_dir(dir: &Path) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read codegen output dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        post_process_file(&entry.path());
    }
}

/// Post-process a single generated `.rs` file in place (idempotent).
pub fn post_process_file(path: &Path) {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read generated file {}: {e}", path.display()));
    if let Some(patched) = patch_generated(&content) {
        std::fs::write(path, patched)
            .unwrap_or_else(|e| panic!("cannot write generated file {}: {e}", path.display()));
    }
}

/// Returns `Some(patched)` when the file contains interface code and needs the
/// alloc imports, `None` when it is already handled / not applicable.
fn patch_generated(content: &str) -> Option<String> {
    if content.contains("alloc::boxed::Box") {
        return None; // already injected (or genuinely referenced)
    }
    if !content.contains("FromClientHook") && !content.contains("DynClientHook") {
        return None; // no interface code in this file
    }
    Some(inject_alloc_imports(content))
}

/// Inject `ALLOC_IMPORTS` at the top of every generated module in `source`,
/// after any inner attributes. Pure function: testable without the filesystem.
pub fn inject_alloc_imports(source: &str) -> String {
    let mut result = String::with_capacity(source.len() + ALLOC_IMPORTS.len() * 8);
    let mut in_module = false;
    let mut brace_depth = 0usize;
    let mut waiting_for_inner_attrs = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("pub mod ") && trimmed.ends_with('{') {
            result.push_str(line);
            result.push('\n');
            in_module = true;
            brace_depth = 1;
            waiting_for_inner_attrs = true;
            continue;
        }

        if in_module {
            // Inner attributes (#![...]) must precede any item in a module:
            // emit them before our injected imports.
            if waiting_for_inner_attrs && trimmed.starts_with("#![") {
                result.push_str(line);
                result.push('\n');
                continue;
            }
            // First non-inner-attribute line: this is the injection point.
            if waiting_for_inner_attrs {
                waiting_for_inner_attrs = false;
                result.push_str(ALLOC_IMPORTS);
            }

            brace_depth = brace_depth.saturating_add(trimmed.matches('{').count());
            brace_depth = brace_depth.saturating_sub(trimmed.matches('}').count());
            if brace_depth == 0 {
                in_module = false;
            }
        }

        result.push_str(line);
        result.push('\n');
    }

    result
}

/// Strip capnp comments (`#` to end of line). capnp has no block comments and
/// no escape for `#` inside text literals that is reachable for our purposes:
/// we only ever run this before parsing top-level `import` statements.
pub fn strip_capnp_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let code = match line.find('#') {
            Some(idx) => &line[..idx],
            None => line,
        };
        out.push_str(code);
        out.push('\n');
    }
    out
}

/// Names of the schemas a `.capnp` file imports, in order of appearance.
///
/// Handles both `using X = import "y.capnp";` and `using import "y.capnp".Z;`.
/// Comment lines are stripped first. Names are returned verbatim (relative
/// file names as written in the source).
pub fn imported_schema_files(source: &str) -> Vec<String> {
    let code = strip_capnp_comments(source);
    let mut names = Vec::new();
    for line in code.lines() {
        let mut rest = line;
        while let Some(idx) = rest.find("import") {
            let before = rest[..idx].chars().next_back();
            rest = &rest[idx + "import".len()..];
            // `import` must be a bare keyword, not the tail of an identifier
            // (e.g. `important`), and must be followed by a text literal.
            if before.is_some_and(char::is_alphanumeric) {
                continue;
            }
            let after = rest.trim_start();
            let quoted = match after.strip_prefix('"') {
                Some(q) => q,
                None => continue,
            };
            match quoted.find('"') {
                Some(end) => names.push(quoted[..end].to_string()),
                None => continue,
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape capnp 0.27 emits: a flat list of `pub mod <type> { ... }` blocks
    /// at file scope (verified against `target/**/out/*_capnp.rs`).
    const GENERATED_SHAPE: &str = concat!(
        "// @generated by the capnpc-rust plugin.\n",
        "pub mod capability {\n",
        "    #[derive(Copy, Clone)]\n",
        "    pub struct Owned(());\n",
        "    pub struct Reader<'a> { reader: ::capnp::private::layout::StructReader<'a> }\n",
        "    impl <> ::capnp::capability::FromClientHook for Client {\n",
        "        fn new(hook: Box<::capnp::capability::DynClientHook>) -> Self { todo!() }\n",
        "        fn into_client_hook(self) -> Box<::capnp::capability::DynClientHook> { todo!() }\n",
        "    }\n",
        "}\n",
        "pub mod capability_condition {\n",
        "    pub struct Owned(());\n",
        "}\n",
    );

    #[test]
    fn finds_imports_in_both_forms() {
        let src = concat!(
            "@0xdeadbeef;\n",
            "using T = import \"types.capnp\";\n",
            "using import \"capability.capnp\".Capability;\n",
        );
        assert_eq!(
            imported_schema_files(src),
            vec!["types.capnp".to_string(), "capability.capnp".to_string()]
        );
    }

    #[test]
    fn ignores_imports_inside_comments() {
        // The real usb.capnp contains exactly this prose; filesystem.capnp is
        // deleted and a naive match would make the build fail on it.
        let src = concat!(
            "# usb.capnp imports `filesystem.capnp` here was a standing violation;\n",
            "using T = import \"types.capnp\";\n",
        );
        assert_eq!(imported_schema_files(src), vec!["types.capnp".to_string()]);
    }

    #[test]
    fn does_not_match_identifier_tails() {
        let src = "struct Metadata { important @0 :Text; }\n";
        assert!(imported_schema_files(src).is_empty());
    }

    #[test]
    fn injects_alloc_imports_before_first_item_of_every_module() {
        let patched = inject_alloc_imports(GENERATED_SHAPE);
        assert!(
            patched.contains(
                "pub mod capability {\n#[allow(unused_imports)]\nuse alloc::boxed::Box;\n\
                 #[allow(unused_imports)]\nuse alloc::string::ToString;\n\
                 #[allow(unused_imports)]\nuse alloc::vec::Vec;\n    #[derive(Copy, Clone)]"
            ),
            "got: {patched:?}"
        );
        // Every top-level module of the file is patched.
        assert_eq!(patched.matches("use alloc::boxed::Box;").count(), 2);
        // The import block sits *inside* the module it belongs to.
        assert!(patched.contains("}\npub mod capability_condition {\n#[allow(unused_imports)]"));
    }

    #[test]
    fn injects_after_inner_attributes() {
        let patched = inject_alloc_imports(concat!(
            "pub mod foo {\n",
            "    #![allow(dead_code)]\n",
            "    pub struct Foo {}\n",
            "}\n",
        ));
        assert!(patched.starts_with(
            "pub mod foo {\n    #![allow(dead_code)]\n#[allow(unused_imports)]\n"
        ));
    }

    #[test]
    fn patch_generated_skips_files_without_interface_code() {
        assert!(patch_generated("pub mod x { pub struct S {} }\npub mod y { pub struct T {} }\n").is_none());
    }

    #[test]
    fn patch_generated_skips_already_patched_files() {
        let once = patch_generated(GENERATED_SHAPE).expect("interface code present");
        assert!(once.contains("use alloc::boxed::Box;"));
        assert!(patch_generated(&once).is_none());
    }

    #[test]
    fn string_is_not_imported() {
        assert!(!ALLOC_IMPORTS.lines().any(|l| l == "use alloc::string::String;"));
    }
}
