# 30 — smart-clipboard: a clipboard app that owns nothing

> Status: **spec + first implementation**. Product: `crates/smart-clipboard` (gpui desktop
> app) and `crates/smart-clipboard-proto` (contract-only client library). This document is
> the authority on *why* the product is shaped the way it is; the code carries the *how*.

---

## 0. The one paragraph

**smart-clipboard is a window onto the clipboard contract.** It reads history, previews and
payloads from whatever service serves `clipboard.capnp`, searches that history, and asks the
service to hold an item again. It has no clipboard backend, no selection transport, no
history ledger and no database — the backend owns all four, and the app learns about them
only by speaking capnp. It is decoupled on purpose: any implementation of the contract, on
any OS, by any third party, is a valid backend for this app, and this app is a valid client
for any implementation.

---

## 1. Why this product exists, and what it refuses to be

A clipboard history is the highest-value, lowest-trust surface on a desktop: it holds
passwords, tokens, half-written messages, client names, screenshots of someone else's
screen. Existing clipboard managers answer "where does this go?" with "our SQLite file".

smart-clipboard answers it with the contract:

| Question | Answer |
|---|---|
| Where is the history? | In the backend that serves the clipboard domain. Not here. |
| Who captured this? | Whatever the backend records, as its own `sourceId`. Not a guess. |
| Why can't I see this image? | Because the service answered `unknown` / `refused` / `unsupported` — and the app shows *that sentence*. |
| What happens if I delete the app? | Nothing. The history was never here. |
| Can the app see my clipboard when it is closed? | No. It has no selection transport at all; it cannot read a clipboard, only ask a service. |
| Does it paste for me? | No. It copies; you paste. Injecting keystrokes is a different domain's capability and this app does not hold it. |

**Non-goals, stated so they cannot creep back in:**

1. **No backend.** No `wl-paste`, no `xclip`, no `wlr-data-control`, no X11 selection code,
   not even as a "fallback". A fallback would make the app an implementation and break the
   decoupling that is the entire point.
2. **No store.** No SQLite, no sled, no JSON cache on disk. Zero bytes written. (An in-memory
   thumbnail cache is not a store: it dies with the process, and it is re-derivable.)
3. **No daemon.** The capture loop belongs to the service. This app is a client that can be
   closed, and closing it must not stop capture.
4. **No input synthesis.** No `Ctrl+V`, no `ydotool`, no portals for remote desktop. The
   footer of the palette says so out loud.
5. **No telemetry, no accounts, no sync.** Those belong to a service, and if a service ever
   offers them it offers them through its own contract surface, not through this window.

---

## 2. Architecture: schemas are the API

```
┌────────────────────────────┐
│  smart-clipboard (gpui)    │   crates/smart-clipboard
│  rail · rows · inspector   │   NO engine dependency, NO capnp readers in views
└─────────────┬──────────────┘
              │ Command ←→ BackendEvent      (two enums, no gpui types)
┌─────────────▼──────────────┐
│ smart-clipboard-proto      │   crates/smart-clipboard-proto
│ worker thread + LocalSet   │   compiles clipboard.capnp, discovery.capnp,
│ discovery → clipboard → cap│   capability.capnp, types.capnp from the canonical tree
└─────────────┬──────────────┘
              │ capnp RPC over the unix carrier
┌─────────────▼──────────────┐
│ smart-service (any impl)   │   the backend: platform selection cells,
│ clipboard · discovery · cap│   ledger/history, database, capture loop
└────────────────────────────┘
```

Three properties make this decoupled in practice, not just in a diagram:

* **The app compiles the contract itself.** `crates/smart-clipboard-proto/build.rs` compiles
  four schemas from `smart-protocol-official` and includes only those generated modules. It
  does not import `smart_lib::schema`, so anything private the engine does tomorrow cannot
  leak into the product.
* **The app links no backend.** `smart-clipboard-proto` has zero dependencies on the engine
  (`cargo tree` on the binary shows capnp, tokio, image, gpui — no smart-lib). The engine
  appears only as a **dev-dependency** of the proto crate's tests, where it serves a mock
  backend so the client can be proven against a real server.
* **The seam is two enums.** `Command` (UI → wire) and `BackendEvent` (wire → UI). No future,
  reader, client handle or `Rc` crosses it. That is also what makes the whole UI testable
  with a fake worker and the reducer testable with no runtime.

### 2.1 Why a separate proto crate

Three reasons, in order of weight:

1. It can be tested and reused headlessly. An agent, a future CLI, or a different shell can
   take exactly this code; the UI crate is the only place gpui exists.
2. It keeps the contract surface honest. Every wire detail lives in one crate of ~1.5k lines
   where a reviewer can see every field the product touches.
3. It bounds the build. gpui's dependency tree is large; `cargo build` for the engine never
   sees it, because both crates are `exclude`d from the engine workspace (charter law #10 —
   never let a product's build fall on the engine's box).

---

## 3. The contract surface this product uses

Only what the UI actually needs. Anything else the app could call is deliberately not called
yet, and the "Method matrix" in the inspector shows **which methods this session actually
exercised** — observed, never assumed, because the contract publishes no method list.

| Contract method | Used for | UI surface |
|---|---|---|
| `discovery.lookup` | find the `clipboard` service + its schema id | connection panel, status bar |
| `capability_service.list` | what this connection may do | status bar, Contract tab |
| `getHistory` | the unfiltered window | list, total count |
| `searchHistory` | the text query | search field (debounced 140 ms) |
| `getPreview` | the row snippet + inspector preview | row subtitle, inspector |
| `convertContent` | the bytes for a thumbnail / "copy as" | gallery, inspector, copy |
| `setContent` | copy one representation back | Enter, Copy |
| `setOffer` | offer every representation at once | Shift+Enter, "Offer all" |
| `pinItem` / `unpinItem` | pin against the backend's expiry | P, inspector |
| `deleteItem` | remove from the backend's history | ⌫ (+ confirmation) |
| `clearHistory` | (wired, behind a confirm; no UI in v1 besides the inspector's verb list) | — |
| `subscribeChanges` + `pull` | the capture stream | status bar "capture: live", ticker, auto-refresh |
| `getContent` | "clipboard now" strip | inspector with no selection |
| `getSupportedMimeTypes` | (called for the provenance block) | Contract tab |

Deliberately not used in v1: `getSizeLimits` / `setSizeLimits` (a policy the backend owns;
the inspector shows the *client's* ceiling instead, labelled as such), `validateContent`
(a backend-side integrity check; the UI shows the MIME the service recorded rather than
re-validating), `clearContent` (clearing the *selection* is not a history operation and the
app has no business erasing what the user currently has).

---

## 4. Discovery and connection: a candidate list, shown

There is no endpoint registry in the contract yet, so "how does a client find the service"
is answered with a **candidate list in one place** (`config.rs`), tried in order:

1. `--socket` / `--addr` on this process's command line;
2. `$SMART_SERVICE_SOCKET`;
3. `$XDG_RUNTIME_DIR/smart/service.sock`;
4. `/tmp/smart-service.sock` — the unix transport's published default.

**The list itself is the UI.** When nothing answers, the list pane becomes the *connection
panel*: every candidate, where it came from, and what it said, verbatim. Then two fixes:
the one-line command to start a service, and the flag to point the app at one. A spinner
would be a lie about what the app knows; the app knows exactly four things it tried.

The handshake, and what each step can fail with:

```
dial(unix) ──▶ bootstrap capability
   │                │
   │                ├─ discovery.lookup{name:"clipboard"}
   │                │     ├─ 0 services → DomainNotServed{published: [...names...]}
   │                │     └─ 1 service  → descriptor{id, state, endpoints, metadata}
   │                │            └─ id ≠ 0xa1b2c3d4e5f60012 → **drift**, shown not fatal
   │                └─ capability.list → Authority{rows, operations, class}
   │
   └─ failure = Disconnected → banner + one automatic retry after 3 s
```

A **refusal never triggers a reconnect.** The service answered; the refusal *is* the answer.

---

## 5. The authority model, in UI terms

The app never decides whether it may write. `capability.list` publishes what this connection
holds (`smart-lib:control` with `write`, or `smart-lib:readonly` without). The app uses that
for **hints only**:

| Published authority | What the UI does |
|---|---|
| `write` present | verbs look normal |
| no `write` | verbs are still callable; the inspector says "this connection is read-only, so the service will refuse — the refusal is the service's answer, not the app's guess" |
| nothing published | "the service published no authority for this connection"; verbs still callable |

And the refusal itself is a first-class object: `Refusal { verb, class, reason, raw }`. The
class token survives the wire because the engine prefixes it (`permissionDenied: …`,
`no-backend: …`), so the UI can say *which kind of no* it got, and the inspector's Contract
tab shows the exact bytes the service sent with a note that they are safe to paste into a
bug report.

---

## 6. UX principles (the eight that decided every layout)

1. **Provenance over polish.** Every pane can answer "where did this come from?" without a
   menu. The status bar is permanent for this reason.
2. **Keyboard is the primary input.** A clipboard is used mid-sentence. Every action has a
   key; every key is discoverable in a footer; nothing is pointer-only.
3. **Reasons, not placeholders.** A missing preview shows the service's reason, not a grey
   box. A refused verb shows the class and the sentence. Nothing is ever "loading…" forever.
4. **Local work is labelled local.** Facets, sort and the secret heuristic are client-side
   and *say so* — "filters apply to the loaded window only — the contract's search is text".
   This is the single most important honesty rule in the product, because it is the easiest
   one to quietly break.
5. **The list is the service's.** A capture event triggers a re-read; the app never invents
   a row from an event payload.
6. **Destructive verbs are confirmed; everything else is instant.** Only `deleteItem`
   (and a future `clearHistory`) gets a dialog.
7. **Density with air.** 64 px rows, 13 px text, 12/16 px gaps: the densest layout that still
   lets a snippet be read, because scanning beats decoration.
8. **No motion.** No shimmer, no spring, no cross-fade. The app's states change because data
   arrived, not to entertain. (It also means reduced-motion users get the same app.)

---

## 7. Information architecture

```
smart-clipboard
├── Library window (normal window, 1180×760, native decorations)
│   ├── Header (52 px)      product mark · search field · gallery · sort · refresh · palette
│   ├── Rail (168 px)       All · Text · Links · Images · Files · Pinned · Sensitive + counts
│   ├── Rows (flex)         window label · rows | gallery · load-more + key hints
│   ├── Inspector (340 px)  tabs: Payload | Contract
│   └── Status bar (26 px)  connection · provenance · class · methods · capture · write
└── Quick palette window (PopUp, 680×480, borderless card)
    ├── Search (52 px)      first focus, type-to-filter
    ├── Six rows (56 px)    34 px mark · snippet · metadata · pin · time
    └── Footer (30 px)      ⏎ copy & close · ⇧⏎ copy & stay · P pin · provenance
```

### 7.1 Wireframes (dark theme tokens in brackets)

**Library, connected, rows selected:**

```
┌───────────────────────────────────────────────────────────────────────────────────┐
│ ▪ smart-clipboard   ⌕ Search the service's history…            Gallery Sort ↻ ⌘K │ [panel]
├──────────┬────────────────────────────────────────────────────┬───────────────────┤
│ KINDS    │ 42 of 318 on the service      [server search] [order]│ PAYLOAD │ Contract│
│ All   318│ ┌──────────────────────────────────────────────────┐ │ ┌───────────────┐ │
│ Text  201│ │ ▤  The deploy script now retries three times…    │ │ │  (image 217×96│ │
│ Links  34│ │    text/plain · 1.4 KB · 2m ago          text    │ │ │   or text)    │ │
│ Images 41│ ├──────────────────────────────────────────────────┤ │ └───────────────┘ │
│ Files  12│ │ ▣  Image — no text preview (the service keeps…   │ │ id        #4412   │
│ Pinned  3│ │    image/png · 220 KB · 8m ago          image    │ │ captured  2026-…  │
│ Sensitive│ ├──────────────────────────────────────────────────┤ │ size      220 KB  │
│          │ │ ▤  https://smart.foundation/spec/clipboard       │ │ source    backend │
│──────────│ │    text/plain · 84 B · 11m ago          link     │ │ tags      none    │
│ filters  │ └──────────────────────────────────────────────────┘ │                   │
│ apply to │                                                       │ representations   │
│ the      │                                                       │ image/png  Copy as│
│ loaded   │                                                       │ text/plain Copy as│
│ window   │                                                       │                   │
│ ● unix://│                                                       │ [Copy][Offer all] │
│ live     │                                                       │ [Pin]  [Delete]   │
├──────────┴───────────────────────────────────────────────────────┴───────────────────┤
│ ● unix:///tmp/smart-service.sock  clipboard · online · schema 0x…12   control · 9/17 │
│   capture: live                                        write: allowed · no disk writes│
└───────────────────────────────────────────────────────────────────────────────────────┘
```

**Connection panel (nothing answered):**

```
┌──────────────────────────────────────────────────────────────────────────┐
│ No service answered — unix:///tmp/smart-service.sock: No such file       │
│                                                                          │
│ smart-clipboard is a client. It needs a service that serves the          │
│ clipboard contract — the backend, its history and its store live there.  │
│                                                                          │
│ ENDPOINTS TRIED, IN ORDER                                                │
│ ┌──────────────────────────────────────────────────────────────────────┐ │
│ │ ● unix:///tmp/smart-service.sock  [transport default]                │ │
│ │   No such file or directory (os error 2)                             │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│ HOW TO FIX IT                                                            │
│ cargo run --example smart-service -- --socket /tmp/smart-service.sock    │
│ or point this app at a running service with --socket / $SMART_SERVICE_SOCKET │
│ [ Retry ]  [ Copy socket path ]   this app never spawns the service      │
└──────────────────────────────────────────────────────────────────────────┘
```

**Quick palette:**

```
        ╭───────────────────────────────────────────────────────────╮
        │ ◫  Search the clipboard…                            [esc] │
        ├───────────────────────────────────────────────────────────┤
        │ ▤  The deploy script now retries three times…    pinned 2m│
        │ ▣  Image — no text preview             image/png  8m ago  │
        │ ▤  https://smart.foundation/spec/clipboard  link 11m ago  │
        ╰───────────────────────────────────────────────────────────╯
           ⏎ copy & close   ⇧⏎ copy & stay   P pin     318 on the service
           smart-clipboard copies; it never pastes for you
```

---

## 8. The layout decisions, and the alternatives that lost

### 8.1 Rail vs tabs vs dropdown

The rail shows **all seven facets and their counts at once**. A clipboard's facet counts are
information, not navigation: seeing "Images 41" is how a user discovers that screenshots are
even in there. Tabs would hide six of seven; a dropdown would hide the counts. Cost: 168 px
of permanent width, which the row area can afford at 1180 px window width.

### 8.2 Rows vs cards vs masonry

Rows win because the dominant task is *text recognition*: a left-aligned column of snippets
at a stable x-position is scannable in a way cards are not (cards scatter the anchor and pad
away 40% of the width). Gallery exists as one keystroke (`G`) for the opposite task, "find
that screenshot", where recognition is visual and a 184×112 thumbnail beats any description.
Neither mode is a fallback for the other; both are first-class and labelled.

### 8.3 Inspector vs modal vs hover card

A modal steals the list; a hover card cannot hold provenance or actions and dies when the
pointer moves. A fixed 340 px column keeps the list visible while the *reason* for a
missing preview is readable at the same time — which is exactly the moment a user is
confused. It also gives the Contract tab a permanent home, so the app's honesty surface is
not hidden behind a debug flag.

### 8.4 Status bar vs toasts-only

Toasts are transient, and provenance must not be. The status bar carries the endpoint, the
provenance string (`clipboard · online · schema 0x…`), the capability class, the observed
method count, the capture state and the write hint. Toasts carry *events* (a copy landed, a
verb was refused). Two channels, two jobs.

### 8.5 Connection panel vs empty state vs spinner

Covered in §4: the panel is the candidate list plus the fix. It is the first screen a new
user sees, so it is the one that must be most honest.

### 8.6 Two windows vs one

The library is a workspace; the palette is a reflex. Reflex must not require the workspace to
be open, focused, or on the right monitor. Both share one `Store` and one protocol session,
so they cannot disagree.

### 8.7 Native chrome vs custom titlebar

Native. A custom titlebar buys 28 px and costs every platform's drag, snap, a11y and
window-menu behaviour. For a utility window that lives *next to* other windows, that trade is
bad.

---

## 9. Interaction model

### 9.1 Keyboard (library)

| Key | Action | Why |
|---|---|---|
| any printable | append to search (always live, 140 ms debounce) | there is no other text field; a mode would be ceremony |
| `↑` `↓` | move selection | — |
| `⏎` | copy the item's first representation via `setContent` | the single most common action |
| `⇧⏎` | `setOffer` with every representation the service can produce | power move: a paste target that wants HTML gets HTML |
| `P` | pin / unpin | — |
| `⌫` | delete, with a confirmation | destructive; confirmed |
| `G` | list ↔ gallery | — |
| `S` | cycle sort (service order → oldest → largest → pinned first) | each is labelled "loaded window only" |
| `T` | theme | shown, not stored (see §13) |
| `R` | refresh | — |
| `⌘/Ctrl+K` | open the palette | — |
| `⌘/Ctrl+F` | read the live selection | — |
| `esc` | clear the query; if empty, deselect | two-stage, matching user intent |

### 9.2 Keyboard (palette)

`⏎` copy & close · `⇧⏎` copy & stay · `↑↓` move · `P` pin · `⌫` edit · `esc` close. The
palette's Escape *closes*, the library's *clears*, because the palette has no other way out.

### 9.3 Pointer

Row click selects (never copies — a mis-click must not overwrite the clipboard). Gallery tile
click selects. Buttons are ≥ 28 px tall. No nested click targets inside a row: a copy button
*inside* a clickable row is how "I clicked copy and it selected instead" bugs happen.

### 9.4 Clipboard writes are always explicit

Nothing in the app changes the clipboard as a side effect of navigation. Only `⏎`, `⇧⏎`,
"Copy", "Copy as", "Offer all", or a palette row click do — and each one says so in a toast
with the MIME and the byte count the service accepted.

---

## 10. States: the complete matrix

Nothing in this app is allowed to be in an undefined state. Every combination renders
something a user can act on.

| Dimension | States | Rendered as |
|---|---|---|
| Connection | `Idle` / `Dialling` / `Connected` / `Lost{retrying}` / `Failed` | status bar badge; the list pane becomes the connection panel when not connected |
| Page | `loading` / loaded / empty / `exhausted` | footer label + empty state; "end of the service's history" is a real state |
| Row preview | `NotRequested` / `Loading` / `Ready` / `Unavailable{reason}` / `Refused{class}` / `Failed{msg}` | skeleton / skeleton / snippet / "no text preview (the service keeps none…)" / class + sentence / error text |
| Payload bytes | `NotRequested` / `Loading` / `Ready` / `Unsupported{target}` / `Refused` / `TooLarge{bytes,limit}` / `Failed` | image / skeleton / image / "the service answered unsupported for …" / class + sentence / client-limit notice / error |
| Thumbnail decode | `Pending` / `Ready` / `Failed{reason}` | family mark / image / "could not be drawn: …" |
| Authority | `Some(true)` / `Some(false)` / `None` | "write: allowed" / "read-only connection" / "not published" |
| Selection | none / item | "clipboard now" card / payload tab |
| Facet | admits N / hides M | "M hidden by filter" in the header; empty state names the facet |
| Schema | matches / drift | status bar suffix `· drift`; Contract tab warning block |
| Toasts | 0..4 | stacked bottom-right, 7 s TTL, duplicates collapse |

---

## 11. Visual design

### 11.1 Tokens

| Token | Dark | Light | Rationale |
|---|---|---|---|
| `canvas` | `#0b0d10` | `#fbfbfc` | near-neutral, 1–2% warm: never tints a screenshot |
| `panel` | `#11141a` | `#f4f5f7` | one step up for chrome |
| `raised` | `#181c23` | `#ffffff` | hover / palette card / modal |
| `selection` | `#1d2536` | `#e8ecfb` | the only large accent fill |
| `line` | `#232833` | `#e2e5ea` | hairlines instead of shadows |
| `text` | `#e7eaf0` | `#14181f` | — |
| `text_muted` | `#9aa3b2` | `#5a6473` | metadata |
| `text_faint` | `#6b7484` | `#848d9c` | units, hints |
| `signal` | `#7c8cff` | `#4154d6` | ONE accent: caret, selection, primary action |
| `good` / `warn` / `bad` | `#49d19a` / `#e8b65a` / `#ff7b72` | `#13855e` / `#996a12` / `#c23b34` | always paired with a word |
| family colours | text `#a8b4c8` · link `#7fd0ff` · image `#c79bff` · file `#f2c46b` | desaturated | marks are distinguishable by **silhouette first** |

Type: 11 / 12 / 13 / 15 / 17 / 22 px at a 1.36 line ratio. Spacing: 2 / 4 / 6 / 8 / 12 / 16 /
24. Radii: 6 / 8 / 12 / pill. Weights: normal / medium / semibold, named by role.

### 11.2 Why no icon font, no shadows, no glass

* **No icon font**: a licence, a fallback-rendering problem per distro, and six shapes. The
  six family marks are composed from divs — identical everywhere, scales with the layout,
  costs nothing to load, and each has a distinct silhouette so it survives greyscale.
* **No shadows on panels**: on compositors without blur a shadow reads as dirt around the
  edges, and a 1 px hairline is crisper at fractional HiDPI scales.
* **No glass/blur**: it makes a *preview* window compete with the content it previews, and
  `WindowBackgroundAppearance::Blurred` is not available on every compositor. The palette
  asks for transparency (so its rounded card floats) and nothing more.
* **No motion**: state changes are data arriving; decorating that is noise, and it would need
  a reduced-motion path for no gain.

### 11.3 Density

Row 64 px = 40 px mark + two 13/11 px lines + 12 px gaps. This is the densest height that
still fits ~160 characters of snippet, which is the point at which two different text
payloads stop being confusable.

---

## 12. Performance model

| Concern | Decision |
|---|---|
| Wire latency | never on the UI thread: one worker thread owns the session; UI sends `Command`, receives `BackendEvent` |
| Page size | 200 items per `getHistory`/`searchHistory` (explicit, because the contract defines `limit == 0` as "one") |
| Previews | a page of 200 metadata-only rows needs 200 `getPreview` calls — asked sequentially for the first 48 visible rows, marked `Loading` so the UI shows skeletons fill top-down rather than flicker |
| Thumbnails | fetched only for image rows the user can see (budget 24), decoded on a dedicated thread, downscaled to 768 px longest side |
| Payload ceiling | 64 MiB, enforced **before** the call from the service's own reported `sizeBytes`, labelled a client policy |
| Sort/filter | local over the loaded window only, O(n log n) on ≤ a few thousand rows |
| Repaint | one 1 Hz tick for relative times and toast expiry; everything else is event-driven |
| Memory | no persistent cache; thumbnails are `Arc<RenderImage>` in a map that dies with the process |

---

## 13. Security and privacy

1. **The app never reads the clipboard itself.** It cannot: it links no selection transport.
2. **Nothing is persisted.** No history, no query log, no window geometry, no theme. The only
   disk-adjacent thing is the app binary. (Theme and layout are session-scoped; persisting
   them would need the settings domain, which is a roadmap item, and doing it in a bespoke
   file would create a second source of truth.)
3. **URLs are not auto-opened.** A URL on the clipboard is untrusted input; the inspector
   shows the full string and explains why there is no "open" button.
4. **Text is never rendered as markup.** No HTML, no SVG rendering from payloads: text is
   text. (SVG is *not* in the drawable set, deliberately.)
5. **Secret heuristic is local and advisory.** Prefix/shape based, conservative, never
   redacts, never uploads, and labelled "a local heuristic on this device".
6. **Refusals are displayed verbatim.** A reason string from the service is data, not code:
   it is rendered as text in a monospaced block, never interpreted.
7. **Deleting is confirmed and is the service's operation.** The app cannot restore what it
   never had.

---

## 14. Accessibility

* **Keyboard-complete.** Every action has a key; nothing is pointer-only.
* **No colour-only signals.** Facet marks differ in silhouette; states differ in words
  ("refused", "read-only connection", "not served").
* **Contrast.** Body text ≥ 7:1 on its surface in both themes; faint text is used only for
  non-essential units and hints, never for a message the user must act on.
* **Type.** 11 px is the floor and only for units; body is 13 px; nothing is smaller than
  10 px (key hints).
* **Hit targets.** Buttons 28 px, rows 64 px, rail items 28 px with 8 px gaps.
* **Motion.** None, so no reduced-motion variant is needed.
* **Focus.** The window root tracks one focus handle; the search field's border turns
  `signal` when focused, so "where do my keystrokes go?" has a visual answer.
* **Screen readers / a11y tree.** gpui exposes an accessibility tree; this version sets
  labels on interactive elements where the API allows it, and the full a11y pass (roles,
  live regions for toasts) is a tracked roadmap item — stated honestly rather than claimed.

---

## 15. Testing plan

| Layer | What is proven | Where |
|---|---|---|
| Pure formatting | byte units, relative/absolute time (incl. a leap day), snippet truncation on char boundaries, colour/URL literals, secret heuristic | `src/format.rs` unit tests |
| Reducer | selection + preview requests; capture triggers a re-read (never a local insert); stale page dropped; facet hide count; sort orders; keyboard clamping; refusal → matrix + toast with class; duplicate toast collapse | `src/store.rs` unit tests |
| Config | flag > env > xdg > default; dedupe; empty env ignored | `proto/src/config.rs` tests |
| Error mapping | reason-class token survives the wire, including the `remote exception:` decoration | `proto/src/error.rs` (tests in-crate) |
| Decoder | small/large/rubbish payloads; drawable MIME set | `src/preview.rs` tests |
| **Contract round-trip** | a real `Session` against a real served backend: discovery descriptor + schema id, history units, preview `unknown` vs served, `setContent` reaching the backend, read-only refusal keeping `permissionDenied`, worker walking every candidate | `proto/tests/contract_roundtrip.rs` (engine as dev-dependency only) |

Not yet covered, and named so it is not implied: visual regression, a11y tree assertions,
and a live-service smoke test on a Wayland/X11 seat (needs a session, and per charter law #8
the live write proof is the founder's to run).

---

## 16. Build and run

```bash
# the app's two crates are excluded from the engine workspace on purpose
cd crates/smart-clipboard
SMART_SCHEMA_DIR=/path/to/smart-protocol-official/schema cargo run --release

# a service to talk to (engine, separate workspace)
cd ../..
cargo run --example smart-service --features rpc,std,domain-clipboard \
  -- --socket /tmp/smart-service.sock
```

Toolchain: gpui 0.2.2 needs a recent stable Rust (async closures, 1.85+). If the published
crate lags, pin the engine's `rev` in `crates/smart-clipboard/Cargo.toml`.

> **Honest status of this commit.** The Rust toolchain and crates.io are unreachable from the
> environment this was written in, so neither crate has been compiled here. The API surfaces
> used were checked against upstream gpui `main` (window options, `uniform_list`,
> `RenderImage::new(image::Frame)`, `Context::{spawn, listener, processor, observe}`), and the
> two places where gpui's API is most likely to drift are isolated: window dismissal
> (`app::close_palette`) and image construction (`app::on_ui`). Everything below the gpui
> line — the protocol crate, the reducer, the formatters — is dependency-light Rust that
> compiles or does not, with no runtime behaviour to guess at.

---

## 17. Roadmap, in the order the honesty argument forces

1. **Compile + run on a real seat**, with the founder's live-service proof (read + one write).
2. **Global shortcut** for the palette. Not a contract feature: a compositor binding that
   runs `smart-clipboard --palette`, or a portal `GlobalShortcuts` client. The window
   management already supports opening the palette on demand.
3. **`clearHistory` UI** behind a typed confirmation (the verb is wired; the surface is not).
4. **Settings via the settings domain** (theme, page size, preview budget) — the first thing
   this app should persist, and it must persist through the contract, not a private file.
5. **Multi-monitor palette placement** via the display list.
6. **Accessibility pass**: roles, labels, live regions for toasts.
7. **Collections / saved searches** — needs a contract surface that does not exist yet; not
   faked in the app.

---

## 18. Open questions (decisions I did not take alone, written down)

* **Should the app remember the theme?** It does not today. Doing it right means
  `settings.capnp`, which means a service that serves settings; a private config file would
  break "the app owns nothing" for 40 bytes of convenience.
* **Should `Sensitive` be a facet at all?** It is a heuristic wearing a filter's clothes. It
  is included because accidentally-shared secrets are the failure this product should
  prevent, and it is labelled local everywhere. If it ever produces noise, delete it — not
  the label.
* **Should the palette paste?** No, and the reasoning is in §9; revisit only if an input
  domain's contract surface arrives and the user grants it explicitly.
* **Should stale previews be re-asked?** Today a preview is asked once per item per session.
  A backend that mutates an item in place would need cache invalidation from `pullChanges`;
  the contract's `expired` event is the hook.
