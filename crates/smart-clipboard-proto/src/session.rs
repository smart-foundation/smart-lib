//! The connection: dial a carrier, take the bootstrap capability, and ask
//! discovery who serves the clipboard.
//!
//! There is deliberately no "connection object" abstraction here. The contract
//! gives us exactly three handles on one bootstrap — `discovery`,
//! `capability_service` and `clipboard` — and holding them is the whole state.
//! A client that wrapped that in a layer of its own would be inventing a
//! vocabulary, which is the thing this repository refuses to do.
//!
//! ## The handshake is a *check*, not a ceremony
//!
//! 1. **Connect** over the unix carrier (the only carrier this app speaks; see
//!    [`crate::config`] for why the others are honestly out of scope).
//! 2. **Ask discovery** for the service named `clipboard`. Three outcomes are
//!    distinguished, because they have three different fixes: nothing
//!    registered (the launcher is configured for the wrong socket or the
//!    service has no clipboard domain compiled in), registered under another
//!    name (a different generation of the engine), or registered and
//!    reachable.
//! 3. **Read the published schema id** and compare it with the one this
//!    binary compiled. A mismatch does not stop the app — an older app talking
//!    to a newer service is a legitimate configuration — but it is recorded in
//!    [`crate::model::ServiceInfo::schema_matches_app`] and shown in the
//!    inspector's provenance block.
//! 4. **Ask capability** what this connection may do. The answer drives the
//!    write affordances' *hints*; the service's own answer still drives the
//!    *behaviour*, because a hint is not an authority.
//!
//! ## Threading
//!
//! Everything here is single-threaded by construction: `capnp-rpc`'s client
//! futures are `!Send`. This module therefore never spawns, and the caller
//! (the worker's `LocalSet`) owns the runtime. That containment is what lets
//! the rest of the app be plain `std` code.

use std::time::Duration;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{twoparty, RpcSystem};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::config::Candidate;
use crate::contract;
use crate::error::ClientError;
use crate::model::{Authority, CapabilityRow, ServiceInfo};
use crate::schema::capability_capnp::capability_service;
use crate::schema::discovery_capnp::discovery;
use crate::schema::discovery_capnp::ServiceState;

/// How long the app waits for a socket to accept before moving to the next
/// candidate. A live local socket accepts in microseconds; anything near this
/// is a wedged listener, and a wedged listener should not cost the user the
/// rest of the candidate list.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// A live client session over one carrier.
pub struct Session {
    /// The peer's bootstrap capability, kept so the typed clients can be
    /// rebuilt without reconnecting (and so a future domain can be reached
    /// by the same connection).
    bootstrap: Box<dyn ClientHook>,
    /// `discovery.capnp`.
    pub discovery: discovery::Client,
    /// `capability.capnp`.
    pub capability: capability_service::Client,
    /// `clipboard.capnp`.
    pub clipboard: crate::schema::clipboard_capnp::clipboard::Client,
    /// What discovery said about the service.
    pub service: ServiceInfo,
    /// What capability said about this connection.
    pub authority: Authority,
    /// The endpoint this session is attached to, `unix://…`.
    pub endpoint: String,
    /// The RPC pump. Aborted when the session is dropped: a connection that
    /// nobody is holding handles to has nothing left to do.
    pump: tokio::task::JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

impl Session {
    /// Dial one candidate and complete the handshake.
    pub async fn connect(candidate: &Candidate) -> Result<Self, ClientError> {
        if candidate.scheme != "unix" {
            return Err(ClientError::Transport(format!(
                "this app speaks the unix carrier only; {}://{} names another scheme",
                candidate.scheme, candidate.address
            )));
        }

        let stream = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::UnixStream::connect(&candidate.address),
        )
        .await
        .map_err(|_| {
            ClientError::Transport(format!(
                "connecting to {} timed out after {} ms",
                candidate.address,
                CONNECT_TIMEOUT.as_millis()
            ))
        })?
        .map_err(|error| ClientError::Transport(format!("{}: {error}", candidate.address)))?;

        // `capnp-rpc` speaks `futures::io`; tokio speaks its own traits. The
        // compat adapters are the documented bridge, and they are why this
        // crate depends on `tokio-util`.
        let (reader, writer) = stream.into_split();
        let network = twoparty::VatNetwork::new(
            reader.compat(),
            writer.compat_write(),
            Side::Client,
            capnp::message::ReaderOptions::new(),
        );
        let mut rpc_system = RpcSystem::new(Box::new(network), None);
        let bootstrap: Box<dyn ClientHook> = rpc_system.bootstrap(Side::Server);
        let pump = tokio::task::spawn_local(async move {
            // The pump ends when the connection does. Its error is not the
            // caller's business here: every call that depends on it reports
            // its own disconnection, with the verb that hit it.
            let _ = rpc_system.await;
        });

        let discovery_client = <discovery::Client as FromClientHook>::new(bootstrap.clone());
        let capability_client =
            <capability_service::Client as FromClientHook>::new(bootstrap.clone());
        let clipboard_client =
            <crate::schema::clipboard_capnp::clipboard::Client as FromClientHook>::new(
                bootstrap.clone(),
            );

        let mut session = Session {
            bootstrap,
            discovery: discovery_client,
            capability: capability_client,
            clipboard: clipboard_client,
            service: ServiceInfo::default(),
            authority: Authority::default(),
            endpoint: format!("{}://{}", candidate.scheme, candidate.address),
            pump,
        };
        session.handshake().await?;
        Ok(session)
    }

    /// The bootstrap capability, for a caller that wants an interface this
    /// struct does not hold yet.
    pub fn bootstrap(&self) -> &dyn ClientHook {
        self.bootstrap.as_ref()
    }

    /// Steps 2–4 of the handshake.
    async fn handshake(&mut self) -> Result<(), ClientError> {
        let name = contract::SERVICE_NAME;

        let mut request = self.discovery.lookup_request();
        {
            let mut params = request.get();
            let mut filter = params.reborrow().init_filter();
            filter.set_name(name);
        }
        let response = request.send().promise.await.map_err(|error| {
            ClientError::Transport(format!("discovery.lookup: {}", error.extra))
        })?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("discovery.lookup results: {error}")))?;
        let services = results
            .get_services()
            .map_err(|error| ClientError::Decode(format!("discovery.lookup list: {error}")))?;

        if services.is_empty() {
            // Ask what *is* published, so the answer can be "you are talking
            // to a service that serves files and network, not clipboard"
            // instead of "nothing found".
            let published = self.published_names().await;
            return Err(ClientError::DomainNotServed { published });
        }

        let descriptor = services.get(0);
        let schema_id = descriptor
            .get_id()
            .map(|id| id.get_id())
            .unwrap_or_default();

        let mut endpoints = Vec::new();
        if let Ok(list) = descriptor.get_endpoints() {
            for endpoint in list.iter() {
                let transport = text_of(endpoint.get_transport());
                let address = text_of(endpoint.get_address());
                endpoints.push(format!("{transport}://{address}"));
            }
        }

        let mut metadata = std::collections::BTreeMap::new();
        if let Ok(map) = descriptor.get_metadata() {
            for entry in map.get_entries().into_iter().flatten() {
                metadata.insert(text_of(entry.get_key()), text_of(entry.get_value()));
            }
        }

        self.service = ServiceInfo {
            name: text_of(descriptor.get_name()),
            category: text_of(descriptor.get_category()),
            schema_id,
            state: state_token(descriptor.get_state().unwrap_or(ServiceState::Unknown)),
            endpoints,
            metadata,
            schema_matches_app: schema_id == contract::EXPECTED_CLIPBOARD_SCHEMA_ID,
            methods: ServiceInfo::new_matrix(),
        };

        self.authority = self.read_authority().await;
        Ok(())
    }

    /// The names discovery publishes, used to explain a miss. Failure here is
    /// not fatal: the caller's error already says the domain is not served,
    /// and an empty list is rendered as "nothing".
    async fn published_names(&self) -> Vec<String> {
        let mut request = self.discovery.lookup_request();
        {
            let mut params = request.get();
            let mut filter = params.reborrow().init_filter();
            filter.set_name("");
            filter.set_category("");
        }
        let Ok(response) = request.send().promise.await else {
            return Vec::new();
        };
        let Ok(results) = response.get() else {
            return Vec::new();
        };
        let Ok(services) = results.get_services() else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for descriptor in services.iter() {
            names.push(text_of(descriptor.get_name()));
        }
        names
    }

    /// What may this connection do? A capability read that fails is an
    /// *unpublished* authority, not an error: the app still works, the write
    /// affordances just say "unpublished" instead of "allowed" or "denied".
    async fn read_authority(&self) -> Authority {
        let request = self.capability.list_request();
        let Ok(response) = request.send().promise.await else {
            return Authority::default();
        };
        let Ok(results) = response.get() else {
            return Authority::default();
        };
        let Ok(caps) = results.get_caps() else {
            return Authority::default();
        };

        let mut rows = Vec::new();
        for capability in caps.iter() {
            let id = id_text(
                capability
                    .get_id()
                    .map(|data| data.to_vec())
                    .unwrap_or_default(),
            );
            let target = text_of(capability.get_target());
            let mut operations = Vec::new();
            if let Ok(list) = capability.get_operations() {
                for operation in list.iter() {
                    // An ordinal this build does not know is still a fact worth
                    // showing: it means the service serves a newer contract.
                    operations.push(match operation {
                        Ok(operation) => operation_token(operation),
                        Err(_) => "unrecognised".to_string(),
                    });
                }
            }
            let issuer = capability
                .get_audit()
                .map(|audit| text_of(audit.get_issuer()))
                .unwrap_or_default();
            rows.push(CapabilityRow {
                id,
                target,
                operations,
                issuer,
            });
        }

        let mut operations: Vec<String> = Vec::new();
        for row in &rows {
            for operation in &row.operations {
                if !operations.contains(operation) {
                    operations.push(operation.clone());
                }
            }
        }
        let class_id = rows
            .iter()
            .map(|row| row.id.clone())
            .find(|id| id.starts_with("smart-lib:"));

        Authority {
            rows,
            operations,
            class_id,
        }
    }

    /// A helper for the worker: ask discovery about *any* service, so the
    /// connection panel can list everything the host serves, not just the one
    /// this app needs.
    pub async fn all_services(&self) -> Vec<ServiceInfo> {
        let mut request = self.discovery.lookup_request();
        {
            let mut params = request.get();
            let mut filter = params.reborrow().init_filter();
            filter.set_name("");
            filter.set_category("");
        }
        let Ok(response) = request.send().promise.await else {
            return Vec::new();
        };
        let Ok(results) = response.get() else {
            return Vec::new();
        };
        let Ok(services) = results.get_services() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for descriptor in services.iter() {
            out.push(ServiceInfo {
                name: text_of(descriptor.get_name()),
                category: text_of(descriptor.get_category()),
                schema_id: descriptor
                    .get_id()
                    .map(|id| id.get_id())
                    .unwrap_or_default(),
                state: state_token(descriptor.get_state().unwrap_or(ServiceState::Unknown)),
                endpoints: Vec::new(),
                metadata: Default::default(),
                schema_matches_app: false,
                methods: Vec::new(),
            });
        }
        out
    }
}

/// Read a contract `Text` losslessly enough for a UI: invalid UTF-8 is
/// replaced rather than dropped, and the replacement is visible.
pub fn text_of(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => String::from_utf8_lossy(text.as_bytes()).into_owned(),
        Err(_) => String::new(),
    }
}

/// Render a capability's opaque id as text: ASCII is shown as-is (the engine
/// publishes `smart-lib:control`), anything else is hexed so it is still
/// distinguishable and copyable.
fn id_text(bytes: Vec<u8>) -> String {
    if bytes.iter().all(|byte| byte.is_ascii_graphic()) {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

/// The capability operation token, lower-cased so the UI's comparisons are
/// against one spelling.
fn operation_token(operation: crate::schema::capability_capnp::Operation) -> String {
    use crate::schema::capability_capnp::Operation;
    match operation {
        Operation::Read => "read",
        Operation::Write => "write",
        Operation::Execute => "execute",
        Operation::Subscribe => "subscribe",
        Operation::Publish => "publish",
        Operation::Create => "create",
        Operation::Destroy => "destroy",
        Operation::Attenuate => "attenuate",
        Operation::Transfer => "transfer",
        Operation::Revoke => "revoke",
        Operation::Custom => "custom",
    }
    .to_string()
}

/// The service-state token, as discovery spells it.
fn state_token(state: crate::schema::discovery_capnp::ServiceState) -> String {
    use crate::schema::discovery_capnp::ServiceState;
    match state {
        ServiceState::Unknown => "unknown",
        ServiceState::Online => "online",
        ServiceState::Offline => "offline",
        ServiceState::Degraded => "degraded",
        ServiceState::Starting => "starting",
        ServiceState::Maintenance => "maintenance",
        ServiceState::Custom => "custom",
    }
    .to_string()
}
