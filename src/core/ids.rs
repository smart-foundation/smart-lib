//! The contract's opaque identifiers — one newtype per `T.*Id` in
//! `schema/types.capnp`.
//!
//! The set is **enumerated from the schema, not from memory**: every struct in
//! `types.capnp` whose body is exactly one integer field (`id @0 :UInt64`, or
//! `handle @0 :UInt64` for `CapabilityHandle`, or `id @0 :UInt32` for `CpuId`)
//! gets a newtype here — **157** of them. Each is `Copy + Eq + Hash + Debug`,
//! allocates nothing, and converts to and from the generated capnp type
//! without loss.
//!
//! Why newtypes at all, when every one of these is a `u64` on the wire: the
//! contract deleted stringly-typed identity precisely so a caller cannot pass
//! a `BufferId` where a `WindowId` was meant (`types.capnp:36-38`). A bare
//! `u64` reintroduces that bug in Rust; the newtype makes it a *compile*
//! error. Cost: zero — the wrapper is one machine word and every conversion
//! is a move.
//!
//! The ids are opaque by contract: core never interprets them, never derives
//! them from host identity (a `CapabilityHandle` is minted per acquisition,
//! never from a uid — `knowledge/02-HOST-CAPABILITIES.md` §3.1), and never
//! persists their meaning.

use crate::schema::types_capnp;

/// Declare a newtype over an opaque `UInt64` identity field of a generated
/// struct: `id64!(NodeId, node_id, get_id, set_id);`
macro_rules! id64 {
    ($name:ident, $module:ident, $getter:ident, $setter:ident) => {
        #[doc = concat!("Opaque identity newtype over `T.", stringify!($name), "` (u64 on the wire).")]
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// Wrap the raw integer the backend issued.
            pub const fn new(id: u64) -> Self {
                $name(id)
            }

            /// The raw integer on the wire.
            pub const fn get(self) -> u64 {
                self.0
            }

            /// Read from the generated capnp reader.
            pub fn from_reader(reader: &types_capnp::$module::Reader<'_>) -> Self {
                $name(reader.$getter())
            }

            /// Write into the generated capnp builder.
            pub fn write_to<'a>(&self, builder: &mut types_capnp::$module::Builder<'a>) {
                builder.$setter(self.0)
            }
        }

        impl From<u64> for $name {
            fn from(id: u64) -> Self {
                $name::new(id)
            }
        }

        impl From<$name> for u64 {
            fn from(id: $name) -> u64 {
                id.0
            }
        }

        impl From<&types_capnp::$module::Reader<'_>> for $name {
            fn from(reader: &types_capnp::$module::Reader<'_>) -> Self {
                $name::from_reader(reader)
            }
        }

        impl From<&types_capnp::$module::Builder<'_>> for $name {
            fn from(builder: &types_capnp::$module::Builder<'_>) -> Self {
                $name(builder.reborrow_as_reader().$getter())
            }
        }
    };
}

/// Declare a newtype over an opaque `UInt32` identity field of a generated
/// struct: `id32!(CpuId, cpu_id, get_id, set_id);`
macro_rules! id32 {
    ($name:ident, $module:ident, $getter:ident, $setter:ident) => {
        #[doc = concat!("Opaque identity newtype over `T.", stringify!($name), "` (u32 on the wire).")]
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            /// Wrap the raw integer the backend issued.
            pub const fn new(id: u32) -> Self {
                $name(id)
            }

            /// The raw integer on the wire.
            pub const fn get(self) -> u32 {
                self.0
            }

            /// Read from the generated capnp reader.
            pub fn from_reader(reader: &types_capnp::$module::Reader<'_>) -> Self {
                $name(reader.$getter())
            }

            /// Write into the generated capnp builder.
            pub fn write_to<'a>(&self, builder: &mut types_capnp::$module::Builder<'a>) {
                builder.$setter(self.0)
            }
        }

        impl From<u32> for $name {
            fn from(id: u32) -> Self {
                $name::new(id)
            }
        }

        impl From<$name> for u32 {
            fn from(id: $name) -> u32 {
                id.0
            }
        }

        impl From<&types_capnp::$module::Reader<'_>> for $name {
            fn from(reader: &types_capnp::$module::Reader<'_>) -> Self {
                $name::from_reader(reader)
            }
        }

        impl From<&types_capnp::$module::Builder<'_>> for $name {
            fn from(builder: &types_capnp::$module::Builder<'_>) -> Self {
                $name(builder.reborrow_as_reader().$getter())
            }
        }
    };
}

// 157 identity structs in types.capnp, each a single opaque integer field.
// Generated mechanically from the schema (single-field id/handle structs) and
// verified against the generated `types_capnp` module names.

// ─── UInt64 identities (`id @0 :UInt64`, 155 + 1 `handle`) ───
id64!(NodeId, node_id, get_id, set_id);
id64!(SessionId, session_id, get_id, set_id);
id64!(SurfaceId, surface_id, get_id, set_id);
id64!(BufferId, buffer_id, get_id, set_id);
id64!(OutputId, output_id, get_id, set_id);
id64!(WindowId, window_id, get_id, set_id);
id64!(WorkspaceId, workspace_id, get_id, set_id);
id64!(ClientId, client_id, get_id, set_id);
id64!(CaptureId, capture_id, get_id, set_id);
id64!(ImeContextId, ime_context_id, get_id, set_id);
id64!(AccessNodeId, access_node_id, get_id, set_id);
id64!(DragSessionId, drag_session_id, get_id, set_id);
id64!(InhibitHandle, inhibit_handle, get_id, set_id);
id64!(NotificationId, notification_id, get_id, set_id);
id64!(ClipboardId, clipboard_id, get_id, set_id);
id64!(EventId, event_id, get_id, set_id);
id64!(PackageId, package_id, get_id, set_id);
id64!(ServiceId, service_id, get_id, set_id);
id64!(JobId, job_id, get_id, set_id);
id64!(TimerId, timer_id, get_id, set_id);
id64!(PrintJobId, print_job_id, get_id, set_id);
id64!(ProcessId, process_id, get_id, set_id);
id64!(FileId, file_id, get_id, set_id);
id64!(SearchProviderId, search_provider_id, get_id, set_id);
id64!(SettingKey, setting_key, get_id, set_id);
id64!(AppId, app_id, get_id, set_id);
id64!(BackupId, backup_id, get_id, set_id);
id64!(CrashDumpId, crash_dump_id, get_id, set_id);
id64!(ProcessGroupId, process_group_id, get_id, set_id);
id64!(BreakpointId, breakpoint_id, get_id, set_id);
id64!(BiometricId, biometric_id, get_id, set_id);
id64!(AuthMethodId, auth_method_id, get_id, set_id);
id64!(AuthTokenId, auth_token_id, get_id, set_id);
id64!(PermissionId, permission_id, get_id, set_id);
id64!(BootStageId, boot_stage_id, get_id, set_id);
id64!(BootEntryId, boot_entry_id, get_id, set_id);
id64!(DeviceAddress, device_address, get_id, set_id);
id64!(UpgradeId, upgrade_id, get_id, set_id);
id64!(ContactId, contact_id, get_id, set_id);
id64!(GroupId, group_id, get_id, set_id);
id64!(ClipboardSourceId, clipboard_source_id, get_id, set_id);
id64!(SensorId, sensor_id, get_id, set_id);
id64!(SpeechVoiceId, speech_voice_id, get_id, set_id);
id64!(TrayIconId, tray_icon_id, get_id, set_id);
id64!(SocketId, socket_id, get_id, set_id);
id64!(CertificateId, certificate_id, get_id, set_id);
id64!(AccessId, access_id, get_id, set_id);
id64!(AccessBackendId, access_backend_id, get_id, set_id);
id64!(AccessCredentialId, access_credential_id, get_id, set_id);
id64!(MfaChallengeId, mfa_challenge_id, get_id, set_id);
id64!(MfaEnrollmentId, mfa_enrollment_id, get_id, set_id);
id64!(SmartCardId, smart_card_id, get_id, set_id);
id64!(RuntimeInstanceId, runtime_instance_id, get_id, set_id);
id64!(FramebufferId, framebuffer_id, get_id, set_id);
id64!(PlaneId, plane_id, get_id, set_id);
id64!(CrtcId, crtc_id, get_id, set_id);
id64!(AudioStreamId, audio_stream_id, get_id, set_id);
id64!(AudioFocusId, audio_focus_id, get_id, set_id);
id64!(AudioDeviceId, audio_device_id, get_id, set_id);
id64!(CodecId, codec_id, get_id, set_id);
id64!(CalendarId, calendar_id, get_id, set_id);
id64!(ReminderId, reminder_id, get_id, set_id);
id64!(AttendeeId, attendee_id, get_id, set_id);
id64!(CameraId, camera_id, get_id, set_id);
id64!(RecordingId, recording_id, get_id, set_id);
id64!(ContentId, content_id, get_id, set_id);
id64!(ContentSubscriptionId, content_subscription_id, get_id, set_id);
id64!(DatabaseId, database_id, get_id, set_id);
id64!(NamespaceId, namespace_id, get_id, set_id);
id64!(DeviceId, device_id, get_id, set_id);
id64!(ExtensionId, extension_id, get_id, set_id);
id64!(LogicalMonitorId, logical_monitor_id, get_id, set_id);
id64!(ColorProfileId, color_profile_id, get_id, set_id);
id64!(DisplayModeId, display_mode_id, get_id, set_id);
id64!(DnsSubscriptionId, dns_subscription_id, get_id, set_id);
id64!(HalDeviceId, hal_device_id, get_id, set_id);
id64!(DmaChannelId, dma_channel_id, get_id, set_id);
id64!(HookId, hook_id, get_id, set_id);
id64!(InputMethodId, input_method_id, get_id, set_id);
id64!(KeyboardLayoutId, keyboard_layout_id, get_id, set_id);
id64!(KeyBindingId, key_binding_id, get_id, set_id);
id64!(BiometricTemplateId, biometric_template_id, get_id, set_id);
id64!(GestureId, gesture_id, get_id, set_id);
id64!(IntentId, intent_id, get_id, set_id);
id64!(ActivityId, activity_id, get_id, set_id);
id64!(ServiceHandleId, service_handle_id, get_id, set_id);
id64!(ServiceName, service_name, get_id, set_id);
id64!(ManifestName, manifest_name, get_id, set_id);
id64!(SchemaId, schema_id, get_id, set_id);
id64!(MediaPlayerId, media_player_id, get_id, set_id);
id64!(ModuleId, module_id, get_id, set_id);
id64!(NetworkInterfaceId, network_interface_id, get_id, set_id);
id64!(VpnProfileId, vpn_profile_id, get_id, set_id);
id64!(VpnConnectionId, vpn_connection_id, get_id, set_id);
id64!(NfcTagId, nfc_tag_id, get_id, set_id);
id64!(NfcServiceId, nfc_service_id, get_id, set_id);
id64!(NotificationActionId, notification_action_id, get_id, set_id);
id64!(PowerProfileId, power_profile_id, get_id, set_id);
id64!(PrinterId, printer_id, get_id, set_id);
id64!(UserId, user_id, get_id, set_id);
id64!(ThreadId, thread_id, get_id, set_id);
id64!(QueueId, queue_id, get_id, set_id);
id64!(QueueMessageId, queue_message_id, get_id, set_id);
id64!(CapabilityHandle, capability_handle, get_handle, set_handle);
id64!(FontId, font_id, get_id, set_id);
id64!(FileHandleId, file_handle_id, get_id, set_id);
id64!(TagId, tag_id, get_id, set_id);
id64!(SubsystemId, subsystem_id, get_id, set_id);
id64!(MappedRegionId, mapped_region_id, get_id, set_id);
id64!(LayerId, layer_id, get_id, set_id);
id64!(EmailEntryId, email_entry_id, get_id, set_id);
id64!(PhoneEntryId, phone_entry_id, get_id, set_id);
id64!(AddressEntryId, address_entry_id, get_id, set_id);
id64!(SoundId, sound_id, get_id, set_id);
id64!(CallId, call_id, get_id, set_id);
id64!(ConferenceId, conference_id, get_id, set_id);
id64!(TelephonyMessageId, telephony_message_id, get_id, set_id);
id64!(ThemeId, theme_id, get_id, set_id);
id64!(SecretNamespaceId, secret_namespace_id, get_id, set_id);
id64!(SecretKeyId, secret_key_id, get_id, set_id);
id64!(MediaItemId, media_item_id, get_id, set_id);
id64!(LocationSourceId, location_source_id, get_id, set_id);
id64!(CapabilityId, capability_id, get_id, set_id);
id64!(FenceHandle, fence_handle, get_id, set_id);
id64!(MmsPartId, mms_part_id, get_id, set_id);
id64!(TaskGroupId, task_group_id, get_id, set_id);
id64!(SeatId, seat_id, get_id, set_id);
id64!(TaskName, task_name, get_id, set_id);
id64!(ComponentName, component_name, get_id, set_id);
id64!(TunnelId, tunnel_id, get_id, set_id);
id64!(AlarmId, alarm_id, get_id, set_id);
id64!(TaskId, task_id, get_id, set_id);
id64!(ActionId, action_id, get_id, set_id);
id64!(WindowGroupId, window_group_id, get_id, set_id);
id64!(SnapZoneId, snap_zone_id, get_id, set_id);
id64!(IntentChainId, intent_chain_id, get_id, set_id);
id64!(IntentTraceId, intent_trace_id, get_id, set_id);
id64!(IntentHistoryId, intent_history_id, get_id, set_id);
id64!(KeyId, key_id, get_id, set_id);
id64!(EndpointId, endpoint_id, get_id, set_id);
id64!(MountId, mount_id, get_id, set_id);
id64!(FilesystemId, filesystem_id, get_id, set_id);
id64!(SandboxId, sandbox_id, get_id, set_id);
id64!(StoragePoolId, storage_pool_id, get_id, set_id);
id64!(VolumeId, volume_id, get_id, set_id);
id64!(SnapshotId, snapshot_id, get_id, set_id);
id64!(SubnetId, subnet_id, get_id, set_id);
id64!(PolicyId, policy_id, get_id, set_id);
id64!(FirmwareId, firmware_id, get_id, set_id);
id64!(ChannelId, channel_id, get_id, set_id);
id64!(RuleId, rule_id, get_id, set_id);
id64!(TemplateId, template_id, get_id, set_id);
id64!(ScopeId, scope_id, get_id, set_id);
id64!(DeviceGroupId, device_group_id, get_id, set_id);
id64!(SearchBookmarkId, search_bookmark_id, get_id, set_id);
id32!(CpuId, cpu_id, get_id, set_id);
id64!(DriverId, driver_id, get_id, set_id);
