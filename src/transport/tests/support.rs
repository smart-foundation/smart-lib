//! Shared test helpers: real contract messages and capability payloads.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::types_capnp;
use crate::transport::channel::CapHandoff;

/// A `types.capnp` `Version` message: the contract type the transport tests
/// round-trip (foundation, always compiled).
pub fn version_message(major: u32, minor: u32, patch: u32) -> capnp::message::Builder<capnp::message::HeapAllocator> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut version = message.init_root::<types_capnp::version::Builder<'_>>();
        version.set_major(major);
        version.set_minor(minor);
        version.set_patch(patch);
        version.set_pre_release("transport-test");
    }
    message
}

/// A message whose root is a `T.DataHandle` with the **inline** arm.
pub fn inline_data_handle_message(bytes: &[u8]) -> capnp::message::Builder<capnp::message::HeapAllocator> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut handle = message.init_root::<types_capnp::data_handle::Builder<'_>>();
        handle.set_inline(bytes);
    }
    message
}

/// A message whose root is a `T.DataHandle` with the **buffer** arm: the
/// zero-copy reference the arena path exists for.
pub fn buffer_data_handle_message(buffer_id: u64) -> capnp::message::Builder<capnp::message::HeapAllocator> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut handle = message.init_root::<types_capnp::data_handle::Builder<'_>>();
        let mut id = handle.reborrow().init_buffer();
        id.set_id(buffer_id);
    }
    message
}

/// A serialized `capability.capnp` `Capability` descriptor: the in-band
/// form a Tcp channel can carry.
pub fn capability_descriptor(target: &str) -> Vec<u8> {
    use crate::schema::capability_capnp;
    let mut message = capnp::message::Builder::new_default();
    {
        let mut cap = message.init_root::<capability_capnp::capability::Builder<'_>>();
        cap.set_id(b"transport-test-token");
        cap.set_target(target);
        let mut ops = cap.reborrow().init_operations(1);
        ops.set(0, capability_capnp::Operation::Read);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize capability");
    buf
}

/// Read the target back out of a descriptor (proves the bytes survived the
/// transport unchanged).
pub fn capability_descriptor_target(descriptor: &[u8]) -> Option<String> {
    use crate::schema::capability_capnp;
    let reader = capnp::serialize::read_message(
        &mut &descriptor[..],
        capnp::message::ReaderOptions::new(),
    )
    .ok()?;
    let cap = reader
        .get_root::<capability_capnp::capability::Reader<'_>>()
        .ok()?;
    cap.get_target().ok().and_then(|t| t.to_str().ok().map(|s| s.to_string()))
}

/// A live in-process capability object (the `CapHandoff::Live` form): a
/// plain value the receiving peer downcasts back.
#[derive(Debug, PartialEq)]
pub struct LiveToken {
    pub name: String,
    pub generation: u32,
}

pub fn live_capability(name: &str) -> CapHandoff {
    CapHandoff::Live(Box::new(LiveToken {
        name: name.to_string(),
        generation: 1,
    }))
}

/// Arena geometry small enough for tests but real (64 KiB slots, 16 slots,
/// 16 ring slots of 64 KiB → a ~1.1 MiB region).
pub fn test_arena_config() -> crate::transport::shmem::ArenaConfig {
    crate::transport::shmem::ArenaConfig {
        slot_bytes: 64 * 1024,
        slot_count: 16,
        ring_slots: 16,
        ring_bytes: 64 * 1024,
    }
}

/// A deterministic 64 KiB payload (large enough that an inline arm would be
/// absurd — exactly what `T.DataHandle`'s buffer arm exists for).
pub fn bulk_payload(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// An `abi.capnp` `AbiHandshakeRequest` builder (the negotiate hook's
/// input). `accept_downgrade` is the one interesting flag; the ABI itself
/// is a minimal, valid declaration.
pub fn handshake_request(
    accept_downgrade: bool,
) -> capnp::message::Builder<capnp::message::HeapAllocator> {
    use crate::schema::abi_capnp;
    let mut message = capnp::message::Builder::new_default();
    {
        let mut request = message.init_root::<abi_capnp::abi_handshake_request::Builder<'_>>();
        let mut abi = request.reborrow().init_caller_abi();
        abi.set_namespace("smart-protocol");
        let mut version = abi.reborrow().init_version();
        version.set_major(0);
        version.set_minor(1);
        version.set_patch(0);
        let mut features = abi.reborrow().init_features(1);
        features.reborrow().get(0).set_name("transport.tcp");
        features.get(0).set_kind("transport");
        request.set_accept_downgrade(accept_downgrade);
        request.reborrow().init_session_id().set_id(1);
    }
    message
}

/// An `AbiHandshakeResponse` with the `matched` arm set (a mock peer's
/// answer to a handshake).
pub fn handshake_response_matched() -> capnp::message::Builder<capnp::message::HeapAllocator> {
    use crate::schema::abi_capnp;
    let mut message = capnp::message::Builder::new_default();
    {
        let mut response = message.init_root::<abi_capnp::abi_handshake_response::Builder<'_>>();
        let mut matched = response.reborrow().init_matched();
        matched.set_namespace("smart-protocol");
        let mut version = matched.reborrow().init_version();
        version.set_major(0);
        version.set_minor(1);
        version.set_patch(0);
    }
    message
}
