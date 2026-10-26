//! The app entity: the store, the worker bridge, the decoder bridge, and the
//! window policy.
//!
//! ## Why one entity owns the state and the views are thin
//!
//! Two windows (the library and the quick palette) render the *same* list. If
//! each view held its own copy, they could disagree about what the service
//! said — and a clipboard tool that shows two different answers in two windows
//! is worse than useless. So there is exactly one [`Store`], one worker
//! connection and one decoder; the views observe this entity and re-render
//! from it.
//!
//! ## What crosses the boundary
//!
//! ```text
//!   worker thread ──BackendEvent──▶ ClipboardApp::on_backend ──▶ Store::apply
//!                                        │                          │
//!                                        │◀──────── effects (Vec<Command>) ─┘
//!                                        ▼
//!                                  WorkerHandle::send
//! ```
//!
//! The store is pure and returns commands; this module is the only place that
//! reaches for the worker. That is what lets the reducer be unit-tested with
//! no runtime at all.
//!
//! ## The one place the UI does its own work
//!
//! Image decoding happens on its own thread (see [`crate::preview`]), because
//! the alternative is dropping frames while scrolling a gallery of 4K
//! screenshots. A 1 Hz tick keeps relative timestamps honest and expires
//! toasts; it is the only timer in the app.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    point, px, size, App, Bounds, Context, Entity, RenderImage, SharedString, WindowBounds,
    WindowHandle, WindowKind, WindowOptions,
};
use smart_clipboard_proto::{
    BackendEvent, Command, ItemId, ServiceLocation, Worker, WorkerHandle,
};

use crate::preview::{self, DecodeRequest, Decoded};
use crate::store::{Store, ToastKind};
use crate::theme::Theme;
use crate::views::palette::PaletteView;

/// How long a toast lives.
pub const TOAST_TTL: Duration = Duration::from_secs(7);

/// How long the search waits after the last keystroke before asking the
/// service. Short enough to feel live on a local socket, long enough that
/// typing "screenshot" is one round trip rather than ten.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(140);

/// A decoded thumbnail, or the reason there is not one.
pub enum Thumb {
    /// The payload is being fetched or decoded.
    Pending,
    /// Ready to draw.
    Ready(Arc<RenderImage>),
    /// The payload cannot be drawn, with a reason the inspector shows.
    Failed(String),
}

/// Work the UI scheduled for itself.
enum UiEvent {
    /// A payload finished decoding (or did not).
    Decoded(Decoded),
    /// The search debounce elapsed for this generation.
    Debounce(u64),
    /// The 1 Hz tick: refresh relative times, expire toasts.
    Tick(u64),
}

/// The application state entity.
pub struct ClipboardApp {
    /// Everything the views render.
    pub store: Store,
    /// The active token set.
    pub theme: Theme,
    /// The protocol worker.
    pub worker: WorkerHandle,
    /// Decoded thumbnails by item id.
    pub thumbs: HashMap<u64, Thumb>,
    /// The palette window, when one is open.
    pub palette: Option<WindowHandle<PaletteView>>,
    /// The item awaiting delete confirmation. A destructive verb the app does
    /// not own gets a confirm step, always.
    pub pending_delete: Option<ItemId>,
    /// Which inspector tab is showing.
    pub inspector_tab: InspectorTab,
    /// The search generation, for the debounce.
    query_generation: u64,
    /// The endpoints the app will try, for the connection panel.
    pub location: ServiceLocation,
    /// The decoder queue.
    decoder: std::sync::mpsc::Sender<DecodeRequest>,
    /// The UI's own event channel (decoder results, debounce, tick).
    ui_tx: async_channel::Sender<UiEvent>,
}

/// The inspector's two faces: the payload, and the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorTab {
    /// What the item *is*.
    Payload,
    /// Where it came from: the service, the method matrix, the capability
    /// rows. The tab nobody opens until something is wrong — and then the
    /// only one that helps.
    Contract,
}

impl ClipboardApp {
    /// Build the app, start the worker and the decoder, and begin streaming.
    pub fn new(location: ServiceLocation, cx: &mut Context<Self>) -> Self {
        let (worker, backend_events) = Worker::spawn(location.clone());
        let (decoder, decoded) = preview::spawn_decoder();
        let (ui_tx, ui_events) = async_channel::unbounded::<UiEvent>();

        // The worker's stream.
        cx.spawn(async move |this, cx| {
            while let Ok(event) = backend_events.recv().await {
                let _ = this.update(cx, |this, cx| this.on_backend(event, cx));
            }
        })
        .detach();

        // The UI's own work (decoded payloads, debounce, tick).
        cx.spawn(async move |this, cx| {
            while let Ok(event) = ui_events.recv().await {
                let _ = this.update(cx, |this, cx| this.on_ui(event, cx));
            }
        })
        .detach();

        // The tick. A thread rather than an executor timer because it must
        // keep running while the foreground executor is busy painting.
        {
            let ui_tx = ui_tx.clone();
            std::thread::Builder::new()
                .name("smart-clipboard-tick".to_string())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(1));
                    if ui_tx.send_blocking(UiEvent::Tick(now_ns())).is_err() {
                        return;
                    }
                })
                .expect("the tick thread must start");
        }

        let mut store = Store::new(true);
        store.toast(
            ToastKind::Info,
            "smart-clipboard",
            "This app stores nothing. Every row comes from the service's own history.",
        );

        ClipboardApp {
            store,
            theme: Theme::dark(),
            worker,
            thumbs: HashMap::new(),
            palette: None,
            pending_delete: None,
            inspector_tab: InspectorTab::Payload,
            query_generation: 0,
            location,
            decoder,
            ui_tx,
        }
    }

    /// Send commands to the worker. The single door to the wire.
    pub fn dispatch(&mut self, commands: Vec<Command>) {
        for command in commands {
            self.worker.send(command);
        }
    }

    /// Ask the worker to connect from the top of the candidate list.
    pub fn connect(&mut self) {
        self.dispatch(vec![Command::Connect]);
    }

    // ── events in ─────────────────────────────────────────────────────────

    fn on_backend(&mut self, event: BackendEvent, cx: &mut Context<Self>) {
        let effects = self.store.apply(event, now_ns());
        self.dispatch(effects);

        // A payload that arrived and can be drawn goes to the decoder.
        self.sweep_decodes(cx);

        // Both windows read this entity, so a change in one shows in the
        // other. gpui re-renders every observer of this entity.
        cx.notify();
    }

    fn on_ui(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::Decoded(Decoded::Ready {
                id,
                frame,
                original,
                animated,
            }) => {
                let _ = (original, animated);
                self.thumbs
                    .insert(id, Thumb::Ready(Arc::new(RenderImage::new(smallvec::smallvec![
                        frame
                    ]))));
            }
            UiEvent::Decoded(Decoded::Failed { id, reason }) => {
                self.thumbs.insert(id, Thumb::Failed(reason));
            }
            UiEvent::Debounce(generation) => {
                if generation == self.query_generation {
                    self.commit_query();
                }
            }
            UiEvent::Tick(_) => {
                self.store.expire_toasts(TOAST_TTL);
            }
        }
        cx.notify();
    }

    // ── search ───────────────────────────────────────────────────────────

    /// A character typed into the always-live search.
    pub fn push_char(&mut self, character: char) {
        self.store.query.push(character);
        self.schedule_search();
    }

    /// Backspace in the always-live search.
    pub fn backspace(&mut self) {
        self.store.query.pop();
        self.schedule_search();
    }

    /// Clear the query and re-read the unfiltered history.
    pub fn clear_query(&mut self) {
        if self.store.query.is_empty() {
            return;
        }
        self.store.query.clear();
        self.query_generation += 1;
        self.commit_query();
    }

    /// Run the query now (Enter), skipping the debounce.
    pub fn commit_query(&mut self) {
        let text = self.store.query.clone();
        let effects = self.store.set_query(text);
        self.dispatch(effects);
    }

    fn schedule_search(&mut self) {
        self.query_generation += 1;
        let generation = self.query_generation;
        let ui_tx = self.ui_tx.clone();
        // A short-lived thread per keystroke burst. Cheaper than a timer
        // wheel for four call sites, and it cannot block the renderer.
        std::thread::spawn(move || {
            std::thread::sleep(SEARCH_DEBOUNCE);
            let _ = ui_tx.send_blocking(UiEvent::Debounce(generation));
        });
    }

    // ── selection and actions ────────────────────────────────────────────

    /// Select an item in both windows.
    pub fn select(&mut self, id: ItemId) {
        let effects = self.store.select(id);
        self.dispatch(effects);
    }

    /// Move the selection by `delta`.
    pub fn move_selection(&mut self, delta: isize) {
        let effects = self.store.move_selection(delta);
        self.dispatch(effects);
        self.sweep_decodes_quiet();
    }

    /// Copy the selection back through the service.
    pub fn copy(&mut self, all_representations: bool) {
        if self.store.selected.is_none() {
            self.store
                .toast(ToastKind::Info, "nothing selected", "Pick a row first.");
            return;
        }
        let effects = self.store.copy_selected(all_representations);
        self.dispatch(effects);
    }

    /// Copy one named representation.
    pub fn copy_representation(&mut self, id: ItemId, mime: String) {
        let effects = self.store.copy_representation(id, mime);
        self.dispatch(effects);
    }

    /// Pin or unpin the selection.
    pub fn toggle_pin(&mut self) {
        if self.store.selected.is_none() {
            return;
        }
        let effects = self.store.pin_selected();
        self.dispatch(effects);
    }

    /// Ask to delete the selection. The confirmation is a UI step, not an
    /// optimisation: the verb removes a record the app did not create.
    pub fn request_delete(&mut self) {
        if let Some(id) = self.store.selected {
            self.pending_delete = Some(id);
        }
    }

    /// Confirm the pending delete.
    pub fn confirm_delete(&mut self) {
        if self.pending_delete.take().is_some() {
            let effects = self.store.delete_selected();
            self.dispatch(effects);
        }
    }

    /// Cancel the pending delete.
    pub fn cancel_delete(&mut self) {
        self.pending_delete = None;
    }

    /// Re-read the live clipboard.
    pub fn read_live(&mut self) {
        let effects = self.store.read_live();
        self.dispatch(effects);
    }

    /// Refresh the window.
    pub fn refresh(&mut self) {
        let effects = self.store.refresh();
        self.dispatch(effects);
    }

    /// Fetch the next page.
    pub fn load_more(&mut self) {
        let effects = self.store.load_more();
        self.dispatch(effects);
    }

    /// Cycle the sort order.
    pub fn cycle_sort(&mut self) {
        self.store.sort = self.store.sort.next();
        self.store.toast(
            ToastKind::Info,
            format!("sort: {}", self.store.sort.label()),
            "Sorting is applied to the loaded window only — the contract has no sort parameter.",
        );
    }

    /// Toggle list/gallery.
    pub fn toggle_layout(&mut self) {
        self.store.layout = match self.store.layout {
            crate::store::Layout::List => crate::store::Layout::Gallery,
            crate::store::Layout::Gallery => crate::store::Layout::List,
        };
        // A gallery needs thumbnails for what is about to be on screen.
        let effects = self.store.request_missing_thumbnails(24);
        self.dispatch(effects);
    }

    /// Toggle the theme.
    pub fn toggle_theme(&mut self) {
        self.store.theme_dark = !self.store.theme_dark;
        self.theme = Theme::for_mode(self.store.theme_dark);
    }

    /// Switch the inspector's tab.
    pub fn set_inspector_tab(&mut self, tab: InspectorTab) {
        self.inspector_tab = tab;
    }

    /// Ask the worker to reconnect.
    pub fn reconnect(&mut self) {
        let effects = self.store.reconnect();
        self.dispatch(effects);
    }

    /// Whether this item's payload can be offered for drawing.
    pub fn thumb(&self, id: u64) -> Option<&Thumb> {
        self.thumbs.get(&id)
    }

    // ── decode plumbing ──────────────────────────────────────────────────

    /// Hand any undecoded image payloads to the decoder.
    fn sweep_decodes(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<(u64, Vec<u8>, String)> = self
            .store
            .contents
            .iter()
            .filter_map(|(id, state)| match state {
                smart_clipboard_proto::ContentState::Ready { mime, bytes } => {
                    if preview::is_drawable(mime) && !self.thumbs.contains_key(id) {
                        Some((*id, bytes.clone(), mime.clone()))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        if pending.is_empty() {
            return;
        }
        for (id, bytes, _mime) in pending {
            self.thumbs.insert(id, Thumb::Pending);
            let _ = self.decoder.send(DecodeRequest { id, bytes });
        }
        cx.notify();
    }

    /// The same sweep, without a context (used from key handlers that already
    /// hold one; kept separate so the borrow rules stay obvious).
    fn sweep_decodes_quiet(&mut self) {
        let pending: Vec<(u64, Vec<u8>)> = self
            .store
            .contents
            .iter()
            .filter_map(|(id, state)| match state {
                smart_clipboard_proto::ContentState::Ready { mime, bytes } => {
                    if preview::is_drawable(mime) && !self.thumbs.contains_key(id) {
                        Some((*id, bytes.clone()))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        for (id, bytes) in pending {
            self.thumbs.insert(id, Thumb::Pending);
            let _ = self.decoder.send(DecodeRequest { id, bytes });
        }
    }

    /// Ask for the thumbnails a gallery or the inspector is about to need.
    pub fn request_thumbnails(&mut self, budget: usize) {
        let effects = self.store.request_missing_thumbnails(budget);
        self.dispatch(effects);
    }

    /// The app's own copy of the theme, for views that clone it for a frame.
    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }
}

/// Nanoseconds since the epoch, the contract's timestamp unit.
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default()
}

// ── window policy ─────────────────────────────────────────────────────────

/// The library window: a normal window, sized for a three-pane layout, with
/// the platform's own decorations. A custom titlebar would buy 28 px of
/// chrome and cost every platform's window-drag, snap and accessibility
/// behaviour; for a utility window open *next to* other windows, native
/// chrome is the better trade.
pub fn library_window_options(title: &str) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(120.0), px(120.0)),
            size: size(px(1180.0), px(760.0)),
        })),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(SharedString::from(title.to_string())),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        kind: WindowKind::Normal,
        focus: true,
        show: true,
        ..Default::default()
    }
}

/// The quick palette: a borderless, always-on-top window with a transparent
/// background so its rounded card can float. It opens docked to the top
/// centre of the screen — the position every command palette uses, because
/// the eye goes up and the pointer is already near the middle.
///
/// The position is a fixed guess at "top centre" (a 680 px card 180 px down):
/// gpui exposes the display list, and centring properly is a one-line change
/// once the app cares which monitor the pointer is on. Until then this is
/// honest about being a default, not a placement engine.
pub fn palette_window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(620.0), px(180.0)),
            size: size(px(680.0), px(480.0)),
        })),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(SharedString::from("smart-clipboard palette".to_string())),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        kind: WindowKind::PopUp,
        focus: true,
        show: true,
        window_background_appearance: gpui::WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// Open the palette, or focus the one already open.
pub fn open_palette(app: &Entity<ClipboardApp>, cx: &mut App) {
    if let Some(handle) = app.read(cx).palette {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let entity = app.clone();
    let opened = cx.open_window(palette_window_options(), move |window, cx| {
        let view = cx.new(|cx| PaletteView::new(entity, window, cx));
        view
    });
    if let Ok(handle) = opened {
        app.update(cx, |this, cx| {
            this.palette = Some(handle);
            cx.notify();
        });
    }
}

/// Note: `open_palette` closes over the app entity so the palette always talks
/// to the same store, and the palette never owns a connection of its own.
pub fn close_palette(app: &Entity<ClipboardApp>, window: &mut gpui::Window, cx: &mut App) {
    app.update(cx, |this, _| this.palette = None);
    window.remove_window();
}
