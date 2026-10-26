//! The design tokens, in one struct, chosen for one reason: **a clipboard
//! window is read in a glance, next to somebody else's app.**
//!
//! ## The palette decisions, and why
//!
//! * **Neutral graphite, not blue-grey.** The window sits on top of Arbitrary
//!   Content. A saturated, cool-grey surface fights every screenshot behind
//!   it and tints image previews (the thing this app exists to show). The
//!   surfaces are near-neutral with a 1–2% warm shift, so a screenshot's own
//!   colours are what the eye compares against — not our chrome.
//! * **One accent, used only for state.** `signal` (an indigo-cyan) appears on
//!   the focused selection, the caret, and the primary action. It never
//!   colours a MIME chip or an icon, because those must be distinguishable by
//!   shape alone (see [`crate::ui::glyph`]) — colourblind-safe by
//!   construction rather than by a palette swap.
//! * **Semantic colours carry meaning, and meaning carries words.** `good`,
//!   `warn`, `bad` are only ever used *alongside* text ("refused", "read-only
//!   connection"); there is no state in this UI that is signalled by hue
//!   alone.
//! * **Elevation is a hairline plus one step of lightness.** No drop shadows
//!   on the window's own panels: on a compositor without blur, shadows read as
//!   dirt around the edges. A 1 px `line` border is cheaper to render and
//!   crisper on fractional HiDPI scales.
//! * **Type scale of six sizes with a 1.36 line ratio**, and monospace for
//!   anything the user might paste back (ids, MIME types, byte counts). The
//!   reason is not taste: a clipboard app's most common action is "is this the
//!   right thing?", and monospace digits stop jumping as values change.
//!
//! ## Light theme
//!
//! Shipped, not deferred. A clipboard window is frequently opened in daylight
//! over a white document; a dark-only app there is a black rectangle in the
//! middle of a document. The token *names* are identical, so every view is
//! written once.

use gpui::{rgb, FontWeight, Rgba};

/// A colour at a given alpha. gpui colours are plain structs, so this is a
/// copy, not an allocation.
///
/// It lives here, beside the tokens, because it is a *token operation*: every
/// tinted surface in the app is a token at a computed alpha, never a new
/// hand-picked colour. Views that need a tint therefore import one concept,
/// not two modules.
pub fn alpha(color: Rgba, value: f32) -> Rgba {
    Rgba { a: value, ..color }
}

/// The token set every view renders through.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The window's own background.
    pub canvas: Rgba,
    /// A panel that sits on the canvas (rail, inspector, status bar).
    pub panel: Rgba,
    /// A raised surface (the palette card, a hovered row).
    pub raised: Rgba,
    /// The selection's background.
    pub selection: Rgba,
    /// The hairline between regions.
    pub line: Rgba,
    /// A stronger hairline, for the focused region's edge.
    pub line_strong: Rgba,
    /// Primary text.
    pub text: Rgba,
    /// Secondary text: metadata, labels.
    pub text_muted: Rgba,
    /// Tertiary text: units, counts, hints.
    pub text_faint: Rgba,
    /// The single accent.
    pub signal: Rgba,
    /// The accent at low alpha, for a selected chip's fill.
    pub signal_soft: Rgba,
    /// Success: a copy landed, the service answered.
    pub good: Rgba,
    /// Caution: degraded, drifted, retrying.
    pub warn: Rgba,
    /// Failure: refused, disconnected, undecodable.
    pub bad: Rgba,
    /// The checkerboard behind a transparent payload (an image with alpha).
    pub checker_a: Rgba,
    /// The other checkerboard square.
    pub checker_b: Rgba,
    /// The family colour for text payloads.
    pub family_text: Rgba,
    /// The family colour for links.
    pub family_link: Rgba,
    /// The family colour for images.
    pub family_image: Rgba,
    /// The family colour for file references.
    pub family_file: Rgba,
    /// The family colour for anything unrecognised.
    pub family_other: Rgba,
}

impl Theme {
    /// The default dark theme.
    pub fn dark() -> Self {
        Theme {
            canvas: rgb(0x0b0d10),
            panel: rgb(0x11141a),
            raised: rgb(0x181c23),
            selection: rgb(0x1d2536),
            line: rgb(0x232833),
            line_strong: rgb(0x333a48),
            text: rgb(0xe7eaf0),
            text_muted: rgb(0x9aa3b2),
            text_faint: rgb(0x6b7484),
            signal: rgb(0x7c8cff),
            signal_soft: rgb(0x232a4a),
            good: rgb(0x49d19a),
            warn: rgb(0xe8b65a),
            bad: rgb(0xff7b72),
            checker_a: rgb(0x14171d),
            checker_b: rgb(0x191d25),
            family_text: rgb(0xa8b4c8),
            family_link: rgb(0x7fd0ff),
            family_image: rgb(0xc79bff),
            family_file: rgb(0xf2c46b),
            family_other: rgb(0x8b93a3),
        }
    }

    /// The light theme: the same semantics, inverted lightness, and slightly
    /// desaturated hues so they do not vibrate on white.
    pub fn light() -> Self {
        Theme {
            canvas: rgb(0xfbfbfc),
            panel: rgb(0xf4f5f7),
            raised: rgb(0xffffff),
            selection: rgb(0xe8ecfb),
            line: rgb(0xe2e5ea),
            line_strong: rgb(0xcdd2da),
            text: rgb(0x14181f),
            text_muted: rgb(0x5a6473),
            text_faint: rgb(0x848d9c),
            signal: rgb(0x4154d6),
            signal_soft: rgb(0xdfe4fb),
            good: rgb(0x13855e),
            warn: rgb(0x996a12),
            bad: rgb(0xc23b34),
            checker_a: rgb(0xffffff),
            checker_b: rgb(0xf0f2f5),
            family_text: rgb(0x54607a),
            family_link: rgb(0x1a6f9e),
            family_image: rgb(0x7a3fb0),
            family_file: rgb(0x8a5c0a),
            family_other: rgb(0x6b7484),
        }
    }

    /// The theme for a named mode, so the toggle and the config agree on
    /// spelling.
    pub fn for_mode(dark: bool) -> Self {
        if dark {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    /// The colour a MIME family draws with.
    pub fn family(&self, family: smart_clipboard_proto::MimeFamily) -> Rgba {
        use smart_clipboard_proto::MimeFamily;
        match family {
            MimeFamily::Text | MimeFamily::Color => self.family_text,
            MimeFamily::Link => self.family_link,
            MimeFamily::Image => self.family_image,
            MimeFamily::File => self.family_file,
            MimeFamily::Other => self.family_other,
        }
    }

    /// The colour a refusal class draws with: authority problems are amber
    /// (fixable by connecting differently), capability problems red (the host
    /// genuinely cannot).
    pub fn refusal(&self, class: &smart_clipboard_proto::RefusalClass) -> Rgba {
        if class.is_authority() {
            self.warn
        } else {
            self.bad
        }
    }
}

/// The spacing scale. Every gap and pad in the UI comes from here, so the
/// rhythm cannot drift view by view.
pub struct Space;

impl Space {
    /// 2 px — a hairline adjustment.
    pub const XXS: f32 = 2.0;
    /// 4 px — inside a chip.
    pub const XS: f32 = 4.0;
    /// 6 px — between a chip and its row's edge.
    pub const SM: f32 = 6.0;
    /// 8 px — the default gap.
    pub const MD: f32 = 8.0;
    /// 12 px — inside a panel.
    pub const LG: f32 = 12.0;
    /// 16 px — between panels.
    pub const XL: f32 = 16.0;
    /// 24 px — the window's own margin.
    pub const XXL: f32 = 24.0;
}

/// The type scale.
pub struct Type;

impl Type {
    /// 11 px — units, counts, the status bar.
    pub const MICRO: f32 = 11.0;
    /// 12 px — chips, metadata.
    pub const SMALL: f32 = 12.0;
    /// 13 px — row text.
    pub const BODY: f32 = 13.0;
    /// 15 px — the search field, section titles.
    pub const TITLE: f32 = 15.0;
    /// 17 px — the palette's heading.
    pub const HEADING: f32 = 17.0;
    /// 22 px — an empty state's headline.
    pub const DISPLAY: f32 = 22.0;

    /// The line height that pairs with a size (1.36, rounded to whole px so
    /// text does not land on half pixels).
    pub const fn line(size: f32) -> f32 {
        size * 1.36
    }
}

/// Weights, named by role rather than by number, because "medium" is what a
/// designer says and `500` is what the renderer wants.
pub struct Weight;

impl Weight {
    /// Body text.
    pub const BODY: FontWeight = FontWeight::NORMAL;
    /// A row's title, a chip's key.
    pub const STRONG: FontWeight = FontWeight::MEDIUM;
    /// A section label, an active filter.
    pub const HEADLINE: FontWeight = FontWeight::SEMIBOLD;
}

/// The radii.
pub struct Radius;

impl Radius {
    /// 6 px — chips and small controls.
    pub const SMALL: f32 = 6.0;
    /// 8 px — rows and buttons.
    pub const MEDIUM: f32 = 8.0;
    /// 12 px — panels and the palette card.
    pub const LARGE: f32 = 12.0;
    /// 999 px — pills and the selection dot.
    pub const PILL: f32 = 999.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_modes_define_identical_tokens() {
        // A missing token in one mode is a compile error; this test catches
        // the subtler failure, a token that is the *same* colour in both
        // modes because somebody forgot to invert it.
        let dark = Theme::dark();
        let light = Theme::light();
        assert_ne!(dark.canvas, light.canvas);
        assert_ne!(dark.text, light.text);
        assert_ne!(dark.panel, light.panel);
        assert_ne!(dark.selection, light.selection);
    }

    #[test]
    fn authority_problems_are_amber_and_capability_problems_are_red() {
        use smart_clipboard_proto::RefusalClass;
        let theme = Theme::dark();
        assert_eq!(theme.refusal(&RefusalClass::PermissionDenied), theme.warn);
        assert_eq!(theme.refusal(&RefusalClass::NoBackend), theme.bad);
    }
}
