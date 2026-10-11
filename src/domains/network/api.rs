//! The public network API: a thin, borrowing wrapper over [`NetworkBackend`]
//! exposing the handful of calls a client actually touches, plus one combined
//! status view. Borrowed and zero-cost — it adds no state over `&B`.

use crate::core::Result;
use crate::core::ids::NetworkInterfaceId;

use super::backend::NetworkBackend;
use super::types::{ConnStatus, InterfaceSnapshot, Page, Pagination, RadioState, SubscriptionId};

/// The combined status view the CLI's `network status` renders: host-networking
/// on/off plus the observable radio set. One read, both facts, no extra
/// concept to learn.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkSummary {
    /// Host-global networking master switch (`NetworkingEnabled`).
    pub networking_enabled: bool,
    /// Host-global networking disabled ⇒ every device is `networking-disabled`.
    pub radios: Page<RadioState>,
}

/// A borrowing handle to any [`NetworkBackend`].
pub struct Network<'a, B: NetworkBackend + ?Sized>(&'a B);

impl<'a, B: NetworkBackend + ?Sized> Network<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Network(backend)
    }

    /// The combined host-networking + radio status.
    #[must_use = "the status is the answer; ignoring it is never useful"]
    pub fn summary(&self) -> Result<NetworkSummary> {
        let networking_enabled = self.0.is_networking_enabled()?;
        let radios = self.0.list_radio_states(Pagination { offset: 0, limit: u32::MAX })?;
        Ok(NetworkSummary {
            networking_enabled,
            radios,
        })
    }

    /// Is host-global networking enabled?
    #[must_use]
    pub fn networking_enabled(&self) -> Result<bool> {
        self.0.is_networking_enabled()
    }

    /// Per-interface status, including the `hostState` diagnostic token.
    #[must_use]
    pub fn status(&self, interface: NetworkInterfaceId) -> Result<ConnStatus> {
        self.0.get_status(interface)
    }

    /// The first page of enumerated interfaces.
    #[must_use]
    pub fn interfaces(&self, page: Pagination) -> Result<Page<InterfaceSnapshot>> {
        self.0.get_interfaces(page)
    }

    /// Subscribe to status changes and pull the current status.
    #[must_use]
    pub fn status_stream(&self, interface: NetworkInterfaceId) -> Result<(SubscriptionId, ConnStatus)> {
        let sub = self.0.subscribe_status(interface)?;
        let first = self.0.pull_status(sub)?;
        Ok((sub, first))
    }

    /// Retire a subscription obtained from [`Network::status_stream`].
    pub fn close_status_stream(&self, sub: SubscriptionId) -> Result<()> {
        self.0.cancel_subscription(sub)
    }
}
