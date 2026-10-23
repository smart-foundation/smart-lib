# Spine S1 (lib): batches 2-3 — media, power, database, queue, appindex, manifest, vcard, content

Served all eight domains through their generated `Server` traits. Each method answers with the backend's result or a `notSupported` carrying the matrix's own reason; none falls through to the generated default. Registry publishes every contract method (shell verb paths where a shell verb exists, method-kebab paths otherwise); discovery descriptor + availability into `listUnavailable`.

Per-domain method counts (matrix rows): media 39, power 22, database 76, queue 33, appindex 19, manifest 16, vcard 8, content 57. Total 270.

Availability splits (real/degraded/unavailable) are the matrix's own; see each `src/domains/{d}/availability.rs`.

Tests: `tests/serve_{domain}.rs` (one binary per domain) build and pass (registry + discovery smoke).
