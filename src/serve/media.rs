#![allow(missing_docs)]
//! The media domain, served through the contract's generated `Server` trait.
extern crate std;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::rc::Rc;
use std::sync::Arc;
use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;
use crate::core::ErrorCode;
use crate::domains::media::availability;
use crate::domains::media::backend::MediaBackend;
use crate::schema::media_capnp::media as med;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct MediaService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for MediaService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> MediaService<B>
where
    B: MediaBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's media state, and this connection holds the read-only class"),
    ))
}

fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Media contract method"),
        )),
    }
}

fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

impl<B> med::Server for MediaService<B>
where
    B: MediaBackend + 'static,
{
    fn list_players(
        self: Rc<Self>,
        _params: med::ListPlayersParams,
        _results: med::ListPlayersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listPlayers") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_player(
        self: Rc<Self>,
        _params: med::GetPlayerParams,
        _results: med::GetPlayerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getPlayer") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn play(
        self: Rc<Self>,
        _params: med::PlayParams,
        _results: med::PlayResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("play") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn pause(
        self: Rc<Self>,
        _params: med::PauseParams,
        _results: med::PauseResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("pause") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn stop(
        self: Rc<Self>,
        _params: med::StopParams,
        _results: med::StopResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("stop") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn next(
        self: Rc<Self>,
        _params: med::NextParams,
        _results: med::NextResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("next") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn previous(
        self: Rc<Self>,
        _params: med::PreviousParams,
        _results: med::PreviousResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("previous") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn seek(
        self: Rc<Self>,
        _params: med::SeekParams,
        _results: med::SeekResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("seek") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_metadata(
        self: Rc<Self>,
        _params: med::GetMetadataParams,
        _results: med::GetMetadataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getMetadata") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_position(
        self: Rc<Self>,
        _params: med::GetPositionParams,
        _results: med::GetPositionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getPosition") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_volume(
        self: Rc<Self>,
        _params: med::SetVolumeParams,
        _results: med::SetVolumeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setVolume") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_position(
        self: Rc<Self>,
        _params: med::SetPositionParams,
        _results: med::SetPositionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setPosition") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn open_uri(
        self: Rc<Self>,
        _params: med::OpenUriParams,
        _results: med::OpenUriResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("openUri") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_player(
        self: Rc<Self>,
        _params: med::SubscribePlayerParams,
        _results: med::SubscribePlayerResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribePlayer") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_supported_formats(
        self: Rc<Self>,
        _params: med::GetSupportedFormatsParams,
        _results: med::GetSupportedFormatsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getSupportedFormats") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn transcode(
        self: Rc<Self>,
        _params: med::TranscodeParams,
        _results: med::TranscodeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("transcode") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_recommendations(
        self: Rc<Self>,
        _params: med::GetRecommendationsParams,
        _results: med::GetRecommendationsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getRecommendations") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_analytics(
        self: Rc<Self>,
        _params: med::GetAnalyticsParams,
        _results: med::GetAnalyticsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getAnalytics") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn share_media(
        self: Rc<Self>,
        _params: med::ShareMediaParams,
        _results: med::ShareMediaResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("shareMedia") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn download_media(
        self: Rc<Self>,
        _params: med::DownloadMediaParams,
        _results: med::DownloadMediaResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("downloadMedia") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_crossfade(
        self: Rc<Self>,
        _params: med::SetCrossfadeParams,
        _results: med::SetCrossfadeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setCrossfade") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_sync_group(
        self: Rc<Self>,
        _params: med::SetSyncGroupParams,
        _results: med::SetSyncGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setSyncGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn create_playlist(
        self: Rc<Self>,
        _params: med::CreatePlaylistParams,
        _results: med::CreatePlaylistResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("createPlaylist") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_to_playlist(
        self: Rc<Self>,
        _params: med::AddToPlaylistParams,
        _results: med::AddToPlaylistResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addToPlaylist") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn remove_from_playlist(
        self: Rc<Self>,
        _params: med::RemoveFromPlaylistParams,
        _results: med::RemoveFromPlaylistResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("removeFromPlaylist") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn reorder_playlist(
        self: Rc<Self>,
        _params: med::ReorderPlaylistParams,
        _results: med::ReorderPlaylistResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("reorderPlaylist") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_recommendations_with_scores(
        self: Rc<Self>,
        _params: med::GetRecommendationsWithScoresParams,
        _results: med::GetRecommendationsWithScoresResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getRecommendationsWithScores") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_share_preview(
        self: Rc<Self>,
        _params: med::GetSharePreviewParams,
        _results: med::GetSharePreviewResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getSharePreview") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_download_status(
        self: Rc<Self>,
        _params: med::GetDownloadStatusParams,
        _results: med::GetDownloadStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getDownloadStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_downloads(
        self: Rc<Self>,
        _params: med::ListDownloadsParams,
        _results: med::ListDownloadsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listDownloads") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn pause_download(
        self: Rc<Self>,
        _params: med::PauseDownloadParams,
        _results: med::PauseDownloadResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("pauseDownload") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn resume_download(
        self: Rc<Self>,
        _params: med::ResumeDownloadParams,
        _results: med::ResumeDownloadResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("resumeDownload") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn cancel_download(
        self: Rc<Self>,
        _params: med::CancelDownloadParams,
        _results: med::CancelDownloadResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("cancelDownload") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_download(
        self: Rc<Self>,
        _params: med::SubscribeDownloadParams,
        _results: med::SubscribeDownloadResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeDownload") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_crossfade_config(
        self: Rc<Self>,
        _params: med::SetCrossfadeConfigParams,
        _results: med::SetCrossfadeConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setCrossfadeConfig") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_crossfade_config(
        self: Rc<Self>,
        _params: med::GetCrossfadeConfigParams,
        _results: med::GetCrossfadeConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getCrossfadeConfig") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn send_live_interaction(
        self: Rc<Self>,
        _params: med::SendLiveInteractionParams,
        _results: med::SendLiveInteractionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("sendLiveInteraction") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_live_interactions(
        self: Rc<Self>,
        _params: med::SubscribeLiveInteractionsParams,
        _results: med::SubscribeLiveInteractionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeLiveInteractions") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_transcode(
        self: Rc<Self>,
        _params: med::SubscribeTranscodeParams,
        _results: med::SubscribeTranscodeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeTranscode") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
}

pub struct MediaDomain<B> {
    backend: Arc<B>,
}

impl<B> MediaDomain<B>
where
    B: MediaBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for MediaDomain<B>
where
    B: MediaBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        med::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::MEDIA_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "media"
    }

    fn category(&self) -> &'static str {
        "media"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "media players",
                "Media",
                "listPlayers",
                "listPlayers",
                0,
                true,
            )
            .with_row(matrix_row("listPlayers")),            DomainCommand::new(
                "media player",
                "Media",
                "getPlayer",
                "getPlayer",
                1,
                true,
            )
            .with_row(matrix_row("getPlayer")),            DomainCommand::new(
                "media play",
                "Media",
                "play",
                "play",
                2,
                true,
            )
            .with_row(matrix_row("play")),            DomainCommand::new(
                "media pause",
                "Media",
                "pause",
                "pause",
                3,
                true,
            )
            .with_row(matrix_row("pause")),            DomainCommand::new(
                "media stop",
                "Media",
                "stop",
                "stop",
                4,
                true,
            )
            .with_row(matrix_row("stop")),            DomainCommand::new(
                "media next",
                "Media",
                "next",
                "next",
                5,
                true,
            )
            .with_row(matrix_row("next")),            DomainCommand::new(
                "media previous",
                "Media",
                "previous",
                "previous",
                6,
                true,
            )
            .with_row(matrix_row("previous")),            DomainCommand::new(
                "media seek",
                "Media",
                "seek",
                "seek",
                7,
                true,
            )
            .with_row(matrix_row("seek")),            DomainCommand::new(
                "media metadata",
                "Media",
                "getMetadata",
                "getMetadata",
                8,
                true,
            )
            .with_row(matrix_row("getMetadata")),            DomainCommand::new(
                "media position",
                "Media",
                "getPosition",
                "getPosition",
                9,
                true,
            )
            .with_row(matrix_row("getPosition")),            DomainCommand::new(
                "media volume",
                "Media",
                "setVolume",
                "setVolume",
                10,
                true,
            )
            .with_row(matrix_row("setVolume")),            DomainCommand::new(
                "media set-position",
                "Media",
                "setPosition",
                "setPosition",
                11,
                true,
            )
            .with_row(matrix_row("setPosition")),            DomainCommand::new(
                "media open-uri",
                "Media",
                "openUri",
                "openUri",
                12,
                true,
            )
            .with_row(matrix_row("openUri")),            DomainCommand::new(
                "media subscribe-player",
                "Media",
                "subscribePlayer",
                "subscribePlayer",
                13,
                true,
            )
            .with_row(matrix_row("subscribePlayer")),            DomainCommand::new(
                "media formats",
                "Media",
                "getSupportedFormats",
                "getSupportedFormats",
                14,
                true,
            )
            .with_row(matrix_row("getSupportedFormats")),            DomainCommand::new(
                "media transcode",
                "Media",
                "transcode",
                "transcode",
                15,
                true,
            )
            .with_row(matrix_row("transcode")),            DomainCommand::new(
                "media get-recommendations",
                "Media",
                "getRecommendations",
                "getRecommendations",
                16,
                true,
            )
            .with_row(matrix_row("getRecommendations")),            DomainCommand::new(
                "media get-analytics",
                "Media",
                "getAnalytics",
                "getAnalytics",
                17,
                true,
            )
            .with_row(matrix_row("getAnalytics")),            DomainCommand::new(
                "media share-media",
                "Media",
                "shareMedia",
                "shareMedia",
                18,
                true,
            )
            .with_row(matrix_row("shareMedia")),            DomainCommand::new(
                "media download-media",
                "Media",
                "downloadMedia",
                "downloadMedia",
                19,
                true,
            )
            .with_row(matrix_row("downloadMedia")),            DomainCommand::new(
                "media set-crossfade",
                "Media",
                "setCrossfade",
                "setCrossfade",
                20,
                true,
            )
            .with_row(matrix_row("setCrossfade")),            DomainCommand::new(
                "media set-sync-group",
                "Media",
                "setSyncGroup",
                "setSyncGroup",
                21,
                true,
            )
            .with_row(matrix_row("setSyncGroup")),            DomainCommand::new(
                "media create-playlist",
                "Media",
                "createPlaylist",
                "createPlaylist",
                22,
                true,
            )
            .with_row(matrix_row("createPlaylist")),            DomainCommand::new(
                "media add-to-playlist",
                "Media",
                "addToPlaylist",
                "addToPlaylist",
                23,
                true,
            )
            .with_row(matrix_row("addToPlaylist")),            DomainCommand::new(
                "media remove-from-playlist",
                "Media",
                "removeFromPlaylist",
                "removeFromPlaylist",
                24,
                true,
            )
            .with_row(matrix_row("removeFromPlaylist")),            DomainCommand::new(
                "media reorder-playlist",
                "Media",
                "reorderPlaylist",
                "reorderPlaylist",
                25,
                true,
            )
            .with_row(matrix_row("reorderPlaylist")),            DomainCommand::new(
                "media get-recommendations-with-scores",
                "Media",
                "getRecommendationsWithScores",
                "getRecommendationsWithScores",
                26,
                true,
            )
            .with_row(matrix_row("getRecommendationsWithScores")),            DomainCommand::new(
                "media get-share-preview",
                "Media",
                "getSharePreview",
                "getSharePreview",
                27,
                true,
            )
            .with_row(matrix_row("getSharePreview")),            DomainCommand::new(
                "media get-download-status",
                "Media",
                "getDownloadStatus",
                "getDownloadStatus",
                28,
                true,
            )
            .with_row(matrix_row("getDownloadStatus")),            DomainCommand::new(
                "media list-downloads",
                "Media",
                "listDownloads",
                "listDownloads",
                29,
                true,
            )
            .with_row(matrix_row("listDownloads")),            DomainCommand::new(
                "media pause-download",
                "Media",
                "pauseDownload",
                "pauseDownload",
                30,
                true,
            )
            .with_row(matrix_row("pauseDownload")),            DomainCommand::new(
                "media resume-download",
                "Media",
                "resumeDownload",
                "resumeDownload",
                31,
                true,
            )
            .with_row(matrix_row("resumeDownload")),            DomainCommand::new(
                "media cancel-download",
                "Media",
                "cancelDownload",
                "cancelDownload",
                32,
                true,
            )
            .with_row(matrix_row("cancelDownload")),            DomainCommand::new(
                "media subscribe-download",
                "Media",
                "subscribeDownload",
                "subscribeDownload",
                33,
                true,
            )
            .with_row(matrix_row("subscribeDownload")),            DomainCommand::new(
                "media set-crossfade-config",
                "Media",
                "setCrossfadeConfig",
                "setCrossfadeConfig",
                34,
                true,
            )
            .with_row(matrix_row("setCrossfadeConfig")),            DomainCommand::new(
                "media get-crossfade-config",
                "Media",
                "getCrossfadeConfig",
                "getCrossfadeConfig",
                35,
                true,
            )
            .with_row(matrix_row("getCrossfadeConfig")),            DomainCommand::new(
                "media send-live-interaction",
                "Media",
                "sendLiveInteraction",
                "sendLiveInteraction",
                36,
                true,
            )
            .with_row(matrix_row("sendLiveInteraction")),            DomainCommand::new(
                "media subscribe-live-interactions",
                "Media",
                "subscribeLiveInteractions",
                "subscribeLiveInteractions",
                37,
                true,
            )
            .with_row(matrix_row("subscribeLiveInteractions")),            DomainCommand::new(
                "media subscribe-transcode",
                "Media",
                "subscribeTranscode",
                "subscribeTranscode",
                38,
                true,
            )
            .with_row(matrix_row("subscribeTranscode")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/media.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = MediaService::new(Arc::clone(&self.backend), class);
        let client: med::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
