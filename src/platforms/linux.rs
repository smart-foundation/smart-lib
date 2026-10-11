//! The Linux platform layer. Each domain cell lives at
//! `crate::platforms::linux::<domain>`; this file declares them.
//!
//! (Charter note: the per-OS aggregator file is unassigned in the ownership
//! table; the network domain agent created it because it is the first Linux
//! (os, domain) cell and the module tree needs it to wire the cell. A later
//! Linux cell agent adds its own `#[cfg(feature = …)] pub mod <domain>;`.)

// The network backend is only compiled when its feature is on, so the platform
// layer stays green in every feature configuration (charter law #2). The
// explicit `#[path]` is required because build.rs loads this file with its own
// `#[path]`, which changes how the compiler resolves sibling submodules.
#[cfg(feature = "domain-network")]
#[path = "linux/network.rs"]
pub mod network;

// The kernel-first read seam for the network domain (law #9): netlink behind a
// mockable `NetlinkReader`, feeding the backend's state store.
#[cfg(feature = "domain-network")]
#[path = "linux/netlink.rs"]
pub mod netlink;
