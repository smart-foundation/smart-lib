# Batch 5 — audio, media, power (smart-lib-ng)

## Host evidence
- uid 1000, runit, PipeWire 1.6.9 (pipewire + pipewire-pulse + wireplumber), X11, NetworkManager.
- Audio: PulseAudio server on PipeWire (`pactl` reports "PulseAudio (on PipeWire 1.6.9)"); libpulse headers present; libasound absent.
- Media: MPRIS players present on session bus (e.g. `org.mpris.MediaPlayer2.firefox.instance_1_92`); dbus crate available.
- Power: UPower daemon running (system bus `org.freedesktop.UPower`); battery BAT1; cpufreq energy_performance preferences present; no power-profiles-daemon.

## Audio (`src/domains/audio`, `src/platforms/linux/audio.rs`)
- Native mechanism: PulseAudio client library (`libpulse`) via FFI threaded mainloop — no shellout, no libasound.
- Implemented: getState (default sink volume/mute), listDevices (sinks + sources), listStreams (sink inputs), setVolume, setMute, setStreamVolume, setStreamMute, subscriptions.
- DEGRADED: setVolume/setMute/setStreamVolume/setStreamMute (host-mutating verbs, live proof founder-run).
- UNAVAILABLE: enableDevice/disableDevice (no PulseAudio surface) and all advanced audio methods (no native host API).
- Shellout-free: grep `Command::new` = 0.

## Media (`src/domains/media`, `src/platforms/linux/media.rs`)
- Native mechanism: MPRIS over D-Bus (session bus) via dbus crate.
- Implemented: listPlayers, getPlayer, play, pause, stop, next, previous, seek, getMetadata, getPosition, setVolume, setPosition, openUri, subscribePlayer, getSupportedFormats — all call real MPRIS methods (Play/Pause/Stop/Next/Previous/Seek/SetPosition/OpenUri and Properties Get/Set).
- UNAVAILABLE: transcode, recommendations, analytics, share/download, playlists, crossfade, etc. (no native MPRIS surface).
- Shellout-free: grep `Command::new` = 0.

## Power (`src/domains/power`, `src/platforms/linux/power.rs`)
- Native mechanism: UPower D-Bus (system bus) for battery + cpufreq sysfs for profiles; /sys/class/power_supply fallback.
- Implemented: getBattery (UPower Percentage/State/TimeToEmpty/TimeToFull), getProfiles (cpufreq energy_performance preferences or scaling governors), setProfile (cpufreq write), getChargingInfo, getFuelGauge, subscriptions.
- UNAVAILABLE: suspend/hibernate/reboot/shutdown (no privilege) and advanced power methods (no native surface).
- Cargo.toml: `domain-power = ["dep:dbus"]`.
- Shellout-free: grep `Command::new` = 0.

## Tests
- `cargo test --features "std,domain-audio,domain-media,domain-power" --lib` → 419 passed, 0 failed.
