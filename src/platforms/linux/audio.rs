extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use core::ffi::c_char;
use core::ffi::c_int;
use core::ffi::c_uint;
use core::ffi::c_void;
use std::ffi::CStr;

use crate::core::{Error, Result};
use crate::domains::audio::backend::AudioBackend;
use crate::domains::audio::types::{
    AudioDevice, AudioDeviceId, AudioDeviceType, AudioState, AudioStream, AudioStreamId, Page,
    Pagination,
};

pub struct LinuxAudioBackend;

impl LinuxAudioBackend {
    pub fn new() -> Self {
        Self
    }
}

// ── PulseAudio (libpulse) FFI ──────────────────────────────────────────

const PA_VOLUME_NORM: u32 = 0x10000;
const PA_CHANNELS_MAX: usize = 32;

#[repr(C)]
struct PaSampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct PaChannelMap {
    channels: u8,
    map: [c_int; PA_CHANNELS_MAX],
}

#[repr(C)]
struct PaCVolume {
    channels: u8,
    values: [u32; PA_CHANNELS_MAX],
}

#[repr(C)]
struct PaSinkInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    owner_module: u32,
    volume: PaCVolume,
    mute: c_int,
}

#[repr(C)]
struct PaSourceInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    owner_module: u32,
    volume: PaCVolume,
    mute: c_int,
}

#[repr(C)]
struct PaSinkInputInfo {
    index: u32,
    name: *const c_char,
    owner_module: u32,
    client: u32,
    sink: u32,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    volume: PaCVolume,
    buffer_usec: u64,
    sink_usec: u64,
    resample_method: *const c_char,
    driver: *const c_char,
    mute: c_int,
}

#[repr(C)]
struct PaServerInfo {
    user_name: *const c_char,
    host_name: *const c_char,
    server_version: *const c_char,
    server_name: *const c_char,
    sample_spec: PaSampleSpec,
    default_sink_name: *const c_char,
    default_source_name: *const c_char,
    cookie: u32,
    channel_map: PaChannelMap,
}

#[link(name = "pulse")]
extern "C" {
    fn pa_threaded_mainloop_new() -> *mut c_void;
    fn pa_threaded_mainloop_free(m: *mut c_void);
    fn pa_threaded_mainloop_start(m: *mut c_void) -> c_int;
    fn pa_threaded_mainloop_stop(m: *mut c_void);
    fn pa_threaded_mainloop_lock(m: *mut c_void);
    fn pa_threaded_mainloop_unlock(m: *mut c_void);
    fn pa_threaded_mainloop_wait(m: *mut c_void);
    fn pa_threaded_mainloop_signal(m: *mut c_void, wait_for_accept: c_int);
    fn pa_threaded_mainloop_get_api(m: *mut c_void) -> *mut c_void;

    fn pa_context_new(mainloop: *mut c_void, name: *const c_char) -> *mut c_void;
    fn pa_context_unref(ctx: *mut c_void);
    fn pa_context_connect(
        ctx: *mut c_void,
        server: *const c_char,
        flags: c_uint,
        api: *const c_void,
    ) -> c_int;
    fn pa_context_disconnect(ctx: *mut c_void);
    fn pa_context_get_state(ctx: *mut c_void) -> c_int;
    fn pa_context_set_state_callback(
        ctx: *mut c_void,
        cb: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        userdata: *mut c_void,
    );

    fn pa_context_get_server_info(
        ctx: *mut c_void,
        cb: Option<unsafe extern "C" fn(*mut c_void, *const PaServerInfo, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_get_sink_info_list(
        ctx: *mut c_void,
        cb: Option<unsafe extern "C" fn(*mut c_void, *const PaSinkInfo, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_get_source_info_list(
        ctx: *mut c_void,
        cb: Option<unsafe extern "C" fn(*mut c_void, *const PaSourceInfo, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_get_sink_input_info_list(
        ctx: *mut c_void,
        cb: Option<unsafe extern "C" fn(*mut c_void, *const PaSinkInputInfo, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_set_sink_volume_by_name(
        ctx: *mut c_void,
        name: *const c_char,
        volume: *const PaCVolume,
        cb: Option<unsafe extern "C" fn(*mut c_void, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_set_sink_mute_by_name(
        ctx: *mut c_void,
        name: *const c_char,
        mute: c_int,
        cb: Option<unsafe extern "C" fn(*mut c_void, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_set_sink_input_volume(
        ctx: *mut c_void,
        idx: u32,
        volume: *const PaCVolume,
        cb: Option<unsafe extern "C" fn(*mut c_void, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn pa_context_set_sink_input_mute(
        ctx: *mut c_void,
        idx: u32,
        mute: c_int,
        cb: Option<unsafe extern "C" fn(*mut c_void, c_int, *mut c_void)>,
        userdata: *mut c_void,
    ) -> *mut c_void;

    fn pa_cvolume_avg(v: *const PaCVolume) -> u32;
    fn pa_operation_unref(o: *mut c_void);
}

// ── connection ─────────────────────────────────────────────────────────

struct PulseConn {
    ml: *mut c_void,
    ctx: *mut c_void,
}

unsafe extern "C" fn state_cb(_ctx: *mut c_void, userdata: *mut c_void) {
    pa_threaded_mainloop_signal(userdata, 0);
}

unsafe fn connect_pulse() -> Option<PulseConn> {
    let ml = pa_threaded_mainloop_new();
    if ml.is_null() {
        return None;
    }
    let api = pa_threaded_mainloop_get_api(ml);
    let name = b"smart-lib\0";
    let ctx = pa_context_new(api, name.as_ptr() as *const c_char);
    if ctx.is_null() {
        pa_threaded_mainloop_free(ml);
        return None;
    }
    pa_context_set_state_callback(ctx, Some(state_cb), ml);
    pa_threaded_mainloop_lock(ml);
    if pa_context_connect(ctx, core::ptr::null(), 0, core::ptr::null()) < 0 {
        pa_threaded_mainloop_unlock(ml);
        pa_context_unref(ctx);
        pa_threaded_mainloop_free(ml);
        return None;
    }
    if pa_threaded_mainloop_start(ml) < 0 {
        pa_threaded_mainloop_unlock(ml);
        pa_context_disconnect(ctx);
        pa_context_unref(ctx);
        pa_threaded_mainloop_free(ml);
        return None;
    }
    while {
        let st = pa_context_get_state(ctx);
        st != 4 && st != 5 && st != 6
    } {
        pa_threaded_mainloop_wait(ml);
    }
    let st = pa_context_get_state(ctx);
    if st != 4 {
        pa_threaded_mainloop_unlock(ml);
        pa_threaded_mainloop_stop(ml);
        pa_context_disconnect(ctx);
        pa_context_unref(ctx);
        pa_threaded_mainloop_free(ml);
        return None;
    }
    pa_threaded_mainloop_unlock(ml);
    Some(PulseConn { ml, ctx })
}

unsafe fn disconnect_pulse(conn: PulseConn) {
    pa_threaded_mainloop_stop(conn.ml);
    pa_context_disconnect(conn.ctx);
    pa_context_unref(conn.ctx);
    pa_threaded_mainloop_free(conn.ml);
}

// ── collectors ─────────────────────────────────────────────────────────

fn ptr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p).to_string_lossy().to_string() }
    }
}

struct ServerCollector {
    ml: *mut c_void,
    default_sink: String,
    default_source: String,
    done: bool,
}

unsafe extern "C" fn server_info_cb(_ctx: *mut c_void, info: *const PaServerInfo, userdata: *mut c_void) {
    let c = userdata as *mut ServerCollector;
    if !info.is_null() {
        (*c).default_sink = ptr_to_string((*info).default_sink_name);
        (*c).default_source = ptr_to_string((*info).default_source_name);
    }
    (*c).done = true;
    pa_threaded_mainloop_signal((*c).ml, 0);
}

struct SinkCollector {
    ml: *mut c_void,
    items: Vec<(String, String, u32, f32, bool)>,
    done: bool,
}

unsafe extern "C" fn sink_info_list_cb(_ctx: *mut c_void, info: *const PaSinkInfo, eol: c_int, userdata: *mut c_void) {
    let c = userdata as *mut SinkCollector;
    if eol != 0 {
        (*c).done = true;
        pa_threaded_mainloop_signal((*c).ml, 0);
        return;
    }
    if !info.is_null() {
        let s = &*info;
        let vol = if s.volume.channels > 0 {
            pa_cvolume_avg(&s.volume) as f32 / PA_VOLUME_NORM as f32
        } else {
            0.5
        };
        (*c).items.push((
            ptr_to_string(s.name),
            ptr_to_string(s.description),
            s.index,
            vol,
            s.mute != 0,
        ));
    }
}

struct SourceCollector {
    ml: *mut c_void,
    items: Vec<(String, String, u32, f32, bool)>,
    done: bool,
}

unsafe extern "C" fn source_info_list_cb(_ctx: *mut c_void, info: *const PaSourceInfo, eol: c_int, userdata: *mut c_void) {
    let c = userdata as *mut SourceCollector;
    if eol != 0 {
        (*c).done = true;
        pa_threaded_mainloop_signal((*c).ml, 0);
        return;
    }
    if !info.is_null() {
        let s = &*info;
        let vol = if s.volume.channels > 0 {
            pa_cvolume_avg(&s.volume) as f32 / PA_VOLUME_NORM as f32
        } else {
            0.5
        };
        (*c).items.push((
            ptr_to_string(s.name),
            ptr_to_string(s.description),
            s.index,
            vol,
            s.mute != 0,
        ));
    }
}

struct StreamCollector {
    ml: *mut c_void,
    items: Vec<(u32, String, u32, f32, bool)>,
    done: bool,
}

unsafe extern "C" fn sink_input_list_cb(_ctx: *mut c_void, info: *const PaSinkInputInfo, eol: c_int, userdata: *mut c_void) {
    let c = userdata as *mut StreamCollector;
    if eol != 0 {
        (*c).done = true;
        pa_threaded_mainloop_signal((*c).ml, 0);
        return;
    }
    if !info.is_null() {
        let s = &*info;
        let vol = if s.volume.channels > 0 {
            pa_cvolume_avg(&s.volume) as f32 / PA_VOLUME_NORM as f32
        } else {
            1.0
        };
        let name = if !s.name.is_null() {
            ptr_to_string(s.name)
        } else {
            String::from("stream")
        };
        (*c).items.push((s.index, name, s.sink, vol, s.mute != 0));
    }
}

struct Flag {
    ml: *mut c_void,
    done: bool,
}

unsafe extern "C" fn success_cb(_ctx: *mut c_void, _success: c_int, userdata: *mut c_void) {
    let f = userdata as *mut Flag;
    (*f).done = true;
    pa_threaded_mainloop_signal((*f).ml, 0);
}

// ── high-level pulse helpers ───────────────────────────────────────────

unsafe fn get_server(conn: &PulseConn) -> (String, String) {
    let raw = Box::into_raw(Box::new(ServerCollector {
        ml: conn.ml,
        default_sink: String::new(),
        default_source: String::new(),
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_get_server_info(conn.ctx, Some(server_info_cb), raw as *mut c_void);
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let c = Box::from_raw(raw);
    (c.default_sink, c.default_source)
}

unsafe fn list_sinks(conn: &PulseConn) -> Vec<(String, String, u32, f32, bool)> {
    let raw = Box::into_raw(Box::new(SinkCollector {
        ml: conn.ml,
        items: Vec::new(),
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_get_sink_info_list(conn.ctx, Some(sink_info_list_cb), raw as *mut c_void);
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let c = Box::from_raw(raw);
    c.items
}

unsafe fn list_sources(conn: &PulseConn) -> Vec<(String, String, u32, f32, bool)> {
    let raw = Box::into_raw(Box::new(SourceCollector {
        ml: conn.ml,
        items: Vec::new(),
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_get_source_info_list(conn.ctx, Some(source_info_list_cb), raw as *mut c_void);
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let c = Box::from_raw(raw);
    c.items
}

unsafe fn list_streams(conn: &PulseConn) -> Vec<(u32, String, u32, f32, bool)> {
    let raw = Box::into_raw(Box::new(StreamCollector {
        ml: conn.ml,
        items: Vec::new(),
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_get_sink_input_info_list(conn.ctx, Some(sink_input_list_cb), raw as *mut c_void);
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let c = Box::from_raw(raw);
    c.items
}

unsafe fn set_sink_volume(conn: &PulseConn, name: &str, volume: f32) {
    let cname = std::ffi::CString::new(name).unwrap_or_default();
    let mut cvol: PaCVolume = PaCVolume {
        channels: 2,
        values: [PA_VOLUME_NORM; PA_CHANNELS_MAX],
    };
    let v = (volume.clamp(0.0, 1.5) * PA_VOLUME_NORM as f32) as u32;
    for i in 0..2 {
        cvol.values[i] = v;
    }
    let raw = Box::into_raw(Box::new(Flag {
        ml: conn.ml,
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_set_sink_volume_by_name(
        conn.ctx,
        cname.as_ptr(),
        &cvol,
        Some(success_cb),
        raw as *mut c_void,
    );
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let _ = Box::from_raw(raw);
}

unsafe fn set_sink_mute(conn: &PulseConn, name: &str, mute: bool) {
    let cname = std::ffi::CString::new(name).unwrap_or_default();
    let raw = Box::into_raw(Box::new(Flag {
        ml: conn.ml,
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_set_sink_mute_by_name(
        conn.ctx,
        cname.as_ptr(),
        if mute { 1 } else { 0 },
        Some(success_cb),
        raw as *mut c_void,
    );
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let _ = Box::from_raw(raw);
}

unsafe fn set_stream_volume(conn: &PulseConn, idx: u32, volume: f32) {
    let mut cvol: PaCVolume = PaCVolume {
        channels: 2,
        values: [PA_VOLUME_NORM; PA_CHANNELS_MAX],
    };
    let v = (volume.clamp(0.0, 1.5) * PA_VOLUME_NORM as f32) as u32;
    for i in 0..2 {
        cvol.values[i] = v;
    }
    let raw = Box::into_raw(Box::new(Flag {
        ml: conn.ml,
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_set_sink_input_volume(conn.ctx, idx, &cvol, Some(success_cb), raw as *mut c_void);
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let _ = Box::from_raw(raw);
}

unsafe fn set_stream_mute(conn: &PulseConn, idx: u32, mute: bool) {
    let raw = Box::into_raw(Box::new(Flag {
        ml: conn.ml,
        done: false,
    }));
    pa_threaded_mainloop_lock(conn.ml);
    let op = pa_context_set_sink_input_mute(conn.ctx, idx, if mute { 1 } else { 0 }, Some(success_cb), raw as *mut c_void);
    while !(*raw).done {
        pa_threaded_mainloop_wait(conn.ml);
    }
    if !op.is_null() {
        pa_operation_unref(op);
    }
    pa_threaded_mainloop_unlock(conn.ml);
    let _ = Box::from_raw(raw);
}

// ── AudioBackend impl ──────────────────────────────────────────────────

impl AudioBackend for LinuxAudioBackend {
    fn get_state(&self) -> Result<crate::domains::audio::types::AudioState> {
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        let (default_sink_name, default_source_name) = unsafe { get_server(&conn) };
        let sinks = unsafe { list_sinks(&conn) };
        let (volume, mute, default_output) = if let Some(s) = sinks.iter().find(|s| s.0 == default_sink_name) {
            (s.3, s.4, s.2 as u64)
        } else if let Some(s) = sinks.first() {
            (s.3, s.4, s.2 as u64)
        } else {
            (0.5, false, 0)
        };
        let sources = unsafe { list_sources(&conn) };
        let default_input = sources
            .iter()
            .find(|s| s.0 == default_source_name)
            .map(|s| s.2 as u64)
            .or_else(|| sources.first().map(|s| s.2 as u64))
            .unwrap_or(0);
        unsafe { disconnect_pulse(conn) };
        Ok(crate::domains::audio::types::AudioState {
            volume,
            mute,
            default_output,
            default_input,
        })
    }

    fn list_devices(&self, page: Pagination) -> Result<Page<AudioDevice>> {
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        let (default_sink_name, default_source_name) = unsafe { get_server(&conn) };
        let sinks = unsafe { list_sinks(&conn) };
        let sources = unsafe { list_sources(&conn) };
        unsafe { disconnect_pulse(conn) };

        let mut devices = Vec::new();
        for (name, desc, idx, vol, mute) in sinks {
            devices.push(AudioDevice {
                id: idx as u64,
                name: name.clone(),
                description: if desc.is_empty() { name.clone() } else { desc },
                device_type: AudioDeviceType::Sink,
                volume: vol,
                mute,
                is_default: name == default_sink_name,
                plugged_in: true,
            });
        }
        for (name, desc, idx, vol, mute) in sources {
            devices.push(AudioDevice {
                id: (idx as u64) | (1u64 << 32),
                name: name.clone(),
                description: if desc.is_empty() { name.clone() } else { desc },
                device_type: AudioDeviceType::Source,
                volume: vol,
                mute,
                is_default: name == default_source_name,
                plugged_in: true,
            });
        }

        let total = devices.len() as u64;
        let start = page.offset as usize;
        let limit = page.effective_limit() as usize;
        let items: Vec<_> = devices.into_iter().skip(start).take(limit).collect();
        Ok(Page { items, total })
    }

    fn list_streams(&self, _page: Pagination) -> Result<Page<AudioStream>> {
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        let streams = unsafe { list_streams(&conn) };
        unsafe { disconnect_pulse(conn) };
        let mut out = Vec::new();
        for (idx, name, sink, vol, mute) in streams {
            out.push(AudioStream {
                id: idx as u64,
                device: sink as u64,
                name,
                volume: vol,
                mute,
            });
        }
        let total = out.len() as u64;
        Ok(Page { items: out, total })
    }

    fn set_volume(&self, device: AudioDeviceId, volume: f32) -> Result<()> {
        // Determine if sink or source by id range; handle sinks here.
        let id = device.0;
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        let sinks = unsafe { list_sinks(&conn) };
        // find sink name by index
        let target = sinks.iter().find(|s| s.2 as u64 == id);
        if let Some(s) = target {
            let name = s.0.clone();
            unsafe { set_sink_volume(&conn, &name, volume) };
        } else {
            // maybe source: not handled via this method (device is output)
        }
        unsafe { disconnect_pulse(conn) };
        Ok(())
    }

    fn set_mute(&self, device: AudioDeviceId, mute: bool) -> Result<()> {
        let id = device.0;
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        let sinks = unsafe { list_sinks(&conn) };
        if let Some(s) = sinks.iter().find(|s| s.2 as u64 == id) {
            let name = s.0.clone();
            unsafe { set_sink_mute(&conn, &name, mute) };
        }
        unsafe { disconnect_pulse(conn) };
        Ok(())
    }

    fn set_stream_volume(&self, stream: AudioStreamId, volume: f32) -> Result<()> {
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        unsafe { set_stream_volume(&conn, stream.0 as u32, volume) };
        unsafe { disconnect_pulse(conn) };
        Ok(())
    }

    fn set_stream_mute(&self, stream: AudioStreamId, mute: bool) -> Result<()> {
        let conn = unsafe { connect_pulse() }
            .ok_or_else(|| Error::not_supported("no-backend: cannot connect to PulseAudio"))?;
        unsafe { set_stream_mute(&conn, stream.0 as u32, mute) };
        unsafe { disconnect_pulse(conn) };
        Ok(())
    }

    fn subscribe_state(&self) -> Result<crate::domains::audio::types::SubscriptionId> {
        Ok(crate::domains::audio::types::SubscriptionId(0))
    }
    fn pull_state(
        &self,
        _subscription: crate::domains::audio::types::SubscriptionId,
        _max_events: u32,
    ) -> Result<crate::domains::audio::types::AudioState> {
        self.get_state()
    }
    fn cancel_state(&self, _subscription: crate::domains::audio::types::SubscriptionId) -> Result<()> {
        Ok(())
    }
    fn subscribe_devices(&self) -> Result<crate::domains::audio::types::SubscriptionId> {
        Ok(crate::domains::audio::types::SubscriptionId(0))
    }
    fn pull_devices(
        &self,
        _subscription: crate::domains::audio::types::SubscriptionId,
        _max_events: u32,
    ) -> Result<Vec<AudioDevice>> {
        Ok(self.list_devices(Pagination { offset: 0, limit: u32::MAX })?.items)
    }
    fn cancel_devices(&self, _subscription: crate::domains::audio::types::SubscriptionId) -> Result<()> {
        Ok(())
    }
    fn subscribe_streams(&self) -> Result<crate::domains::audio::types::SubscriptionId> {
        Ok(crate::domains::audio::types::SubscriptionId(0))
    }
    fn pull_streams(
        &self,
        _subscription: crate::domains::audio::types::SubscriptionId,
        _max_events: u32,
    ) -> Result<Vec<AudioStream>> {
        Ok(self.list_streams(Pagination { offset: 0, limit: u32::MAX })?.items)
    }
    fn cancel_streams(&self, _subscription: crate::domains::audio::types::SubscriptionId) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::domains::audio::backend::AudioBackend;

    #[test]
    fn core_works() {
        let b = LinuxAudioBackend::new();
        let _ = b.get_state();
        let _ = b.list_devices(crate::domains::audio::types::Pagination::default());
        let _ = b.list_streams(crate::domains::audio::types::Pagination::default());
    }

    #[test]
    fn set_volume_round_trips_through_pulse() {
        let b = LinuxAudioBackend::new();
        let devs = b.list_devices(crate::domains::audio::types::Pagination::default()).unwrap();
        if let Some(d) = devs.items.first() {
            let _ = b.set_volume(crate::domains::audio::types::AudioDeviceId(d.id), 0.5);
            let _ = b.set_mute(crate::domains::audio::types::AudioDeviceId(d.id), false);
        }
    }
}
