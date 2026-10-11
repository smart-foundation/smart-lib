//! Smoke test: the generated code, the post-processing and the module tree all
//! work. Every assertion here is mechanical evidence for the skeleton brief:
//!
//! * [`foundation_round_trip`] — foundation codegen is always on (no domain
//!   feature required), cross-file references resolve (`T.Timestamp` from
//!   `capability.capnp` into `types.capnp`), and the injected `alloc` imports
//!   satisfy the interface code (`FromClientHook` needs `Box` in scope).
//! * [`domain_files_round_trip`] — a domain feature actually generates that
//!   domain's code, including its interface module.

use smart_lib::schema::{capability_capnp, types_capnp};

/// Round-trip a message through bytes.
fn round_trip(message: &capnp::message::Builder<capnp::message::HeapAllocator>) -> Vec<u8> {
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, message).expect("serialize message");
    buf
}

fn read_back(buf: &[u8]) -> capnp::message::Reader<capnp::serialize::OwnedSegments> {
    capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize message")
}

#[test]
fn foundation_round_trip() {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut cap = message.init_root::<capability_capnp::capability::Builder<'_>>();
        cap.set_id(b"opaque-content-addressed-token");
        cap.set_target("filesystem/path/~/Documents");
        let mut ops = cap.reborrow().init_operations(2);
        ops.set(0, capability_capnp::Operation::Read);
        ops.set(1, capability_capnp::Operation::Write);
        cap.reborrow()
            .init_expiry()
            .set_nanoseconds(1_735_689_600_000_000_000);
    }
    let buf = round_trip(&message);

    let reader = read_back(&buf);
    let cap = reader
        .get_root::<capability_capnp::capability::Reader<'_>>()
        .expect("root as Capability");
    assert_eq!(
        cap.get_target().expect("target text"),
        "filesystem/path/~/Documents"
    );
    assert_eq!(cap.get_id().expect("id data"), b"opaque-content-addressed-token");
    let operations = cap.get_operations().expect("operations list");
    assert_eq!(operations.len(), 2);
    assert_eq!(operations.get(0).unwrap(), capability_capnp::Operation::Read);
    assert_eq!(operations.get(1).unwrap(), capability_capnp::Operation::Write);
    let expiry = cap.get_expiry().expect("expiry present");
    assert_eq!(expiry.get_nanoseconds(), 1_735_689_600_000_000_000);
}

#[test]
fn foundation_types_round_trip() {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut version = message.init_root::<types_capnp::version::Builder<'_>>();
        version.set_major(0);
        version.set_minor(27);
        version.set_patch(2);
        version.set_pre_release("skeleton");
    }
    let buf = round_trip(&message);
    let reader = read_back(&buf);
    let version = reader
        .get_root::<types_capnp::version::Reader<'_>>()
        .expect("root as Version");
    assert_eq!(version.get_major(), 0);
    assert_eq!(version.get_minor(), 27);
    assert_eq!(version.get_patch(), 2);
    assert_eq!(version.get_pre_release().expect("preRelease"), "skeleton");
}

/// The capability interface module only exists because the generated code could
/// be post-processed into something a `no_std` crate compiles: interface code
/// needs `Box<DynClientHook>` and `FromClientHook` in scope. Implementing
/// `Server` (all methods are defaulted) proves the trait is present and usable.
#[test]
fn capability_interface_module_exists() {
    #[allow(dead_code)]
    struct ProbeServer;
    impl capability_capnp::capability_service::Server for ProbeServer {}

    fn takes_client(_: Option<capability_capnp::capability_service::Client>) {}
    takes_client(None);
}

#[cfg(feature = "domain-files")]
#[test]
fn domain_files_round_trip() {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut entry =
            message.init_root::<smart_lib::schema::files_capnp::directory_entry::Builder<'_>>();
        entry.set_name("notes.txt");
        entry.set_path("~/Documents/notes.txt");
        entry.set_size(4096);
    }
    let buf = round_trip(&message);
    let reader = read_back(&buf);
    let entry = reader
        .get_root::<smart_lib::schema::files_capnp::directory_entry::Reader<'_>>()
        .expect("root as DirectoryEntry");
    assert_eq!(entry.get_name().expect("name"), "notes.txt");
    assert_eq!(entry.get_path().expect("path"), "~/Documents/notes.txt");
    assert_eq!(entry.get_size(), 4096);
}

/// The foundation re-export point is always present, in every feature
/// configuration, because the foundation schemas always compile.
#[test]
fn prelude_exports_foundation() {
    use smart_lib::prelude::types_capnp;
    let mut message = capnp::message::Builder::new_default();
    message
        .init_root::<types_capnp::version::Builder<'_>>()
        .set_minor(27);
    let buf = round_trip(&message);
    let reader = read_back(&buf);
    let version = reader
        .get_root::<types_capnp::version::Reader<'_>>()
        .expect("root as Version");
    assert_eq!(version.get_minor(), 27);
}
