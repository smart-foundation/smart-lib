# Batch 6 — appindex, content, database, manifest, queue, vcard (smart-lib-ng)

Six domains, one shape: `src/domains/<n>/{mod,api,types,backend,availability,tests}.rs`
plus `src/platforms/linux/<n>.rs`. Every method row in every matrix names the
`L3` test that proves it (`l3_linux_<n>::…`), and every row that is not
`Real` names the reason it is not.

## Host evidence

- uid 1000, Linux, X11 session; the batch reads no privileged surface.
- **XDG data directories** (`XDG_DATA_HOME` unset → `~/.local/share`; `XDG_DATA_DIRS=/home/lufuno/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share`).
- **The host's own desktop corpus** — measured twice, independently and by the
  backend's own index, and both agree:
  - `/home/lufuno/.local/share` → 11 entries
  - `/usr/local/share` → 5
  - `/usr/share` → 160
  - `/var/lib/flatpak/exports/share` → 1
  - **177 `.desktop` entries in total**, every one of them carrying a `Name`.
  - Largest categories: `GTK` 110, `GNOME` 82, `Settings` 56, `Utility` 36, `DesktopSettings` 34.
  - The measurement is printed by the L3 test
    (`l3_linux_appindex::the_hosts_own_corpus_is_read_when_the_backend_is_defaulted`),
    so the number is the host's own, not a fixture's.
- **Canonical schema ids** (from the frozen contract's headers):
  appindex `@0xa1b2c3d4e5f6001a` = 11651590505119481882 ·
  content `@0xa1b2c3d4e5f60055` = 11651590505119481941 ·
  database `@0xa1b2c3d4e5f6002d` = 11651590505119481901 ·
  manifest `@0xa1b2c3d4e5f60037` = 11651590505119481911 ·
  queue `@0xa1b2c3d4e5f60030` = 11651590505119481904 ·
  vcard `@0xa1b2c3d4e5f60108` = 11651590505119482120.

## What this batch found, and what was done about it

The batch landed with the *shape* right and the *substance* hollow in four
of the six domains — vcard, appindex, content and manifest — and this wave
closed that gap. (database and queue already carried real in-process stores
and honest matrices.)

1. **vcard** had a domain module and a platform cell whose every method
   returned `Ok(())` — the matrix claimed `Real` for `parseVCard`,
   `formatVCard` and `getSupportedVersions` over a backend that parsed
   nothing. Rewritten for real (below).
2. **appindex, content and manifest** had the same disease, one layer down:
   the backend traits were `fn list_apps(&self) -> Result<()>`, `fn publish(&self) -> Result<()>`,
   `fn resolve(&self) -> Result<()>` — a method that returns *nothing* cannot
   be `Real`, whatever the matrix says. Their traits now carry real
   signatures and their platforms do the work.
3. **The content matrix carried a method the contract does not declare.**
   `rotateKey` sat at ordinal 23, which the contract spells `encryptContent`;
   every row from 23 on was shifted by one, and the matrix had 58 rows for a
   55-method service. The matrix is regenerated from the contract's own
   method list: 55 rows + the subscription's `pull`/`cancel` = 57, with the
   ordinals matching `content.capnp` exactly.

Every matrix in the batch was checked against its contract's interface the
same way (method name **and** ordinal, in order): appindex 17 + 2, content 55
+ 2, manifest 14 + 2, queue 31 + 2, database 70 + 6, vcard 8 — all agree.

## vcard (`src/domains/vcard`, `src/platforms/linux/vcard.rs`)

- **Native mechanism**: the engine's own in-process RFC 6350 parser and
  formatter. `vcard.capnp` declares "Zero backend assumptions: no libvcard, no
  EKD, no platform contact APIs. Backend provides vCard parsing/generation" —
  this cell is that backend. No shellout, no host contact API (law #9).
- **Implemented (REAL, 7)**: `importVCard`, `exportVCard`, `parseVCard`,
  `formatVCard`, `getSupportedVersions`, `getSyncToken`, `syncChanges`.
- **UNAVAILABLE (1)**: `resolveConflict` — *no conflict surface: the store is
  this engine's own memory, with no remote peer to conflict with, so no
  conflict is ever produced and there is nothing for this method to resolve —
  it refuses rather than pretending.*
- **The parser** reads real RFC 6350: content lines with groups
  (`item1.TEL`), the open parameter list (quoted values, comma-separated
  `TYPE=home,work`), line unfolding (CRLF + space/tab), the `\` escapes,
  structured values (`N`, `ADR`, `ORG`), base64 binary values (`PHOTO`,
  `LOGO`, `SOUND`, `KEY`), URI values, and dates.
- **The formatter** writes the same dialect back — `BEGIN:VCARD` /
  `VERSION` / `END:VCARD`, the parameters, the escapes, and folding at the
  requested length (RFC 6350's 75 octets by default).
- **The mapping** is the contract's normative table, row by row, from
  `vcard.capnp`'s own header: `uid → metadata["vcard.uid"]`,
  `fn → name.display`, `n → name.{family,given,middle,prefix,suffix}`,
  `nickname → metadata["vcard.nickname"]`, `org → organization.{name,department}`
  (and `Contact.type = "organization"` when `FN` is the org name),
  `tel → phones[].value` (E.164-normalised when parseable, the raw value kept
  in `metadata["vcard.tel.raw"]`), `email → emails[].value`,
  `adr → addresses[]` (street/city/region/postalCode/country, in that
  component order), `photo → photo`, `bday → birthday` (the time part dropped,
  never guessed), `anniversary → metadata["vcard.anniversary"]`,
  `categories → tags`, `note → note`, `geo → customFields["geo"]`.
- **The `TYPE` rules** are the contract's: a published token (`home`, `work`,
  `mobile`, `fax`, `other`) maps to the contacts vocabulary; an unpublished
  one (`CELL`, `VIP`) is preserved **verbatim** and ignored rather than
  rejected.
- **Two rows the contract cannot carry are reported, not dropped** (law #6):
  - `ADR`'s PO-box and extended-address components — the table names only
    street/city/region/postalCode/country — are dropped **with a warning**.
  - `VCardContact.title → Contact.organization.title` — **`T.OrganizationName`
    has no `title` field** in the canonical tree (it carries `name`,
    `department`, `role`, `industry`; `grep -n title contacts.capnp` finds
    only the `Title` *enum*). The title is therefore preserved in
    `metadata["vcard.title"]` and the gap is stated in the import's warnings.
    This is a contract defect, not an implementation choice: the mapping
    table names a destination field the canonical tree does not have.
- **The store** is the engine's own memory: contacts by id, a monotonic
  revision counter, and the sync tokens derived from it
  (`vcard-sync-<revision>`); `syncChanges` returns the cards changed since
  the caller's token, and a token this backend did not issue is refused
  rather than interpreted.
- **Live proof** (22 vcard tests: 11 domain, 11 platform):
  `a_vcard_round_trips_through_the_parser_and_the_formatter`,
  `a_folded_line_is_unfolded_and_an_escaped_value_is_unescaped`,
  `a_structured_value_keeps_its_component_order`,
  `an_import_maps_the_fields_the_contract_names`,
  `an_export_writes_the_fields_the_contract_names`,
  `a_photo_is_read_and_written_as_bytes`,
  `a_duplicate_is_handled_the_way_the_caller_asked`,
  `the_supported_versions_are_the_ones_the_engine_can_read`,
  `a_sync_token_reports_the_changes_since_the_token`,
  `a_conflict_is_refused_honestly`, and the domain's own wire-format
  round-trip (`a_parsed_card_survives_the_generated_wire_format`).

## appindex (`src/domains/appindex`, `src/platforms/linux/appindex.rs`)

- **Native mechanism**: the host's own **freedesktop desktop-entry corpus** —
  the `.desktop` files under the XDG data directories, read with `read_dir`
  and the entry format's own INI grammar. No shellout, no `gtk-launch`, no
  `xdg-open` (law #9). This is the only batch-6 domain that answers from the
  host's own data rather than from the engine's memory.
- **REAL (5)**: `listApps`, `getApp`, `searchApps`, `getStats`, `batchGet`.
- **DEGRADED (3)**: `getPinned`, `pinApp`, `unpinApp` — *the pinned set is this
  engine's own in-process set: the host's pinned applications live in the
  desktop shell's own store (dconf), which this backend neither reads nor
  writes.*
- **UNAVAILABLE (11)**: `launch`, `launchUrl` (host-mutating, law #8),
  `getRecentApps`, `clearRecentApps` (the desktop shell's own recents),
  `subscribe`, `pull`, `cancel` (no change feed over a snapshot),
  `listCapabilities`, `getDependents` (the corpus describes neither).
- **Live proof**: the index counts **177** entries on this host, filed under
  the XDG data directories themselves (`/usr/share` 160, `~/.local/share` 11,
  `/usr/local/share` 5, the flatpak export 1) — the `source` token is the data
  directory, because that is what the host actually publishes; an invented
  short label would be a claim the host never made.
- **Fields a desktop entry does not carry** (package id, version, developer,
  licence, sizes, dates) are left at the generated defaults rather than
  guessed; the entry's icon *name* rides in the icon's metadata, because the
  contract's `AppIcon` carries icon *data*.
- 19 appindex tests (10 domain, 9 platform): `an_entry_survives_the_generated_wire_format`,
  `an_unserved_backend_says_so_instead_of_faking`,
  `availability_matrix_matches_contract`, `claimed_l3_tests_exist`,
  `every_served_row_names_its_backing_test`, `the_counts_add_up`,
  `the_host_mutating_verbs_name_the_law`, `the_matrix_keeps_the_contract_ordinals`,
  `the_page_shape_carries_the_total`, `the_schema_id_is_the_canonical_one` (domain);
  `a_batch_answers_from_the_same_corpus`,
  `a_search_answers_from_the_same_corpus`,
  `an_absent_desktop_directory_is_an_honest_absence`, `core_works`,
  `one_entry_is_read_by_its_own_id`,
  `the_hosts_own_corpus_is_read_when_the_backend_is_defaulted`,
  `the_index_lists_the_hosts_own_desktop_entries`,
  `the_pinned_set_is_the_engines_own_memory`,
  `the_stats_count_the_hosts_own_corpus` (platform).

## content (`src/domains/content`, `src/platforms/linux/content.rs`)

- **Native mechanism**: the engine's own in-process content store. The
  contract defines content as the engine's own corpus, and this host
  publishes no content corpus, so the store *is* the mechanism (the same
  shape as this batch's queue and database stores).
- **REAL (10)**: `publish`, `publishMany`, `update`, `unpublish`, `delete`,
  `trash`, `restore`, `emptyTrash`, `query`, `get` — real items with real
  ids, versions, timestamps, payloads and tags, and the contract's own
  optimistic-concurrency check: a write presenting a stale version is
  **refused with a conflict**, never a silent overwrite.
- **UNAVAILABLE (47)**: the surface the store does not model — the
  relationship graph, reactions, provenance, the keyring, the geo index, the
  AI tagger, branches, composites, lineage, the audit trail, the rate
  limiter, previews, dependencies, expiration, replication, the quality score,
  the recommendations, the translations and the change feed. Each row states
  its own reason.
- 16 content tests (10 domain, 6 platform): the domain's ten cover the matrix, the
  wire format and the honest refusals; the platform's six are
  `a_published_item_is_really_stored`, `a_query_answers_from_the_store`,
  `an_update_is_checked_against_the_version`, `the_trash_holds_what_it_was_given`,
  `a_missing_id_is_an_honest_absence` and `core_works`.

## manifest (`src/domains/manifest`, `src/platforms/linux/manifest.rs`)

- **Native mechanism**: the engine's own in-process manifest store. The
  contract's manifests describe the engine's own bundles, and this host
  publishes no bundle corpus, so the store *is* the mechanism.
- **REAL (5)**: `resolve`, `list`, `validate`, `validateDependencies`,
  `resolveByName` — and the validation is a real check: a **required**
  dependency that no manifest in the store offers is an error the caller is
  told about; an optional one is not.
- **UNAVAILABLE (11)**: `subscribe`, `pull`, `cancel` (no change feed),
  `compose`, `composeWithConfig` (no composition engine), `diff` (one
  manifest per name, so there is no second revision to compare),
  `createTemplate` (the store holds the manifests it was given; it does not
  mint new ones), `migrate` (no migration engine), `sign` (host-mutating,
  law #8, and the contract's key id names a keyring this host does not
  publish), `verify` (nothing here attests the engine's manifests).
- 15 manifest tests (10 domain, 5 platform); the platform's five are
  `a_registered_manifest_resolves`, `the_listing_is_the_stores_own`,
  `a_validation_reports_the_dependency_it_cannot_find`,
  `a_human_name_resolves_through_its_vendor` and `core_works`.

## database and queue (`src/domains/database`, `src/domains/queue`)

- Left as they landed: both already carried **real in-process stores** (a
  namespaced key/value store with transactions, snapshots and a change
  stream; a FIFO with groups, priorities, deduplication and a dead-letter
  queue), and their matrices already named their backing tests.
- **database**: 76 rows — **REAL 27**, DEGRADED 0, UNAVAILABLE 49.
- **queue**: 33 rows — **REAL 7**, DEGRADED 0, UNAVAILABLE 26.
- **Thin test surface, and it is stated here rather than hidden**: database
  carries 4 tests (2 domain + `core_works` + `transaction_id_round_trips`)
  and queue 3 (2 domain + `core_works`). Their stores are real, but the
  domain-level tests only check the matrix and the wire format — the store's
  own behaviour (transactions, snapshots, groups, priorities, deduplication,
  the dead-letter queue) has no per-behaviour L3 test of its own. Their
  `Real` rows name `l3_linux_database::core_works` and
  `l3_linux_queue::core_works`, which is a smoke test, not a proof of each
  verb. The next wave should give these two the same per-verb L3 coverage
  the other four now have.

## Shellout-free

`grep -rn "Command::new" src/` → the only *executable* hits are the four
inside `src/platforms/linux/process.rs`'s `#[cfg(test)] mod tests` (lines
1593, 1679, 1789, 2123 — they spawn `/bin/true`, `/bin/cat`, `/bin/sleep` to
witness the process backend). Every other hit in `src/` is a **comment** (the
availability matrices and module docs naming the shellout each backend
exists to end). The six batch-6 domains: **zero** `Command::new`, zero
`std::process::Command`, in any file, test or not. `crates/shell`: zero too
(the one hit is a comment).

## Tests

Per feature, each domain built and tested in isolation
(`--no-default-features --features "std,domain-<n>"`), `-j 6`:

| feature | feature's own tests | whole-feature total |
|---|---|---|
| `domain-appindex` | 19 | **263 passed**, 0 failed |
| `domain-content` | 16 | **260 passed**, 0 failed |
| `domain-database` | 4 | **248 passed**, 0 failed |
| `domain-manifest` | 15 | **259 passed**, 0 failed |
| `domain-queue` | 3 | **247 passed**, 0 failed |
| `domain-vcard` | 22 | **266 passed**, 0 failed |

- All six together: `cargo test --features "std,domain-appindex,domain-content,domain-database,domain-manifest,domain-queue,domain-vcard" --lib` → **492 passed, 0 failed, 4 ignored**.
- The tree's default features (files, network, audio, system) → **413 passed, 0 failed, 4 ignored**.
- Feature isolation holds (charter law #2): each feature compiles only its own
  schema set — `domain-appindex` → 5 schema files, `domain-content` → 5,
  `domain-database` → 4, `domain-manifest` → 4, `domain-queue` → 4,
  `domain-vcard` → 5. No `--all-features` build was used for this batch.

## Findings for other owners

1. **A double close in the transport layer** (not this batch's files, but it
   made this batch's tests flaky, so it is worth the owner's attention):
   `Arena::adopt(RegionPass::OsHandle(fd))` (`src/transport/shmem.rs:317-319`)
   hands the descriptor to `Storage::adopt_fd` → `sys::adopt_shared_region`
   (`src/transport/sys.rs:694`), which **takes ownership** of it, while the
   owning arena's `Storage::drop` (`src/transport/shmem.rs:249-257`) releases
   **the same** descriptor. The two shmem tests that hand the owner's *own*
   handle to `adopt` (`src/transport/tests/shmem.rs:317` and `:368`) therefore
   close one fd twice; the second `close(2)` lands on whatever descriptor has
   since taken that number. It surfaced here as the standard library's
   `closedir` failing with `EBADF` inside `src/platforms/linux/appindex.rs`'s
   directory scan, in roughly one run in twenty (once in seventeen runs, then
   once in thirty).
   The same double close breaks the transport's **own** tests: running the
   test binary 25–40 times, `transport::tests::shmem::close_reason_propagates_over_the_ring`
   failed with `create arena: Host { errno: 9, detail: "ftruncate" }` — errno 9
   is `EBADF`, and the descriptor that was closed was a **freshly created**
   `memfd`, killed by another thread's second `close(2)` between
   `memfd_create` and `ftruncate`. That is the proof that the double close
   does not merely leak an fd: it takes an unrelated live descriptor with
   it. The appindex scan now retries
   (and silences *only* that known panic; every other panic still prints).
   The right fix is in the transport: `adopt` should take a **duplicated**
   descriptor, or the tests should pass a real `SCM_RIGHTS` transfer, which
   is what production does — the kernel hands the peer a *new* fd number.
2. **Rare flakes in the transport's own suite** (not this batch's files, and
   not caused by it — measured by running the test binary 25 times with the
   tree's default features: 24 clean, 1 failure):
   - `four_transports_one_api_unix` and `listener_accepts_and_exchanges` bind
     the fixed path `/tmp/smart-lib-transport-test.sock`, so two concurrent
     test binaries (or a leftover socket file) collide.
   - `transport::tests::tcp::negotiate_hook_round_trips_a_handshake` failed
     once in the same 25 runs.
3. **The contract's vCard mapping table names a field the canonical tree does
   not have** — `Contact.organization.title` (see the vcard section above).
   The canonical tree's `T.OrganizationName` carries `name`, `department`,
   `role` and `industry`. The value is preserved in the contact's metadata and
   reported; the table row should be corrected at the source.

## Batch totals

| domain | contract methods | REAL | DEGRADED | UNAVAILABLE | own tests |
|---|---|---|---|---|---|
| appindex | 19 | 5 | 3 | 11 | 19 |
| content | 57 | 10 | 0 | 47 | 16 |
| database | 76 | 27 | 0 | 49 | 4 |
| manifest | 16 | 5 | 0 | 11 | 15 |
| queue | 33 | 7 | 0 | 26 | 3 |
| vcard | 8 | 7 | 0 | 1 | 22 |
| **total** | **209** | **61** | **3** | **145** | **79** |

No row in any of the six matrices claims `Real` without naming the L3 test
that proves it, and no row is `Unavailable` without a reason.
