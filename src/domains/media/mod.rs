//! The **media** domain: MPRIS playback control — over one object-safe
//! [`MediaBackend`] trait. Native mechanism: MPRIS over D-Bus (session bus).

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Media;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MEDIA_AVAILABILITY,
    MEDIA_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::MediaBackend;
pub use types::{MediaMetadata, MediaPlayer, Page, Pagination, PlaybackStatus};
