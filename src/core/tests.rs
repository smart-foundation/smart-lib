//! Core's mechanical witnesses. Every assertion here is either a build-time
//! total (an exhaustive `match`) or a runtime round-trip through **real
//! serialized capnp messages** — nothing is asserted from one type to itself
//! without crossing the wire.
//!
//! The suite is `no_std`-clean (core + alloc only; the test harness is the only
//! std in the process): the tiny [`block_on`] drives capnp's local promises
//! with a no-op waker, which is sound because those promises resolve on their
//! first poll.

use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use core::cell::Cell;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use capnp::any_pointer;
use capnp::capability::{FromClientHook, Promise, RemotePromise, Request, Response};
use capnp::private::capability::{ClientHook, PipelineHook, PipelineOp, RequestHook, ResponseHook};
use capnp::MessageSize;

use crate::core::capability::{Acquire, Cap, Interface};
use crate::core::error::{Error, ErrorCode, Result};
use crate::core::values;
use crate::schema::{capability_capnp, types_capnp};

// ────────────────────────────── helpers ──────────────────────────────

/// Serialize a message to bytes.
fn round_trip(message: &capnp::message::Builder<capnp::message::HeapAllocator>) -> Vec<u8> {
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, message).expect("serialize message");
    buf
}

/// Read a message back from bytes.
fn read_back(buf: &[u8]) -> capnp::message::Reader<capnp::serialize::OwnedSegments> {
    capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize message")
}

/// Serialize a message whose root is a `T.Error` built by `build`.
fn error_bytes(build: impl FnOnce(&mut types_capnp::error::Builder<'_>)) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut root = message.init_root::<types_capnp::error::Builder<'_>>();
        build(&mut root);
    }
    round_trip(&message)
}

/// Read a `T.Error` message into the core error type.
fn error_from_bytes(buf: &[u8]) -> Error {
    let reader = read_back(buf);
    let root = reader
        .get_root::<types_capnp::error::Reader<'_>>()
        .expect("root as T.Error");
    Error::from(&root)
}

fn waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &WAKER_VTABLE)
}
fn waker_noop(_: *const ()) {}
static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone, waker_noop, waker_noop, waker_noop);

/// Drive an immediate promise to completion. The promises capnp produces from
/// local hooks resolve on their first poll; a `Pending` here would be a mock
/// bug, and the loop spins loudly instead of passing quietly.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &WAKER_VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => continue,
        }
    }
}

// ───────────────────────── error: totality, both directions ─────────────────────────

/// The contract's declared code set, in schema order
/// (`smart-protocol-official/schema/types.capnp:324-346`). Hard-coded here on
/// purpose: if core ever drifts from the schema, this list fails.
const CONTRACT_ERROR_CODES: [&str; 20] = [
    "notFound",
    "permissionDenied",
    "failed",
    "temporary",
    "unavailable",
    "limitExceeded",
    "outOfMemory",
    "invalidArgument",
    "timeout",
    "cancelled",
    "alreadyExists",
    "notSupported",
    "conflict",
    "unknown",
    "hardwareFault",
    "driverError",
    "protocolViolation",
    "busError",
    "thermalThrottling",
    "powerStateFailure",
];

#[test]
fn error_code_covers_exactly_the_contracts_20_codes() {
    assert_eq!(ErrorCode::ALL.len(), 20);
    for (index, code) in ErrorCode::ALL.iter().enumerate() {
        // schema order, no gaps, no duplicates
        assert_eq!(code.ordinal() as usize, index, "ordinal gap at {index}");
        assert_eq!(code.name(), CONTRACT_ERROR_CODES[index]);
        // total both directions against the generated enum
        let generated: types_capnp::ErrorCode = (*code).into();
        assert_eq!(generated as u16, index as u16);
        let back: ErrorCode = generated.into();
        assert_eq!(&back, code);
    }
}

#[test]
fn error_round_trips_every_variant_through_the_wire() {
    // one witness per code, each with a distinct message, context and
    // retryable value (both true and false are exercised across the 20)
    for (index, code) in ErrorCode::ALL.iter().enumerate() {
        let original = Error::with_message(*code, format!("reason {index}"))
            .with_context(format!("while doing step {index}"))
            .with_retryable(index % 2 == 0);

        // direction 1: core -> T.Error builder -> bytes -> core
        let buf = error_bytes(|b| original.write_to(b));
        let back = error_from_bytes(&buf);
        assert_eq!(back, original, "core -> wire -> core failed for {code:?}");

        // direction 2: bytes -> core -> bytes must be byte-identical
        let buf2 = error_bytes(|b| back.write_to(b));
        assert_eq!(buf, buf2, "wire -> core -> wire changed the message for {code:?}");
    }
}

#[test]
fn unknown_error_discriminant_is_preserved_not_defaulted() {
    // Write a real T.Error (code timeout @8), then patch the code field to a
    // discriminant the compiled schema does not declare — the forward-compat
    // case a peer on a newer contract version produces. The offset is derived
    // from capnp's framing (segment word count, then root struct pointer, then
    // the struct's data word) and proven by the pre-patch assertion, not
    // assumed.
    let buf = error_bytes(|b| b.set_code(types_capnp::ErrorCode::Timeout));
    // The stream is an empty first segment plus the 5-word data segment; the
    // root struct pointer precedes the struct's data word, so the code lives
    // at byte 16. The pre-patch scan *proves* the offset instead of assuming
    // it: exactly one occurrence of the written code, found by search.
    let needle = 8u16.to_le_bytes();
    let hits: Vec<usize> = buf
        .windows(2)
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits, vec![16], "code offset must be the struct's first data field");

    let mut patched = buf.clone();
    patched[16..18].copy_from_slice(&42u16.to_le_bytes());
    let err = error_from_bytes(&patched);

    assert_eq!(err.code, ErrorCode::Unknown);
    assert_ne!(err.code, ErrorCode::Timeout, "the patched-away code must not survive");
    assert!(
        err.message.contains("42"),
        "the raw discriminant must survive in the message, got: {}",
        err.message
    );
    assert!(!err.retryable, "an uninterpretable error is not retryable");
}

#[test]
fn retryable_defaults_are_honest() {
    assert!(Error::temporary("transient").retryable);
    for code in [
        ErrorCode::NotFound,
        ErrorCode::PermissionDenied,
        ErrorCode::Failed,
        ErrorCode::InvalidArgument,
        ErrorCode::NotSupported,
        ErrorCode::AlreadyExists,
        ErrorCode::Conflict,
        ErrorCode::Unknown,
    ] {
        assert!(
            !Error::new(code).retryable,
            "{code} must not default to retryable"
        );
    }
}

#[test]
fn constructors_carry_the_codes_and_reasons_backends_need() {
    assert_eq!(Error::not_found().code, ErrorCode::NotFound);
    assert_eq!(Error::permission_denied().code, ErrorCode::PermissionDenied);
    assert_eq!(Error::failed("disk").code, ErrorCode::Failed);
    assert_eq!(Error::out_of_memory().code, ErrorCode::OutOfMemory);
    assert_eq!(Error::timeout().code, ErrorCode::Timeout);
    assert_eq!(Error::cancelled().code, ErrorCode::Cancelled);
    assert_eq!(Error::already_exists().code, ErrorCode::AlreadyExists);
    assert_eq!(Error::conflict().code, ErrorCode::Conflict);
    assert_eq!(Error::thermal_throttling().code, ErrorCode::ThermalThrottling);
    assert_eq!(Error::power_state_failure().code, ErrorCode::PowerStateFailure);

    // law #6: the two honest-degradation codes REQUIRE a reason
    let unavailable = Error::unavailable("no memfd_create on this kernel");
    assert_eq!(unavailable.code, ErrorCode::Unavailable);
    assert!(!unavailable.message.is_empty());
    let not_supported = Error::not_supported("not-applicable: no loudness limiter API");
    assert_eq!(not_supported.code, ErrorCode::NotSupported);
    assert!(!not_supported.message.is_empty());

    // context and explicit retryability survive to the wire
    let e = Error::failed("boom").with_context("opening /x").with_retryable(true);
    let buf = error_bytes(|b| e.write_to(b));
    assert_eq!(error_from_bytes(&buf), e);
    assert_eq!(e.to_string(), "failed: boom (while opening /x) [retryable]");
}

#[test]
fn capnp_runtime_errors_classify_without_inventing_facts() {
    let e = Error::from(capnp::Error {
        kind: capnp::ErrorKind::Unimplemented,
        extra: "method files::readFile not implemented".into(),
    });
    assert_eq!(e.code, ErrorCode::NotSupported);
    assert_eq!(e.message, "method files::readFile not implemented");

    let e = Error::from(capnp::Error {
        kind: capnp::ErrorKind::Disconnected,
        extra: "connection lost".into(),
    });
    assert_eq!(e.code, ErrorCode::Temporary);
    assert!(e.retryable);

    let e = Error::from(capnp::Error {
        kind: capnp::ErrorKind::Overloaded,
        extra: "queue full".into(),
    });
    assert!(e.retryable);

    let e = Error::from(capnp::Error {
        kind: capnp::ErrorKind::Failed,
        extra: "misc".into(),
    });
    assert_eq!(e.code, ErrorCode::Failed);
    assert!(!e.retryable);
}

#[test]
fn result_alias_is_the_crate_wide_type() {
    fn uses_alias() -> Result<()> {
        Ok(())
    }
    fn propagates(r: Result<u32>) -> Result<u32> {
        let v = r?;
        Ok(v + 1)
    }
    assert!(uses_alias().is_ok());
    assert_eq!(propagates(Ok(1)), Ok(2));
    assert!(propagates(Err(Error::not_found())).is_err());
}

// ───────────────────────── ids: 157 wire round-trips ─────────────────────────

/// One witness per identity newtype: raw integer, generated builder,
/// serialized bytes, generated reader, and both `From` directions.
macro_rules! id_round_trips {
    ($test:ident, $name:ident, $module:ident, $getter:ident, $setter:ident, $int:ty, $raw:expr) => {
        #[test]
        fn $test() {
            use crate::core::ids::$name;

            let raw: $int = $raw;
            let id = $name::new(raw);
            assert_eq!(id.get(), raw);
            assert_eq!(<$int>::from(id), raw);
            assert_eq!($name::from(raw), id);

            // core -> generated builder -> bytes -> generated reader -> core
            let mut message = capnp::message::Builder::new_default();
            {
                let mut builder = message.init_root::<types_capnp::$module::Builder<'_>>();
                id.write_to(&mut builder);
                // the builder-side read path
                assert_eq!($name::from(&builder), id);
            }
            let buf = round_trip(&message);
            let reader = read_back(&buf);
            let root = reader
                .get_root::<types_capnp::$module::Reader<'_>>()
                .expect("root as identity struct");
            assert_eq!($name::from_reader(&root), id);
            assert_eq!($name::from(&root), id);

            // bytes written from a raw integer read back through core
            let mut message2 = capnp::message::Builder::new_default();
            {
                let mut builder = message2.init_root::<types_capnp::$module::Builder<'_>>();
                builder.$setter(raw);
            }
            let buf2 = round_trip(&message2);
            let reader2 = read_back(&buf2);
            let root2 = reader2
                .get_root::<types_capnp::$module::Reader<'_>>()
                .expect("root as identity struct");
            assert_eq!($name::from_reader(&root2), id);
            assert_eq!(root2.$getter(), raw);
        }
    };
}

// ─── one round-trip witness per identity newtype (157 tests) ───
id_round_trips!(node_id_round_trips, NodeId, node_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(session_id_round_trips, SessionId, session_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(surface_id_round_trips, SurfaceId, surface_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(buffer_id_round_trips, BufferId, buffer_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(output_id_round_trips, OutputId, output_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(window_id_round_trips, WindowId, window_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(workspace_id_round_trips, WorkspaceId, workspace_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(client_id_round_trips, ClientId, client_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(capture_id_round_trips, CaptureId, capture_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(ime_context_id_round_trips, ImeContextId, ime_context_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(access_node_id_round_trips, AccessNodeId, access_node_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(drag_session_id_round_trips, DragSessionId, drag_session_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(inhibit_handle_round_trips, InhibitHandle, inhibit_handle, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(notification_id_round_trips, NotificationId, notification_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(clipboard_id_round_trips, ClipboardId, clipboard_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(event_id_round_trips, EventId, event_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(package_id_round_trips, PackageId, package_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(service_id_round_trips, ServiceId, service_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(job_id_round_trips, JobId, job_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(timer_id_round_trips, TimerId, timer_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(print_job_id_round_trips, PrintJobId, print_job_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(process_id_round_trips, ProcessId, process_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(file_id_round_trips, FileId, file_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(search_provider_id_round_trips, SearchProviderId, search_provider_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(setting_key_round_trips, SettingKey, setting_key, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(app_id_round_trips, AppId, app_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(backup_id_round_trips, BackupId, backup_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(crash_dump_id_round_trips, CrashDumpId, crash_dump_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(process_group_id_round_trips, ProcessGroupId, process_group_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(breakpoint_id_round_trips, BreakpointId, breakpoint_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(biometric_id_round_trips, BiometricId, biometric_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(auth_method_id_round_trips, AuthMethodId, auth_method_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(auth_token_id_round_trips, AuthTokenId, auth_token_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(permission_id_round_trips, PermissionId, permission_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(boot_stage_id_round_trips, BootStageId, boot_stage_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(boot_entry_id_round_trips, BootEntryId, boot_entry_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(device_address_round_trips, DeviceAddress, device_address, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(upgrade_id_round_trips, UpgradeId, upgrade_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(contact_id_round_trips, ContactId, contact_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(group_id_round_trips, GroupId, group_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(clipboard_source_id_round_trips, ClipboardSourceId, clipboard_source_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(sensor_id_round_trips, SensorId, sensor_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(speech_voice_id_round_trips, SpeechVoiceId, speech_voice_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(tray_icon_id_round_trips, TrayIconId, tray_icon_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(socket_id_round_trips, SocketId, socket_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(certificate_id_round_trips, CertificateId, certificate_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(access_id_round_trips, AccessId, access_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(access_backend_id_round_trips, AccessBackendId, access_backend_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(access_credential_id_round_trips, AccessCredentialId, access_credential_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(mfa_challenge_id_round_trips, MfaChallengeId, mfa_challenge_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(mfa_enrollment_id_round_trips, MfaEnrollmentId, mfa_enrollment_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(smart_card_id_round_trips, SmartCardId, smart_card_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(runtime_instance_id_round_trips, RuntimeInstanceId, runtime_instance_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(framebuffer_id_round_trips, FramebufferId, framebuffer_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(plane_id_round_trips, PlaneId, plane_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(crtc_id_round_trips, CrtcId, crtc_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(audio_stream_id_round_trips, AudioStreamId, audio_stream_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(audio_focus_id_round_trips, AudioFocusId, audio_focus_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(audio_device_id_round_trips, AudioDeviceId, audio_device_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(codec_id_round_trips, CodecId, codec_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(calendar_id_round_trips, CalendarId, calendar_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(reminder_id_round_trips, ReminderId, reminder_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(attendee_id_round_trips, AttendeeId, attendee_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(camera_id_round_trips, CameraId, camera_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(recording_id_round_trips, RecordingId, recording_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(content_id_round_trips, ContentId, content_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(content_subscription_id_round_trips, ContentSubscriptionId, content_subscription_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(database_id_round_trips, DatabaseId, database_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(namespace_id_round_trips, NamespaceId, namespace_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(device_id_round_trips, DeviceId, device_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(extension_id_round_trips, ExtensionId, extension_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(logical_monitor_id_round_trips, LogicalMonitorId, logical_monitor_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(color_profile_id_round_trips, ColorProfileId, color_profile_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(display_mode_id_round_trips, DisplayModeId, display_mode_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(dns_subscription_id_round_trips, DnsSubscriptionId, dns_subscription_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(hal_device_id_round_trips, HalDeviceId, hal_device_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(dma_channel_id_round_trips, DmaChannelId, dma_channel_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(hook_id_round_trips, HookId, hook_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(input_method_id_round_trips, InputMethodId, input_method_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(keyboard_layout_id_round_trips, KeyboardLayoutId, keyboard_layout_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(key_binding_id_round_trips, KeyBindingId, key_binding_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(biometric_template_id_round_trips, BiometricTemplateId, biometric_template_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(gesture_id_round_trips, GestureId, gesture_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(intent_id_round_trips, IntentId, intent_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(activity_id_round_trips, ActivityId, activity_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(service_handle_id_round_trips, ServiceHandleId, service_handle_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(service_name_round_trips, ServiceName, service_name, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(manifest_name_round_trips, ManifestName, manifest_name, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(schema_id_round_trips, SchemaId, schema_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(media_player_id_round_trips, MediaPlayerId, media_player_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(module_id_round_trips, ModuleId, module_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(network_interface_id_round_trips, NetworkInterfaceId, network_interface_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(vpn_profile_id_round_trips, VpnProfileId, vpn_profile_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(vpn_connection_id_round_trips, VpnConnectionId, vpn_connection_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(nfc_tag_id_round_trips, NfcTagId, nfc_tag_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(nfc_service_id_round_trips, NfcServiceId, nfc_service_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(notification_action_id_round_trips, NotificationActionId, notification_action_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(power_profile_id_round_trips, PowerProfileId, power_profile_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(printer_id_round_trips, PrinterId, printer_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(user_id_round_trips, UserId, user_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(thread_id_round_trips, ThreadId, thread_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(queue_id_round_trips, QueueId, queue_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(queue_message_id_round_trips, QueueMessageId, queue_message_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(capability_handle_round_trips, CapabilityHandle, capability_handle, get_handle, set_handle, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(font_id_round_trips, FontId, font_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(file_handle_id_round_trips, FileHandleId, file_handle_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(tag_id_round_trips, TagId, tag_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(subsystem_id_round_trips, SubsystemId, subsystem_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(mapped_region_id_round_trips, MappedRegionId, mapped_region_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(layer_id_round_trips, LayerId, layer_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(email_entry_id_round_trips, EmailEntryId, email_entry_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(phone_entry_id_round_trips, PhoneEntryId, phone_entry_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(address_entry_id_round_trips, AddressEntryId, address_entry_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(sound_id_round_trips, SoundId, sound_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(call_id_round_trips, CallId, call_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(conference_id_round_trips, ConferenceId, conference_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(telephony_message_id_round_trips, TelephonyMessageId, telephony_message_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(theme_id_round_trips, ThemeId, theme_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(secret_namespace_id_round_trips, SecretNamespaceId, secret_namespace_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(secret_key_id_round_trips, SecretKeyId, secret_key_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(media_item_id_round_trips, MediaItemId, media_item_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(location_source_id_round_trips, LocationSourceId, location_source_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(capability_id_round_trips, CapabilityId, capability_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(fence_handle_round_trips, FenceHandle, fence_handle, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(mms_part_id_round_trips, MmsPartId, mms_part_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(task_group_id_round_trips, TaskGroupId, task_group_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(seat_id_round_trips, SeatId, seat_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(task_name_round_trips, TaskName, task_name, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(component_name_round_trips, ComponentName, component_name, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(tunnel_id_round_trips, TunnelId, tunnel_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(alarm_id_round_trips, AlarmId, alarm_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(task_id_round_trips, TaskId, task_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(action_id_round_trips, ActionId, action_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(window_group_id_round_trips, WindowGroupId, window_group_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(snap_zone_id_round_trips, SnapZoneId, snap_zone_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(intent_chain_id_round_trips, IntentChainId, intent_chain_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(intent_trace_id_round_trips, IntentTraceId, intent_trace_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(intent_history_id_round_trips, IntentHistoryId, intent_history_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(key_id_round_trips, KeyId, key_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(endpoint_id_round_trips, EndpointId, endpoint_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(mount_id_round_trips, MountId, mount_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(filesystem_id_round_trips, FilesystemId, filesystem_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(sandbox_id_round_trips, SandboxId, sandbox_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(storage_pool_id_round_trips, StoragePoolId, storage_pool_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(volume_id_round_trips, VolumeId, volume_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(snapshot_id_round_trips, SnapshotId, snapshot_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(subnet_id_round_trips, SubnetId, subnet_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(policy_id_round_trips, PolicyId, policy_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(firmware_id_round_trips, FirmwareId, firmware_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(channel_id_round_trips, ChannelId, channel_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(rule_id_round_trips, RuleId, rule_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(template_id_round_trips, TemplateId, template_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(scope_id_round_trips, ScopeId, scope_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(device_group_id_round_trips, DeviceGroupId, device_group_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(search_bookmark_id_round_trips, SearchBookmarkId, search_bookmark_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);
id_round_trips!(cpu_id_round_trips, CpuId, cpu_id, get_id, set_id, u32, 0x0FED_CBA9_u32);
id_round_trips!(driver_id_round_trips, DriverId, driver_id, get_id, set_id, u64, 0x0123_4567_89AB_CDEF_u64);

// ───────────────────────── values: wire round-trips ─────────────────────────

macro_rules! value_round_trips {
    ($test:ident, $name:path, $module:ident, $value:expr) => {
        #[test]
        fn $test() {
            // a local alias so the path fragment can be used with `::`
            type V = $name;
            let v = $value;
            let mut message = capnp::message::Builder::new_default();
            {
                let mut builder = message.init_root::<types_capnp::$module::Builder<'_>>();
                v.write_to(&mut builder);
                assert_eq!(V::from(&builder.reborrow_as_reader()), v);
            }
            let buf = round_trip(&message);
            let reader = read_back(&buf);
            let root = reader
                .get_root::<types_capnp::$module::Reader<'_>>()
                .expect("root as value struct");
            assert_eq!(V::from_reader(&root), v);
        }
    };
}

value_round_trips!(vec2_round_trips, values::Vec2, vec2, values::Vec2::new(1.5, -2.25));
value_round_trips!(vec2i_round_trips, values::Vec2i, vec2i, values::Vec2i::new(-7, 9));
value_round_trips!(vec3_round_trips, values::Vec3, vec3, values::Vec3::new(1.0, 2.0, 3.0));
value_round_trips!(vec4_round_trips, values::Vec4, vec4, values::Vec4::new(0.25, 0.5, 0.75, 1.0));
value_round_trips!(
    quaternion_round_trips,
    values::Quaternion,
    quaternion,
    values::Quaternion::new(0.0, 0.0, 0.0, 1.0)
);
value_round_trips!(point_round_trips, values::Point, point, values::Point::new(12, 40));
value_round_trips!(point3d_round_trips, values::Point3D, point3_d, values::Point3D::new(1.0, 2.0, 3.0));
value_round_trips!(size_round_trips, values::Size, size, values::Size::new(1920, 1080));
value_round_trips!(
    corner_radius_round_trips,
    values::CornerRadius,
    corner_radius,
    values::CornerRadius::new(1.5, 2.5, 3.5, 4.5)
);
value_round_trips!(
    frame_rate_round_trips,
    values::FrameRate,
    frame_rate,
    values::FrameRate::new(24000, 1001)
);
value_round_trips!(
    rect_round_trips,
    values::Rect,
    rect,
    values::Rect::new(-10, 20, 640, 480, false)
);
value_round_trips!(
    rect_degenerate_round_trips,
    values::Rect,
    rect,
    values::Rect::new(0, 0, 0, 0, true)
);
value_round_trips!(
    region_round_trips,
    values::Region,
    region,
    values::Region {
        rectangles: alloc::vec![
            values::Rect::new(0, 0, 100, 100, false),
            values::Rect::new(100, 0, 50, 100, false),
        ],
    }
);
value_round_trips!(
    spatial_pose_round_trips,
    values::SpatialPose,
    spatial_pose,
    values::SpatialPose {
        position: values::Vec3::new(1.0, 2.0, 3.0),
        rotation: values::Quaternion::new(0.0, 0.0, 0.0, 1.0),
        velocity: values::Vec3::new(0.1, 0.2, 0.3),
        angular_velocity: values::Vec3::new(0.0, 0.0, 0.0),
    }
);
value_round_trips!(
    duration_round_trips,
    values::Duration,
    duration,
    values::Duration::from_nanos(16_666_667)
);
value_round_trips!(
    timestamp_round_trips,
    values::Timestamp,
    timestamp,
    values::Timestamp::from_nanos(1_735_689_600_000_000_000)
);
value_round_trips!(
    monotonic_timestamp_round_trips,
    values::MonotonicTimestamp,
    monotonic_timestamp,
    values::MonotonicTimestamp::from_nanos(987_654_321)
);
value_round_trips!(
    version_round_trips,
    values::Version,
    version,
    {
        let mut v = values::Version::new(0, 27, 2);
        v.epoch = 1;
        v.pre_release = "rc.1".into();
        v.build_metadata = "smart".into();
        v
    }
);

#[test]
fn version_displays_like_the_contract_names_it() {
    let v = values::Version::new(1, 2, 3);
    assert_eq!(v.to_string(), "1.2.3");
    let mut v = v;
    v.epoch = 2;
    v.pre_release = "beta".into();
    assert_eq!(v.to_string(), "2:1.2.3-beta");
}

// ───────────────────────── capability: the handle and its revocation ─────────────────────────

/// A capability whose backing hook can be revoked — the mock of what a
/// capability service's `revoke()` does to the hook behind a held client.
/// Revocation lives in the hook, never in the handle: that is the whole point
/// the test proves.
struct RevocableHook {
    revoked: Rc<Cell<bool>>,
}

impl RevocableHook {
    /// What the capability system answers on a call to a revoked capability.
    fn disconnected() -> capnp::Error {
        capnp::Error {
            kind: capnp::ErrorKind::Disconnected,
            extra: "capability revoked".into(),
        }
    }
}

impl ClientHook for RevocableHook {
    fn add_ref(&self) -> Box<dyn ClientHook> {
        Box::new(RevocableHook {
            revoked: self.revoked.clone(),
        })
    }
    fn new_call(
        &self,
        _interface_id: u64,
        _method_id: u16,
        _size_hint: Option<MessageSize>,
    ) -> Request<any_pointer::Owned, any_pointer::Owned> {
        Request::new(Box::new(RevocableRequest {
            revoked: self.revoked.clone(),
        }))
    }
    fn call(
        &self,
        _interface_id: u64,
        _method_id: u16,
        _params: Box<dyn capnp::private::capability::ParamsHook>,
        _results: Box<dyn capnp::private::capability::ResultsHook>,
    ) -> Promise<(), capnp::Error> {
        if self.revoked.get() {
            Promise::err(Self::disconnected())
        } else {
            Promise::ok(())
        }
    }
    fn get_brand(&self) -> usize {
        0
    }
    fn get_ptr(&self) -> usize {
        0
    }
    fn get_resolved(&self) -> Option<Box<dyn ClientHook>> {
        None
    }
    fn when_more_resolved(&self) -> Option<Promise<Box<dyn ClientHook>, capnp::Error>> {
        None
    }
    fn when_resolved(&self) -> Promise<(), capnp::Error> {
        if self.revoked.get() {
            Promise::err(Self::disconnected())
        } else {
            Promise::ok(())
        }
    }
}

struct RevocableRequest {
    revoked: Rc<Cell<bool>>,
}

impl RequestHook for RevocableRequest {
    fn get(&mut self) -> any_pointer::Builder<'_> {
        // The mock methods take no arguments, so this is never reached; it is
        // here because the trait requires it, not because it is used.
        unimplemented!("mock RevocableRequest: params are never read by these tests")
    }
    fn get_brand(&self) -> usize {
        0
    }
    fn send(self: Box<Self>) -> RemotePromise<any_pointer::Owned> {
        // This is the revocation discovery point: the call is the check.
        let pipeline = any_pointer::Pipeline::new(Box::new(StubPipeline));
        if self.revoked.get() {
            RemotePromise {
                promise: Promise::err(RevocableHook::disconnected()),
                pipeline,
            }
        } else {
            RemotePromise {
                promise: Promise::ok(Response::new(Box::new(StubResponse))),
                pipeline,
            }
        }
    }
    fn send_streaming(self: Box<Self>) -> Promise<(), capnp::Error> {
        if self.revoked.get() {
            Promise::err(RevocableHook::disconnected())
        } else {
            Promise::ok(())
        }
    }
    fn tail_send(
        self: Box<Self>,
    ) -> Option<(
        u32,
        Promise<(), capnp::Error>,
        Box<dyn PipelineHook>,
    )> {
        None
    }
}

struct StubResponse;

impl ResponseHook for StubResponse {
    fn get(&self) -> capnp::Result<any_pointer::Reader<'_>> {
        // The tests assert that the call *resolved*, not what it resolved to.
        unimplemented!("mock StubResponse: results are never read by these tests")
    }
}

struct StubPipeline;

impl PipelineHook for StubPipeline {
    fn add_ref(&self) -> Box<dyn PipelineHook> {
        Box::new(StubPipeline)
    }
    fn get_pipelined_cap(&self, _ops: &[PipelineOp]) -> Box<dyn ClientHook> {
        // Pipelining is never used by these tests.
        unimplemented!("mock StubPipeline: pipelining is never used by these tests")
    }
}

/// A live handle on a not-yet-revoked capability.
fn live_cap(revoked: &Rc<Cell<bool>>) -> Cap<capability_capnp::capability_service::Client> {
    Cap::new(
        capability_capnp::capability_service::Client::new(Box::new(RevocableHook {
            revoked: revoked.clone(),
        })),
    )
}

#[test]
fn cap_is_zero_cost_around_the_client() {
    let revoked = Rc::new(Cell::new(false));
    let cap = live_cap(&revoked);
    // the wrapper adds nothing to the representation
    assert_eq!(
        core::mem::size_of::<Cap<capability_capnp::capability_service::Client>>(),
        core::mem::size_of::<capability_capnp::capability_service::Client>(),
    );
    // deref reaches every generated method; unwrap hands the authority back
    let client = cap.into_client();
    let _: capability_capnp::capability_service::Client = client;
}

#[test]
fn cap_clone_shares_the_underlying_authority() {
    let revoked = Rc::new(Cell::new(false));
    let cap = live_cap(&revoked);
    let cloned = cap.clone();
    assert!(block_on(cap.when_resolved()).is_ok());
    assert!(block_on(cloned.when_resolved()).is_ok());
    // one revocation, seen through both handles — they share the backing
    revoked.set(true);
    assert!(block_on(cap.when_resolved()).is_err());
    assert!(block_on(cloned.when_resolved()).is_err());
}

#[test]
fn cap_revocation_reaches_the_holder_at_the_next_call() {
    let revoked = Rc::new(Cell::new(false));
    let cap = live_cap(&revoked);

    // while held and live, the call goes through the handle (Deref)
    let resolved = block_on(cap.snapshot_request().send().promise);
    assert!(resolved.is_ok(), "a live capability's call must resolve");

    // revoke the backing capability — the handle itself is unchanged
    revoked.set(true);

    // the SAME handle, still held, now fails honestly at the next call:
    // there is no cached validity bit that could go stale
    let outcome = block_on(cap.snapshot_request().send().promise);
    let err = match outcome {
        Err(e) => e,
        Ok(_) => panic!("a revoked capability's call must fail"),
    };
    assert_eq!(err.kind, capnp::ErrorKind::Disconnected);

    // and mapped through the error model, the failure stays honest
    let mapped = Error::from(err);
    assert_eq!(mapped.code, ErrorCode::Temporary);
    assert!(mapped.retryable);
}

/// A backend that refuses acquisition answers `permissionDenied` with a
/// reason — never a handle that fails on first use. The trait is a *hole*:
/// core defines the signature, the platform module supplies the host logic.
struct DenyAll;

impl Acquire<capability_capnp::capability_service::Client> for DenyAll {
    fn acquire(
        &self,
        _presented: &capability_capnp::capability::Reader<'_>,
        target: &str,
    ) -> Result<Cap<capability_capnp::capability_service::Client>> {
        Err(Error::permission_denied().with_context(format!("no authority for {target}")))
    }
}

#[test]
fn acquire_hook_signature_is_implementable_without_host_logic() {
    let gate = DenyAll;
    let mut message = capnp::message::Builder::new_default();
    let mut presented = message.init_root::<capability_capnp::capability::Builder<'_>>();
    presented.set_target("filesystem/path/~/Documents");
    let reader = presented.reborrow_as_reader();

    let refused = gate.acquire(&reader, "filesystem/path/~/Documents/notes.txt");
    let err = refused.expect_err("a refusal must be an error, not a fake handle");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.context.contains("notes.txt"), "the reason names the target");

    // the trait is generic over the interface: a different Cap<T> is a
    // different impl, checked here at the type level
    fn takes_interface_bound<T: Interface>(_: &T) {}
    let client = capability_capnp::capability_service::Client::new(Box::new(RevocableHook {
        revoked: Rc::new(Cell::new(false)),
    }));
    takes_interface_bound(&client);
}
