//! The inspector: what the selected item *is*, and what the service *said*.
//!
//! Two tabs, because these are two different questions and mixing them makes
//! both harder to read:
//!
//! * **Payload** — the thing itself: the image, the text, the colour, the
//!   metadata the service recorded, and the verbs that act on it.
//! * **Contract** — the provenance: the endpoint, the schema id and whether it
//!   matches the contract this binary compiled, the capability rows, the live
//!   per-method matrix, and the last refusal verbatim.
//!
//! The Contract tab is the unusual one, and it is the reason this app is worth
//! writing. Every other clipboard manager hides its data source; here the data
//! source is a first-class view, including the parts that are missing. When a
//! preview does not appear, the answer is one click away and it is a sentence
//! from the service, not a shrug.

use std::sync::Arc;

use gpui::{
    div, img, px, AnyElement, ClickEvent, Context, Div, Entity, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    StyledImage, Subscription,
};
use smart_clipboard_proto::{
    ContentState, MimeFamily, Observation, PreviewState, RefusalClass,
};

use crate::app::{ClipboardApp, InspectorTab, Thumb};
use crate::format;
use crate::theme::{alpha, Radius, Space, Theme, Type, Weight};
use crate::ui;

/// A thumbnail, snapshotted out of the app entity.
///
/// The snapshot exists for a borrow reason worth writing down: the panel reads
/// the app through `cx`, and then needs `cx` mutably to build click listeners.
/// Holding a `&Thumb` across that would alias the context. Cloning an `Arc` is
/// free, and it makes the data flow obvious: what a view draws is a value, not
/// a borrow into a lock.
enum ThumbSnapshot {
    /// Fetched or decoding.
    Pending,
    /// Drawable.
    Ready(Arc<gpui::RenderImage>),
    /// Cannot be drawn, with the reason.
    Failed(String),
}

/// The inspector panel, as its own view so both windows can embed it and it
/// can own its scroll position.
pub struct InspectorView {
    app: Entity<ClipboardApp>,
    _observed: Subscription,
}

impl InspectorView {
    /// Build the panel.
    pub fn new(app: Entity<ClipboardApp>, cx: &mut Context<Self>) -> Self {
        let observed = cx.observe(&app, |_, _, cx| cx.notify());
        InspectorView {
            app,
            _observed: observed,
        }
    }

    fn tabs(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let active = self.app.read(cx).inspector_tab;
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(Space::XS))
            .p(px(Space::SM))
            .border_b_1()
            .border_color(theme.line);
        for (tab, label) in [
            (InspectorTab::Payload, "Payload"),
            (InspectorTab::Contract, "Contract"),
        ] {
            let selected = tab == active;
            let background = if selected {
                theme.raised
            } else {
                alpha(theme.text_faint, 0.0)
            };
            let foreground = if selected { theme.text } else { theme.text_faint };
            row = row.child(
                div()
                    .id(gpui::ElementId::named_usize(
                        "inspector-tab",
                        match tab {
                            InspectorTab::Payload => 0,
                            InspectorTab::Contract => 1,
                        },
                    ))
                    .px(px(Space::MD))
                    .py(px(Space::XS))
                    .rounded(px(Radius::SMALL))
                    .bg(background)
                    .text_size(px(Type::SMALL))
                    .font_weight(if selected {
                        Weight::HEADLINE
                    } else {
                        Weight::BODY
                    })
                    .text_color(foreground)
                    .cursor_pointer()
                    .hover(move |style| style.bg(theme.raised))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.app.update(cx, |app, cx| {
                            app.set_inspector_tab(tab);
                            cx.notify();
                        });
                    }))
                    .child(label),
            );
        }
        row
    }

    fn payload(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.app.read(cx).store.selected_item().cloned() else {
            return self.nothing_selected(theme, cx);
        };
        let app = self.app.read(cx);
        let preview = app.store.previews.get(&item.id.0).cloned();
        let content = app.store.contents.get(&item.id.0).cloned();
        let thumb = match app.thumbs.get(&item.id.0) {
            Some(Thumb::Ready(image)) => Some(ThumbSnapshot::Ready(Arc::clone(image))),
            Some(Thumb::Failed(reason)) => Some(ThumbSnapshot::Failed(reason.clone())),
            Some(Thumb::Pending) => Some(ThumbSnapshot::Pending),
            None => None,
        };
        let now = crate::app::now_ns();
        let probe = app.store.preview_text(item.id).map(|text| text.to_string());
        let can_write = app.store.can_write();
        drop(app);

        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(Space::LG))
            .p(px(Space::LG))
            .w_full();

        // ── the payload itself ───────────────────────────────────────────
        column = column.child(self.preview_block(theme, &item, preview, content, thumb, cx));

        // ── a colour, when the payload is one ────────────────────────────
        if let Some(color) = probe.as_deref().and_then(format::color_literal) {
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Space::LG))
                    .p(px(Space::MD))
                    .rounded(px(Radius::SMALL))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.line)
                    .child(ui::swatch(color, 48.0, theme))
                    .child(ui::field(
                        theme,
                        "colour",
                        format!("#{:06x}", color).to_uppercase(),
                    )),
            );
        }

        // ── a link, when the payload is one ──────────────────────────────
        if let Some(url) = probe.as_deref().and_then(format::url_literal) {
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::XS))
                    .p(px(Space::MD))
                    .rounded(px(Radius::SMALL))
                    .bg(alpha(theme.family_link, 0.08))
                    .border_1()
                    .border_color(alpha(theme.family_link, 0.30))
                    .child(ui::section_label(theme, "link"))
                    .child(
                        div()
                            .text_size(px(Type::SMALL))
                            .text_color(theme.text)
                            .child(SharedString::from(url.to_string())),
                    )
                    .child(ui::provenance(
                        theme,
                        "the app will not open this: a URL on the clipboard is untrusted input",
                    )),
            );
        }

        // ── the metadata the service actually recorded ──────────────────
        column = column
            .child(ui::divider(theme))
            .child(ui::field(theme, "id", item.id.to_string()))
            .child(ui::field(
                theme,
                "captured",
                item.captured_at_ns
                    .map(format::absolute_time)
                    .unwrap_or_else(|| {
                        "not recorded — the service reported no clock reading".to_string()
                    }),
            ))
            .child(ui::field(
                theme,
                "size",
                format!(
                    "{} ({} bytes, measured by the service)",
                    format::bytes(item.size_bytes),
                    item.size_bytes
                ),
            ))
            .child(ui::field(
                theme,
                "source",
                "the backend's own source vocabulary — the contract carries an id, not an app name",
            ))
            .child(ui::field(
                theme,
                "tags",
                if item.tags.is_empty() {
                    "none recorded".to_string()
                } else {
                    item.tags.join(", ")
                },
            ));

        if let Some(text) = probe.as_deref() {
            if format::looks_sensitive(text) {
                column = column.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::XXS))
                        .p(px(Space::MD))
                        .rounded(px(Radius::SMALL))
                        .bg(alpha(theme.warn, 0.10))
                        .border_1()
                        .border_color(alpha(theme.warn, 0.35))
                        .child(
                            div()
                                .text_size(px(Type::SMALL))
                                .font_weight(Weight::HEADLINE)
                                .text_color(theme.warn)
                                .child("looks like a secret"),
                        )
                        .child(ui::provenance(
                            theme,
                            "a local heuristic on this device — the payload is untouched and the \
                             service is not consulted",
                        )),
                );
            }
        }

        // ── representations, one button each ────────────────────────────
        column = column
            .child(ui::divider(theme))
            .child(ui::section_label(theme, "representations"));
        for mime in &item.mimes {
            let id = item.id;
            let wanted = mime.clone();
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(Space::MD))
                    .child(ui::mono(theme, mime.clone()))
                    .child(
                        ui::button(theme, SharedString::from(format!("copy-{mime}")), "Copy as", ui::ButtonKind::Quiet)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.app.update(cx, |app, cx| {
                                    app.copy_representation(id, wanted.clone());
                                    cx.notify();
                                });
                            })),
                    ),
            );
        }

        // ── the verbs ───────────────────────────────────────────────────
        let write_hint = match can_write {
            Some(false) => Some(
                "This connection is read-only, so the service will refuse. The app still tries: \
                 the refusal is the service's answer, not the app's guess.",
            ),
            None => Some(
                "The service published no authority for this connection. The verbs are still \
                 callable; their refusals are the truth.",
            ),
            Some(true) => None,
        };

        let pinned = item.pinned;
        let id = item.id;
        column = column
            .child(ui::divider(theme))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(Space::MD))
                    .child(
                        ui::button(theme, "inspector-copy", "Copy", ui::ButtonKind::Primary)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.app.update(cx, |app, cx| {
                                    app.copy(false);
                                    cx.notify();
                                });
                            })),
                    )
                    .child(
                        ui::button(
                            theme,
                            "inspector-offer",
                            "Offer all",
                            ui::ButtonKind::Secondary,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.app.update(cx, |app, cx| {
                                app.copy(true);
                                cx.notify();
                            });
                        })),
                    )
                    .child(
                        ui::button(
                            theme,
                            "inspector-pin",
                            if pinned { "Unpin" } else { "Pin" },
                            ui::ButtonKind::Quiet,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.app.update(cx, |app, cx| {
                                app.toggle_pin();
                                cx.notify();
                            });
                        })),
                    )
                    .child(
                        ui::button(theme, "inspector-delete", "Delete", ui::ButtonKind::Danger)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let _ = id;
                                this.app.update(cx, |app, cx| {
                                    app.request_delete();
                                    cx.notify();
                                });
                            })),
                    ),
            );

        if let Some(hint) = write_hint {
            column = column.child(
                div()
                    .p(px(Space::MD))
                    .rounded(px(Radius::SMALL))
                    .bg(alpha(theme.warn, 0.08))
                    .text_size(px(Type::MICRO))
                    .text_color(theme.text_muted)
                    .child(hint),
            );
        }

        column
            .child(ui::provenance(
                theme,
                format!(
                    "asked for {} · {}",
                    format::relative_time(now, item.captured_at_ns.unwrap_or(now)),
                    "every value above came from the service"
                ),
            ))
            .into_any_element()
    }

    /// The picture, or the reason there is not one.
    fn preview_block(
        &mut self,
        theme: &Theme,
        item: &smart_clipboard_proto::Item,
        preview: Option<PreviewState>,
        content: Option<ContentState>,
        thumb: Option<ThumbSnapshot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let family = item.family();
        let frame = div()
            .w_full()
            .rounded(px(Radius::MEDIUM))
            .bg(theme.canvas)
            .border_1()
            .border_color(theme.line)
            .p(px(Space::MD));

        match (family, thumb, content) {
            (MimeFamily::Image, Some(ThumbSnapshot::Ready(image)), _) => frame
                .child(
                    div()
                        .w_full()
                        .max_h(px(240.0))
                        .overflow_hidden()
                        .rounded(px(Radius::SMALL))
                        .bg(theme.checker_a)
                        .child(
                            img(image)
                                .w_full()
                                .max_h(px(240.0))
                                .object_fit(ObjectFit::Contain),
                        ),
                )
                .into_any_element(),
            (MimeFamily::Image, Some(ThumbSnapshot::Failed(reason)), _) => frame
                .child(ui::section_label(theme, "payload could not be drawn"))
                .child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(reason.clone())),
                )
                .into_any_element(),
            (MimeFamily::Image, _, Some(ContentState::TooLarge { bytes, limit })) => frame
                .child(ui::section_label(theme, "too large for this app"))
                .child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format!(
                            "The service recorded {}. This app's own fetch ceiling is {} — a \
                             client policy, not a service refusal. Copy it to use it.",
                            format::bytes(bytes),
                            format::bytes(limit)
                        ))),
                )
                .into_any_element(),
            (MimeFamily::Image, _, _) => frame
                .child(ui::section_label(theme, "fetching the payload"))
                .child(ui::skeleton(theme, 240.0, 120.0))
                .into_any_element(),
            (_, _, Some(ContentState::Refused { refusal })) => frame
                .child(ui::section_label(theme, "the service refused"))
                .child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.refusal(&refusal.class))
                        .child(SharedString::from(refusal.headline())),
                )
                .into_any_element(),
            (_, _, Some(ContentState::Unsupported { target })) => frame
                .child(ui::section_label(theme, "representation not produced"))
                .child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format!(
                            "The service answered `unsupported` for {target}: it holds the item, \
                             but not in that shape."
                        ))),
                )
                .into_any_element(),
            _ => {
                let body: AnyElement = match &preview {
                    Some(PreviewState::Ready { text, available, .. }) if *available => div()
                        // Scrolling is a *stateful* behaviour in gpui: the element
                        // needs an id to own its scroll offset, which is also why
                        // the offset survives a re-render.
                        .id("inspector-preview-text")
                        .max_h(px(220.0))
                        .overflow_y_scroll()
                        .text_size(px(Type::SMALL))
                        .text_color(theme.text)
                        .child(SharedString::from(text.clone()))
                        .into_any_element(),
                    Some(PreviewState::Ready { available: false, .. }) => div()
                        .text_size(px(Type::SMALL))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(
                            "This payload has no text form. The bytes are in the item; \
                             copy them as their own MIME type.",
                        ))
                        .into_any_element(),
                    Some(PreviewState::Loading) | Some(PreviewState::NotRequested) | None => div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::SM))
                        .child(ui::skeleton(theme, 220.0, 10.0))
                        .child(ui::skeleton(theme, 180.0, 10.0))
                        .child(ui::skeleton(theme, 200.0, 10.0))
                        .into_any_element(),
                    Some(PreviewState::Unavailable { reason }) => div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::XS))
                        .child(ui::section_label(theme, "no preview served"))
                        .child(
                            div()
                                .text_size(px(Type::MICRO))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(reason.clone())),
                        )
                        .into_any_element(),
                    Some(PreviewState::Refused { refusal }) => div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::XS))
                        .child(ui::section_label(theme, "preview refused"))
                        .child(
                            div()
                                .text_size(px(Type::MICRO))
                                .text_color(theme.refusal(&refusal.class))
                                .child(SharedString::from(refusal.headline())),
                        )
                        .into_any_element(),
                    Some(PreviewState::Failed { message }) => div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.bad)
                        .child(SharedString::from(format!("preview failed: {message}")))
                        .into_any_element(),
                };
                let _ = cx;
                frame.child(body).into_any_element()
            }
        }
    }

    fn nothing_selected(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        let live = app.store.live.clone();
        let mimes = app
            .store
            .service
            .as_ref()
            .map(|service| service.state.clone())
            .unwrap_or_else(|| "no service".to_string());
        drop(app);

        let live_block: AnyElement = match live {
            None => ui::skeleton(theme, 180.0, 10.0).into_any_element(),
            Some(Ok(smart_clipboard_proto::LiveContent::Empty)) => div()
                .text_size(px(Type::SMALL))
                .text_color(theme.text_muted)
                .child("The clipboard is empty, per the service.")
                .into_any_element(),
            Some(Ok(smart_clipboard_proto::LiveContent::Content { mime, bytes, text })) => div()
                .flex()
                .flex_col()
                .gap(px(Space::XS))
                .child(ui::mono(theme, mime))
                .child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format::bytes(bytes))),
                )
                .child(
                    div()
                        .text_size(px(Type::SMALL))
                        .text_color(theme.text)
                        .truncate()
                        .child(SharedString::from(
                            text.map(|text| format::snippet(&text, 120))
                                .unwrap_or_else(|| "binary payload".to_string()),
                        )),
                )
                .into_any_element(),
            Some(Err(refusal)) => div()
                .flex()
                .flex_col()
                .gap(px(Space::XS))
                .child(
                    div()
                        .text_size(px(Type::MICRO))
                        .text_color(theme.refusal(&refusal.class))
                        .child(SharedString::from(refusal.headline())),
                )
                .child(ui::provenance(
                    theme,
                    "the app asked the service; this is the service's answer",
                ))
                .into_any_element(),
        };

        let read_live = {
            let app = self.app.clone();
            cx.listener(move |_: &mut Self, _: &ClickEvent, _, cx| {
                app.update(cx, |app, cx| {
                    app.read_live();
                    cx.notify();
                });
            })
        };

        div()
            .flex()
            .flex_col()
            .gap(px(Space::LG))
            .p(px(Space::LG))
            .w_full()
            .child(ui::section_label(theme, "select an item"))
            .child(
                div()
                    .text_size(px(Type::SMALL))
                    .text_color(theme.text_muted)
                    .child("The inspector shows the payload, its provenance and the verbs the service allows."),
            )
            .child(ui::divider(theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ui::section_label(theme, "clipboard now"))
                    .child(
                        ui::button(theme, "read-live", "Read", ui::ButtonKind::Quiet)
                            .on_click(read_live),
                    ),
            )
            .child(live_block)
            .child(ui::divider(theme))
            .child(ui::field(theme, "service state", mimes))
            .into_any_element()
    }

    fn contract(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        let service = app.store.service.clone();
        let authority = app.store.authority.clone();
        let published = app.store.published.clone();
        let last_refusal = app.store.last_refusal.clone();
        let endpoint = app.store.connection.label();
        drop(app);

        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(Space::LG))
            .p(px(Space::LG))
            .w_full();

        column = column
            .child(ui::section_label(theme, "connection"))
            .child(ui::field(theme, "endpoint", endpoint));

        match service {
            None => {
                column = column.child(
                    div()
                        .text_size(px(Type::SMALL))
                        .text_color(theme.text_muted)
                        .child("No service has answered yet."),
                );
            }
            Some(service) => {
                column = column
                    .child(ui::field(theme, "service", service.name.clone()))
                    .child(ui::field(theme, "category", service.category.clone()))
                    .child(ui::field(theme, "declared state", service.state.clone()))
                    .child(ui::field(
                        theme,
                        "schema id",
                        format!(
                            "0x{:016x}{}",
                            service.schema_id,
                            if service.schema_matches_app {
                                " · matches this binary"
                            } else {
                                " · DRIFT from this binary"
                            }
                        ),
                    ))
                    .child(ui::field(
                        theme,
                        "endpoints",
                        if service.endpoints.is_empty() {
                            "none published".to_string()
                        } else {
                            service.endpoints.join(", ")
                        },
                    ));
                for (key, value) in &service.metadata {
                    column = column.child(ui::field(theme, key.clone(), value.clone()));
                }
                if !service.schema_matches_app {
                    column = column.child(
                        div()
                            .p(px(Space::MD))
                            .rounded(px(Radius::SMALL))
                            .bg(alpha(theme.warn, 0.10))
                            .border_1()
                            .border_color(alpha(theme.warn, 0.35))
                            .text_size(px(Type::MICRO))
                            .text_color(theme.warn)
                            .child(
                                "The service publishes a different contract generation than this \
                                 app compiled. Calls usually still work; a field that changed shape \
                                 will show up as a decode failure with the contract's own message.",
                            ),
                    );
                }

                column = column
                    .child(ui::divider(theme))
                    .child(ui::section_label(theme, "this connection's authority"));
                if authority.rows.is_empty() {
                    column = column.child(
                        div()
                            .text_size(px(Type::MICRO))
                            .text_color(theme.text_muted)
                            .child("The service published no capability rows for this connection."),
                    );
                }
                for row in &authority.rows {
                    column = column.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(Space::XXS))
                            .p(px(Space::MD))
                            .rounded(px(Radius::SMALL))
                            .bg(theme.canvas)
                            .border_1()
                            .border_color(theme.line)
                            .child(ui::mono(theme, row.id.clone()))
                            .child(ui::provenance(
                                theme,
                                format!(
                                    "target {} · operations {} · issuer {}",
                                    row.target,
                                    row.operations.join(", "),
                                    row.issuer
                                ),
                            )),
                    );
                }

                column = column
                    .child(ui::divider(theme))
                    .child(ui::section_label(theme, "method matrix (observed, this session)"));
                let mut matrix = div().flex().flex_col().gap(px(Space::XXS));
                for row in &service.methods {
                    let (dot, label) = match &row.observation {
                        Observation::Untested => (theme.text_faint, "not called".to_string()),
                        Observation::Answered => (theme.good, "answered".to_string()),
                        Observation::Refused(refusal) => {
                            (theme.refusal(&refusal.class), refusal.class.to_string())
                        }
                    };
                    matrix = matrix.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(Space::SM))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(Space::SM))
                                    .child(div().w(px(5.0)).h(px(5.0)).rounded(px(2.5)).bg(dot))
                                    .child(
                                        div()
                                            .text_size(px(Type::MICRO))
                                            .text_color(theme.text)
                                            .child(SharedString::from(row.name)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(Type::MICRO))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(label)),
                            ),
                    );
                }
                column = column
                    .child(matrix)
                    .child(ui::provenance(
                        theme,
                        "The contract publishes no method list, so this matrix is what the app \
                         observed — never what it assumed.",
                    ));
            }
        }

        if let Some(refusal) = last_refusal {
            column = column
                .child(ui::divider(theme))
                .child(ui::section_label(theme, "last refusal, verbatim"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::XS))
                        .child(ui::badge(
                            theme,
                            if refusal.class.is_authority() {
                                theme.warn
                            } else {
                                theme.bad
                            },
                            format!("{} · {}", refusal.verb, refusal.class),
                        ))
                        .child(ui::code_block(theme, refusal.raw.clone()))
                        .child(ui::provenance(
                            theme,
                            "this is the exact text the service sent; it is safe to paste into a \
                             bug report",
                        )),
                );
        }

        if !published.is_empty() {
            column = column
                .child(ui::divider(theme))
                .child(ui::section_label(theme, "what this host publishes"))
                .child(ui::provenance(theme, published.join(", ")));
        }

        column = column
            .child(ui::divider(theme))
            .child(ui::section_label(theme, "contract this binary compiled"))
            .child(ui::field(
                theme,
                "schema tree",
                smart_clipboard_proto::contract::SCHEMA_DIR,
            ))
            .child(ui::field(
                theme,
                "clipboard interface",
                format!("0x{:016x}", smart_clipboard_proto::contract::CLIPBOARD_INTERFACE_ID),
            ))
            .child(ui::provenance(
                theme,
                "No payload is ever written to disk by this app. The only state that persists \
                 anywhere is the backend's.",
            ));

        column.into_any_element()
    }

    /// Render the panel body (used by the library and by the palette when it
    /// wants the full inspector).
    pub fn render_panel(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.app.read(cx).inspector_tab;
        let tabs = self.tabs(theme, cx);
        let body = match tab {
            InspectorTab::Payload => self.payload(theme, cx),
            InspectorTab::Contract => self.contract(theme, cx),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(tabs)
            .child(
                div()
                    .id("inspector-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for InspectorView {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.app.read(cx).theme();
        self.render_panel(&theme, cx)
    }
}

/// Kept for the palette: a compact "why is this refused" strip, so a refusal
/// can be read without opening the library.
pub fn compact_refusal(theme: &Theme, refusal: &smart_clipboard_proto::Refusal) -> Div {
    let color = theme.refusal(&refusal.class);
    div()
        .flex()
        .flex_col()
        .gap(px(Space::XXS))
        .child(
            div()
                .text_size(px(Type::MICRO))
                .font_weight(Weight::HEADLINE)
                .text_color(color)
                .child(SharedString::from(format!(
                    "{} · {}",
                    refusal.verb, refusal.class
                ))),
        )
        .child(
            div()
                .text_size(px(Type::MICRO))
                .text_color(theme.text_muted)
                .child(SharedString::from(refusal.reason.clone())),
        )
}

/// A one-line summary of a refusal class, for tooltips.
pub fn class_hint(class: &RefusalClass) -> &'static str {
    if class.is_authority() {
        "this connection lacks the authority; reconnect as a control-capable peer"
    } else {
        "the host or backend cannot do this; see the reason"
    }
}
