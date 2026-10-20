# Batch 5 — audio, media, power (smart-lib-ng shell side)

## Host evidence
- uid 1000, runit, PipeWire 1.6.9 (pipewire + pipewire-pulse + wireplumber), X11, UPower.
- Audio: PulseAudio server on PipeWire; libpulse FFI threaded mainloop; no libasound.
- Media: MPRIS players on the session bus (e.g. `org.mpris.MediaPlayer2.firefox.instance_1_92`).
- Power: UPower daemon on the system bus; battery BAT1; cpufreq energy_performance preferences; no power-profiles-daemon.

## Shell modules
- `crates/shell/src/domains/audio/{mod,render}.rs` — `AudioCmd`/`AudioAction`.
- `crates/shell/src/domains/media/{mod,render}.rs` — `MediaCmd`/`MediaAction`.
- `crates/shell/src/domains/power/{mod,render}.rs` — `PowerCmd`/`PowerAction`.
- Wired in `crates/shell/src/domains/mod.rs`, `crates/shell/src/shell.rs` (`exec_audio`/`exec_media`/`exec_power`, `selector_root`), and `crates/shell/src/main.rs` (`DomainCommand::Audio/Media/Power`).

## Commands
### audio
`audio` (default state), `devices`, `streams`, `volume <device> <value>`, `mute <device> <bool>`, `stream-volume <stream> <value>`, `stream-mute <stream> <bool>`, `caps` — all gated writes through the C1 gate (law #8).

### media
`players`, `player <id>`, `play/pause/stop/next/previous <id>`, `seek <id> <ms>`, `metadata <id>`, `position <id>`, `volume <id> <value>`, `set-position <id> <ms>`, `open-uri <id> <uri>`, `formats <id>`, `caps`.

### power
`battery`, `profiles`, `set-profile <id>`, `charging`, `fuel-gauge`, `suspend`, `hibernate`, `reboot`, `shutdown`, `caps`.

## Rendering
Every value is built from the engine's owned types (`AudioState`, `AudioDevice`, `AudioStream`, `MediaPlayer`, `MediaMetadata`, `BatteryState`, `PowerProfile`); no capnp readers, no `Box::leak`. `@audio/*`, `@media/*`, `@power/*` selectors resolve from the same `selector_root` the commands render.

## Live
```
$ smart audio
  🎵 Audio — {volume: 1.49, mute: false, default_output: 61, default_input: 62}
$ smart media players
  🎬 Media — [{id: 0, name: org.mpris.MediaPlayer2.firefox.instance_1_92, identity: Mozilla Firefox, status: Playing, volume: 1}]
$ smart power
  🔋 Power — {present: true, percentage: 95, charging: true, plugged: true}
```

## Gated
`audio volume/mute/stream-volume/stream-mute`, every `media` transport verb, `power set-profile/suspend/hibernate/reboot/shutdown` refuse without `--force`/REPL acceptance and name the verb; accepted writes reach the engine (recorded by the scripted backends). `suspend/hibernate/reboot/shutdown` are also refused by the engine with its own no-privilege reason.

## Board
`migrate.rs` board updated: audio, media, power moved from `migrated-pending` to `real-today` (12 real today, 5 queued). `session.rs` live-domain list and migrate/selectors/shell_language tests updated to the new honest example (`database`).

## Tests
`cargo test -j6 -p shell` → 178 passed, 0 failed (warning-free shell build).
- `audio_cmd`/`media_cmd`/`power_cmd` follow the scripted-backend pattern over the engine's owned types; gated verbs prove the engine is not called without acceptance.
