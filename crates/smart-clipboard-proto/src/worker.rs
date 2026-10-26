//! The actor that owns the wire.
//!
//! ## Why a thread and not a future in the UI
//!
//! `capnp-rpc`'s client futures are `!Send` (they hold `Rc` tables and local
//! vats), and a GPU-driven UI must never await. Both facts point at the same
//! answer: **one thread owns one session**, commands go in through a channel,
//! events come out through another, and the UI thread never sees a future, a
//! reader, or a client handle. The seam is two enums, which is also the seam
//! that makes the whole UI testable with a fake worker.
//!
//! ```text
//!   UI thread ──Command──▶ [ worker thread: tokio current_thread + LocalSet ]
//!      ▲                          │  Session, clipboard client, subscription
//!      └────BackendEvent──────────┘
//! ```
//!
//! ## The pull loop is the contract's model, not invented polling
//!
//! `subscribeChanges` mints a capability; `pull` reads a bounded batch from
//! it. There is no callback in the contract, so a consumer *must* pull. The
//! worker ticks at [`PULL_INTERVAL`] and asks for at most
//! [`PULL_BATCH`] events per tick, which is the contract's own backpressure
//! shape: a slow UI drops nothing (the service keeps its buffer) and a fast
//! one costs one round trip per second. When no subscription exists, the tick
//! is a no-op and the app says "no capture stream" rather than pretending to
//! watch the clipboard.
//!
//! ## Reconnection
//!
//! A `Disconnected` transport error is reported as itself, the session is
//! dropped, and the worker tries the candidate list once more after
//! [`RECONNECT_DELAY`]. A *refusal* never triggers a reconnect: the service
//! answered, so the connection is fine and the refusal is the point.

use std::collections::HashMap;
use std::time::Duration;

use crate::client::{
    ClipboardClient, CopyReport, CopyTarget, LiveContent, Page, Query, Representation,
};
use crate::config::ServiceLocation;
use crate::contract;
use crate::error::{ClientError, Refusal};
use crate::model::{Authority, ContentState, Item, ItemId, PreviewState, ServiceInfo};
use crate::session::Session;

/// How often the worker pulls the change subscription.
pub const PULL_INTERVAL: Duration = Duration::from_millis(1200);
/// How many events one pull may return.
pub const PULL_BATCH: u32 = 64;
/// How long the worker waits before one automatic reconnect after a drop.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(3);

/// A request from the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Find the service and complete the handshake, from the start of the
    /// candidate list.
    Connect,
    /// Fetch the first page for a query (replacing the current window).
    Refresh(Query),
    /// Fetch the next page of the current query and append it.
    LoadMore,
    /// Ask for previews of these items (the visible window plus the
    /// selection).
    Preview(Vec<ItemId>),
    /// Fetch the bytes of one representation, for a payload view that needs
    /// them (an image thumbnail, the inspector's full view).
    Content {
        /// The item.
        id: ItemId,
        /// The representation to ask for.
        mime: String,
    },
    /// Ask the service to hold this item's representation(s).
    Copy(CopyTarget),
    /// `pinItem` / `unpinItem`.
    SetPinned {
        /// The item.
        id: ItemId,
        /// The new state.
        pinned: bool,
    },
    /// `deleteItem`.
    Delete(ItemId),
    /// `clearHistory` — the whole history, behind the UI's confirm.
    ClearHistory,
    /// `getContent`, for the "clipboard now" strip.
    ReadLive {
        /// The MIME to ask for.
        mime: String,
    },
    /// Re-read discovery and the capability list, and re-emit the matrix.
    Describe,
    /// Stop the worker.
    Shutdown,
}

/// Something the worker observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    /// One candidate is about to be dialled.
    Dialling {
        /// The endpoint, `unix://…`.
        endpoint: String,
        /// Where the candidate came from (`--socket`, env, …).
        source: &'static str,
    },
    /// A candidate failed. The user sees the list grow as each one is tried.
    DialFailed {
        /// The endpoint.
        endpoint: String,
        /// Why, verbatim.
        reason: String,
    },
    /// The handshake completed.
    Connected {
        /// The endpoint the session is on.
        endpoint: String,
        /// What discovery said.
        service: ServiceInfo,
        /// What capability said.
        authority: Authority,
        /// Everything the host publishes — the "what else is here" list.
        published: Vec<String>,
    },
    /// No candidate answered.
    ConnectFailed {
        /// Every candidate that was tried, with its reason.
        attempts: Vec<(String, String)>,
        /// The last reason, for the banner's headline.
        reason: String,
    },
    /// The connection died.
    Disconnected {
        /// Why.
        reason: String,
        /// Whether the worker will try again on its own.
        retrying: bool,
    },
    /// A page of history arrived.
    Page {
        /// The page.
        page: Page,
        /// Whether it replaces the window (`Refresh`) or extends it
        /// (`LoadMore`).
        append: bool,
    },
    /// A preview arrived.
    Preview {
        /// The item.
        id: ItemId,
        /// Its preview state.
        state: PreviewState,
    },
    /// Bytes arrived, or a reason they did not.
    Content {
        /// The item.
        id: ItemId,
        /// The state.
        state: ContentState,
    },
    /// The service accepted the copy.
    Copied {
        /// The item copied, when the copy came from the history.
        id: Option<ItemId>,
        /// The numbers the toast reports.
        report: CopyReport,
    },
    /// Change events arrived from the subscription.
    Changes {
        /// The batch.
        events: Vec<crate::model::ChangeEvent>,
    },
    /// The live selection was read.
    Live {
        /// The answer, or the reason there is none.
        content: Result<LiveContent, Refusal>,
    },
    /// A verb was refused, with the contract's class.
    Refused {
        /// The verb.
        verb: String,
        /// The refusal.
        refusal: Refusal,
    },
    /// A call failed on the client side (timeout, decode, transport).
    Failed {
        /// The verb.
        verb: String,
        /// The message.
        message: String,
    },
    /// A session-level notice for the status bar (reconnect scheduled, etc.).
    Notice {
        /// The sentence.
        message: String,
    },
    /// The worker is stopping.
    Stopped,
}

/// A handle the UI holds. Cloning it clones the command channel, so the
/// palette and the library share one worker.
#[derive(Clone)]
pub struct WorkerHandle {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
}

impl WorkerHandle {
    /// Send one command. A closed channel means the worker is gone, which the
    /// UI renders as "the app's protocol worker stopped" rather than panicking.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// `true` while the worker thread is alive.
    pub fn is_alive(&self) -> bool {
        !self.commands.is_closed()
    }
}

/// The worker itself: spawn it once at boot.
pub struct Worker;

impl Worker {
    /// Start the worker thread and return the handle plus the event stream.
    ///
    /// The event stream is `async_channel` rather than tokio's because the UI
    /// side is driven by gpui's executor, not by the worker's runtime: the
    /// only thing the two share is a channel that works under both.
    pub fn spawn(location: ServiceLocation) -> (WorkerHandle, async_channel::Receiver<BackendEvent>) {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = async_channel::unbounded();
        // The actor gets its own sender so a reconnect timer can post
        // `Command::Connect` through the same door the UI uses.
        let command_tx_for_actor = command_tx.clone();

        std::thread::Builder::new()
            .name("smart-clipboard-rpc".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_tx.send_blocking(BackendEvent::Failed {
                            verb: "runtime".to_string(),
                            message: format!("cannot start the RPC runtime: {error}"),
                        });
                        return;
                    }
                };
                let events = event_tx.clone();
                let self_sender = command_tx_for_actor.clone();
                let local = tokio::task::LocalSet::new();
                local.block_on(&runtime, async move {
                    Actor::new(location, command_rx, self_sender, events)
                        .run()
                        .await;
                });
                let _ = event_tx.send_blocking(BackendEvent::Stopped);
            })
            .expect("the protocol worker thread must start");

        (WorkerHandle { commands: command_tx }, event_rx)
    }
}

/// The actor's private state.
///
/// It keeps a sender to its own command queue — not for elegance, but so a
/// timer can re-enter the same door the UI uses. A reconnect is then a normal
/// `Command::Connect`, and there is exactly one code path that dials.
struct Actor {
    location: ServiceLocation,
    commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    self_sender: tokio::sync::mpsc::UnboundedSender<Command>,
    events: async_channel::Sender<BackendEvent>,
    session: Option<Session>,
    subscription: Option<crate::schema::clipboard_capnp::clipboard_change_subscription::Client>,
    query: Query,
    /// How many items the current query's window holds, so `LoadMore` knows
    /// its offset without trusting a UI-side count.
    window_len: usize,
    /// The items this session has seen, so a copy or a size guard can answer
    /// without a second round trip.
    known: HashMap<u64, Item>,
    /// Whether the current window has reached the service's total.
    exhausted: bool,
}

impl Actor {
    fn new(
        location: ServiceLocation,
        commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
        self_sender: tokio::sync::mpsc::UnboundedSender<Command>,
        events: async_channel::Sender<BackendEvent>,
    ) -> Self {
        Actor {
            location,
            commands,
            self_sender,
            events,
            session: None,
            subscription: None,
            query: Query::first(String::new()),
            window_len: 0,
            known: HashMap::new(),
            exhausted: false,
        }
    }

    async fn run(mut self) {
        self.send(BackendEvent::Notice {
            message: "looking for a smart service…".to_string(),
        });
        self.connect().await;

        let mut ticker = tokio::time::interval(PULL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;
                command = self.commands.recv() => {
                    match command {
                        Some(Command::Shutdown) | None => break,
                        Some(command) => self.handle(command).await,
                    }
                }
                _ = ticker.tick() => {
                    self.pull_changes().await;
                }
            }
        }
    }

    /// Send one event. A closed channel means the UI is gone, which is a
    /// reason to stop, not to panic.
    fn send(&self, event: BackendEvent) {
        let _ = self.events.send_blocking(event);
    }

    /// Walk the candidate list until one answers.
    async fn connect(&mut self) {
        let mut attempts: Vec<(String, String)> = Vec::new();
        for candidate in self.location.candidates.clone() {
            let endpoint = candidate.display();
            self.send(BackendEvent::Dialling {
                endpoint: endpoint.clone(),
                source: candidate.source.label(),
            });
            match Session::connect(&candidate).await {
                Ok(session) => {
                    self.session = Some(session);
                    self.announce_connected().await;
                    self.subscribe().await;
                    self.refresh().await;
                    self.read_live().await;
                    return;
                }
                Err(error) => {
                    let reason = error.to_string();
                    self.send(BackendEvent::DialFailed {
                        endpoint: endpoint.clone(),
                        reason: reason.clone(),
                    });
                    attempts.push((endpoint, reason));
                }
            }
        }
        let reason = attempts
            .last()
            .map(|(endpoint, why)| format!("{endpoint}: {why}"))
            .unwrap_or_else(|| "no endpoint was configured".to_string());
        self.send(BackendEvent::ConnectFailed { attempts, reason });
    }

    /// Emit what discovery and capability said.
    async fn announce_connected(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let endpoint = session.endpoint.clone();
        let service = session.service.clone();
        let authority = session.authority.clone();
        let published = session
            .all_services()
            .await
            .into_iter()
            .map(|service| service.name)
            .collect();

        // A schema drift is a *notice*, not an error: the app still works,
        // and the user deserves to know which contract each side speaks.
        if !service.schema_matches_app {
            self.send(BackendEvent::Notice {
                message: format!(
                    "schema drift: the service publishes 0x{:016x}, this app compiled 0x{:016x}",
                    service.schema_id,
                    crate::contract::EXPECTED_CLIPBOARD_SCHEMA_ID
                ),
            });
        }

        self.send(BackendEvent::Connected {
            endpoint,
            service,
            authority,
            published,
        });
    }

    /// Mint a change subscription, if the service serves one.
    async fn subscribe(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        match client.subscribe().await {
            Ok(subscription) => {
                self.subscription = Some(subscription);
                self.send(BackendEvent::Notice {
                    message: format!(
                        "capture stream open (pull every {} ms)",
                        PULL_INTERVAL.as_millis()
                    ),
                });
            }
            Err(ClientError::Refused(refusal)) => {
                self.send(BackendEvent::Refused {
                    verb: refusal.verb.clone(),
                    refusal,
                });
            }
            Err(error) => {
                self.send(BackendEvent::Failed {
                    verb: "subscribeChanges".to_string(),
                    message: error.to_string(),
                });
            }
        }
    }

    /// Pull a bounded batch, if a subscription exists.
    async fn pull_changes(&mut self) {
        let Some(subscription) = self.subscription.clone() else {
            return;
        };
        match ClipboardClient::pull(&subscription, PULL_BATCH).await {
            Ok(events) if !events.is_empty() => {
                self.send(BackendEvent::Changes { events });
            }
            Ok(_) => {}
            Err(ClientError::Refused(refusal)) => {
                // A refused pull retires the subscription: continuing to ask
                // a service that said no is noise, not persistence.
                self.subscription = None;
                self.send(BackendEvent::Refused {
                    verb: refusal.verb.clone(),
                    refusal,
                });
            }
            Err(error) => {
                self.subscription = None;
                self.drop_session(error.to_string());
            }
        }
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Connect => {
                self.session = None;
                self.subscription = None;
                self.known.clear();
                self.window_len = 0;
                self.exhausted = false;
                self.connect().await;
            }
            Command::Refresh(query) => {
                self.query = query;
                self.window_len = 0;
                self.exhausted = false;
                self.refresh().await;
            }
            Command::LoadMore => self.load_more().await,
            Command::Preview(ids) => self.previews(ids).await,
            Command::Content { id, mime } => self.content(id, &mime).await,
            Command::Copy(target) => self.copy(target).await,
            Command::SetPinned { id, pinned } => self.set_pinned(id, pinned).await,
            Command::Delete(id) => self.delete(id).await,
            Command::ClearHistory => self.clear_history().await,
            Command::ReadLive { .. } => self.read_live().await,
            Command::Describe => self.announce_connected().await,
            Command::Shutdown => {}
        }
    }

    async fn refresh(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let query = self.query.clone();
        let mut client = ClipboardClient::new(session);
        match client.page(&query).await {
            Ok(page) => {
                self.remember(&page);
                self.window_len = page.items.len();
                self.exhausted = self.window_len as u64 >= page.total
                    || page.items.is_empty()
                    || (page.items.len() as u32) < query.limit;
                self.send(BackendEvent::Page { page, append: false });
            }
            Err(error) => self.report("history", error),
        }
    }

    async fn load_more(&mut self) {
        if self.exhausted || self.session.is_none() {
            return;
        }
        let query = self.query.next(self.window_len);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        match client.page(&query).await {
            Ok(page) => {
                if page.items.is_empty() {
                    self.exhausted = true;
                }
                self.window_len += page.items.len();
                self.remember(&page);
                self.send(BackendEvent::Page { page, append: true });
            }
            Err(error) => self.report("history", error),
        }
    }

    /// Fetch previews one at a time. Sequential is deliberate: a page of 200
    /// previews over a local socket is ~200 sub-millisecond calls, and a
    /// bounded, ordered stream keeps the UI's skeletons filling top-down
    /// instead of flickering in arrival order.
    async fn previews(&mut self, ids: Vec<ItemId>) {
        for id in ids {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let mut client = ClipboardClient::new(session);
            let state = client.preview(id).await;
            self.send(BackendEvent::Preview { id, state });
        }
    }

    async fn content(&mut self, id: ItemId, mime: &str) {
        // The client's ceiling is applied *before* the call: asking a service
        // for a 2 GiB payload to then discard it is rude to the service and
        // fatal to the UI.
        if let Some(item) = self.known.get(&id.0) {
            if item.size_bytes > crate::contract::MAX_FETCH_BYTES {
                self.send(BackendEvent::Content {
                    id,
                    state: ContentState::TooLarge {
                        bytes: item.size_bytes,
                        limit: crate::contract::MAX_FETCH_BYTES,
                    },
                });
                return;
            }
        }
        self.send(BackendEvent::Content {
            id,
            state: ContentState::Loading,
        });
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        let state = match client.representation(id, mime).await {
            Ok(Representation::Content { mime, bytes }) => ContentState::Ready { mime, bytes },
            Ok(Representation::Unsupported { target }) => ContentState::Unsupported { target },
            Err(error) => ContentState::from(error),
        };
        self.send(BackendEvent::Content { id, state });
    }

    async fn copy(&mut self, target: CopyTarget) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match target {
            CopyTarget::Representation { id, mime } => {
                let mut client = ClipboardClient::new(session);
                let (mime, bytes) = match client.representation(id, &mime).await {
                    Ok(Representation::Content { mime, bytes }) => (mime, bytes),
                    Ok(Representation::Unsupported { target }) => {
                        self.send(BackendEvent::Refused {
                            verb: "convertContent".to_string(),
                            refusal: Refusal::from_reason_text(
                                "convertContent",
                                &format!(
                                    "notSupported: this service cannot produce {target} for \
                                     item {id}; it offers only the representations it stored"
                                ),
                            ),
                        });
                        return;
                    }
                    Err(error) => {
                        self.report("convertContent", error);
                        return;
                    }
                };
                let size = bytes.len() as u64;
                let mut client = ClipboardClient::new(session);
                match client.set_content(&mime, &bytes).await {
                    Ok(()) => self.send(BackendEvent::Copied {
                        id: Some(id),
                        report: CopyReport {
                            mime,
                            bytes: size,
                            offered: 1,
                        },
                    }),
                    Err(error) => self.report("setContent", error),
                }
            }
            CopyTarget::AllRepresentations { id } => {
                let mimes = self
                    .known
                    .get(&id.0)
                    .map(|item| item.mimes.clone())
                    .unwrap_or_default();
                if mimes.is_empty() {
                    self.send(BackendEvent::Failed {
                        verb: "setOffer".to_string(),
                        message: format!(
                            "item {id} publishes no representations, so there is nothing to offer"
                        ),
                    });
                    return;
                }
                let mut produced: Vec<(String, Vec<u8>)> = Vec::new();
                let mut unsupported: Vec<String> = Vec::new();
                for mime in mimes {
                    let mut client = ClipboardClient::new(session);
                    match client.representation(id, &mime).await {
                        Ok(Representation::Content { mime, bytes }) => produced.push((mime, bytes)),
                        Ok(Representation::Unsupported { target }) => unsupported.push(target),
                        Err(error) => {
                            self.report("convertContent", error);
                            return;
                        }
                    }
                }
                if produced.is_empty() {
                    self.send(BackendEvent::Refused {
                        verb: "setOffer".to_string(),
                        refusal: Refusal::from_reason_text(
                            "setOffer",
                            &format!(
                                "notSupported: this service can produce none of the recorded \
                                 representations (tried: {})",
                                if unsupported.is_empty() {
                                    "none recorded".to_string()
                                } else {
                                    unsupported.join(", ")
                                }
                            ),
                        ),
                    });
                    return;
                }
                let primary = produced[0].0.clone();
                let primary_bytes = produced[0].1.len() as u64;
                let offered = produced.len();
                let mut client = ClipboardClient::new(session);
                match client.set_offer(&produced).await {
                    Ok(()) => self.send(BackendEvent::Copied {
                        id: Some(id),
                        report: CopyReport {
                            mime: primary,
                            bytes: primary_bytes,
                            offered,
                        },
                    }),
                    Err(error) => self.report("setOffer", error),
                }
            }
        }
    }

    async fn set_pinned(&mut self, id: ItemId, pinned: bool) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        match client.set_pinned(id, pinned).await {
            Ok(()) => {
                self.send(BackendEvent::Notice {
                    message: format!("{} {id}", if pinned { "pinned" } else { "unpinned" }),
                });
                // The service owns the list; re-read it rather than patching
                // a local copy into a shape the backend might not have taken.
                self.refresh().await;
            }
            Err(error) => self.report(if pinned { "pinItem" } else { "unpinItem" }, error),
        }
    }

    async fn delete(&mut self, id: ItemId) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        match client.delete(id).await {
            Ok(()) => {
                self.known.remove(&id.0);
                self.send(BackendEvent::Notice {
                    message: format!("removed {id} from the service's history"),
                });
                self.refresh().await;
            }
            Err(error) => self.report("deleteItem", error),
        }
    }

    async fn clear_history(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        match client.clear_history().await {
            Ok(()) => {
                self.known.clear();
                self.window_len = 0;
                self.send(BackendEvent::Notice {
                    message: "the service cleared its history".to_string(),
                });
                self.refresh().await;
            }
            Err(error) => self.report("clearHistory", error),
        }
    }

    async fn read_live(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut client = ClipboardClient::new(session);
        // The live read asks for text first: it is the representation every
        // backend that serves `getContent` can produce, and the strip only
        // shows a size and a one-line sample.
        let answer = client
            .live_content(crate::contract::PREFERRED_TEXT_MIME)
            .await
            .map_err(|error| match error {
                ClientError::Refused(refusal) => refusal,
                other => Refusal::from_reason_text("getContent", &other.to_string()),
            });
        self.send(BackendEvent::Live { content: answer });
    }

    fn remember(&mut self, page: &Page) {
        for item in &page.items {
            self.known.insert(item.id.0, item.clone());
        }
    }

    /// Report an error, distinguishing "the service said no" from "the
    /// connection died".
    fn report(&mut self, verb: &str, error: ClientError) {
        match error {
            ClientError::Refused(refusal) => self.send(BackendEvent::Refused {
                verb: refusal.verb.clone(),
                refusal,
            }),
            ClientError::Transport(detail) | ClientError::Decode(detail) => {
                self.drop_session(detail)
            }
            other => self.send(BackendEvent::Failed {
                verb: verb.to_string(),
                message: other.to_string(),
            }),
        }
    }

    /// The connection is gone: drop it, say so, and schedule one retry.
    fn drop_session(&mut self, reason: String) {
        let was_connected = self.session.take().is_some();
        self.subscription = None;
        self.send(BackendEvent::Disconnected {
            reason: reason.clone(),
            retrying: true,
        });
        if !was_connected {
            return;
        }
        self.send(BackendEvent::Notice {
            message: format!("reconnecting in {} s", RECONNECT_DELAY.as_secs()),
        });
        let commands = self.self_sender.clone();
        // One retry, scheduled off the pull ticker so a busy stream cannot
        // starve it. The retry is a `Connect` command, which re-walks the
        // candidate list — the same code path the Retry button uses.
        tokio::task::spawn_local(async move {
            tokio::time::sleep(RECONNECT_DELAY).await;
            let _ = commands.send(Command::Connect);
        });
    }
}
