//! The quick palette: 680×480, six rows, keyboard only.
//!
//! ## Why a second window instead of a modal in the first
//!
//! The library is a *workspace* — you go there to browse, pin, read
//! provenance. The palette is a *reflex* — you are typing in another app, you
//! want the thing you copied ninety seconds ago, and you want to be back
//! before your sentence ends. Those are different postures, and the second one
//! must never depend on the first being open, focused, or on the right monitor.
//! A modal inside the library would drag the whole workspace to the front; a
//! small always-on-top window does not.
//!
//! ## The layout, and what each decision buys
//!
//! * **Search at the very top, first focus, type-to-filter.** The palette has
//!   no chrome, no title, no tabs: the first pixel row is the thing you type
//!   into. Any header would be a thing to read instead of a thing to use.
//! * **Six rows, 56 px.** Enough to recognise a match by its snippet, few
//!   enough that the list never scrolls under a fast arrow-key burst. The
//!   selection is the *third* row from the top when results are long, so the
//!   eye does not have to move as it narrows.
//! * **One line of footer.** It carries the three keys that matter and the
//!   provenance ("the service's history"), because "where did these rows come
//!   from" is a question a clipboard tool must answer even at 56 px per row.
//! * **Enter copies and closes; Shift+Enter copies and stays.** The common
//!   case ends the interaction. Holding the window open costs a keystroke and
//!   saves a re-open when the user is collecting several things.
//!
//! ## What it will not do
//!
//! It will not paste. Not on Enter, not on a hotkey, not later. Sending a
//! synthetic Ctrl+V is input injection — a different capability, in a
//! different domain, which this product has no authority to hold. Copying and
//! letting the user paste is the honest division, and the footer says so.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    div, img, px, uniform_list, AnyElement, ClickEvent, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ObjectFit, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, Window,
};
use smart_clipboard_proto::{Item, PreviewState};

use crate::app::{close_palette, ClipboardApp, Thumb};
use crate::format;
use crate::store::Connection;
use crate::theme::{alpha, Radius, Space, Theme, Type, Weight};
use crate::ui;

/// The palette window's root view.
pub struct PaletteView {
    app: Entity<ClipboardApp>,
    focus: FocusHandle,
    _observed: Subscription,
}

impl PaletteView {
    /// Build the view, focus its search and ask for the rows it will show.
    pub fn new(app: Entity<ClipboardApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let observed = cx.observe(&app, |_, _, cx| cx.notify());
        window.focus(&focus);
        app.update(cx, |app, cx| {
            let effects = app.store.request_missing_previews();
            app.dispatch(effects);
            cx.notify();
        });
        PaletteView {
            app,
            focus,
            _observed: observed,
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;
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
                if !modifiers.shift {
                    close_palette(&self.app, window, cx);
                }
            }
            "escape" => {
                close_palette(&self.app, window, cx);
            }
            "k" if command => {
                close_palette(&self.app, window, cx);
            }
            "p" if !command => {
                self.app.update(cx, |app, cx| {
                    app.toggle_pin();
                    cx.notify();
                });
            }
            "backspace" => {
                self.app.update(cx, |app, cx| {
                    app.backspace();
                    cx.notify();
                });
            }
            _ => {
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

    fn row(
        &mut self,
        index: usize,
        rows: &Arc<Vec<Item>>,
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

        let snippet = match &preview {
            Some(PreviewState::Ready { text, available, .. }) if *available && !text.is_empty() => {
                format::snippet(text, 96)
            }
            Some(PreviewState::Loading) | Some(PreviewState::NotRequested) | None => {
                "…".to_string()
            }
            // The remaining `Ready` is "there is a preview object but no usable
            // text in it" — an image, a file, a binary, or an empty string. It
            // shares the words with `Unavailable` because to a reader they are
            // the same fact; the family name is what makes the row identifiable.
            Some(PreviewState::Ready { .. }) | Some(PreviewState::Unavailable { .. }) => {
                format!("{} — no text preview", format::family_title(item.family()))
            }
            Some(PreviewState::Refused { refusal }) => refusal.headline(),
            Some(PreviewState::Failed { message }) => format!("failed: {message}"),
        };

        let mark: AnyElement = match (thumb, item.family()) {
            (Some(Thumb::Ready(image)), _) => div()
                .w(px(34.0))
                .h(px(34.0))
                .rounded(px(Radius::SMALL))
                .overflow_hidden()
                .child(
                    img(Arc::clone(image))
                        .w(px(34.0))
                        .h(px(34.0))
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            _ => ui::glyph(&theme, item.family(), 34.0).into_any_element(),
        };

        let id = item.id;
        let time = item
            .captured_at_ns
            .map(|nanos| format::relative_time(now, nanos))
            .unwrap_or_default();

        div()
            .id(gpui::ElementId::named_usize("palette-row", index))
            .flex()
            .items_center()
            .gap(px(Space::LG))
            .h(px(56.0))
            .w_full()
            .px(px(Space::LG))
            .rounded(px(Radius::SMALL))
            .bg(if selected { theme.selection } else { theme.canvas })
            .cursor_pointer()
            .hover(move |style| style.bg(theme.raised))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.app.update(cx, |app, cx| {
                    app.select(id);
                    app.copy(false);
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
                            .text_color(theme.text)
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
                    .gap(px(Space::SM))
                    .child(if item.pinned {
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.signal)
                            .child("pinned")
                    } else {
                        div()
                    })
                    .child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(time)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for PaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.app.read(cx).theme();
        let (query, connected, total, loading) = {
            let app = self.app.read(cx);
            (
                app.store.query.clone(),
                app.store.connection.is_connected(),
                app.store.total,
                app.store.loading,
            )
        };
        let rows = Arc::new(self.app.read(cx).store.visible());
        let count = rows.len().min(24);
        // Focus is a property of the *window*, not of the context in gpui 0.2:
        // a handle can only answer whether it is focused given the window it
        // might have been focused in.
        let focused = self.focus.is_focused(window);

        let list: AnyElement = if !connected {
            let connection = self.app.read(cx).store.connection.clone();
            div()
                .p(px(Space::LG))
                .text_size(px(Type::SMALL))
                .text_color(theme.text_muted)
                .child(SharedString::from(match connection {
                    Connection::Failed { reason } => format!("No service — {reason}"),
                    Connection::Lost { reason, .. } => format!("Connection lost — {reason}"),
                    _ => "Looking for a smart service…".to_string(),
                }))
                .into_any_element()
        } else if count == 0 && !loading {
            div()
                .p(px(Space::LG))
                .text_size(px(Type::SMALL))
                .text_color(theme.text_muted)
                .child(if query.is_empty() {
                    "Nothing captured yet."
                } else {
                    "No matches in the service's history."
                })
                .into_any_element()
        } else {
            uniform_list(
                "palette-rows",
                count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    range
                        .map(|index| this.row(index, &rows, cx))
                        .collect::<Vec<AnyElement>>()
                }),
            )
            .into_any_element()
        };

        let search_text = if query.is_empty() {
            div()
                .text_size(px(Type::HEADING))
                .text_color(theme.text_faint)
                .child("Search the clipboard…")
        } else {
            div()
                .text_size(px(Type::HEADING))
                .text_color(theme.text)
                .child(SharedString::from(query.clone()))
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(Space::MD))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .rounded(px(Radius::LARGE))
                    .bg(theme.panel)
                    .border_1()
                    .border_color(theme.line_strong)
                    .overflow_hidden()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.on_key(event, window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(Space::MD))
                            .h(px(52.0))
                            .px(px(Space::LG))
                            .border_b_1()
                            .border_color(theme.line)
                            .child(ui::glyph(&theme, smart_clipboard_proto::MimeFamily::Text, 20.0))
                            .child(search_text)
                            .child(div().w(px(1.5)).h(px(20.0)).bg(if focused {
                                theme.signal
                            } else {
                                theme.panel
                            }))
                            .child(div().flex_1())
                            .child(ui::key_hint(&theme, "esc")),
                    )
                    .child(list)
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(30.0))
                            .px(px(Space::LG))
                            .border_t_1()
                            .border_color(theme.line)
                            .bg(theme.canvas)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(Space::MD))
                                    .child(ui::hint(&theme, "⏎", "copy & close"))
                                    .child(ui::hint(&theme, "⇧⏎", "copy & stay"))
                                    .child(ui::hint(&theme, "P", "pin")),
                            )
                            .child(ui::provenance(
                                &theme,
                                format!("{total} on the service · nothing is stored by this app"),
                            )),
                    ),
            )
            .child(
                div()
                    .px(px(Space::SM))
                    .pt(px(2.0))
                    .text_size(px(10.0))
                    .text_color(alpha(theme.text_faint, 0.9))
                    .child("smart-clipboard copies; it never pastes for you"),
            )
    }
}
