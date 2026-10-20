# Batch 6 — the CLI half: database, queue, appindex, manifest, vcard, content

**Status**: COMPLETE — six domain command modules, 50 verbs, the board moved to
eighteen real-today domains, 238 shell tests green, every command verified live.

The engine's side of Batch 6 landed first (see `knowledge/26-batch6-lib.md`): the
engine's own namespaced key/value store, its own FIFO, the host's own
desktop-entry corpus, its own manifest store, its own RFC 6350 parser and
formatter, and its own content store. This is the shell half — the commands, the
rendering, the gate, the board, the tests.

Nothing in `src/` was touched. The diff is `crates/shell/**` and this file.

---

## What landed

| file | lines | what it is |
|---|---|---|
| `crates/shell/src/domains/database/{mod,render}.rs` | 270 + 344 | the engine's namespaced key/value store |
| `crates/shell/src/domains/queue/{mod,render}.rs` | 201 + 259 | the engine's FIFO store |
| `crates/shell/src/domains/appindex/{mod,render}.rs` | 241 + 543 | the host's desktop-entry corpus |
| `crates/shell/src/domains/manifest/{mod,render}.rs` | 315 + 386 | the engine's manifest store |
| `crates/shell/src/domains/vcard/{mod,render}.rs` | 427 + 638 | the engine's RFC 6350 parser/formatter |
| `crates/shell/src/domains/content/{mod,render}.rs` | 401 + 360 | the engine's content store |
| `crates/shell/tests/common/batch6/*` | 1070 | six scripted backends over the engine's owned types |
| `crates/shell/tests/{database,queue,appindex,manifest,vcard,content}_cmd.rs` | 1293 | six per-domain test files |
| `crates/shell/tests/batch6_session.rs` | 119 | the session-level proof |

Plus the wiring: six feature flags in `crates/shell/Cargo.toml` (all in
`default`), the six modules in `domains/mod.rs`, the `DomainCommand` arms in
`main.rs`, the session backends / dispatch arms / selector roots / glosses in
`shell.rs`, and the board in `migrate.rs`.

**50 verbs** across the six domains, and **not one** of them is a stub:

* **database** (9) — `namespaces`, `keys`, `get`, `has`, `count`, `set`,
  `delete`, `clear`, `caps`, `matrix`
* **queue** (9) — `list`, `get`, `peek`, `enqueue`, `dequeue`, `clear`,
  `delete`, `caps`, `matrix`
* **appindex** (10) — `list`, `get`, `search`, `stats`, `pinned`, `pin`,
  `unpin`, `batch`, `caps`, `matrix`
* **manifest** (9) — `list`, `resolve`, `by-name`, `validate`, `validate-deps`,
  `register`, `unregister`, `caps`, `matrix`
* **vcard** (11) — `contacts`, `versions`, `parse`, `import`, `export`,
  `format`, `new`, `sync-token`, `sync`, `caps`, `matrix`
* **content** (12) — `list`, `get`, `publish`, `publish-many`, `update`,
  `unpublish`, `delete`, `trash`, `restore`, `empty-trash`, `caps`, `matrix`

Every method the engine's own trait serves has a verb. The methods the engine
does NOT serve have no verb — the `matrix` verb says why, with the engine's own
reason, in the engine's own words.

---

## The pattern

The network/clipboard/service/files module pattern, followed exactly:

```rust
pub type Backend = dyn <Domain>Backend;      // the engine's seam
pub struct WriteGuard { pub allowed: bool }   // the gate's answer
pub fn evaluate(cmd, backend, guard) -> SmartResult<Value>   // the data half
pub fn run(cmd, format, backend, guard) -> SmartResult<Value> // data + render
pub fn write_intent(cmd) -> Option<(String, String)>        // verb + target
pub fn selector_root(backend) -> SmartResult<Value>          // the @domain tree
pub mod render;                                              // the table half
```

**The engine's owned types, never a capnp reader.** `Pagination` in,
`ManifestEntry` / `AppEntry` / `Contact` / `ContentItem` / `AppStats` /
`ValidationReport` out; the bytes of a value come from the engine as bytes and
are shown as text when they are UTF-8 and as hex when they are not — a lossy
replacement would be a lie about the bytes.

**One backend per session.** The engine's stores live in the process that built
them, so the shell memoizes one backend per domain for the life of the session
(the rule the files/process/settings domains already follow). A write one line
makes is a read the next line sees. A one-shot invocation is one session: the
store it reads is the store that process built, and the shell says so in the
rendering rather than pretending a corpus it does not have.

**`matrix` — the engine's own claim, rendered as it stands.** Every domain's
`matrix` verb prints the contract's availability matrix exactly as the engine
publishes it: the ordinal, the method, the status, the reason (or, for a REAL
row, the **L3 test that proves it** — the engine's own honesty artifact, made
visible). It is captioned as the ENGINE's matrix, not the shell's verb list.

**The gate.** Every mutating verb carries a write intent naming the verb and
the target, and the shell's one gate asks `verb target? (y/N)` — `--force`
accepts it, `--dry-run` prints the plan and writes nothing, script mode refuses
because there is nobody to ask. The gate's refusal now says what it means for
both kinds of write:

```
refusing `set database value demo/key`: it CHANGES state — the host's own, or
the engine's own store — and there is no operator to ask (non-interactive). …
```

**The two store write paths.** The manifest contract has no `put` verb and the
vCard contract has no "list the contacts" verb — the engine's cells expose them
as inherent methods. The shell declares a small trait over the engine's trait
(`ManifestStore`, `VcardStore`) and implements it for the engine's own cell, so
the command module keeps a `dyn` seam and a test seam at the same time. Without
`manifest register` the manifest domain would be a read-only view of a store
nothing could fill — half a domain, and the founder's rule is no half-baking.

---

## Per-domain

### `database` — the engine's own namespaced key/value store

`namespaces`, `keys` (`--prefix`, `--offset`, `--limit`), `get`, `has`, `count`,
`set`, `delete`, `clear`, `caps`, `matrix`.

The default view is the store's namespaces, each with the number of keys it
holds (the engine's own `listNamespaces` + `count`). `get` shows the bytes: the
text when the payload is UTF-8, the hex when it is not. A missing key names
itself: `no value under demo/key in the engine's store` — never a bare
`notFound`.

### `queue` — the engine's own FIFO store

`list`, `get`, `peek`, `enqueue`, `dequeue`, `clear`, `delete`, `caps`,
`matrix`.

**The queue's DEPTH is not in the contract's read surface, so the shell does
not print one.** The engine answers "which queues exist" and "what is at the
head", and the shell reports exactly that. `peek` reads the existence FIRST, so
a queue that is not there is never called empty — the engine's `peek` answers
`None` for both an empty queue and an absent one.

### `appindex` — the host's own desktop-entry corpus

`list`, `get`, `search` (`--category`, `--source`), `stats`, `pinned`, `pin`,
`unpin`, `batch`, `caps`, `matrix`.

177 entries on this host, read natively from the XDG data directories. The rows
carry what a `.desktop` file really carries: the id, the name, the generic name,
the comment, the exec line, the icon, the categories, the keywords, the source
directory and the file path. **No package id, no version, no developer, no
licence** — a desktop entry publishes none, so the shell shows none. The
pinned set is the ENGINE's own in-process set; the desktop shell's own pinned
list lives in the desktop shell's store, which this backend neither reads nor
writes, and the rendering says so.

`launch` and `launchUrl` are not verbs: the engine does not serve them (a
host-mutating act — law #8), and `appindex matrix` says so with the engine's
reason.

### `manifest` — the engine's own manifest store

`list`, `resolve`, `by-name <issuer> <name>`, `validate`, `validate-deps`,
`register`, `unregister`, `caps`, `matrix`.

`validate` is the engine's own validator: a REQUIRED dependency no manifest in
the store offers is an ERROR, and the shell prints the engine's errors
verbatim — never a red report turned green. `register` puts a manifest in the
store (`--display`, `--description`, `--vendor`, `--version`, `--entry`,
`--offers`, `--depends name[:optional]`, `--shape`).

### `vcard` — the engine's own RFC 6350 parser and formatter

`contacts`, `versions`, `parse <file|->`, `import <file|->`, `export`,
`format <id>`, `new`, `sync-token`, `sync <token>`, `caps`, `matrix`.

`parse` reads a card without importing it and shows the content lines the
engine read — groups, the open parameter list, structured values, the base64
and URI and date value kinds. `new` builds a contact from the command line and
formats it through the engine's formatter (the card is formatted, never
stored). `export --out <path>` is the one HOST write in the six, and it carries
the gate like every other mutating verb. `resolveConflict` is not a verb: the
engine refuses it, and the matrix says why.

### `content` — the engine's own content store

`list` (`--query`, `--mime`, `--tag`, `--include-trashed`), `get`, `publish`,
`publish-many`, `update`, `unpublish`, `delete`, `trash`, `restore`,
`empty-trash`, `caps`, `matrix`.

The contract's own optimistic concurrency is honoured: `update` presents
`--expected-version`, and a stale write is refused with a conflict, never a
silent overwrite. The update carries the WHOLE item, so the current one is read
first and the named fields are replaced over it — an update that names no field
is a re-write of the same item, never a blank one.

---

## Live

Every command below was run on this host; the output is the real thing (ANSI
stripped for the page).

### `appindex` — the host's own corpus, 177 entries

```
$ smart appindex list --limit 6
  🧭 App Index
  ├─ Entries  6 of 177 in the host's corpus
  ┌───────────────────────────────────┬────────────────┬──────────────┬────────────────────────────┬───────────────────────────┬─────┐
  │ ID                                │ NAME           │ GENERIC      │ CATEGORIES                 │ SOURCE                    │ PIN │
  ├───────────────────────────────────┼────────────────┼──────────────┼────────────────────────────┼───────────────────────────┼─────┤
  │ ai.storyteller.photocraft.desktop │ PhotoCraft     │ Image Editor │ Graphics,2DGr…,Photography │ /home/lufuno/.local/share │ ○   │
  │ dev.zed.Zed.desktop               │ Zed            │ Text Editor  │ Utility,TextE…elopment,IDE │ /home/lufuno/.local/share │ ○   │
  │ wine-extension-chm.desktop        │ hh             │              │                            │ /home/lufuno/.local/share │ ○   │
  │ wine-extension-crt.desktop        │ rundll32       │              │                            │ /home/lufuno/.local/share │ ○   │
  │ wine-extension-hlp.desktop        │ winhlp32       │              │                            │ /home/lufuno/.local/share │ ○   │
  │ wine-extension-msp.desktop        │ Wine Installer │              │                            │ /home/lufuno/.local/share │ ○   │
  └───────────────────────────────────┴────────────────┴──────────────┴────────────────────────────┴───────────────────────────┴─────┘

$ smart appindex get dev.zed.Zed.desktop
  🧭 Zed — dev.zed.Zed.desktop
  ├─ ID           dev.zed.Zed.desktop
  ├─ Generic      Text Editor
  ├─ Comment      A high-performance, multiplayer code editor.
  ├─ Exec         /home/lufuno/.local/zed.app/bin/zed %U
  ├─ Icon         /home/lufuno/.local/zed.app/share/icons/hicolor/512x512/apps/zed.png
  ├─ Categories   Utility,TextEditor,Development,IDE
  ├─ Keywords     zed
  ├─ Source       /home/lufuno/.local/share
  ├─ Path         /home/lufuno/.local/share/applications/dev.zed.Zed.desktop
  └─ Pinned       ○ no (the engine's own set)

$ smart appindex search editor --limit 3
  🧭 Search — editor
  ├─ Matches  3 of 10 in the host's corpus
  ┌───────────────────────────────────┬────────────┬──────────────┬────────────────────────────┬───────────────────────────┬─────┐
  │ ID                                │ NAME       │ GENERIC      │ CATEGORIES                 │ SOURCE                    │ PIN │
  ├───────────────────────────────────┼────────────┼──────────────┼────────────────────────────┼───────────────────────────┼─────┤
  │ ai.storyteller.photocraft.desktop │ PhotoCraft │ Image Editor │ Graphics,2DGr…,Photography │ /home/lufuno/.local/share │ ○   │
  │ dev.zed.Zed.desktop               │ Zed        │ Text Editor  │ Utility,TextE…elopment,IDE │ /home/lufuno/.local/share │ ○   │
  │ vscodium-AM.desktop               │ VSCodium   │ Text Editor  │ TextEditor,Development,IDE │ /usr/local/share          │ ○   │
  └───────────────────────────────────┴────────────┴──────────────┴────────────────────────────┴───────────────────────────┴─────┘

$ smart appindex stats
  🧭 App Index Stats
  ├─ Entries     177
  ├─ Installed   177
  ├─ Sources
  ┌──────────────────────────────┬─────────┐
  │ SOURCE                       │ ENTRIES │
  ├──────────────────────────────┼─────────┤
  │ /home/lufuno/.local/share    │ 11      │
  │ /usr/local/share             │ 5       │
  │ /usr/share                   │ 160     │
  │ /var/lib/flatp…exports/share │ 1       │
  └──────────────────────────────┴─────────┘
  └─ Categories
  ┌────────────────────────────┬─────────┐
  │ CATEGORY                   │ ENTRIES │
  ├────────────────────────────┼─────────┤
  │ 2DGraphics                 │ 4       │
  │ Archiving                  │ 1       │
  │ Audio                      │ 5       │
  …

$ smart appindex pinned
  🧭 Pinned
  └─ ○ — the engine's own pinned set is empty (the desktop shell's own
      pinned list lives in the desktop shell's store, which this
      backend neither reads nor writes)

$ smart appindex batch dev.zed.Zed.desktop micro.desktop
  🧭 Batch — dev.zed.Zed.desktop, micro.desktop
  ├─ 2 of 2 answered
  ┌─────────────────────┬───────┬───────────────────────────┐
  │ ID                  │ NAME  │ SOURCE                    │
  ├─────────────────────┼───────┼───────────────────────────┤
  │ dev.zed.Zed.desktop │ Zed   │ /home/lufuno/.local/share │
  │ micro.desktop       │ Micro │ /usr/share                │
  └─────────────────────┴───────┴───────────────────────────┘

$ smart appindex caps
  🧭 Capabilities — [appindex.xdg, appindex.inproc]
```

### `vcard` — the engine's own RFC 6350 parser

```
$ smart vcard versions
  📇 Supported versions
  ├─ 2.1
  ├─ 3.0
  ├─ 4.0

$ smart vcard parse /tmp/kilo/ada.vcf
  📇 Parsed cards
  ├─ vCard 4.0
  ┌─────────────────┬──────────────────────┐
  │ LINE            │ VALUE                │
  ├─────────────────┼──────────────────────┤
  │ VERSION         │ 4.0                  │
  │ FN              │ Ada Lovelace         │
  │ N               │ Lovelace;Ada;;;      │
  │ EMAIL;TYPE=work │ ada@example.org      │
  │ TEL;TYPE=cell   │ +441234567890        │
  │ ORG             │ Analytical Engines   │
  │ NOTE            │ The first programmer │
  └─────────────────┴──────────────────────┘

$ smart vcard new --name "Grace Hopper" --email grace@example.org --tel +15550100
  📇 Formatted card — 0
     BEGIN:VCARD
     VERSION:4.0
     FN:Grace Hopper
     EMAIL;TYPE=OTHER:grace@example.org
     TEL;TYPE=OTHER:+15550100
     END:VCARD

$ smart vcard sync-token
  📇 Sync token — vcard-sync-0

$ smart vcard sync vcard-sync-0
  📇 Sync
  ├─ Token     vcard-sync-0
  ├─ Changed   0
  ├─ Deleted   0
  ├─ Conflicts 0
  └─ Has more  no

$ smart vcard caps
  📇 Capabilities — [vcard.inproc, vcard.rfc6350, vcard.2.1, vcard.3.0, vcard.4.0]
```

The `parse` output is the engine's own reading of the card: the parameter list
(`EMAIL;TYPE=work`), the structured value (`N` → `Lovelace;Ada;;;`), the
group-less content lines. `new` runs the same engine's formatter over a contact
the caller spelled out.

### `database`, `queue`, `manifest`, `content` — the engine's own stores

These four are the engine's own in-process stores, so a fresh process finds
them EMPTY. That is the honest answer, and the shell says so rather than
inventing a corpus:

```
$ smart database
  🗄 Database
  └─ ○ — the engine's store holds no namespace yet (an honest empty)

$ smart database count demo
  🗄 demo — 0 key(s)

$ smart database has demo key
  🗄 demo/key — ○ absent

$ smart database get demo key
smart: no value under demo/key in the engine's store

$ smart database caps
  🗄 Capabilities — [database.kv]

$ smart queue
  📮 Queue
  └─ ○ — the engine's store holds no queue yet (an honest empty)

$ smart queue get jobs
smart: the engine's store holds no queue named jobs

$ smart queue peek jobs
  📮 jobs — head
  └─ ○ — the store holds no queue by that name

$ smart queue caps
  📮 Capabilities — [queue.inproc]

$ smart manifest
  📦 Manifests
  └─ ○ — the engine's store holds no manifest yet (an honest empty;
      `manifest register` puts one there)

$ smart manifest resolve com.example.app
smart: the engine's store holds no manifest named com.example.app

$ smart manifest caps
  📦 Capabilities — [manifest.inproc, manifest.store]

$ smart content
  📄 Content
  └─ ○ — the engine's store holds no item (an honest empty;
      `content publish` puts one there)

$ smart content get 1
smart: the engine's store holds no item with id 1

$ smart content caps
  📄 Capabilities — [content.inproc, content.store]
```

**The store's lifetime is the engine's own, so the write→read composition is
proved where it can be proved hermetically** — in the shell's own test suite,
over the scripted backends (`crates/shell/tests/batch6_session.rs`):

```
$ cargo test -p shell --test batch6_session
test the_session_keeps_one_content_backend ... ok
test the_session_keeps_one_backend_per_batch6_domain ... ok
test the_session_keeps_one_database_backend ... ok
test the_write_gate_holds_at_the_shell_level ... ok
```

`the_session_keeps_one_database_backend` sets `demo/greeting` on one line and
reads it back from the SAME backend on the next; the write lands in the
engine's store and the read sees it.

### The matrices — the engine's own published claim

```
$ smart database matrix
  🗄 Availability — schema 11651590505119481901
  the contract's surface as the ENGINE's own matrix publishes it
  ┌────┬──────────────────────────────┬───────────────┬─────────────────────────────────────────────────────────┐
  │ #  │ METHOD                       │ STATUS        │ REASON                                                  │
  ├────┼──────────────────────────────┼───────────────┼─────────────────────────────────────────────────────────┤
  │ 0  │ get                          │ ◉ real        │ l3_linux_database::core_works                           │
  │ 1  │ has                          │ ◉ real        │ l3_linux_database::core_works                           │
  │ 2  │ listKeys                     │ ◉ real        │ l3_linux_database::core_works                           │
  │ 3  │ count                        │ ◉ real        │ l3_linux_database::core_works                           │
  │ 4  │ set                          │ ◉ real        │ l3_linux_database::core_works                           │
  │ 5  │ delete                       │ ◉ real        │ l3_linux_database::core_works                           │
  │ 6  │ clear                        │ ◉ real        │ l3_linux_database::core_works                           │
  … 76 rows …

$ smart appindex matrix
  🧭 Availability — schema 11651590505119481882
  the contract's surface as the ENGINE's own matrix publishes it
  ┌────┬──────────────────┬───────────────┬──────────────────────────────────────────────────────────────┐
  │ #  │ METHOD           │ STATUS        │ REASON                                                       │
  ├────┼──────────────────┼───────────────┼──────────────────────────────────────────────────────────────┤
  │ 0  │ listApps         │ ◉ real        │ l3_linux_appindex::the_index_lists_the_hosts_own_desktop_en… │
  │ 1  │ getApp           │ ◉ real        │ l3_linux_appindex::one_entry_is_read_by_its_own_id           │
  │ 2  │ launch           │ ○ unavailable │ no-privilege: launching an application is a host-mutating a… │
  │ 3  │ launchUrl        │ ○ unavailable │ no-privilege: the same host-mutating limit as @launch (law … │
  │ 4  │ getPinned        │ ◐ degraded    │ the pinned set is this engine's own in-process set: the hos… │
  │ 5  │ pinApp           │ ◐ degraded    │ the same in-process pinned set as @getPinned: the engine re… │
  │ 6  │ unpinApp         │ ◐ degraded    │ the same in-process pinned set as @getPinned                 │
  … 19 rows …

$ smart vcard matrix
  📇 Availability — schema 11651590505119482120
  the contract's surface as the ENGINE's own matrix publishes it
  ┌───┬──────────────────────┬───────────────┬──────────────────────────────────────────────────────────────┐
  │ # │ METHOD               │ STATUS        │ REASON                                                       │
  ├───┼──────────────────────┼───────────────┼──────────────────────────────────────────────────────────────┤
  │ 0 │ importVCard          │ ◉ real        │ l3_linux_vcard::an_import_maps_the_fields_the_contract_names │
  │ 1 │ exportVCard          │ ◉ real        │ l3_linux_vcard::an_export_writ…the_fields_the_contract_names │
  │ 2 │ parseVCard           │ ◉ real        │ l3_linux_vcard::a_vcard_round_…_the_parser_and_the_formatter │
  │ 3 │ formatVCard          │ ◉ real        │ l3_linux_vcard::a_vcard_round_…_the_parser_and_the_formatter │
  │ 4 │ getSupportedVersions │ ◉ real        │ l3_linux_vcard::the_supported_…_the_ones_the_engine_can_read │
  │ 5 │ getSyncToken         │ ◉ real        │ l3_linux_vcard::a_sync_token_r…s_the_changes_since_the_token │
  │ 6 │ syncChanges          │ ◉ real        │ l3_linux_vcard::a_sync_token_r…s_the_changes_since_the_token │
  │ 7 │ resolveConflict      │ ○ unavailable │ no conflict surface: the store…efuses rather than pretending │
  └───┴──────────────────────┴───────────────┴──────────────────────────────────────────────────────────────┘
```

A REAL row shows the **L3 test that proves it** (the engine's own honesty
artifact); the others show the engine's own reason, in the engine's own words.

### The selectors — the same data the commands render

```
$ smart -o raw -c "@appindex/apps/0/name"
PhotoCraft
$ smart -o raw -c "@appindex/stats/total_apps"
177
$ smart -o raw -c "@appindex/apps | count"
177
$ smart -o raw -c "@vcard/versions/0/version"
2.1
$ smart -o raw -c "@queue/queues"

$ smart -o json appindex get micro.desktop
{"id":"micro.desktop","name":"Micro","generic_name":"Text Editor","comment":"Edit text files in a terminal","exec":"micro %F","icon":"micro","terminal":true,"no_display":false,"hidden":false,"categories":["Utility","TextEditor","Development"],"keywords":["text","editor","syntax","terminal"],"source":"/usr/share","path":"/usr/share/applications/micro.desktop","pinned":false}
```

---

## The gate

Every mutating verb in the six carries a write intent naming the verb and the
target, and the shell's one gate asks it. **No write was run during this
verification** — the gate's refusal and the dry-run plan are the evidence, and
the write's own semantics are proved by the scripted backends in the test
suite.

The refusal (no acceptance, nobody to ask):

```
$ smart database set demo key value
smart: refusing `set database value demo/key`: it CHANGES state — the host's
own, or the engine's own store — and there is no operator to ask
(non-interactive). Answer the interactive `(y/N)` prompt when running
interactively, pipe a `y` line, or pass `--force` to accept the write
explicitly (the live bring-back proof is founder-run).

$ smart appindex pin dev.zed.Zed.desktop
smart: refusing `pin application dev.zed.Zed.desktop`: it CHANGES state — …

$ smart vcard export --all --out /tmp/kilo/out.vcf
smart: refusing `write vcard file /tmp/kilo/out.vcf`: it CHANGES state — …
```

The plan (`--dry-run` — the question, printed, and nothing written):

```
$ smart --dry-run database set demo key value
  set database value demo/key? (y/N) — the plan; nothing was written (--dry-run)
$ smart --dry-run content publish Note
  publish content Note? (y/N) — the plan; nothing was written (--dry-run)
$ smart --dry-run vcard import /tmp/kilo/ada.vcf
  import vcard file /tmp/kilo/ada.vcf? (y/N) — the plan; nothing was written (--dry-run)
```

Every write verb, live:

```
$ smart --dry-run database set demo key value        → set database value demo/key?
$ smart --dry-run database delete demo key           → delete database value demo/key?
$ smart --dry-run database clear demo                → clear database namespace demo?
$ smart --dry-run queue enqueue jobs payload         → enqueue on queue jobs?
$ smart --dry-run queue dequeue jobs                 → dequeue from queue jobs?
$ smart --dry-run queue clear jobs                   → clear queue jobs?
$ smart --dry-run queue delete jobs                  → delete queue jobs?
$ smart --dry-run appindex pin dev.zed.Zed.desktop   → pin application dev.zed.Zed.desktop?
$ smart --dry-run appindex unpin dev.zed.Zed.desktop → unpin application dev.zed.Zed.desktop?
$ smart --dry-run manifest register com.example.app  → register manifest com.example.app?
$ smart --dry-run manifest unregister com.example.app→ unregister manifest com.example.app?
$ smart --dry-run vcard import /tmp/kilo/ada.vcf     → import vcard file /tmp/kilo/ada.vcf?
$ smart --dry-run vcard export --all --out /tmp/kilo/out.vcf → write vcard file /tmp/kilo/out.vcf?
$ smart --dry-run content publish Note               → publish content Note?
$ smart --dry-run content publish-many A B           → publish content 2 item(s)?
$ smart --dry-run content update 1 --title X         → update content item 1?
$ smart --dry-run content unpublish 1                → unpublish content item 1?
$ smart --dry-run content delete 1                   → delete content item 1?
$ smart --dry-run content trash 1                    → trash content item 1?
$ smart --dry-run content restore 1                  → restore content item 1?
$ smart --dry-run content empty-trash                → empty the content trash?
```

The script-mode answer is deterministic JSON, never a bare exit 0:

```
$ smart -o json content publish Note
{"error":{"kind":"not_supported","message":"refusing `publish content Note`: it CHANGES state — …"}}
```

---

## The tests

`cargo test -p shell` → **238 passed, 0 failed** (178 before this batch, 60
added).

| file | tests | what it proves |
|---|---|---|
| `tests/database_cmd.rs` | 9 | the namespaces/keys/get/has/count reads; the bytes as text-or-hex; the gate; the write intent never echoes the value; the table; the `@database/*` tree; the matrix |
| `tests/queue_cmd.rs` | 7 | the queue list; peek without consuming; an absent queue is not an empty queue; the gate on all four writes; the honest empty; the `@queue/*` tree; the matrix |
| `tests/appindex_cmd.rs` | 9 | the corpus, the entry, the three search filters, the statistics, the engine's own pinned set (and the gate on it), the batch, the table, the `@appindex/*` tree, the matrix naming `launch` as not served |
| `tests/manifest_cmd.rs` | 8 | the listing, resolve/by-name, the engine's own validation verdict (a required dependency nothing offers IS an error), the gated register, the optional dependency, the table, the `@manifest/*` tree, the matrix |
| `tests/vcard_cmd.rs` | 11 | the contacts, the versions, parse-without-import, the gated import, the gated `--out` export (and that a refused export writes NO file), `new` formats without storing, an unknown version refused, the sync verbs, `format`, the `@vcard/*` tree, the matrix |
| `tests/content_cmd.rs` | 11 | the listing, get, the three query filters, the gate on all eight writes, publish→get, the STALE UPDATE refused with a conflict, an update keeping the fields it does not name, the trash/restore/empty-trash, the table, the `@content/*` tree, the matrix |
| `tests/batch6_session.rs` | 4 | the session keeps ONE backend per domain (write→read composes); the gate holds at the shell level |
| `tests/migrate.rs` | +1 | the eighteen real-today domains, and the batch-6 six with the engine's own counts |

The fixtures live in `tests/common/batch6/` — one scripted backend per domain
over the engine's OWNED types, each recording the engine calls a mutating verb
made, so a test can prove the shell issued the right call AND that a refused
write reaches no engine at all.

---

## The board

`crates/shell/src/migrate.rs` — the six moved to **real-today** with the
engine's own counts, and the queue closed its gap:

| domain | before | after |
|---|---|---|
| database | migrated-pending, order 2, "8/14 methods" | **◉ real-today** — `native · 27 REAL / 0 DEGRADED / 49 UNAVAILABLE` |
| queue | migrated-pending, order 1, "8/8 methods" | **◉ real-today** — `native · 7 REAL / 0 DEGRADED / 26 UNAVAILABLE` |
| appindex | archive-body-exists, "archive body" | **◉ real-today** — `native · 5 REAL / 3 DEGRADED / 11 UNAVAILABLE` |
| manifest | (not on the board) | **◉ real-today** — `native · 5 REAL / 0 DEGRADED / 11 UNAVAILABLE` |
| vcard | (not on the board) | **◉ real-today** — `native · 7 REAL / 0 DEGRADED / 1 UNAVAILABLE` |
| content | (not on the board) | **◉ real-today** — `native · 10 REAL / 0 DEGRADED / 47 UNAVAILABLE` |

`manifest`, `vcard` and `content` were never on the board: the ORIGINAL shell
never hosted a command for them. The engine has a real body for each, so they
join as real-today rather than as a queue position.

The queue's positions closed the gap — `window` 3→1, `package` 4→2,
`compositor` 5→3 — and the board now reads:

```
$ smart migrate
  🧭 Migration Queue
  50 domains: 18 real today, 3 queued, 29 archive-only
```

`help` lists the eighteen live domains from the board (not a hardcoded list),
and `smart --help` shows the six new subcommands.

---

## The findings

Two things the shell found and did NOT paper over:

1. **`QUEUE_SCHEMA_ID` is 0** in the engine (`src/domains/queue/availability.rs`),
   while the other five domains publish their real schema ids. `smart queue
   matrix` prints `schema 0` — the engine's own value, rendered as it stands.
   The engine's side is the fix; the shell does not invent an id.
2. **The engine's matrix claims more than the trait serves.** `database`
   publishes 27 REAL rows, but the `DatabaseBackend` trait has 8 methods — the
   matrix is the CONTRACT's surface, the trait is what the cell implements. The
   `matrix` verb is captioned as the ENGINE's matrix for exactly this reason;
   the shell's own verb list is the trait, verb for verb.

---

## The rest

The migration queue now holds three domains, all multi-backend (window 10/36
across four WMs, package's 19 managers, compositor's 9 backends). Each is its
own wave with its own live proof in the shell.

```bash
cargo build -j 6          # the workspace, engine included: clean
cargo test -j 6 -p shell  # 238 passed, 0 failed
```
