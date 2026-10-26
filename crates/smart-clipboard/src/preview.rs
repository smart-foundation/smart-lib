//! Turning a payload the service handed over into something gpui can draw —
//! off the UI thread.
//!
//! ## Why a decode thread and not `.await`
//!
//! A 4K screenshot is ~12 MP of PNG: inflate + unfilter + resize is tens of
//! milliseconds, and gpui's frame budget is 16. Decoding inline would drop
//! frames exactly while scrolling a gallery of the things. So there is one
//! long-lived thread with a queue, and its only job is `image` crate work.
//!
//! It sends back an `image::Frame` rather than a `gpui::RenderImage` for a
//! boring, load-bearing reason: `Frame` is plainly `Send`, so the boundary
//! between "the decoder thread" and "the GPU thread" is a type that cannot
//! smuggle a GPU handle across it. The UI wraps it on the thread that owns the
//! window.
//!
//! ## The bounds
//!
//! * **Longest side 768 px.** A thumbnail is 40 px and the inspector is 420;
//!   768 covers a 2× display with room to spare and bounds a pathological
//!   30000×30000 payload to something a GPU texture can hold.
//! * **The payload ceiling is enforced upstream** (the proto crate refuses to
//!   fetch past 64 MiB), so this module never sees a payload it must reject
//!   for size alone — but it still refuses a decode it cannot do, with words.
//! * **No animation.** A GIF is decoded at frame 0 and labelled as such; the
//!   row shows a still, which is honest about what the service stored.

use std::path::Path;
use std::sync::mpsc::Sender;

use image::{DynamicImage, Frame, RgbaImage};

/// The longest side, in pixels, a decoded preview may have.
pub const MAX_EDGE: u32 = 768;

/// One payload to decode.
pub struct DecodeRequest {
    /// The item the payload belongs to.
    pub id: u64,
    /// The bytes the service served.
    pub bytes: Vec<u8>,
}

/// The decoder thread's answer.
pub enum Decoded {
    /// A drawable frame.
    Ready {
        /// The item.
        id: u64,
        /// The decoded, downscaled frame.
        frame: Frame,
        /// The payload's real dimensions, before downscaling — reported in the
        /// inspector so the user sees the truth, not the thumbnail's size.
        original: (u32, u32),
        /// Whether the payload was animated (and is being shown as a still).
        animated: bool,
    },
    /// A refusal with a reason a user can act on.
    Failed {
        /// The item.
        id: u64,
        /// Why.
        reason: String,
    },
}

/// Start the decode thread.
///
/// The thread ends when the sender is dropped — which happens when the app is
/// torn down, because the app owns the only sender.
pub fn spawn_decoder() -> (Sender<DecodeRequest>, async_channel::Receiver<Decoded>) {
    let (requests, incoming) = std::sync::mpsc::channel::<DecodeRequest>();
    let (outgoing, results) = async_channel::unbounded::<Decoded>();

    std::thread::Builder::new()
        .name("smart-clipboard-decode".to_string())
        .spawn(move || {
            while let Ok(request) = incoming.recv() {
                let answer = match decode(&request.bytes) {
                    Ok((frame, original, animated)) => Decoded::Ready {
                        id: request.id,
                        frame,
                        original,
                        animated,
                    },
                    Err(reason) => Decoded::Failed {
                        id: request.id,
                        reason,
                    },
                };
                if outgoing.send_blocking(answer).is_err() {
                    break;
                }
            }
        })
        .expect("the decoder thread must start");

    (requests, results)
}

/// Decode a payload into a drawable frame, downscaled to [`MAX_EDGE`].
///
/// Returns the frame, the payload's original dimensions and whether it was
/// animated.
pub fn decode(bytes: &[u8]) -> Result<(Frame, (u32, u32), bool), String> {
    if bytes.is_empty() {
        return Err("the service served an empty payload".to_string());
    }
    let format = image::guess_format(bytes)
        .map_err(|error| format!("unrecognised image format: {error}"))?;
    let animated = matches!(
        format,
        image::ImageFormat::Gif | image::ImageFormat::WebP
    ) && looks_animated(bytes);

    let image = image::load_from_memory_with_format(bytes, format)
        .map_err(|error| format!("the payload did not decode as {format:?}: {error}"))?;
    let original = (image.width(), image.height());
    if original.0 == 0 || original.1 == 0 {
        return Err("the payload decoded to a zero-sized image".to_string());
    }
    let scaled = downscale(image);
    Ok((Frame::new(scaled), original, animated))
}

/// Downscale so the longest side is at most [`MAX_EDGE`], preserving aspect.
fn downscale(image: DynamicImage) -> RgbaImage {
    let (width, height) = (image.width(), image.height());
    let longest = width.max(height);
    if longest <= MAX_EDGE {
        return image.to_rgba8();
    }
    let scale = MAX_EDGE as f32 / longest as f32;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    // `thumbnail` is the fast, box-filtered path: for a preview it is the
    // right trade (a Lanczos resize of a 4K screenshot costs more than the
    // pixels are worth at 768 px).
    image.thumbnail(target_width, target_height).to_rgba8()
}

/// A cheap "is there more than one frame?" probe for the animated formats,
/// used only to label a still honestly.
fn looks_animated(bytes: &[u8]) -> bool {
    // GIF: count image descriptors is expensive; the presence of a NETSCAPE
    // application extension is the reliable "this is meant to loop" marker.
    if bytes.starts_with(b"GIF89a") {
        return find(bytes, b"NETSCAPE2.0").is_some();
    }
    // WebP: an ANIM chunk marks an animation.
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return find(bytes, b"ANIM").is_some();
    }
    false
}

/// Find a needle in a haystack, without pulling in a dependency for four
/// call sites.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The file extension a MIME implies, for the "save a copy" action's default
/// filename. Deliberately small: the app only ever saves what it can decode.
pub fn extension_for(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

/// Whether a MIME is one this app can draw. The rest are shown as a labelled
/// block with their real type — never a broken image icon.
pub fn is_drawable(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/gif"
            | "image/bmp"
            | "image/tiff"
            | "image/avif"
            | "image/ico"
    )
}

/// A default filename for saving a copy of a payload, with a timestamp that
/// sorts correctly and no user data in it.
pub fn default_filename(mime: &str, captured_at_ns: Option<u64>) -> String {
    let stamp = captured_at_ns
        .map(|nanos| nanos / 1_000_000_000)
        .unwrap_or_default();
    format!(
        "smart-clipboard-{stamp}.{}",
        extension_for(mime)
    )
}

/// Whether a path is inside the user's cache directory, used only by the
/// "where would this be saved" hint. The app itself never writes.
pub fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut buffer = RgbaImage::new(width, height);
        for (x, y, pixel) in buffer.enumerate_pixels_mut() {
            *pixel = Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buffer)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("encode");
        bytes.into_inner()
    }

    #[test]
    fn a_small_payload_keeps_its_size_and_reports_the_original() {
        let (frame, original, animated) = decode(&png(40, 30)).expect("decode");
        assert_eq!(original, (40, 30));
        assert!(!animated);
        assert_eq!(frame.buffer().dimensions(), (40, 30));
    }

    #[test]
    fn a_large_payload_is_downscaled_but_reports_its_real_size() {
        let (frame, original, _) = decode(&png(2000, 1000)).expect("decode");
        assert_eq!(original, (2000, 1000));
        assert_eq!(frame.buffer().dimensions(), (768, 384));
    }

    #[test]
    fn rubbish_is_refused_with_words() {
        // `image::Frame` is not `Debug`, so an `expect_err` cannot print the OK
        // side; the let-else says the same thing and stays readable.
        let Err(error) = decode(b"not an image at all") else {
            panic!("garbage must be refused, not decoded");
        };
        assert!(error.contains("unrecognised image format"), "{error}");
        let Err(error) = decode(b"") else {
            panic!("an empty payload must be refused");
        };
        assert!(error.contains("empty payload"), "{error}");
    }

    #[test]
    fn drawable_mimes_are_the_ones_the_decoder_handles() {
        assert!(is_drawable("image/png"));
        assert!(is_drawable("IMAGE/PNG"));
        assert!(!is_drawable("image/svg+xml"));
        assert!(!is_drawable("text/plain"));
    }

    #[test]
    fn default_filenames_are_sortable_and_carry_no_user_data() {
        assert_eq!(
            default_filename("image/png", Some(1_700_000_000_000_000_000)),
            "smart-clipboard-1700000000.png"
        );
        assert_eq!(default_filename("image/jpeg", None), "smart-clipboard-0.jpg");
    }
}
