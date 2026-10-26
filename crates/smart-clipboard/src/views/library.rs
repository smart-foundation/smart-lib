//! The library window: **rail · rows · inspector**, plus a status bar that
//! never lies.
//!
//! ## Why this layout, in one paragraph each
//!
//! **A rail, not tabs.** A clipboard has exactly one axis a person sorts by
//! ("what kind of thing is it?"), and the rail shows all seven values at once
//! with their counts. Tabs would hide six behind one, and a dropdown would
//! hide the counts. The rail is 168 px because the longest label is "Sensitive"
//! at 12 px type; anything wider is a column of whitespace.
//!
//! **Rows, not cards.** The dominant task is "find the text I copied". Rows
//! give every item the same textual anchor at the same x-position, so the eye
//! scans a left-aligned column of snippets; cards scatter the anchor and waste
//! 40% of the width on padding. Rows are 64 px — two lines of 13 px text plus
//! a 40 px thumbnail — which is the densest height that still fits a
//! distinguishing amount of the payload. A gallery exists for the second task
//! ("find that screenshot"), one keystroke away.
//!
//! **An inspector, not a modal.** Selecting a row must not cost the user the
//! list. The inspector is a fixed 340 px column that shows the payload, the
//! provenance and the actions — and, crucially, the *reasons*: a preview that
//! was not served, a verb that was refused, a schema that drifted. A modal
//! would hide the list behind the explanation of why the list is incomplete.
//!
//! **A status bar, not a toast.** Every claim the service makes about itself
//! (endpoint, capability class, schema id, how many contract methods have
//! answered) lives in one 26 px strip at the bottom. It is always visible,
//! never dismissible, and never decorative: this app's whole proposition is
//! that it shows you where the data came from.
//!
//! **A connection panel, not a spinner.** When no service answers, the list
//! pane becomes the *candidate list*: every endpoint the app tried, where that
//! candidate came from, and what each one said. The fix (`--socket`, or start
//! the service) is one line below it. A spinner there would be an insult.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    div, img, px, uniform_list, AnyElement, ClickEvent, Context, Div, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, ObjectFit, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, Window,
};
use smart_clipboard_proto::{Item, ItemId, MimeFamily, PreviewState};

use crate::app::{open_palette, ClipboardApp, Thumb};
use crate::format;
use crate::store::{Connection, Facet, Layout, Sort, ToastKind};
use crate::theme::{alpha, Radius, Space, Theme, Type, Weight};
use crate::ui;
use crate::views::inspector::InspectorView;

/// The library window's root view.
pub struct LibraryView {
    app: Entity<ClipboardApp>,
    focus: FocusHandle,
    /// The inspector is its own view so it can keep its own scroll position
    /// and its own tab, and so both windows can embed the same panel.
    inspector: Entity<InspectorView>,
    _observed: Subscription,
}

impl LibraryView {
    /// Build the view and give it the keyboard.
    pub fn new(app: Entity<ClipboardApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let observed = cx.observe(&app, |_, _, cx| cx.notify());
        let inspector = {
            let app = app.clone();
            cx.new(|cx| InspectorView::new(app, cx))
        };
        window.focus(&focus, cx);
        // The first thing the app does is connect, once, from the view that
        // is guaranteed to exist.
        app.update(cx, |app, _| app.connect());
        LibraryView {
            app,
            focus,
            inspector,
            _observed: observed,
        }
    }

    // ── keyboard ─────────────────────────────────────────────────────────

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;

        // A delete confirmation swallows the keyboard: a destructive verb must
        // not race a shortcut.
        if self.app.read(cx).pending_delete.is_some() {
            match keystroke.key.as_str() {
                "enter" => self.app.update(cx, |app, cx| {
                    app.confirm_delete();
                    cx.notify();
                }),
                "escape" => self.app.update(cx, |app, cx| {
                    app.cancel_delete();
                    cx.notify();
                }),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        match keystroke.key.as_str() {
            "up" => {
                self.app.update(cx, |app, cx| {
                    app.move_selection(-1);
                    cx.notify();
                });
            }
            "down" => {
                self.app.update(cx, |app, cx| {
                    app.move_selection(1);
                    cx.notify();
                });
            }
            "enter" => {
                self.app.update(cx, |app, cx| {
                    app.copy(modifiers.shift);
                    cx.notify();
                });
            }
            "escape" => {
                self.app.update(cx, |app, cx| {
                    if app.store.query.is_empty() {
                        app.store.selected = None;
                    } else {
                        app.clear_query();
                    }
                    cx.notify();
                });
            }
            "backspace" => {
                // Backspace edits the search when there is a search to edit;
                // the destructive verb is Delete, deliberately.
                self.app.update(cx, |app, cx| {
                    if !app.store.query.is_empty() {
                        app.backspace();
                        cx.notify();
                    }
                });
            }
            "delete" => {
                self.app.update(cx, |app, cx| {
                    app.request_delete();
                    cx.notify();
                });
            }
            "/" if !command => {
                // The search is always live; "/" is the muscle memory from
                // every other tool and simply clears the field to start.
                self.app.update(cx, |app, cx| {
                    app.clear_query();
                    cx.notify();
                });
            }
            "p" if !command => {
                self.app.update(cx, |app, cx| {
                    app.toggle_pin();
                    cx.notify();
                });
            }
            "g" if !command => {
                self.app.update(cx, |app, cx| {
                    app.toggle_layout();
                    cx.notify();
                });
            }
            "s" if !command => {
                self.app.update(cx, |app, cx| {
                    app.cycle_sort();
                    cx.notify();
                });
            }
            "t" if !command => {
                self.app.update(cx, |app, cx| {
                    app.toggle_theme();
                    cx.notify();
                });
            }
            "r" if !command => {
                self.app.update(cx, |app, cx| {
                    app.refresh();
                    cx.notify();
                });
            }
            "k" if command => {
                open_palette(&self.app, cx);
                window.focus(&self.focus, cx);
            }
            "f" if command => {
                self.app.update(cx, |app, cx| {
                    app.read_live();
                    cx.notify();
                });
            }
            _ => {
                // Type-to-search: with no other text field in the window, a
                // printable key is a query character. This is the behaviour
                // every launcher has trained people to expect.
                if !command && !modifiers.alt {
                    if let Some(text) = keystroke.key_char.as_ref() {
                        if let Some(character) = text.chars().next() {
                            if !character.is_control() {
                                self.app.update(cx, |app, cx| {
                                    app.push_char(character);
                                    cx.notify();
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // ── the three panes ──────────────────────────────────────────────────

    fn header(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let (query, focused, layout) = {
            let app = self.app.read(cx);
            (
                app.store.query.clone(),
                self.focus.is_focused(cx),
                app.store.layout,
            )
        };
        let search_border = if focused { theme.signal } else { theme.line };
        let placeholder = "Search the service's history — type anything";
        let shown = if query.is_empty() {
            div()
                .text_size(px(Type::TITLE))
                .text_color(theme.text_faint)
                .child(placeholder)
        } else {
            div()
                .text_size(px(Type::TITLE))
                .text_color(theme.text)
                .child(SharedString::from(query.clone()))
        };

        div()
            .flex()
            .items_center()
            .gap(px(Space::LG))
            .h(px(52.0))
            .px(px(Space::XL))
            .bg(theme.panel)
            .child(ui::product_header(theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .gap(px(Space::MD))
                    .h(px(32.0))
                    .px(px(Space::LG))
                    .rounded(px(Radius::MEDIUM))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(search_border)
                    .child(ui::glyph(theme, MimeFamily::Text, 18.0))
                    .child(shown)
                    .child(div().w(px(1.5)).h(px(16.0)).bg(if focused {
                        theme.signal
                    } else {
                        theme.panel
                    })),
            )
            .child(ui::button(
                theme,
                "layout-toggle",
                match layout {
                    Layout::List => "Gallery",
                    Layout::Gallery => "List",
                },
                ui::ButtonKind::Quiet,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.app.update(cx, |app, cx| {
                    app.toggle_layout();
                    cx.notify();
                });
            })))
            .child(ui::button(theme, "sort-toggle", "Sort", ui::ButtonKind::Quiet).on_click(
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.app.update(cx, |app, cx| {
                        app.cycle_sort();
                        cx.notify();
                    });
                }),
            ))
            .child(
                ui::button(theme, "refresh", "Refresh", ui::ButtonKind::Quiet).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.app.update(cx, |app, cx| {
                            app.refresh();
                            cx.notify();
                        });
                    }),
                ),
            )
            .child(ui::button(theme, "palette", "Palette", ui::ButtonKind::Primary).on_click(
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    open_palette(&this.app, cx);
                    window.focus(&this.focus, cx);
                }),
            ))
    }

    fn rail(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let app = self.app.read(cx);
        let store = &app.store;
        let active = store.facet;
        let counts: Vec<(Facet, usize)> = Facet::ORDER
            .iter()
            .map(|facet| {
                let count = store
                    .items
                    .iter()
                    .filter(|item| facet.admits(item, store.preview_text(item.id)))
                    .count();
                (*facet, count)
            })
            .collect();
        let connection = store.connection.clone();
        let capture = store.capture_label();
        drop(app);

        let mut rail = div()
            .flex()
            .flex_col()
            .w(px(168.0))
            .h_full()
            .py(px(Space::XL))
            .gap(px(Space::XXS))
            .bg(theme.panel)
            .border_color(theme.line)
            .child(
                div()
                    .px(px(Space::LG))
                    .pb(px(Space::SM))
                    .child(ui::section_label(theme, "Kinds")),
            );

        for (facet, count) in counts {
            let is_active = facet == active;
            let background = if is_active {
                theme.selection
            } else {
                theme.panel
            };
            let foreground = if is_active { theme.text } else { theme.text_muted };
            rail = rail.child(
                div()
                    .id(gpui::ElementId::named_usize("facet", facet as usize))
                    .flex()
                    .items_center()
                    .justify_between()
                    .mx(px(Space::SM))
                    .px(px(Space::MD))
                    .h(px(28.0))
                    .rounded(px(Radius::SMALL))
                    .bg(background)
                    .text_size(px(Type::SMALL))
                    .text_color(foreground)
                    .cursor_pointer()
                    .hover(move |style| style.bg(theme.raised))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.app.update(cx, |app, cx| {
                            app.store.facet = facet;
                            let mut effects = app.store.request_missing_previews();
                            // If the filter just hid the selection, move it
                            // to the first row that survived — and forward the
                            // commands that move implies, so the new row gets
                            // its preview.
                            if app.store.selected_item().is_none() {
                                effects.extend(app.store.select_first());
                            }
                            app.dispatch(effects);
                            cx.notify();
                        });
                    }))
                    .child(SharedString::from(facet.label()))
                    .child(ui::count(theme, count)),
            );
        }

        // A footer that says what the rail is: a local filter over the loaded
        // window, not a search of the service.
        rail = rail
            .child(div().flex_1())
            .child(ui::divider(theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::SM))
                    .px(px(Space::LG))
                    .py(px(Space::MD))
                    .child(ui::provenance(
                        theme,
                        "filters apply to the loaded window only — the contract's search is text",
                    ))
                    .child(ui::badge(
                        theme,
                        if connection_is_live(&connection) {
                            theme.good
                        } else {
                            theme.warn
                        },
                        connection.label(),
                    ))
                    .child(ui::provenance(theme, capture)),
            );
        rail
    }

    fn list_panel(&mut self, theme: &Theme, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (connected, loading, visible, facet, hidden, label, sort, local) = {
            let app = self.app.read(cx);
            (
                app.store.connection.is_connected(),
                app.store.loading,
                app.store.visible(),
                app.store.facet,
                app.store.hidden_by_facet(),
                app.store.window_label(),
                app.store.sort,
                app.store.local_filter_label(),
            )
        };

        if !connected {
            return self.connection_panel(theme, cx).into_any_element();
        }
        if visible.is_empty() && !loading {
            let (headline, body) = if let Some(local) = local.as_ref() {
                (
                    "Nothing matches the search predicates",
                    format!(
                        "“{local}” is applied by this app to the loaded rows, not by the \
                         service — its search is text-only. {hidden} item(s) are hidden by \
                         the current filter; clear the field to see the whole window."
                    ),
                )
            } else if hidden > 0 {
                (
                    "Nothing matches this filter",
                    format!(
                        "{hidden} item(s) are loaded but hidden by “{}”. Clear the filter to see them.",
                        facet.label()
                    ),
                )
            } else if self.app.read(cx).store.applied_query.is_empty() {
                (
                    "Nothing captured yet",
                    "The service's history is empty. Copies appear here once the \
                     backend records them — this app never watches the clipboard itself."
                        .to_string(),
                )
            } else {
                (
                    "No matches",
                    format!(
                        "The service searched its own history for “{}” and found nothing.",
                        self.app.read(cx).store.applied_query
                    ),
                )
            };
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .h_full()
                .child(list_header_strip(theme, &label, sort, local.as_deref()))
                .child(ui::empty_state(theme, headline, body))
                .into_any_element();
        }

        let rows = Arc::new(visible);
        let count = rows.len();
        let list = uniform_list(
            "history-rows",
            count,
            cx.processor(move |this, range: Range<usize>, window, cx| {
                range
                    .map(|index| this.row(index, &rows, window, cx))
                    .collect::<Vec<AnyElement>>()
            }),
        );

        let gallery = self.gallery(theme, cx);

        let body: AnyElement = match self.app.read(cx).store.layout {
            Layout::List => list.into_any_element(),
            Layout::Gallery => gallery,
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(list_header_strip(theme, &label, sort))
            .child(div().flex_1().w_full().overflow_hidden().child(body))
            .child(self.list_footer(theme, cx))
            .into_any_element()
    }

    fn row(
        &mut self,
        index: usize,
        rows: &Arc<Vec<Item>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(item) = rows.get(index).cloned() else {
            return div().into_any_element();
        };
        let app = self.app.read(cx);
        let theme = app.theme();
        let selected = app.store.selected == Some(item.id);
        let preview = app.store.previews.get(&item.id.0).cloned();
        let thumb = app.thumbs.get(&item.id.0);
        let now = crate::app::now_ns();
        let family = item.family();

        let (snippet, snippet_color) = match &preview {
            Some(PreviewState::Ready { text, available, .. }) if *available && !text.is_empty() => {
                (format::snippet(text, 160), theme.text)
            }
            Some(PreviewState::Loading) | Some(PreviewState::NotRequested) | None => (
                "asking the service for a preview…".to_string(),
                theme.text_faint,
            ),
            Some(PreviewState::Unavailable { .. }) => (
                format!(
                    "{} — no text preview (the service keeps none for this payload)",
                    format::family_title(family)
                ),
                theme.text_faint,
            ),
            Some(PreviewState::Refused { refusal }) => {
                (refusal.headline(), theme.text_faint)
            }
            Some(PreviewState::Failed { message }) => {
                (format!("preview failed: {message}"), theme.text_faint)
            }
        };

        // A colour payload gets a swatch where its thumbnail would be; that is
        // the one place a picture beats a word.
        let mark: AnyElement = match (&thumb, family) {
            (Some(Thumb::Ready(image)), _) => div()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(Radius::SMALL))
                .overflow_hidden()
                .child(
                    img(Arc::clone(image))
                        .w(px(40.0))
                        .h(px(40.0))
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            (Some(Thumb::Failed(_)), _) | (None, MimeFamily::Image) => div()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(Radius::SMALL))
                .bg(alpha(theme.family_image, 0.12))
                .flex()
                .items_center()
                .justify_center()
                .child(ui::glyph(&theme, MimeFamily::Image, 22.0))
                .into_any_element(),
            _ => {
                let as_text = app.store.preview_text(item.id);
                match as_text.and_then(format::color_literal) {
                    Some(color) => div()
                        .w(px(40.0))
                        .h(px(40.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui::swatch(color, 26.0, &theme))
                        .into_any_element(),
                    None => ui::glyph(&theme, family, 40.0).into_any_element(),
                }
            }
        };

        let selected_background = theme.selection;
        let hover_background = theme.raised;
        let border = theme.line;
        let id = item.id;

        div()
            .id(gpui::ElementId::named_usize("row", index))
            .flex()
            .items_center()
            .gap(px(Space::LG))
            .h(px(64.0))
            .w_full()
            .px(px(Space::XL))
            .bg(if selected {
                selected_background
            } else {
                theme.canvas
            })
            .border_b_1()
            .border_color(border)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_background))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.app.update(cx, |app, cx| {
                    app.select(id);
                    cx.notify();
                });
            }))
            .child(mark)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::XXS))
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(px(Type::BODY))
                            .font_weight(Weight::STRONG)
                            .text_color(snippet_color)
                            .truncate()
                            .child(SharedString::from(snippet)),
                    )
                    .child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text_faint)
                            .truncate()
                            .child(SharedString::from(format::row_meta(&item, now))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Space::MD))
                    .child(if item.pinned {
                        div()
                            .px(px(Space::SM))
                            .py(px(1.0))
                            .rounded(px(Radius::PILL))
                            .bg(alpha(theme.signal, 0.18))
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text)
                            .child("pinned")
                    } else {
                        div()
                    })
                    .child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(family_token(family))),
                    ),
            )
            .into_any_element()
    }

    fn gallery(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        let rows = app.store.visible();
        let selected = app.store.selected;
        let thumbs: Vec<(ItemId, Option<Arc<gpui::RenderImage>>)> = rows
            .iter()
            .map(|item| {
                let thumb = match app.thumbs.get(&item.id.0) {
                    Some(Thumb::Ready(image)) => Some(image.clone()),
                    _ => None,
                };
                (item.id, thumb)
            })
            .collect();
        let now = crate::app::now_ns();

        let mut grid = div()
            .id("gallery")
            .flex()
            .flex_wrap()
            .gap(px(Space::LG))
            .p(px(Space::XL))
            .w_full();
        for (item, (id, thumb)) in rows.iter().zip(thumbs.into_iter()) {
            let theme = theme.clone();
            let is_selected = selected == Some(id);
            let border = if is_selected { theme.signal } else { theme.line };
            let background = theme.panel;
            let hover = theme.raised;
            let title = rows_title(item, now);
            let tile = div()
                .id(gpui::ElementId::named_usize("tile", id.0 as usize))
                .flex()
                .flex_col()
                .w(px(184.0))
                .h(px(152.0))
                .rounded(px(Radius::MEDIUM))
                .border_1()
                .border_color(border)
                .bg(background)
                .overflow_hidden()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.app.update(cx, |app, cx| {
                        app.select(id);
                        cx.notify();
                    });
                }))
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .bg(theme.checker_a)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(match thumb {
                            Some(image) => div()
                                .w(px(184.0))
                                .h(px(112.0))
                                .overflow_hidden()
                                .child(
                                    img(image)
                                        .w(px(184.0))
                                        .h(px(112.0))
                                        .object_fit(ObjectFit::Cover),
                                )
                                .into_any_element(),
                            None => ui::glyph(&theme, item.family(), 40.0).into_any_element(),
                        }),
                )
                .child(
                    div()
                        .px(px(Space::MD))
                        .py(px(Space::SM))
                        .text_size(px(Type::MICRO))
                        .text_color(theme.text_muted)
                        .truncate()
                        .child(SharedString::from(title)),
                );
            grid = grid.child(tile);
        }
        grid.into_any_element()
    }

    fn list_footer(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let (exhausted, connected, loading) = {
            let app = self.app.read(cx);
            (
                app.store.exhausted,
                app.store.connection.is_connected(),
                app.store.loading,
            )
        };
        let label = if !connected {
            "no service".to_string()
        } else if loading {
            "asking the service…".to_string()
        } else if exhausted {
            "end of the service's history".to_string()
        } else {
            "load more".to_string()
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(34.0))
            .px(px(Space::XL))
            .border_t_1()
            .border_color(theme.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Space::MD))
                    .child(ui::hint(theme, "↑↓", "move"))
                    .child(ui::hint(theme, "⏎", "copy"))
                    .child(ui::hint(theme, "⇧⏎", "offer all"))
                    .child(ui::hint(theme, "P", "pin"))
                    .child(ui::hint(theme, "⌫", "delete"))
                    .child(ui::hint(theme, "G", "gallery"))
                    .child(ui::hint(theme, "⌘K", "palette")),
            )
            .child(
                div()
                    .text_size(px(Type::MICRO))
                    .text_color(if exhausted {
                        theme.text_faint
                    } else {
                        theme.signal
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.app.update(cx, |app, cx| {
                            app.load_more();
                            cx.notify();
                        });
                    }))
                    .child(SharedString::from(label)),
            )
    }

    /// The panel that replaces the list when no service answers.
    ///
    /// It is the app's most important screen, because it is the one a new user
    /// sees: the candidate list, each attempt's own words, and the two things
    /// that fix it.
    fn connection_panel(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let app = self.app.read(cx);
        let connection = app.store.connection.clone();
        let dials = app.store.dials.clone();
        let notices = app.store.notices.clone();
        let location = app.location.clone();
        drop(app);

        let headline = match &connection {
            Connection::Failed { reason } => format!("No service answered — {reason}"),
            Connection::Dialling { endpoint } => format!("Dialling {endpoint}…"),
            Connection::Lost { reason, .. } => format!("Connection lost — {reason}"),
            _ => "Looking for a smart service…".to_string(),
        };

        let mut panel = div()
            .flex()
            .flex_col()
            .gap(px(Space::LG))
            .p(px(Space::XXL))
            .w_full()
            .flex_1()
            .child(
                div()
                    .text_size(px(Type::DISPLAY))
                    .font_weight(Weight::HEADLINE)
                    .text_color(theme.text)
                    .child(SharedString::from(headline)),
            )
            .child(
                div()
                    .max_w(px(560.0))
                    .text_size(px(Type::BODY))
                    .text_color(theme.text_muted)
                    .child(
                        "smart-clipboard is a client. It needs a service that serves the \
                         clipboard contract — the backend, its history and its store all live \
                         there, not here.",
                    ),
            )
            .child(ui::section_label(theme, "Endpoints tried, in order"));

        for candidate in &location.candidates {
            let endpoint = candidate.display();
            let record = dials.iter().find(|record| record.endpoint == endpoint);
            let (dot, outcome) = match record.and_then(|record| record.failure.clone()) {
                Some(failure) => (theme.bad, failure),
                None if record.is_some() => (theme.warn, "dialling…".to_string()),
                None => (
                    theme.text_faint,
                    format!("not tried yet ({})", candidate.source.label()),
                ),
            };
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::XXS))
                    .p(px(Space::LG))
                    .rounded(px(Radius::SMALL))
                    .bg(theme.panel)
                    .border_1()
                    .border_color(theme.line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(Space::MD))
                            .child(div().w(px(6.0)).h(px(6.0)).rounded(px(3.0)).bg(dot))
                            .child(
                                div()
                                    .text_size(px(Type::SMALL))
                                    .text_color(theme.text)
                                    .child(SharedString::from(endpoint)),
                            )
                            .child(ui::mono(theme, candidate.source.label())),
                    )
                    .child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(outcome)),
                    ),
            );
        }

        panel = panel
            .child(ui::section_label(theme, "How to fix it"));
        let hint = div()
            .flex()
            .flex_col()
            .gap(px(Space::SM))
            .p(px(Space::LG))
            .rounded(px(Radius::SMALL))
            .bg(alpha(theme.signal, 0.07))
            .border_1()
            .border_color(alpha(theme.signal, 0.30))
            // The fix is the *installed* one: the daemon is a binary with a
            // session unit now, not a cargo example. A user who copied a command
            // from this panel gets a clipboard that records copies, not a build
            // that has to stay open in a terminal.
            .child(ui::mono(theme, "systemctl --user enable --now smart-service"))
            .child(ui::mono(theme, "smart-service --socket /tmp/smart-service.sock"))
            .child(ui::provenance(
                theme,
                "or point this app at a running service with --socket <path> / $SMART_SERVICE_SOCKET",
            ))
            .child(ui::provenance(
                theme,
                "nothing installed yet: ./install.sh in the smart-lib checkout builds and starts it",
            ));
        panel = panel.child(hint).child(
            div()
                .flex()
                .items_center()
                .gap(px(Space::MD))
                .child(
                    ui::button(theme, "retry-connect", "Retry", ui::ButtonKind::Primary).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.app.update(cx, |app, cx| {
                                app.reconnect();
                                cx.notify();
                            });
                        }),
                    ),
                )
                .child(ui::button(theme, "copy-endpoint", "Copy socket path", ui::ButtonKind::Quiet))
                .child(ui::provenance(
                    theme,
                    "this app never spawns the service, and never pretends to be one",
                )),
        );

        if !notices.is_empty() {
            panel = panel.child(ui::section_label(theme, "Session log"));
            let mut log = div().flex().flex_col().gap(px(Space::XXS));
            for notice in notices.iter().take(6) {
                log = log.child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(notice.clone())),
                );
            }
            panel = panel.child(log);
        }
        panel
    }

    fn status_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let app = self.app.read(cx);
        let connection = app.store.connection.clone();
        let provenance = app.store.provenance();
        let class = app.store.authority.class_label().to_string();
        let can_write = app.store.can_write();
        let methods = app
            .store
            .service
            .as_ref()
            .map(|service| service.counts())
            .unwrap_or((0, 0, 0));
        let capture = app.store.capture_label();

        let write_color = match can_write {
            Some(true) => theme.good,
            Some(false) => theme.warn,
            None => theme.text_faint,
        };
        let write_label = match can_write {
            Some(true) => "write: allowed by the service".to_string(),
            Some(false) => "write: read-only connection".to_string(),
            None => "write: not published".to_string(),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(26.0))
            .px(px(Space::XL))
            .bg(theme.panel)
            .border_t_1()
            .border_color(theme.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Space::LG))
                    .child(ui::badge(
                        theme,
                        if connection.is_connected() {
                            theme.good
                        } else {
                            theme.bad
                        },
                        connection.label(),
                    ))
                    .child(ui::provenance(theme, provenance))
                    .child(ui::provenance(
                        theme,
                        format!("{} · {} of {} methods answered", class, methods.0, methods.2),
                    ))
                    .child(ui::provenance(theme, capture)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Space::LG))
                    .child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(write_color)
                            .child(SharedString::from(write_label)),
                    )
                    .child(ui::provenance(theme, "nothing is written to disk")),
            )
    }

    fn toasts(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.app.read(cx).store.toasts.is_empty() {
            return None;
        }
        let toasts = self.app.read(cx).store.toasts.clone();
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(Space::MD))
            .w(px(360.0));
        for toast in toasts {
            let color = match toast.kind {
                ToastKind::Info => theme.signal,
                ToastKind::Good => theme.good,
                ToastKind::Warn => theme.warn,
                ToastKind::Bad => theme.bad,
            };
            let background = theme.raised;
            let border = color;
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::XXS))
                    .p(px(Space::LG))
                    .rounded(px(Radius::MEDIUM))
                    .bg(background)
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(px(Type::SMALL))
                            .font_weight(Weight::HEADLINE)
                            .text_color(color)
                            .child(SharedString::from(toast.title)),
                    )
                    .child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(toast.detail)),
                    ),
            );
        }
        Some(
            div()
                .absolute()
                .bottom(px(38.0))
                .right(px(Space::XL))
                .child(column)
                .into_any_element(),
        )
    }

    fn confirm_overlay(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.app.read(cx).pending_delete?;
        Some(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(theme.canvas, 0.86))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::LG))
                        .w(px(420.0))
                        .p(px(Space::XL))
                        .rounded(px(Radius::LARGE))
                        .bg(theme.raised)
                        .border_1()
                        .border_color(theme.line_strong)
                        .child(
                            div()
                                .text_size(px(Type::TITLE))
                                .font_weight(Weight::HEADLINE)
                                .text_color(theme.text)
                                .child("Delete this item?"),
                        )
                        .child(
                            div()
                                .text_size(px(Type::SMALL))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(format!(
                                    "The service will drop {pending} from its history. \
                                     This app keeps no copy, so it cannot undo it."
                                ))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(Space::MD))
                                .child(
                                    ui::button(
                                        theme,
                                        "confirm-delete",
                                        "Delete",
                                        ui::ButtonKind::Danger,
                                    )
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.app.update(cx, |app, cx| {
                                            app.confirm_delete();
                                            cx.notify();
                                        });
                                    })),
                                )
                                .child(
                                    ui::button(theme, "cancel-delete", "Keep", ui::ButtonKind::Secondary)
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.app.update(cx, |app, cx| {
                                                app.cancel_delete();
                                                cx.notify();
                                            });
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.app.read(cx).theme();
        let header = self.header(&theme, cx);
        let rail = self.rail(&theme, cx);
        let list = self.list_panel(&theme, window, cx);
        let status = self.status_bar(&theme, cx);
        let inspector = self.inspector.clone();
        let toasts = self.toasts(&theme, cx);
        let confirm = self.confirm_overlay(&theme, cx);
        let focus = self.focus.clone();

        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.canvas)
            .text_color(theme.text)
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key(event, window, cx)
            }))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .child(rail)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .border_l_1()
                            .border_color(theme.line)
                            .child(list),
                    )
                    .child(
                        div()
                            .w(px(340.0))
                            .h_full()
                            .border_l_1()
                            .border_color(theme.line)
                            .bg(theme.panel)
                            .child(inspector),
                    ),
            )
            .child(status);

        if let Some(toasts) = toasts {
            root = root.child(toasts);
        }
        if let Some(confirm) = confirm {
            root = root.child(confirm);
        }
        root
    }
}

// ── small helpers ─────────────────────────────────────────────────────────

fn connection_is_live(connection: &Connection) -> bool {
    connection.is_connected()
}

fn family_token(family: MimeFamily) -> &'static str {
    match family {
        MimeFamily::Text => "text",
        MimeFamily::Link => "link",
        MimeFamily::Image => "image",
        MimeFamily::File => "file",
        MimeFamily::Color => "colour",
        MimeFamily::Other => "payload",
    }
}

/// The tile caption: what it is, how big, when.
fn rows_title(item: &Item, now: u64) -> String {
    let mut parts = vec![format::family_title(item.family()).to_string()];
    parts.push(format::bytes(item.size_bytes));
    if let Some(nanos) = item.captured_at_ns {
        parts.push(format::relative_time(now, nanos));
    }
    parts.join(" · ")
}

/// The strip above the list: what the window is, and how it was ordered.
fn list_header_strip(theme: &Theme, label: &str, sort: Sort, local: Option<&str>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(30.0))
        .px(px(Space::XL))
        .bg(theme.panel)
        .border_b_1()
        .border_color(theme.line)
        .child(
            div()
                .text_size(px(Type::MICRO))
                .text_color(theme.text_muted)
                .child(SharedString::from(label.to_string())),
        )
        .child({
            // The local half of the search field is *named*, with its cost: a
            // predicate over the loaded window is a different promise from a
            // search over the service's history, and this strip is the only place
            // a user can tell which one they got.
            let chips = div()
                .flex()
                .items_center()
                .gap(px(Space::MD))
                .child(ui::chip(theme, "server search", true));
            let chips = match local {
                Some(local) => chips.child(ui::chip(
                    theme,
                    SharedString::from(format!("local: {local}")),
                    true,
                )),
                None => chips,
            };
            chips.child(ui::chip(
                theme,
                SharedString::from(format!("order: {}", sort.label())),
                false,
            ))
        })
}
