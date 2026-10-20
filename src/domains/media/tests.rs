use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::media_capnp;

use super::availability::{status_counts, AvailStatus, MEDIA_AVAILABILITY, MEDIA_SCHEMA_ID};
use super::backend::MediaBackend;
use super::types::{MediaMetadata, MediaPlayer};

const CONTRACT_METHODS: &[&str] = &[
    "listPlayers","getPlayer","play","pause","stop","next","previous","seek","getMetadata","getPosition",
    "setVolume","setPosition","openUri","subscribePlayer","getSupportedFormats","transcode",
    "getRecommendations","getAnalytics","shareMedia","downloadMedia","setCrossfade","setSyncGroup",
    "createPlaylist","addToPlaylist","removeFromPlaylist","reorderPlaylist","getRecommendationsWithScores",
    "getSharePreview","getDownloadStatus","listDownloads","pauseDownload","resumeDownload","cancelDownload",
    "subscribeDownload","setCrossfadeConfig","getCrossfadeConfig","sendLiveInteraction",
    "subscribeLiveInteractions","subscribeTranscode",
];

struct HonestBackend;
impl MediaBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(MEDIA_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in MEDIA_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn media_metadata_round_trips() {
    let m = MediaMetadata { title: "t".to_string(), artist: "a".to_string(), album: "b".to_string(), duration: crate::core::values::Duration::from_nanos(10), art_uri: "u".to_string() };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<media_capnp::media_metadata::Builder<'_>>();
        m.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new()).expect("deserialize");
    let wire = reader.get_root::<media_capnp::media_metadata::Reader<'_>>().expect("root");
    assert_eq!(MediaMetadata::from_reader(&wire), m);
}
