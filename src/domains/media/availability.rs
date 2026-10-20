//! media availability matrix.

pub const MEDIA_SCHEMA_ID: u64 = 11651590505119481873;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass { NoBackend, NoPrivilege, NotApplicable, DisabledByPolicy, Unknown }
impl ReasonClass { pub const fn as_str(self) -> &'static str { match self { ReasonClass::NoBackend => "no-backend", ReasonClass::NoPrivilege => "no-privilege", ReasonClass::NotApplicable => "not-applicable", ReasonClass::DisabledByPolicy => "disabled-by-policy", ReasonClass::Unknown => "unknown" } } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus { Real, Degraded, Unavailable }

#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability { pub name: &'static str, pub ordinal: u16, pub status: AvailStatus, pub reason_class: Option<ReasonClass>, pub reason: &'static str, pub l3_test: Option<&'static str> }
impl MethodAvailability {
  const fn real(name: &'static str, ordinal: u16, l3: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Real, reason_class: None, reason: "", l3_test: Some(l3) } }
  const fn degraded(name: &'static str, ordinal: u16, l3: &'static str, detail: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Degraded, reason_class: None, reason: detail, l3_test: Some(l3) } }
  const fn unavailable(name: &'static str, ordinal: u16, class: ReasonClass, reason: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Unavailable, reason_class: Some(class), reason, l3_test: None } }
}

pub const MEDIA_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("listPlayers", 0, "l3_linux_media::core_works"),
    MethodAvailability::real("getPlayer", 1, "l3_linux_media::core_works"),
    MethodAvailability::real("play", 2, "l3_linux_media::core_works"),
    MethodAvailability::real("pause", 3, "l3_linux_media::core_works"),
    MethodAvailability::real("stop", 4, "l3_linux_media::core_works"),
    MethodAvailability::real("next", 5, "l3_linux_media::core_works"),
    MethodAvailability::real("previous", 6, "l3_linux_media::core_works"),
    MethodAvailability::real("seek", 7, "l3_linux_media::core_works"),
    MethodAvailability::real("getMetadata", 8, "l3_linux_media::core_works"),
    MethodAvailability::real("getPosition", 9, "l3_linux_media::core_works"),
    MethodAvailability::real("setVolume", 10, "l3_linux_media::core_works"),
    MethodAvailability::real("setPosition", 11, "l3_linux_media::core_works"),
    MethodAvailability::real("openUri", 12, "l3_linux_media::core_works"),
    MethodAvailability::real("subscribePlayer", 13, "l3_linux_media::core_works"),
    MethodAvailability::real("getSupportedFormats", 14, "l3_linux_media::core_works"),
    MethodAvailability::unavailable("transcode", 15, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("getRecommendations", 16, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("getAnalytics", 17, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("shareMedia", 18, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("downloadMedia", 19, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("setCrossfade", 20, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("setSyncGroup", 21, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("createPlaylist", 22, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("addToPlaylist", 23, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("removeFromPlaylist", 24, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("reorderPlaylist", 25, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("getRecommendationsWithScores", 26, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("getSharePreview", 27, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("getDownloadStatus", 28, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("listDownloads", 29, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("pauseDownload", 30, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("resumeDownload", 31, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("cancelDownload", 32, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("subscribeDownload", 33, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("setCrossfadeConfig", 34, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("getCrossfadeConfig", 35, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("sendLiveInteraction", 36, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("subscribeLiveInteractions", 37, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
    MethodAvailability::unavailable("subscribeTranscode", 38, ReasonClass::NoBackend, "no native MPRIS support for this surface on this host"),
];

pub const PLATFORM: &str = "linux";
pub const TRANSPORTS: &[&str] = &["InProcess"];
pub const CAPS: &[&str] = &[];

pub fn status_counts() -> (usize, usize, usize) {
    let mut r=0; let mut d=0; let mut u=0; for row in MEDIA_AVAILABILITY { match row.status { AvailStatus::Real=>r+=1, AvailStatus::Degraded=>d+=1, AvailStatus::Unavailable=>u+=1 } } (r,d,u)
}

pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    MEDIA_AVAILABILITY.iter().filter(|r| r.l3_test.is_some()).map(|r| (r.name, r.l3_test.unwrap()))
}

pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    MEDIA_AVAILABILITY.iter().find(|r| r.name == name)
}
