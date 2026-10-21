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
  /// A method this platform serves with a stated limit and NO automated proof:
  /// the verb is host-mutating, so a test that exercised it would change the
  /// host (charter law #8). The row says so instead of naming a test that
  /// proves nothing.
  const fn degraded_unproven(name: &'static str, ordinal: u16, detail: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Degraded, reason_class: None, reason: detail, l3_test: None } }
  const fn unavailable(name: &'static str, ordinal: u16, class: ReasonClass, reason: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Unavailable, reason_class: Some(class), reason, l3_test: None } }
}

pub const MEDIA_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("listPlayers", 0, "l3_linux_media::list_players_answers_with_the_hosts_own_players"),
    MethodAvailability::real("getPlayer", 1, "l3_linux_media::get_player_reads_one_entry_natively"),
    MethodAvailability::degraded_unproven("play", 2, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("pause", 3, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("stop", 4, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("next", 5, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("previous", 6, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("seek", 7, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::real("getMetadata", 8, "l3_linux_media::get_metadata_reads_the_players_own_metadata"),
    MethodAvailability::real("getPosition", 9, "l3_linux_media::get_position_reads_the_players_own_position"),
    MethodAvailability::degraded_unproven("setVolume", 10, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("setPosition", 11, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::degraded_unproven("openUri", 12, "the MPRIS write path is host-mutating: a live proof would move the founder's own player (law #8), so no automated test exercises this verb; the founder runs it against the real bus"),
    MethodAvailability::real("subscribePlayer", 13, "l3_linux_media::subscribe_player_mints_a_subscription"),
    MethodAvailability::real("getSupportedFormats", 14, "l3_linux_media::get_supported_formats_answers_with_the_hosts_own_list"),
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
