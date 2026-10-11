//! Unit tests for the build-time codegen helpers (build.rs is not a test
//! target, so the module it includes is exercised here instead).

#[path = "../src/codegen/mod.rs"]
mod codegen;

#[test]
fn import_resolution_ignores_prose() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../smart-protocol-official/schema/usb.capnp"
    ))
    .expect("read the contract's usb.capnp");
    let imports = codegen::imported_schema_files(&source);
    assert!(imports.contains(&"types.capnp".to_string()));
    assert!(imports.contains(&"mount.capnp".to_string()));
    assert!(
        !imports.contains(&"filesystem.capnp".to_string()),
        "deleted filesystem.capnp is only mentioned in prose and must not resolve"
    );
}

#[test]
fn post_processing_is_idempotent_on_generated_files() {
    let out_dir = std::env::temp_dir().join("smart-lib-codegen-test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let with_interface = "pub mod capability_capnp {\n    pub mod _private {}\n    impl FromClientHook for Client { fn new(h: Box<DynClientHook>) -> Self { todo!() } }\n}\n";
    let without_interface = "pub mod types_capnp {\n    pub struct Version {}\n}\n";
    let path_with = out_dir.join("capability_capnp.rs");
    let path_without = out_dir.join("types_capnp.rs");
    std::fs::write(&path_with, with_interface).unwrap();
    std::fs::write(&path_without, without_interface).unwrap();

    codegen::post_process_dir(&out_dir);

    let patched = std::fs::read_to_string(&path_with).unwrap();
    assert!(patched.contains("use alloc::boxed::Box;"));
    assert!(patched.contains("use alloc::string::ToString;"));
    assert!(patched.contains("use alloc::vec::Vec;"));
    // Generated code must stay warning-free, hence the allow on each import.
    assert!(patched.contains("#[allow(unused_imports)]\nuse alloc::boxed::Box;"));
    // Running it twice must not inject twice.
    codegen::post_process_dir(&out_dir);
    let patched_twice = std::fs::read_to_string(&path_with).unwrap();
    assert_eq!(patched, patched_twice);
    // Files without interface code are untouched.
    assert_eq!(
        std::fs::read_to_string(&path_without).unwrap(),
        without_interface
    );
}
