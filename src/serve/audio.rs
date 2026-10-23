//! The audio domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `audio.capnp`'s `interface Audio` block (`@0`–`@99`) and its
//! `AudioStateSubscription` / `DevicesSubscription` / `StreamsSubscription`
//! interfaces (`@0`–`@1` each). The dispatch is the generated
//! `ServerDispatch`; there is no ordinal, no method table and no envelope of
//! this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5). The audio domain's
//!   mutating verbs are `setVolume`, `setMute`, `setStreamVolume` and
//!   `setStreamMute`.
//!
//! # The subscription is the contract's own capability
//!
//! `subscribeState @9` hands back an `AudioStateSubscription` reference, and
//! the pull rides it — through the subscription's own `pull` and through the
//! `Audio` interface's `pullState @10`, which takes the same reference. The
//! capability IS the authority; there is no token to present.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::core::ErrorCode;
use crate::domains::audio::availability;
use crate::domains::audio::backend::AudioBackend;
use crate::domains::audio::types::{AudioDevice, AudioState, AudioStream, Page, Pagination, SubscriptionId};
use crate::schema::audio_capnp::audio as aud;
use crate::schema::audio_capnp::audio_state_subscription as state_sub;
use crate::schema::audio_capnp::devices_subscription as devices_sub;
use crate::schema::audio_capnp::streams_subscription as streams_sub;
use crate::schema::types_capnp;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The audio service: the domain's backend, and the class this connection
/// earned.
pub struct AudioService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for AudioService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> AudioService<B>
where
    B: AudioBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

/// The class's answer for a mutating verb on a connection that did not earn
/// the right to change this host's audio state (law #5). The verb is refused
/// *before* the backend is reached: a change that is refused afterwards has
/// already happened.
fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() {
        return None;
    }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!(
            "permissionDenied: the {verb} verb changes this host's audio state, and this \
             connection holds the read-only class"
        ),
    ))
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
///
/// `None` when the matrix says the method IS served here — the arm then goes
/// to the backend. The answer comes BEFORE the class's gate: what the host
/// cannot do, no class can do.
fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not an Audio contract method"),
        )),
    }
}

/// Write the engine's `VoidResult` into the contract's `T.VoidResult`.
fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

/// The contract's pagination as the domain's.
fn pagination(reader: &types_capnp::pagination::Reader<'_>) -> Pagination {
    Pagination {
        offset: reader.get_offset(),
        limit: reader.get_limit(),
    }
}

/// Write the domain's devices into the contract's list.
fn write_devices<'a>(
    devices: &[AudioDevice],
    list: &mut capnp::struct_list::Builder<'a, crate::schema::audio_capnp::audio_device::Owned>,
) {
    for (index, device) in devices.iter().enumerate() {
        let mut entry = list.reborrow().get(index as u32);
        entry.reborrow().init_id().set_id(device.id);
        entry.set_name(&device.name);
        entry.set_description(&device.description);
        entry.set_volume(device.volume);
        entry.set_mute(device.mute);
        entry.set_is_default(device.is_default);
        entry.set_plugged_in(device.plugged_in);
    }
}

/// Write the domain's streams into the contract's list.
fn write_streams<'a>(
    streams: &[AudioStream],
    list: &mut capnp::struct_list::Builder<'a, crate::schema::audio_capnp::audio_stream::Owned>,
) {
    for (index, stream) in streams.iter().enumerate() {
        let mut entry = list.reborrow().get(index as u32);
        entry.reborrow().init_id().set_id(stream.id);
        entry.reborrow().init_device().set_id(stream.device);
        entry.set_name(&stream.name);
        entry.set_volume(stream.volume);
        entry.set_mute(stream.mute);
    }
}

/// The state subscription the service hands back for `subscribeState @9`.
///
/// Holding the reference IS the authority to pull — the contract's own rule.
pub struct AudioStateSubscriptionService<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for AudioStateSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> state_sub::Server for AudioStateSubscriptionService<B>
where
    B: AudioBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: state_sub::PullParams,
        mut results: state_sub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_events();
            let state = backend
                .pull_state(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            state.write_to(&mut builder.init_state());
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: state_sub::CancelParams,
        mut results: state_sub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_state(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The devices subscription the service hands back for `subscribeDevices @12`.
pub struct AudioDevicesSubscriptionService<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for AudioDevicesSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> devices_sub::Server for AudioDevicesSubscriptionService<B>
where
    B: AudioBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: devices_sub::PullParams,
        mut results: devices_sub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_events();
            let devices = backend
                .pull_devices(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_devices(devices.len() as u32);
            write_devices(&devices, &mut list);
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: devices_sub::CancelParams,
        mut results: devices_sub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_devices(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The streams subscription the service hands back for `subscribeStreams @15`.
pub struct AudioStreamsSubscriptionService<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for AudioStreamsSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> streams_sub::Server for AudioStreamsSubscriptionService<B>
where
    B: AudioBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: streams_sub::PullParams,
        mut results: streams_sub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_events();
            let streams = backend
                .pull_streams(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_streams(streams.len() as u32);
            write_streams(&streams, &mut list);
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: streams_sub::CancelParams,
        mut results: streams_sub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_streams(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

impl<B> aud::Server for AudioService<B>
where
    B: AudioBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn get_state(
        self: Rc<Self>,
        _params: aud::GetStateParams,
        mut results: aud::GetStateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getState") {
                return Err(refusal);
            }
            let state: AudioState = backend
                .get_state()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            state.write_to(&mut builder.init_state());
            Ok(())
        }
    }

    fn list_devices(
        self: Rc<Self>,
        params: aud::ListDevicesParams,
        mut results: aud::ListDevicesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("listDevices") {
                return Err(refusal);
            }
            let params = params.get()?;
            let page: Page<AudioDevice> = backend
                .list_devices(pagination(&params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_devices(page.items.len() as u32);
            write_devices(&page.items, &mut list);
            Ok(())
        }
    }

    fn list_streams(
        self: Rc<Self>,
        params: aud::ListStreamsParams,
        mut results: aud::ListStreamsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("listStreams") {
                return Err(refusal);
            }
            let params = params.get()?;
            let page: Page<AudioStream> = backend
                .list_streams(pagination(&params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_streams(page.items.len() as u32);
            write_streams(&page.items, &mut list);
            Ok(())
        }
    }

    // ── The mutating verbs: the class decides, before the backend ────────

    fn set_volume(
        self: Rc<Self>,
        params: aud::SetVolumeParams,
        mut results: aud::SetVolumeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setVolume") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setVolume") {
                return Err(refusal);
            }
            let params = params.get()?;
            let device = crate::domains::audio::types::AudioDeviceId(params.get_device().get_id());
            let volume = params.get_volume();
            let outcome = backend.set_volume(device, volume);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_mute(
        self: Rc<Self>,
        params: aud::SetMuteParams,
        mut results: aud::SetMuteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setMute") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setMute") {
                return Err(refusal);
            }
            let params = params.get()?;
            let device = crate::domains::audio::types::AudioDeviceId(params.get_device().get_id());
            let mute = params.get_mute();
            let outcome = backend.set_mute(device, mute);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_stream_volume(
        self: Rc<Self>,
        params: aud::SetStreamVolumeParams,
        mut results: aud::SetStreamVolumeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setStreamVolume") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setStreamVolume") {
                return Err(refusal);
            }
            let params = params.get()?;
            let stream = crate::domains::audio::types::AudioStreamId(params.get_stream_id().get_id());
            let volume = params.get_volume();
            let outcome = backend.set_stream_volume(stream, volume);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_stream_mute(
        self: Rc<Self>,
        params: aud::SetStreamMuteParams,
        mut results: aud::SetStreamMuteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setStreamMute") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setStreamMute") {
                return Err(refusal);
            }
            let params = params.get()?;
            let stream = crate::domains::audio::types::AudioStreamId(params.get_stream_id().get_id());
            let mute = params.get_mute();
            let outcome = backend.set_stream_mute(stream, mute);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    // ── The subscriptions: the contract's own capabilities ───────────────

    fn subscribe_state(
        self: Rc<Self>,
        _params: aud::SubscribeStateParams,
        mut results: aud::SubscribeStateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("subscribeState") {
                return Err(refusal);
            }
            let subscription = backend
                .subscribe_state()
                .map_err(|error| errors::to_capnp(&error))?;
            let server = AudioStateSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: state_sub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn pull_state(
        self: Rc<Self>,
        params: aud::PullStateParams,
        mut results: aud::PullStateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            if let Some(refusal) = unavailable("pullState") {
                return Err(refusal);
            }
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            // The pull rides the subscription the caller holds: the
            // capability is the handle (law #5).
            let response = subscription.pull_request().send().promise.await?;
            let answer = response.get()?;
            let state = answer.get_state()?;
            results.get().set_state(state)?;
            Ok(())
        }
    }

    fn cancel_state(
        self: Rc<Self>,
        params: aud::CancelStateParams,
        mut results: aud::CancelStateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            if let Some(refusal) = unavailable("cancelState") {
                return Err(refusal);
            }
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            // The cancel rides the same capability: retiring a subscription
            // is the holder's own act.
            let response = subscription.cancel_request().send().promise.await?;
            let answer = response.get()?;
            results.get().set_result(answer.get_result()?);
            Ok(())
        }
    }

    fn subscribe_devices(
        self: Rc<Self>,
        _params: aud::SubscribeDevicesParams,
        mut results: aud::SubscribeDevicesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("subscribeDevices") {
                return Err(refusal);
            }
            let subscription = backend
                .subscribe_devices()
                .map_err(|error| errors::to_capnp(&error))?;
            let server = AudioDevicesSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: devices_sub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn pull_devices(
        self: Rc<Self>,
        params: aud::PullDevicesParams,
        mut results: aud::PullDevicesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            if let Some(refusal) = unavailable("pullDevices") {
                return Err(refusal);
            }
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            let response = subscription.pull_request().send().promise.await?;
            let answer = response.get()?;
            let devices = answer.get_devices()?;
            let builder = results.get();
            let mut list = builder.init_devices(devices.len() as u32);
            for (index, device) in devices.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                entry.reborrow().init_id().set_id(device.get_id().get_id());
                entry.set_name(&super::text_of(device.get_name()));
                entry.set_description(&super::text_of(device.get_description()));
                entry.set_volume(device.get_volume());
                entry.set_mute(device.get_mute());
                entry.set_is_default(device.get_is_default());
                entry.set_plugged_in(device.get_plugged_in());
            }
            Ok(())
        }
    }

    fn cancel_devices(
        self: Rc<Self>,
        params: aud::CancelDevicesParams,
        mut results: aud::CancelDevicesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            if let Some(refusal) = unavailable("cancelDevices") {
                return Err(refusal);
            }
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            let response = subscription.cancel_request().send().promise.await?;
            let answer = response.get()?;
            results.get().set_result(answer.get_result()?);
            Ok(())
        }
    }

    fn subscribe_streams(
        self: Rc<Self>,
        _params: aud::SubscribeStreamsParams,
        mut results: aud::SubscribeStreamsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("subscribeStreams") {
                return Err(refusal);
            }
            let subscription = backend
                .subscribe_streams()
                .map_err(|error| errors::to_capnp(&error))?;
            let server = AudioStreamsSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: streams_sub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn pull_streams(
        self: Rc<Self>,
        params: aud::PullStreamsParams,
        mut results: aud::PullStreamsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            if let Some(refusal) = unavailable("pullStreams") {
                return Err(refusal);
            }
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            let response = subscription.pull_request().send().promise.await?;
            let answer = response.get()?;
            let streams = answer.get_streams()?;
            let builder = results.get();
            let mut list = builder.init_streams(streams.len() as u32);
            for (index, stream) in streams.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                entry.reborrow().init_id().set_id(stream.get_id().get_id());
                entry.reborrow().init_device().set_id(stream.get_device().get_id());
                entry.set_name(&super::text_of(stream.get_name()));
                entry.set_volume(stream.get_volume());
                entry.set_mute(stream.get_mute());
            }
            Ok(())
        }
    }

    fn cancel_streams(
        self: Rc<Self>,
        params: aud::CancelStreamsParams,
        mut results: aud::CancelStreamsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            if let Some(refusal) = unavailable("cancelStreams") {
                return Err(refusal);
            }
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            let response = subscription.cancel_request().send().promise.await?;
            let answer = response.get()?;
            results.get().set_result(answer.get_result()?);
            Ok(())
        }
    }

    // ── The methods this backend does not serve ──────────────────────────
    //
    // One arm per contract method, so the answer is the matrix's answer and
    // never a silent absence. The arm's body is the matrix row, read as
    // data.
    fn enable_device(
        self: Rc<Self>,
        _params: aud::EnableDeviceParams,
        _results: aud::EnableDeviceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("enableDevice")))
    }

    fn disable_device(
        self: Rc<Self>,
        _params: aud::DisableDeviceParams,
        _results: aud::DisableDeviceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("disableDevice")))
    }

    fn request_focus(
        self: Rc<Self>,
        _params: aud::RequestFocusParams,
        _results: aud::RequestFocusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("requestFocus")))
    }

    fn release_focus(
        self: Rc<Self>,
        _params: aud::ReleaseFocusParams,
        _results: aud::ReleaseFocusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("releaseFocus")))
    }

    fn subscribe_focus(
        self: Rc<Self>,
        _params: aud::SubscribeFocusParams,
        _results: aud::SubscribeFocusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeFocus")))
    }

    fn pull_focus(
        self: Rc<Self>,
        _params: aud::PullFocusParams,
        _results: aud::PullFocusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullFocus")))
    }

    fn cancel_focus(
        self: Rc<Self>,
        _params: aud::CancelFocusParams,
        _results: aud::CancelFocusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("cancelFocus")))
    }

    fn set_stream_position(
        self: Rc<Self>,
        _params: aud::SetStreamPositionParams,
        _results: aud::SetStreamPositionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setStreamPosition")))
    }

    fn set_listener_position(
        self: Rc<Self>,
        _params: aud::SetListenerPositionParams,
        _results: aud::SetListenerPositionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setListenerPosition")))
    }

    fn set_stream_spatial_profile(
        self: Rc<Self>,
        _params: aud::SetStreamSpatialProfileParams,
        _results: aud::SetStreamSpatialProfileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setStreamSpatialProfile")))
    }

    fn record_consent(
        self: Rc<Self>,
        _params: aud::RecordConsentParams,
        _results: aud::RecordConsentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("recordConsent")))
    }

    fn declare_recording_policy(
        self: Rc<Self>,
        _params: aud::DeclareRecordingPolicyParams,
        _results: aud::DeclareRecordingPolicyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("declareRecordingPolicy")))
    }

    fn list_codecs(
        self: Rc<Self>,
        _params: aud::ListCodecsParams,
        _results: aud::ListCodecsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listCodecs")))
    }

    fn negotiate_codec(
        self: Rc<Self>,
        _params: aud::NegotiateCodecParams,
        _results: aud::NegotiateCodecResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("negotiateCodec")))
    }

    fn subscribe_codecs(
        self: Rc<Self>,
        _params: aud::SubscribeCodecsParams,
        _results: aud::SubscribeCodecsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeCodecs")))
    }

    fn pull_codecs(
        self: Rc<Self>,
        _params: aud::PullCodecsParams,
        _results: aud::PullCodecsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullCodecs")))
    }

    fn cancel_codecs(
        self: Rc<Self>,
        _params: aud::CancelCodecsParams,
        _results: aud::CancelCodecsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("cancelCodecs")))
    }

    fn set_route(
        self: Rc<Self>,
        _params: aud::SetRouteParams,
        _results: aud::SetRouteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setRoute")))
    }

    fn duck_on_call(
        self: Rc<Self>,
        _params: aud::DuckOnCallParams,
        _results: aud::DuckOnCallResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("duckOnCall")))
    }

    fn list_effects(
        self: Rc<Self>,
        _params: aud::ListEffectsParams,
        _results: aud::ListEffectsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listEffects")))
    }

    fn add_effect(
        self: Rc<Self>,
        _params: aud::AddEffectParams,
        _results: aud::AddEffectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("addEffect")))
    }

    fn remove_effect(
        self: Rc<Self>,
        _params: aud::RemoveEffectParams,
        _results: aud::RemoveEffectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("removeEffect")))
    }

    fn set_effect_enabled(
        self: Rc<Self>,
        _params: aud::SetEffectEnabledParams,
        _results: aud::SetEffectEnabledResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setEffectEnabled")))
    }

    fn set_effect_parameter(
        self: Rc<Self>,
        _params: aud::SetEffectParameterParams,
        _results: aud::SetEffectParameterResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setEffectParameter")))
    }

    fn list_presets(
        self: Rc<Self>,
        _params: aud::ListPresetsParams,
        _results: aud::ListPresetsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listPresets")))
    }

    fn apply_preset(
        self: Rc<Self>,
        _params: aud::ApplyPresetParams,
        _results: aud::ApplyPresetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("applyPreset")))
    }

    fn save_custom_preset(
        self: Rc<Self>,
        _params: aud::SaveCustomPresetParams,
        _results: aud::SaveCustomPresetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("saveCustomPreset")))
    }

    fn delete_custom_preset(
        self: Rc<Self>,
        _params: aud::DeleteCustomPresetParams,
        _results: aud::DeleteCustomPresetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("deleteCustomPreset")))
    }

    fn set_global_duck_policy(
        self: Rc<Self>,
        _params: aud::SetGlobalDuckPolicyParams,
        _results: aud::SetGlobalDuckPolicyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setGlobalDuckPolicy")))
    }

    fn get_global_duck_policy(
        self: Rc<Self>,
        _params: aud::GetGlobalDuckPolicyParams,
        _results: aud::GetGlobalDuckPolicyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getGlobalDuckPolicy")))
    }

    fn get_stream_metering(
        self: Rc<Self>,
        _params: aud::GetStreamMeteringParams,
        _results: aud::GetStreamMeteringResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getStreamMetering")))
    }

    fn get_device_metering(
        self: Rc<Self>,
        _params: aud::GetDeviceMeteringParams,
        _results: aud::GetDeviceMeteringResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getDeviceMetering")))
    }

    fn create_session(
        self: Rc<Self>,
        _params: aud::CreateSessionParams,
        _results: aud::CreateSessionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("createSession")))
    }

    fn join_session(
        self: Rc<Self>,
        _params: aud::JoinSessionParams,
        _results: aud::JoinSessionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("joinSession")))
    }

    fn leave_session(
        self: Rc<Self>,
        _params: aud::LeaveSessionParams,
        _results: aud::LeaveSessionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("leaveSession")))
    }

    fn get_session(
        self: Rc<Self>,
        _params: aud::GetSessionParams,
        _results: aud::GetSessionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getSession")))
    }

    fn list_sessions(
        self: Rc<Self>,
        _params: aud::ListSessionsParams,
        _results: aud::ListSessionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listSessions")))
    }

    fn set_session_routing(
        self: Rc<Self>,
        _params: aud::SetSessionRoutingParams,
        _results: aud::SetSessionRoutingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setSessionRouting")))
    }

    fn get_audio_transport(
        self: Rc<Self>,
        _params: aud::GetAudioTransportParams,
        _results: aud::GetAudioTransportResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getAudioTransport")))
    }

    fn list_channel_layouts(
        self: Rc<Self>,
        _params: aud::ListChannelLayoutsParams,
        _results: aud::ListChannelLayoutsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listChannelLayouts")))
    }

    fn get_per_role_duck_levels(
        self: Rc<Self>,
        _params: aud::GetPerRoleDuckLevelsParams,
        _results: aud::GetPerRoleDuckLevelsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getPerRoleDuckLevels")))
    }

    fn set_hrtf_data(
        self: Rc<Self>,
        _params: aud::SetHrtfDataParams,
        _results: aud::SetHrtfDataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setHrtfData")))
    }

    fn get_hrtf_data(
        self: Rc<Self>,
        _params: aud::GetHrtfDataParams,
        _results: aud::GetHrtfDataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getHrtfData")))
    }

    fn play_sound_event(
        self: Rc<Self>,
        _params: aud::PlaySoundEventParams,
        _results: aud::PlaySoundEventResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("playSoundEvent")))
    }

    fn stop_sound_event(
        self: Rc<Self>,
        _params: aud::StopSoundEventParams,
        _results: aud::StopSoundEventResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("stopSoundEvent")))
    }

    fn set_sound_event_volume(
        self: Rc<Self>,
        _params: aud::SetSoundEventVolumeParams,
        _results: aud::SetSoundEventVolumeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setSoundEventVolume")))
    }

    fn get_sound_event_volume(
        self: Rc<Self>,
        _params: aud::GetSoundEventVolumeParams,
        _results: aud::GetSoundEventVolumeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getSoundEventVolume")))
    }

    fn list_sound_events(
        self: Rc<Self>,
        _params: aud::ListSoundEventsParams,
        _results: aud::ListSoundEventsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listSoundEvents")))
    }

    fn load_sound_theme(
        self: Rc<Self>,
        _params: aud::LoadSoundThemeParams,
        _results: aud::LoadSoundThemeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("loadSoundTheme")))
    }

    fn get_active_sound_theme(
        self: Rc<Self>,
        _params: aud::GetActiveSoundThemeParams,
        _results: aud::GetActiveSoundThemeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getActiveSoundTheme")))
    }

    fn get_stream_analysis(
        self: Rc<Self>,
        _params: aud::GetStreamAnalysisParams,
        _results: aud::GetStreamAnalysisResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getStreamAnalysis")))
    }

    fn get_device_analysis(
        self: Rc<Self>,
        _params: aud::GetDeviceAnalysisParams,
        _results: aud::GetDeviceAnalysisResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getDeviceAnalysis")))
    }

    fn subscribe_analysis(
        self: Rc<Self>,
        _params: aud::SubscribeAnalysisParams,
        _results: aud::SubscribeAnalysisResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeAnalysis")))
    }

    fn pull_analysis(
        self: Rc<Self>,
        _params: aud::PullAnalysisParams,
        _results: aud::PullAnalysisResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullAnalysis")))
    }

    fn cancel_analysis(
        self: Rc<Self>,
        _params: aud::CancelAnalysisParams,
        _results: aud::CancelAnalysisResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("cancelAnalysis")))
    }

    fn get_mic_access(
        self: Rc<Self>,
        _params: aud::GetMicAccessParams,
        _results: aud::GetMicAccessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getMicAccess")))
    }

    fn request_mic_access(
        self: Rc<Self>,
        _params: aud::RequestMicAccessParams,
        _results: aud::RequestMicAccessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("requestMicAccess")))
    }

    fn revoke_mic_access(
        self: Rc<Self>,
        _params: aud::RevokeMicAccessParams,
        _results: aud::RevokeMicAccessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("revokeMicAccess")))
    }

    fn list_mic_access(
        self: Rc<Self>,
        _params: aud::ListMicAccessParams,
        _results: aud::ListMicAccessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listMicAccess")))
    }

    fn set_mic_policy(
        self: Rc<Self>,
        _params: aud::SetMicPolicyParams,
        _results: aud::SetMicPolicyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setMicPolicy")))
    }

    fn get_mic_policy(
        self: Rc<Self>,
        _params: aud::GetMicPolicyParams,
        _results: aud::GetMicPolicyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getMicPolicy")))
    }

    fn set_loudness_limit(
        self: Rc<Self>,
        _params: aud::SetLoudnessLimitParams,
        _results: aud::SetLoudnessLimitResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setLoudnessLimit")))
    }

    fn get_loudness_limit(
        self: Rc<Self>,
        _params: aud::GetLoudnessLimitParams,
        _results: aud::GetLoudnessLimitResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getLoudnessLimit")))
    }

    fn get_loudness_history(
        self: Rc<Self>,
        _params: aud::GetLoudnessHistoryParams,
        _results: aud::GetLoudnessHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getLoudnessHistory")))
    }

    fn set_safety_mode(
        self: Rc<Self>,
        _params: aud::SetSafetyModeParams,
        _results: aud::SetSafetyModeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setSafetyMode")))
    }

    fn get_safety_mode(
        self: Rc<Self>,
        _params: aud::GetSafetyModeParams,
        _results: aud::GetSafetyModeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getSafetyMode")))
    }

    fn suggest_route(
        self: Rc<Self>,
        _params: aud::SuggestRouteParams,
        _results: aud::SuggestRouteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("suggestRoute")))
    }

    fn get_routing_rules(
        self: Rc<Self>,
        _params: aud::GetRoutingRulesParams,
        _results: aud::GetRoutingRulesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getRoutingRules")))
    }

    fn get_active_routes(
        self: Rc<Self>,
        _params: aud::GetActiveRoutesParams,
        _results: aud::GetActiveRoutesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getActiveRoutes")))
    }

    fn list_transports(
        self: Rc<Self>,
        _params: aud::ListTransportsParams,
        _results: aud::ListTransportsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listTransports")))
    }

    fn get_transport_info(
        self: Rc<Self>,
        _params: aud::GetTransportInfoParams,
        _results: aud::GetTransportInfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getTransportInfo")))
    }

    fn get_device_health(
        self: Rc<Self>,
        _params: aud::GetDeviceHealthParams,
        _results: aud::GetDeviceHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getDeviceHealth")))
    }

    fn subscribe_device_health(
        self: Rc<Self>,
        _params: aud::SubscribeDeviceHealthParams,
        _results: aud::SubscribeDeviceHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeDeviceHealth")))
    }

    fn pull_device_health(
        self: Rc<Self>,
        _params: aud::PullDeviceHealthParams,
        _results: aud::PullDeviceHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullDeviceHealth")))
    }

    fn cancel_device_health(
        self: Rc<Self>,
        _params: aud::CancelDeviceHealthParams,
        _results: aud::CancelDeviceHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("cancelDeviceHealth")))
    }

    fn create_audio_zone(
        self: Rc<Self>,
        _params: aud::CreateAudioZoneParams,
        _results: aud::CreateAudioZoneResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("createAudioZone")))
    }

    fn remove_audio_zone(
        self: Rc<Self>,
        _params: aud::RemoveAudioZoneParams,
        _results: aud::RemoveAudioZoneResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("removeAudioZone")))
    }

    fn list_audio_zones(
        self: Rc<Self>,
        _params: aud::ListAudioZonesParams,
        _results: aud::ListAudioZonesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listAudioZones")))
    }

    fn set_zone_route(
        self: Rc<Self>,
        _params: aud::SetZoneRouteParams,
        _results: aud::SetZoneRouteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setZoneRoute")))
    }

    fn set_zone_volume(
        self: Rc<Self>,
        _params: aud::SetZoneVolumeParams,
        _results: aud::SetZoneVolumeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setZoneVolume")))
    }

    fn open_driver(
        self: Rc<Self>,
        _params: aud::OpenDriverParams,
        _results: aud::OpenDriverResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("openDriver")))
    }
}

/// The audio domain, as a served domain: the contract interface it serves,
/// the descriptor discovery publishes for it, and the per-connection server.
pub struct AudioDomain<B> {
    backend: Arc<B>,
}

impl<B> AudioDomain<B>
where
    B: AudioBackend + 'static,
{
    /// The audio domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for AudioDomain<B>
where
    B: AudioBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        // The contract's own interface id, from the generated code.
        aud::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::AUDIO_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "audio"
    }

    fn category(&self) -> &'static str {
        "audio"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the audio domain answers part of its
    /// contract on this host, so it is `degraded` — and the missing part is
    /// named in the registry.
    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    /// The commands this domain publishes through the registry (cli.capnp).
    ///
    /// The paths are the PRODUCT's own words — what `smart audio …` types —
    /// and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "audio state",
                "Audio",
                "getState",
                "the default sink's volume, mute and devices",
                0,
                true,
            )
            .with_row(matrix_row("getState")),
            DomainCommand::new(
                "audio devices",
                "Audio",
                "listDevices",
                "every sink and source the host reports",
                1,
                false,
            )
            .with_row(matrix_row("listDevices")),
            DomainCommand::new(
                "audio streams",
                "Audio",
                "listStreams",
                "the active playback streams",
                2,
                false,
            )
            .with_row(matrix_row("listStreams")),
            DomainCommand::new(
                "audio volume",
                "Audio",
                "setVolume",
                "set a device's volume",
                3,
                false,
            )
            .with_row(matrix_row("setVolume")),
            DomainCommand::new(
                "audio mute",
                "Audio",
                "setMute",
                "set a device's mute",
                4,
                false,
            )
            .with_row(matrix_row("setMute")),
            DomainCommand::new(
                "audio stream-volume",
                "Audio",
                "setStreamVolume",
                "set a stream's volume",
                5,
                false,
            )
            .with_row(matrix_row("setStreamVolume")),
            DomainCommand::new(
                "audio stream-mute",
                "Audio",
                "setStreamMute",
                "set a stream's mute",
                6,
                false,
            )
            .with_row(matrix_row("setStreamMute")),
            DomainCommand::new(
                "audio enable-device",
                "Audio",
                "enableDevice",
                "enable device",
                7,
                false,
            )
            .with_row(matrix_row("enableDevice")),
            DomainCommand::new(
                "audio disable-device",
                "Audio",
                "disableDevice",
                "disable device",
                8,
                false,
            )
            .with_row(matrix_row("disableDevice")),
            DomainCommand::composing(
                "audio watch",
                "Audio",
                "subscribeState",
                "subscribe to the audio state",
                9,
                true,
                "pullState, cancelState",
            )
            .with_row(matrix_row("subscribeState")),
            DomainCommand::new(
                "audio watch pull",
                "Audio",
                "pullState",
                "the next bounded batch of state readings",
                10,
                false,
            )
            .with_row(matrix_row("pullState")),
            DomainCommand::new(
                "audio watch cancel",
                "Audio",
                "cancelState",
                "retire a state subscription",
                11,
                false,
            )
            .with_row(matrix_row("cancelState")),
            DomainCommand::composing(
                "audio watch-devices",
                "Audio",
                "subscribeDevices",
                "subscribe to the device list",
                12,
                true,
                "pullDevices, cancelDevices",
            )
            .with_row(matrix_row("subscribeDevices")),
            DomainCommand::new(
                "audio watch-devices pull",
                "Audio",
                "pullDevices",
                "the next bounded batch of device readings",
                13,
                false,
            )
            .with_row(matrix_row("pullDevices")),
            DomainCommand::new(
                "audio watch-devices cancel",
                "Audio",
                "cancelDevices",
                "retire a device subscription",
                14,
                false,
            )
            .with_row(matrix_row("cancelDevices")),
            DomainCommand::composing(
                "audio watch-streams",
                "Audio",
                "subscribeStreams",
                "subscribe to the stream list",
                15,
                true,
                "pullStreams, cancelStreams",
            )
            .with_row(matrix_row("subscribeStreams")),
            DomainCommand::new(
                "audio watch-streams pull",
                "Audio",
                "pullStreams",
                "the next bounded batch of stream readings",
                16,
                false,
            )
            .with_row(matrix_row("pullStreams")),
            DomainCommand::new(
                "audio watch-streams cancel",
                "Audio",
                "cancelStreams",
                "retire a stream subscription",
                17,
                false,
            )
            .with_row(matrix_row("cancelStreams")),
            DomainCommand::new(
                "audio request-focus",
                "Audio",
                "requestFocus",
                "request focus",
                18,
                false,
            )
            .with_row(matrix_row("requestFocus")),
            DomainCommand::new(
                "audio release-focus",
                "Audio",
                "releaseFocus",
                "release focus",
                19,
                false,
            )
            .with_row(matrix_row("releaseFocus")),
            DomainCommand::new(
                "audio subscribe-focus",
                "Audio",
                "subscribeFocus",
                "subscribe focus",
                20,
                true,
            )
            .with_row(matrix_row("subscribeFocus")),
            DomainCommand::new(
                "audio pull-focus",
                "Audio",
                "pullFocus",
                "pull focus",
                21,
                false,
            )
            .with_row(matrix_row("pullFocus")),
            DomainCommand::new(
                "audio cancel-focus",
                "Audio",
                "cancelFocus",
                "cancel focus",
                22,
                false,
            )
            .with_row(matrix_row("cancelFocus")),
            DomainCommand::new(
                "audio set-stream-position",
                "Audio",
                "setStreamPosition",
                "set the stream position",
                23,
                false,
            )
            .with_row(matrix_row("setStreamPosition")),
            DomainCommand::new(
                "audio set-listener-position",
                "Audio",
                "setListenerPosition",
                "set the listener position",
                24,
                false,
            )
            .with_row(matrix_row("setListenerPosition")),
            DomainCommand::new(
                "audio set-stream-spatial-profile",
                "Audio",
                "setStreamSpatialProfile",
                "set the stream spatial profile",
                25,
                false,
            )
            .with_row(matrix_row("setStreamSpatialProfile")),
            DomainCommand::new(
                "audio record-consent",
                "Audio",
                "recordConsent",
                "record consent",
                26,
                false,
            )
            .with_row(matrix_row("recordConsent")),
            DomainCommand::new(
                "audio declare-recording-policy",
                "Audio",
                "declareRecordingPolicy",
                "declare recording policy",
                27,
                false,
            )
            .with_row(matrix_row("declareRecordingPolicy")),
            DomainCommand::new(
                "audio list-codecs",
                "Audio",
                "listCodecs",
                "the codecs",
                28,
                false,
            )
            .with_row(matrix_row("listCodecs")),
            DomainCommand::new(
                "audio negotiate-codec",
                "Audio",
                "negotiateCodec",
                "negotiate codec",
                29,
                false,
            )
            .with_row(matrix_row("negotiateCodec")),
            DomainCommand::new(
                "audio subscribe-codecs",
                "Audio",
                "subscribeCodecs",
                "subscribe codecs",
                30,
                true,
            )
            .with_row(matrix_row("subscribeCodecs")),
            DomainCommand::new(
                "audio pull-codecs",
                "Audio",
                "pullCodecs",
                "pull codecs",
                31,
                false,
            )
            .with_row(matrix_row("pullCodecs")),
            DomainCommand::new(
                "audio cancel-codecs",
                "Audio",
                "cancelCodecs",
                "cancel codecs",
                32,
                false,
            )
            .with_row(matrix_row("cancelCodecs")),
            DomainCommand::new(
                "audio set-route",
                "Audio",
                "setRoute",
                "set the route",
                33,
                false,
            )
            .with_row(matrix_row("setRoute")),
            DomainCommand::new(
                "audio duck-on-call",
                "Audio",
                "duckOnCall",
                "duck on call",
                34,
                false,
            )
            .with_row(matrix_row("duckOnCall")),
            DomainCommand::new(
                "audio list-effects",
                "Audio",
                "listEffects",
                "the effects",
                35,
                false,
            )
            .with_row(matrix_row("listEffects")),
            DomainCommand::new(
                "audio add-effect",
                "Audio",
                "addEffect",
                "add effect",
                36,
                false,
            )
            .with_row(matrix_row("addEffect")),
            DomainCommand::new(
                "audio remove-effect",
                "Audio",
                "removeEffect",
                "remove effect",
                37,
                false,
            )
            .with_row(matrix_row("removeEffect")),
            DomainCommand::new(
                "audio set-effect-enabled",
                "Audio",
                "setEffectEnabled",
                "set the effect enabled",
                38,
                false,
            )
            .with_row(matrix_row("setEffectEnabled")),
            DomainCommand::new(
                "audio set-effect-parameter",
                "Audio",
                "setEffectParameter",
                "set the effect parameter",
                39,
                false,
            )
            .with_row(matrix_row("setEffectParameter")),
            DomainCommand::new(
                "audio list-presets",
                "Audio",
                "listPresets",
                "the presets",
                40,
                false,
            )
            .with_row(matrix_row("listPresets")),
            DomainCommand::new(
                "audio apply-preset",
                "Audio",
                "applyPreset",
                "apply preset",
                41,
                false,
            )
            .with_row(matrix_row("applyPreset")),
            DomainCommand::new(
                "audio save-custom-preset",
                "Audio",
                "saveCustomPreset",
                "save custom preset",
                42,
                false,
            )
            .with_row(matrix_row("saveCustomPreset")),
            DomainCommand::new(
                "audio delete-custom-preset",
                "Audio",
                "deleteCustomPreset",
                "delete custom preset",
                43,
                false,
            )
            .with_row(matrix_row("deleteCustomPreset")),
            DomainCommand::new(
                "audio set-global-duck-policy",
                "Audio",
                "setGlobalDuckPolicy",
                "set the global duck policy",
                44,
                false,
            )
            .with_row(matrix_row("setGlobalDuckPolicy")),
            DomainCommand::new(
                "audio get-global-duck-policy",
                "Audio",
                "getGlobalDuckPolicy",
                "the global duck policy",
                45,
                true,
            )
            .with_row(matrix_row("getGlobalDuckPolicy")),
            DomainCommand::new(
                "audio get-stream-metering",
                "Audio",
                "getStreamMetering",
                "the stream metering",
                46,
                false,
            )
            .with_row(matrix_row("getStreamMetering")),
            DomainCommand::new(
                "audio get-device-metering",
                "Audio",
                "getDeviceMetering",
                "the device metering",
                47,
                false,
            )
            .with_row(matrix_row("getDeviceMetering")),
            DomainCommand::new(
                "audio create-session",
                "Audio",
                "createSession",
                "create session",
                48,
                false,
            )
            .with_row(matrix_row("createSession")),
            DomainCommand::new(
                "audio join-session",
                "Audio",
                "joinSession",
                "join session",
                49,
                false,
            )
            .with_row(matrix_row("joinSession")),
            DomainCommand::new(
                "audio leave-session",
                "Audio",
                "leaveSession",
                "leave session",
                50,
                false,
            )
            .with_row(matrix_row("leaveSession")),
            DomainCommand::new(
                "audio get-session",
                "Audio",
                "getSession",
                "the session",
                51,
                false,
            )
            .with_row(matrix_row("getSession")),
            DomainCommand::new(
                "audio list-sessions",
                "Audio",
                "listSessions",
                "the sessions",
                52,
                false,
            )
            .with_row(matrix_row("listSessions")),
            DomainCommand::new(
                "audio set-session-routing",
                "Audio",
                "setSessionRouting",
                "set the session routing",
                53,
                false,
            )
            .with_row(matrix_row("setSessionRouting")),
            DomainCommand::new(
                "audio get-audio-transport",
                "Audio",
                "getAudioTransport",
                "the audio transport",
                54,
                false,
            )
            .with_row(matrix_row("getAudioTransport")),
            DomainCommand::new(
                "audio list-channel-layouts",
                "Audio",
                "listChannelLayouts",
                "the channel layouts",
                55,
                false,
            )
            .with_row(matrix_row("listChannelLayouts")),
            DomainCommand::new(
                "audio set-per-role-duck-levels",
                "Audio",
                "setPerRoleDuckLevels",
                "set the per role duck levels",
                56,
                false,
            )
            .with_row(matrix_row("setPerRoleDuckLevels")),
            DomainCommand::new(
                "audio get-per-role-duck-levels",
                "Audio",
                "getPerRoleDuckLevels",
                "the per role duck levels",
                57,
                true,
            )
            .with_row(matrix_row("getPerRoleDuckLevels")),
            DomainCommand::new(
                "audio set-hrtf-data",
                "Audio",
                "setHrtfData",
                "set the hrtf data",
                58,
                false,
            )
            .with_row(matrix_row("setHrtfData")),
            DomainCommand::new(
                "audio get-hrtf-data",
                "Audio",
                "getHrtfData",
                "the hrtf data",
                59,
                false,
            )
            .with_row(matrix_row("getHrtfData")),
            DomainCommand::new(
                "audio play-sound-event",
                "Audio",
                "playSoundEvent",
                "play sound event",
                60,
                false,
            )
            .with_row(matrix_row("playSoundEvent")),
            DomainCommand::new(
                "audio stop-sound-event",
                "Audio",
                "stopSoundEvent",
                "stop sound event",
                61,
                false,
            )
            .with_row(matrix_row("stopSoundEvent")),
            DomainCommand::new(
                "audio set-sound-event-volume",
                "Audio",
                "setSoundEventVolume",
                "set the sound event volume",
                62,
                false,
            )
            .with_row(matrix_row("setSoundEventVolume")),
            DomainCommand::new(
                "audio get-sound-event-volume",
                "Audio",
                "getSoundEventVolume",
                "the sound event volume",
                63,
                false,
            )
            .with_row(matrix_row("getSoundEventVolume")),
            DomainCommand::new(
                "audio list-sound-events",
                "Audio",
                "listSoundEvents",
                "the sound events",
                64,
                false,
            )
            .with_row(matrix_row("listSoundEvents")),
            DomainCommand::new(
                "audio load-sound-theme",
                "Audio",
                "loadSoundTheme",
                "load sound theme",
                65,
                false,
            )
            .with_row(matrix_row("loadSoundTheme")),
            DomainCommand::new(
                "audio get-active-sound-theme",
                "Audio",
                "getActiveSoundTheme",
                "the active sound theme",
                66,
                true,
            )
            .with_row(matrix_row("getActiveSoundTheme")),
            DomainCommand::new(
                "audio get-stream-analysis",
                "Audio",
                "getStreamAnalysis",
                "the stream analysis",
                67,
                false,
            )
            .with_row(matrix_row("getStreamAnalysis")),
            DomainCommand::new(
                "audio get-device-analysis",
                "Audio",
                "getDeviceAnalysis",
                "the device analysis",
                68,
                false,
            )
            .with_row(matrix_row("getDeviceAnalysis")),
            DomainCommand::new(
                "audio subscribe-analysis",
                "Audio",
                "subscribeAnalysis",
                "subscribe analysis",
                69,
                false,
            )
            .with_row(matrix_row("subscribeAnalysis")),
            DomainCommand::new(
                "audio pull-analysis",
                "Audio",
                "pullAnalysis",
                "pull analysis",
                70,
                false,
            )
            .with_row(matrix_row("pullAnalysis")),
            DomainCommand::new(
                "audio cancel-analysis",
                "Audio",
                "cancelAnalysis",
                "cancel analysis",
                71,
                false,
            )
            .with_row(matrix_row("cancelAnalysis")),
            DomainCommand::new(
                "audio get-mic-access",
                "Audio",
                "getMicAccess",
                "the mic access",
                72,
                false,
            )
            .with_row(matrix_row("getMicAccess")),
            DomainCommand::new(
                "audio request-mic-access",
                "Audio",
                "requestMicAccess",
                "request mic access",
                73,
                false,
            )
            .with_row(matrix_row("requestMicAccess")),
            DomainCommand::new(
                "audio revoke-mic-access",
                "Audio",
                "revokeMicAccess",
                "revoke mic access",
                74,
                false,
            )
            .with_row(matrix_row("revokeMicAccess")),
            DomainCommand::new(
                "audio list-mic-access",
                "Audio",
                "listMicAccess",
                "the mic access",
                75,
                true,
            )
            .with_row(matrix_row("listMicAccess")),
            DomainCommand::new(
                "audio set-mic-policy",
                "Audio",
                "setMicPolicy",
                "set the mic policy",
                76,
                false,
            )
            .with_row(matrix_row("setMicPolicy")),
            DomainCommand::new(
                "audio get-mic-policy",
                "Audio",
                "getMicPolicy",
                "the mic policy",
                77,
                true,
            )
            .with_row(matrix_row("getMicPolicy")),
            DomainCommand::new(
                "audio set-loudness-limit",
                "Audio",
                "setLoudnessLimit",
                "set the loudness limit",
                78,
                false,
            )
            .with_row(matrix_row("setLoudnessLimit")),
            DomainCommand::new(
                "audio get-loudness-limit",
                "Audio",
                "getLoudnessLimit",
                "the loudness limit",
                79,
                true,
            )
            .with_row(matrix_row("getLoudnessLimit")),
            DomainCommand::new(
                "audio get-loudness-history",
                "Audio",
                "getLoudnessHistory",
                "the loudness history",
                80,
                false,
            )
            .with_row(matrix_row("getLoudnessHistory")),
            DomainCommand::new(
                "audio set-safety-mode",
                "Audio",
                "setSafetyMode",
                "set the safety mode",
                81,
                false,
            )
            .with_row(matrix_row("setSafetyMode")),
            DomainCommand::new(
                "audio get-safety-mode",
                "Audio",
                "getSafetyMode",
                "the safety mode",
                82,
                true,
            )
            .with_row(matrix_row("getSafetyMode")),
            DomainCommand::new(
                "audio suggest-route",
                "Audio",
                "suggestRoute",
                "suggest route",
                83,
                false,
            )
            .with_row(matrix_row("suggestRoute")),
            DomainCommand::new(
                "audio get-routing-rules",
                "Audio",
                "getRoutingRules",
                "the routing rules",
                84,
                true,
            )
            .with_row(matrix_row("getRoutingRules")),
            DomainCommand::new(
                "audio set-routing-rules",
                "Audio",
                "setRoutingRules",
                "set the routing rules",
                85,
                false,
            )
            .with_row(matrix_row("setRoutingRules")),
            DomainCommand::new(
                "audio get-active-routes",
                "Audio",
                "getActiveRoutes",
                "the active routes",
                86,
                true,
            )
            .with_row(matrix_row("getActiveRoutes")),
            DomainCommand::new(
                "audio list-transports",
                "Audio",
                "listTransports",
                "the transports",
                87,
                true,
            )
            .with_row(matrix_row("listTransports")),
            DomainCommand::new(
                "audio get-transport-info",
                "Audio",
                "getTransportInfo",
                "the transport info",
                88,
                false,
            )
            .with_row(matrix_row("getTransportInfo")),
            DomainCommand::new(
                "audio set-transport-priorities",
                "Audio",
                "setTransportPriorities",
                "set the transport priorities",
                89,
                false,
            )
            .with_row(matrix_row("setTransportPriorities")),
            DomainCommand::new(
                "audio get-device-health",
                "Audio",
                "getDeviceHealth",
                "the device health",
                90,
                false,
            )
            .with_row(matrix_row("getDeviceHealth")),
            DomainCommand::new(
                "audio subscribe-device-health",
                "Audio",
                "subscribeDeviceHealth",
                "subscribe device health",
                91,
                true,
            )
            .with_row(matrix_row("subscribeDeviceHealth")),
            DomainCommand::new(
                "audio pull-device-health",
                "Audio",
                "pullDeviceHealth",
                "pull device health",
                92,
                false,
            )
            .with_row(matrix_row("pullDeviceHealth")),
            DomainCommand::new(
                "audio cancel-device-health",
                "Audio",
                "cancelDeviceHealth",
                "cancel device health",
                93,
                false,
            )
            .with_row(matrix_row("cancelDeviceHealth")),
            DomainCommand::new(
                "audio create-audio-zone",
                "Audio",
                "createAudioZone",
                "create audio zone",
                94,
                false,
            )
            .with_row(matrix_row("createAudioZone")),
            DomainCommand::new(
                "audio remove-audio-zone",
                "Audio",
                "removeAudioZone",
                "remove audio zone",
                95,
                false,
            )
            .with_row(matrix_row("removeAudioZone")),
            DomainCommand::new(
                "audio list-audio-zones",
                "Audio",
                "listAudioZones",
                "the audio zones",
                96,
                true,
            )
            .with_row(matrix_row("listAudioZones")),
            DomainCommand::new(
                "audio set-zone-route",
                "Audio",
                "setZoneRoute",
                "set the zone route",
                97,
                false,
            )
            .with_row(matrix_row("setZoneRoute")),
            DomainCommand::new(
                "audio set-zone-volume",
                "Audio",
                "setZoneVolume",
                "set the zone volume",
                98,
                false,
            )
            .with_row(matrix_row("setZoneVolume")),
            DomainCommand::new(
                "audio open-driver",
                "Audio",
                "openDriver",
                "open driver",
                99,
                false,
            )
            .with_row(matrix_row("openDriver")),
        ]
    }

    /// The audio schema's node tree: what a client that never compiled
    /// `audio.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/audio.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = AudioService::new(Arc::clone(&self.backend), class);
        let client: aud::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
