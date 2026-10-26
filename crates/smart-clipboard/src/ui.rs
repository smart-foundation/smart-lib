//! The small, repeated visual parts: marks, chips, key hints, empty states.
//!
//! Keeping them here rather than in each view is not only about repetition.
//! A clipboard window's credibility comes from *consistency of meaning*: a
//! MIME family must look the same in the rail's counts, in a row, in the
//! palette and in the inspector, or the user has to re-learn it four times.
//! One function per meaning is the cheapest way to guarantee that.
//!
//! ## Why the marks are drawn, not icons
//!
//! There is no icon font and no SVG asset pipeline in this app, for two
//! reasons: an icon font is a licence and a fallback-rendering problem on
//! every distro, and an SVG pipeline is an asset loader plus a cache for six
//! shapes. So the six marks are composed from divs — a rect, a bar, a dot —
//! which renders identically everywhere, scales with the layout, and costs
//! nothing to load. The shapes are also *distinct silhouettes*, so the marks
//! survive a colourblind user, a monochrome screenshot, and a low-contrast
//! display.

use gpui::{
    div, px, Div, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Rgba,
    SharedString, Stateful, Styled,
};
use smart_clipboard_proto::MimeFamily;

use crate::theme::{Radius, Space, Theme, Type, Weight};

/// Re-exported from the tokens: `ui::alpha` and `theme::alpha` are the same
/// function, so the two import paths a view might reach for cannot drift.
pub use crate::theme::alpha;

/// A hairline.
pub fn divider(theme: &Theme) -> Div {
    div().h(px(1.0)).w_full().bg(theme.line)
}

/// A section label above a group: small, upper-case, faint. The one piece of
/// all-caps in the UI, used only for labels that are not sentences.
pub fn section_label(theme: &Theme, label: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(Type::MICRO))
        .font_weight(Weight::HEADLINE)
        .text_color(theme.text_faint)
        .child(label.into())
}

/// A short inline code chip: ids, MIME types, byte counts.
pub fn mono(theme: &Theme, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .px(px(Space::XS))
        .py(px(1.0))
        .rounded(px(Radius::SMALL))
        .bg(alpha(theme.text_faint, 0.10))
        .text_size(px(Type::MICRO))
        .text_color(theme.text_muted)
        .child(value.into())
}

/// A label chip, optionally active (the selected facet, the active sort).
pub fn chip(theme: &Theme, label: impl Into<SharedString>, active: bool) -> Div {
    let (background, foreground) = if active {
        (theme.signal_soft, theme.text)
    } else {
        (alpha(theme.text_faint, 0.08), theme.text_muted)
    };
    div()
        .flex()
        .items_center()
        .gap(px(Space::XS))
        .px(px(Space::MD))
        .py(px(3.0))
        .rounded(px(Radius::PILL))
        .bg(background)
        .text_size(px(Type::SMALL))
        .text_color(foreground)
        .child(label.into())
}

/// A state badge: a dot plus a word. The word is what carries the meaning;
/// the dot reinforces it. Nothing here is colour-only.
pub fn badge(theme: &Theme, color: Rgba, label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(Space::SM))
        .child(div().w(px(6.0)).h(px(6.0)).rounded(px(3.0)).bg(color))
        .child(
            div()
                .text_size(px(Type::SMALL))
                .text_color(theme.text_muted)
                .child(label.into()),
        )
}

/// A button. `kind` picks the emphasis; every kind is a real hit target of at
/// least 28 px, because a clipboard window is used with a trackpad in a hurry.
/// A button.
///
/// The return type is `Stateful<Div>`, not `Div`, because a button must have an
/// id (it is a click target, a hover target and a focus stop) and in gpui 0.2 an
/// `id` is what makes an element stateful. Returning `Div` would compile the id
/// away and then fail at every `.on_click`.
pub fn button(
    theme: &Theme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
) -> Stateful<Div> {
    let (background, foreground, border) = match kind {
        ButtonKind::Primary => (theme.signal_soft, theme.text, theme.signal),
        ButtonKind::Secondary => (alpha(theme.text_faint, 0.10), theme.text, theme.line),
        ButtonKind::Quiet => (alpha(theme.text_faint, 0.0), theme.text_muted, theme.line),
        ButtonKind::Danger => (alpha(theme.bad, 0.12), theme.bad, theme.bad),
    };
    let hover_background = match kind {
        ButtonKind::Primary => alpha(theme.signal, 0.28),
        ButtonKind::Danger => alpha(theme.bad, 0.20),
        _ => alpha(theme.text_faint, 0.18),
    };
    let hover_border = match kind {
        _ => theme.line_strong,
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(Space::XS))
        .h(px(28.0))
        .px(px(Space::LG))
        .rounded(px(Radius::MEDIUM))
        .border_1()
        .border_color(border)
        .bg(background)
        .text_size(px(Type::SMALL))
        .font_weight(Weight::STRONG)
        .text_color(foreground)
        .cursor_pointer()
        .hover(move |style| style.bg(hover_background).border_color(hover_border))
        .child(label.into())
}

/// A button's emphasis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main action of a region.
    Primary,
    /// A normal action.
    Secondary,
    /// An action that is present but not shouting.
    Quiet,
    /// A destructive action.
    Danger,
}

/// A keyboard hint: the keys in a boxed style so the eye separates "what I
/// press" from "what it does".
pub fn key_hint(theme: &Theme, keys: impl Into<SharedString>) -> Div {
    div()
        .px(px(Space::XS))
        .py(px(1.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme.line)
        .text_size(px(10.0))
        .font_weight(Weight::STRONG)
        .text_color(theme.text_faint)
        .child(keys.into())
}

/// A hint row: keys, then what they do.
pub fn hint(theme: &Theme, keys: &'static str, what: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(Space::SM))
        .child(key_hint(theme, keys))
        .child(
            div()
                .text_size(px(Type::MICRO))
                .text_color(theme.text_faint)
                .child(what),
        )
}

/// A one-pixel bar, the atom of every drawn mark.
fn bar(width: f32, height: f32, color: Rgba) -> Div {
    div()
        .w(px(width))
        .h(px(height))
        .rounded(px(height / 2.0))
        .bg(color)
}

/// The family mark: a rounded tile with a silhouette inside.
///
/// The six silhouettes are deliberately different in *shape* first (bars,
/// a chain, a framed picture, a page, a circle, a ring) and only secondarily
/// in colour.
pub fn glyph(theme: &Theme, family: MimeFamily, size: f32) -> Div {
    let color = theme.family(family);
    let tint = alpha(color, 0.14);
    let tile = div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(size))
        .h(px(size))
        .rounded(px(Radius::SMALL))
        .bg(tint);

    match family {
        MimeFamily::Text => tile.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(bar(size * 0.52, 2.0, color))
                .child(bar(size * 0.34, 2.0, color))
                .child(bar(size * 0.44, 2.0, color)),
        ),
        MimeFamily::Link => tile.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(bar(size * 0.44, 3.0, color))
                .child(
                    div()
                        .flex()
                        .gap(px(1.0))
                        .child(bar(size * 0.18, 3.0, color))
                        .child(bar(size * 0.18, 3.0, color)),
                ),
        ),
        MimeFamily::Image => tile.child(
            div()
                .relative()
                .w(px(size * 0.54))
                .h(px(size * 0.42))
                .rounded(px(2.0))
                .border_1()
                .border_color(color)
                .child(
                    div()
                        .absolute()
                        .top(px(3.0))
                        .left(px(3.0))
                        .w(px(3.0))
                        .h(px(3.0))
                        .rounded(px(1.5))
                        .bg(color),
                ),
        ),
        MimeFamily::File => tile.child(
            div()
                .w(px(size * 0.40))
                .h(px(size * 0.50))
                .rounded(px(1.5))
                .border_1()
                .border_color(color),
        ),
        MimeFamily::Color => tile.child(
            div()
                .w(px(size * 0.44))
                .h(px(size * 0.44))
                .rounded(px(size * 0.22))
                .bg(color),
        ),
        MimeFamily::Other => tile.child(
            div()
                .w(px(size * 0.40))
                .h(px(size * 0.40))
                .rounded(px(size * 0.20))
                .border_1()
                .border_color(color),
        ),
    }
}

/// A colour swatch, for a payload that is literally a colour.
pub fn swatch(color: u32, size: f32, theme: &Theme) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .bg(Rgba {
            r: ((color >> 16) & 0xff) as f32 / 255.0,
            g: ((color >> 8) & 0xff) as f32 / 255.0,
            b: (color & 0xff) as f32 / 255.0,
            a: 1.0,
        })
        .border_1()
        .border_color(theme.line_strong)
}

/// The checkerboard behind a payload with transparency, so "transparent" never
/// reads as "white background" or "broken".
pub fn checker(theme: &Theme, size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(Radius::SMALL))
        .bg(theme.checker_a)
        .flex()
        .flex_wrap()
        .child(
            div()
                .w(px(size / 2.0))
                .h(px(size / 2.0))
                .bg(theme.checker_b),
        )
        .child(
            div()
                .w(px(size / 2.0))
                .h(px(size / 2.0))
                .bg(theme.checker_a),
        )
        .child(
            div()
                .w(px(size / 2.0))
                .h(px(size / 2.0))
                .bg(theme.checker_a),
        )
        .child(
            div()
                .w(px(size / 2.0))
                .h(px(size / 2.0))
                .bg(theme.checker_b),
        )
}

/// A skeleton line: what the UI draws while a *service* answer is in flight.
///
/// It is a rectangle of the panel's own colour, not a shimmer: an animation
/// that says "loading" for a 2 ms round trip is a flash of noise, and
/// disabled-motion users would see the skeleton jump. The row's own state
/// label ("not requested" / "loading" / "not served") does the real work.
pub fn skeleton(theme: &Theme, width: f32, height: f32) -> Div {
    div()
        .w(px(width))
        .h(px(height))
        .rounded(px(Radius::SMALL))
        .bg(alpha(theme.text_faint, 0.12))
}

/// An empty state: a headline, a body and a hint. Every empty state in the app
/// is one of these, because "nothing here" must always say *why* and *what to
/// do*, and a blank panel says neither.
pub fn empty_state(
    theme: &Theme,
    headline: impl Into<SharedString>,
    body: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(Space::MD))
        .p(px(Space::XXL))
        .child(
            div()
                .text_size(px(Type::DISPLAY))
                .font_weight(Weight::HEADLINE)
                .text_color(theme.text)
                .child(headline.into()),
        )
        .child(
            div()
                .max_w(px(420.0))
                .text_size(px(Type::BODY))
                .text_color(theme.text_muted)
                .child(body.into()),
        )
}

/// A labelled value in the inspector's provenance block.
pub fn field(theme: &Theme, label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(Space::XXS))
        .py(px(Space::XS))
        .child(section_label(theme, label))
        .child(
            div()
                .text_size(px(Type::SMALL))
                .text_color(theme.text)
                .child(value.into()),
        )
}

/// A monospaced, wrapping block for text the user may paste back.
pub fn code_block(theme: &Theme, value: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .p(px(Space::MD))
        .rounded(px(Radius::SMALL))
        .bg(alpha(theme.text_faint, 0.07))
        .text_size(px(Type::SMALL))
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.text)
        .child(value.into())
}

/// The window's own title strip content: a mark, the product name and a tag
/// line that says what the window is a window *onto*.
pub fn product_header(theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(Space::MD))
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded(px(Radius::SMALL))
                .bg(theme.signal_soft)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(2.0))
                        .bg(theme.signal),
                ),
        )
        .child(
            div()
                .text_size(px(Type::TITLE))
                .font_weight(Weight::HEADLINE)
                .text_color(theme.text)
                .child("smart-clipboard"),
        )
}

/// A right-aligned count, for the rail.
pub fn count(theme: &Theme, value: usize) -> Div {
    div()
        .text_size(px(Type::MICRO))
        .text_color(theme.text_faint)
        .child(SharedString::from(value.to_string()))
}

/// The provenance line under a section: the one place a view says where a
/// number came from.
pub fn provenance(theme: &Theme, label: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(Type::MICRO))
        .text_color(theme.text_faint)
        .child(label.into())
}
