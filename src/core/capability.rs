//! Law #5: **authority is a handle**.
//!
//! No permission string appears anywhere in this crate. A caller holds a
//! [`Cap<T>`] for the interface `T` it was granted, and *holding it is the
//! authority to call it* — the Cap'n Proto capability model the contract is
//! built on (`schema/capability.capnp:20-27`: "a capability IS a reference:
//! holding it conveys the authority to call it"). Host credentials (uid, caps,
//! entitlements) are mapped into handles **once, at acquisition**, by the
//! platform backend implementing [`Acquire`]; they never appear as parameters,
//! names, or paths on any call after that.
//!
//! ```text
//! host credential ──acquire()──▶ Cap<Files::Client> ──calls──▶ the granted surface
//!   (uid, caps, …)   (platform      (the handle; no          and nothing else
//!                     backend)       permission inside)
//! ```
//!
//! ## Revocation — how it reaches a held `Cap<T>`
//!
//! There is **no cached validity bit** anywhere in this type, so there is
//! nothing to go stale: revocation is discovered *at the next call*. The
//! capability system routes a call on a revoked capability to an error — in the
//! capnp runtime's vocabulary `capnp::ErrorKind::Disconnected` — and the holder
//! must re-acquire (or fail honestly with [`crate::core::Error::permission_denied`]).
//! The contract's *push* channel is `CapabilityService.subscribe` →
//! `CapabilitySubscription` → `CapabilityEvent.revocationNotice`
//! (`capability.capnp:313-348`): a backend holding long-lived handles should
//! subscribe and drop them on notice; either way, **the call is the check**.
//! `src/core/tests.rs::cap_revocation_reaches_the_holder_at_the_next_call`
//! proves the mechanism with a mock capability.
//!
//! A backend may keep a *bounded* revocation cache, but its staleness must be
//! declared (`knowledge/02-HOST-CAPABILITIES.md` §3.1) — a stale "still valid"
//! is a fabricated authority, which is the unforgivable defect.

use crate::core::error::Result;
use crate::schema::capability_capnp;

/// The bound every capability client must satisfy to be wrapped in a [`Cap`]:
/// it can be built from a client hook, cloned cheaply (a capability is
/// ref-counted, so duplicating a handle is normal), and owns no borrowed data.
///
/// Implemented for free by capnp codegen for every interface client
/// (`files_capnp::files::Client`, `network_capnp::network::Client`, …) and for
/// the universal [`capnp::capability::Client`].
pub trait Interface: capnp::capability::FromClientHook + Clone + 'static {}

impl<T: capnp::capability::FromClientHook + Clone + 'static> Interface for T {}

/// A held capability for the interface `T`.
///
/// Zero-cost: one word of wrapping around the generated client (which is itself
/// one `Box<dyn ClientHook>`). The wrapper adds the acquisition contract and
/// the revocation semantics as *documentation and types*, never as state:
/// `Cap<T>` is exactly as large and as fast as `T` (it derefs to `T`, so every
/// generated method is reachable directly).
///
/// # Authority invariants (law #5)
///
/// 1. Holding a `Cap<T>` IS the authority to call everything `T` exposes —
///     there is no second check, and no permission string to present.
/// 2. Authority is granted once, at acquisition (see [`Acquire`]); the handle
///     carries no host identity and cannot be re-scoped by mutating it.
/// 3. Attenuation (dropping operations) and revocation are the capability
///     service's operations (`capability.capnp`), never a client-side edit:
///     derive a weaker handle with `attenuate` + `transfer`, never by copying
///     fields around.
/// 4. On a transport that cannot carry capabilities (Tcp), a `Cap<T>` can only
///     be minted as an attenuated, time-boxed in-band token or refused —
///     `knowledge/02-HOST-CAPABILITIES.md` §4.2. Core makes no promise here;
///     the platform backend declares what it can do.
#[must_use = "holding a Cap is the authority; dropping it ends the authority"]
pub struct Cap<T: Interface>(T);

impl<T: Interface> Cap<T> {
    /// Wrap a client the backend already holds authority for.
    ///
    /// This is the *hand-over* point: whoever calls `new` must have obtained
    /// the client through the acquisition path (see [`Acquire`]) — a `Cap` built
    /// from a client the host never granted is a fabricated authority.
    pub fn new(client: T) -> Self {
        Cap(client)
    }

    /// The wrapped client, for direct method calls (also reachable via `Deref`).
    pub fn client(&self) -> &T {
        &self.0
    }

    /// Unwrap, handing the raw client back (the authority moves with it).
    pub fn into_client(self) -> T {
        self.0
    }

    /// Resolve the underlying capability promise.
    ///
    /// Mostly useful for error-checking when no call is being made: if the
    /// capability was revoked or never resolved, this fails — the same
    /// information a call would surface. Capnp documents that there is no need
    /// to wait for this before making calls.
    pub fn when_resolved(&self) -> capnp::capability::Promise<(), capnp::Error> {
        self.0.as_client_hook().when_resolved()
    }
}

impl<T: Interface> core::ops::Deref for Cap<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Interface> Clone for Cap<T> {
    /// Cloning a handle duplicates the authority — that is the capability
    /// model (`Constraints.shareable` defaults to true), and it is cheap: one
    /// refcount bump on the client hook.
    fn clone(&self) -> Self {
        Cap(self.0.clone())
    }
}

impl<T: Interface> From<T> for Cap<T> {
    fn from(client: T) -> Self {
        Cap::new(client)
    }
}

impl<T: Interface> core::fmt::Debug for Cap<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // A capability is opaque by design: its Debug says *that* it is a
        // capability handle and nothing else — never a permission, target or
        // host identity.
        f.write_str("Cap(<opaque capability handle>)")
    }
}

/// The acquisition-time mapping every platform backend implements — the one
/// place OS identity enters the library (law #5, `02-HOST-CAPABILITIES.md` §3).
///
/// The signature is deliberately host-agnostic: core names no platform noun
/// (charter product standard: *agnostic*). The peer identity the mapping needs
/// — `SO_PEERCRED` on UnixSocket, a shmem pairing token, nothing at all on
/// InProcess, none available on Tcp — is the *platform module's own session
/// state*, captured once per session, not a parameter.
///
/// # The contract an implementation must honour
///
/// 1. Match the **presented** capability (`id`, `target`, `operations`,
///    `constraints`, `expiry`, `chain`) against the requested target and
///    against the peer's host authority snapshot.
/// 2. On a match, mint or return the bound handle: the handle IS the authority
///    from then on.
/// 3. When a constraint cannot be *evaluated* (not merely failed), return an
///    `Error` whose code is **not** `permissionDenied` — the capability
///    service's own `verify` contract (`capability.capnp:193-196`) forbids
///    answering "denied" for two different reasons.
/// 4. On refusal, return [`crate::core::Error::permission_denied`] with the
///    reason — never a handle that will fail on first use, and never a
///    fabricated success.
/// 5. Never mint a handle the host did not grant, never derive a handle id from
///    a uid, and never claim an authority set that was not measured.
///
/// The contract's acquisition points are exactly three, plus the host-injected
/// roots: `Files.acquireFile @73`, `Network.nicProvider @71`,
/// `Audio.openDriver @99`, and the `Files` / `Network` / `Audio` / `System` /
/// `CapabilityService` root references themselves.
pub trait Acquire<T: Interface> {
    /// Mint — or refuse — the capability handle for `target` on the strength of
    /// `presented`.
    fn acquire(
        &self,
        presented: &capability_capnp::capability::Reader<'_>,
        target: &str,
    ) -> Result<Cap<T>>;
}
