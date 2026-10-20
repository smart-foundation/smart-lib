//! The logging domain, served through the contract — the spine's own tests.

#![cfg(all(feature = "rpc", feature = "std", feature = "domain-logging"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::logging::availability;
use smart_lib::platforms::linux::logging::LinuxLoggingBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::schema::logging_capnp::logging as lg;
use smart_lib::schema::types_capnp;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::logging::LoggingDomain;
use smart_lib::serve::service::Service;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{assert_honest, metadata_of, run_client, socket_path, text};

/// The service under test: the logging domain plus the foundation interfaces.
fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::LOGGING_SCHEMA_ID,
        name: "smart-protocol/logging".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = LoggingDomain::new(Arc::new(LinuxLoggingBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-logging.sock",
    )]);
    Arc::new(service)
}

/// A contract-only client of the `Logging` interface, in process.
fn logging_client() -> lg::Client {
    let server = smart_lib::serve::logging::LoggingService::new(
        Arc::new(LinuxLoggingBackend::new()),
        CapabilityClass::Control,
    );
    capnp_rpc::new_client(server)
}

// ═══════════════════════════════════════════════════════════════════════
// Every contract method answers, or refuses with a reason
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn every_method_of_the_logging_interface_answers_or_refuses_honestly() {
    // log @0 — the hot path. The outcome must say what happened: accepted,
    // sampled or dropped. Never a silent success.
    let client = logging_client();
    let logged = futures::executor::block_on(async move {
        let mut request = client.log_request();
        {
            let mut params = request.get();
            params.set_severity(types_capnp::LogLevel::Info);
            params.set_domain("smart-lib.test");
            params.set_message("the spine's own test entry");
        }
        request.send().promise.await
    })
    .expect("the log answers");
    let outcome = logged
        .get()
        .expect("the results")
        .get_outcome()
        .expect("the outcome");
    // The outcome is one of the contract's own arms — not a void.
    assert!(outcome.which().is_ok(), "the outcome must be a union arm");

    // getEntries @1 — the bounded query.
    let client = logging_client();
    let entries = futures::executor::block_on(async move {
        let mut request = client.get_entries_request();
        {
            let mut params = request.get();
            {
                let mut filter = params.reborrow().init_filter();
                filter.set_min_level(types_capnp::LogLevel::Trace);
            }
            let mut page = params.init_page();
            page.set_offset(0);
            page.set_limit(16);
        }
        request.send().promise.await
    })
    .expect("the query answers");
    let results = entries.get().expect("the results");
    let list = results.get_entries().expect("the entries");
    // The total is a count the caller may page through, not a zero standing
    // in for "unknown".
    assert!(results.get_total() >= list.len() as u64);

    // getLevel @4 — one domain's configured level.
    let client = logging_client();
    let level = futures::executor::block_on(async move {
        let mut request = client.get_level_request();
        {
            let mut params = request.get();
            params.set_domain("smart-lib.test");
        }
        request.send().promise.await
    });
    assert_honest("getLevel", level.map(|_| ()));

    // listDomains @8 — the domains this host knows.
    let client = logging_client();
    let domains = futures::executor::block_on(async move {
        let mut request = client.list_domains_request();
        {
            let mut params = request.get();
            let mut page = params.init_page();
            page.set_offset(0);
            page.set_limit(16);
        }
        request.send().promise.await
    })
    .expect("the domains answer");
    let results = domains.get().expect("the results");
    let list = results.get_domains().expect("the domains");
    assert!(results.get_total() >= list.len() as u64);

    // export @6 — the open content type.
    let client = logging_client();
    let exported = futures::executor::block_on(async move {
        let mut request = client.export_request();
        {
            let mut params = request.get();
            {
                let mut filter = params.reborrow().init_filter();
                filter.set_min_level(types_capnp::LogLevel::Trace);
            }
            let mut format = params.init_format();
            format.set_type("text");
            format.set_subtype("plain");
        }
        request.send().promise.await
    });
    assert_honest("export", exported.map(|_| ()));

    // setLevel @3 — the mutating verb.
    let client = logging_client();
    let set = futures::executor::block_on(async move {
        let mut request = client.set_level_request();
        {
            let mut params = request.get();
            params.set_domain("smart-lib.test");
            params.set_level(types_capnp::LogLevel::Warn);
        }
        request.send().promise.await
    });
    assert_honest("setLevel", set.map(|_| ()));

    // setSamplingRate @7 — the declared sampling.
    let client = logging_client();
    let sampling = futures::executor::block_on(async move {
        let mut request = client.set_sampling_rate_request();
        {
            let mut params = request.get();
            params.set_domain("smart-lib.test");
            params.set_rate(1.0);
        }
        request.send().promise.await
    });
    assert_honest("setSamplingRate", sampling.map(|_| ()));

    // clear @5 — the mutating verb that empties a domain.
    let client = logging_client();
    let cleared = futures::executor::block_on(async move {
        let mut request = client.clear_request();
        {
            let mut params = request.get();
            params.set_domain("smart-lib.test");
        }
        request.send().promise.await
    });
    assert_honest("clear", cleared.map(|_| ()));
}

#[test]
fn the_subscription_is_the_contracts_own_capability() {
    let client = logging_client();
    let subscribed = futures::executor::block_on(async move {
        let mut request = client.subscribe_request();
        {
            let mut params = request.get();
            let mut filter = params.init_filter();
            filter.set_min_level(types_capnp::LogLevel::Trace);
        }
        request.send().promise.await
    })
    .expect("the subscription answers");
    let subscription = subscribed
        .get()
        .expect("the results")
        .get_subscription()
        .expect("a subscription capability");

    // The reference IS the authority to pull: no id, no token.
    let puller = subscription.clone();
    let pulled = futures::executor::block_on(async move {
        let mut request = puller.pull_request();
        {
            let mut params = request.get();
            params.set_max_entries(8);
        }
        request.send().promise.await
    })
    .expect("the pull answers");
    let entries = pulled
        .get()
        .expect("the results")
        .get_entries()
        .expect("the entries");
    assert!(entries.len() <= 8, "the pull is bounded by the caller");

    // And the subscription can be retired.
    let cancelled = futures::executor::block_on(async move {
        let request = subscription.cancel_request();
        request.send().promise.await
    });
    assert_honest("LogSubscription.cancel", cancelled.map(|_| ()));
}

// ═══════════════════════════════════════════════════════════════════════
// The registry
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_registry_publishes_every_method_of_the_logging_domain() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = cli.list_commands_request();
                {
                    let params = request.get();
                    let mut page = params.init_pagination();
                    page.set_offset(0);
                    page.set_limit(1024);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let commands = results.get_commands().expect("the commands");
        let mut entries = Vec::new();
        for index in 0..commands.len() {
            let registration = commands.get(index);
            entries.push((
                text(registration.get_command_path().expect("the path")),
                text(registration.get_interface_name().expect("the interface")),
                text(registration.get_method_name().expect("the method")),
                metadata_of(registration.get_metadata().ok(), "methodId"),
                metadata_of(registration.get_metadata().ok(), "composes"),
            ));
        }
        entries
    });

    let paths: Vec<&str> = published.iter().map(|entry| entry.0.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "logging sinks",
            "logging domains",
            "logging tail",
            "logging entries",
            "logging follow",
            "logging emit",
            "logging level",
            "logging set-level",
            "logging sampling",
            "logging clear",
            "logging export",
            "logging pull",
            "logging cancel",
        ],
        "the published surface is the domain's own"
    );
    // The subscription's methods are published under the interface that
    // declares them.
    assert_eq!(published[4].1, "Logging");
    assert_eq!(published[4].2, "subscribe");
    let pull = published
        .iter()
        .find(|entry| entry.0 == "logging pull")
        .expect("the subscription's pull must be published");
    assert_eq!(pull.1, "LogSubscription");
    assert_eq!(pull.2, "pull");
    // The composition: "logging follow" needs the pull and the cancel.
    assert_eq!(published[4].4.as_deref(), Some("pull,cancel"));
    // The method's identity: the ordinal the schema declares.
    assert_eq!(published[0].3.as_deref(), Some("8"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_logging_domain_has_no_unavailable_command() {
    // The logging matrix marks every row served: nothing of this domain's
    // surface is missing here, so the unavailable list is empty for it.
    let (real, degraded, unavailable) = availability::status_counts();
    assert_eq!(unavailable, 0);
    assert_eq!(real + degraded, 11, "the matrix's own rows");

    let service = build_service();
    let path = socket_path();
    let count = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = cli.list_unavailable_request();
                {
                    let params = request.get();
                    let mut page = params.init_pagination();
                    page.set_offset(0);
                    page.set_limit(1024);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let commands = results.get_commands().expect("the commands");
        commands.len()
    });
    assert_eq!(count, 0, "nothing of the logging surface is unavailable");

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The discovery descriptor
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_discovery_publishes_the_logging_domains_descriptor() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.lookup_request();
                {
                    let params = request.get();
                    let mut filter = params.init_filter();
                    filter.set_name("logging");
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let services = results.get_services().expect("the service list");
        let descriptor = services.get(0);
        let endpoints = descriptor.get_endpoints().expect("the endpoints");
        (
            text(descriptor.get_name().expect("the name")),
            text(descriptor.get_category().expect("the category")),
            descriptor.get_id().expect("the id").get_id(),
            descriptor.get_state().expect("the state"),
            endpoints.len(),
            metadata_of(descriptor.get_metadata().ok(), "capabilities"),
        )
    });

    assert_eq!(published.0, "logging");
    assert_eq!(published.1, "logging");
    assert_eq!(published.2, availability::LOGGING_SCHEMA_ID);
    // Every row of the matrix is served here, so the service is online.
    assert_eq!(
        published.3,
        smart_lib::schema::discovery_capnp::ServiceState::Online
    );
    assert_eq!(published.4, 1);
    let tokens = published.5.expect("the capability tokens must be published");
    assert!(!tokens.is_empty(), "the tokens must be the domain's own");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_surface_names_every_contract_method_of_the_domain() {
    // The surface must not hide a method the engine serves: EVERY contract
    // method of this domain is named by at least one published path, and the
    // names are the contract's own.
    let service = build_service();
    let path = socket_path();

    let methods = run_client(service, &path, CapabilityClass::Control, |session| {
        let mut names: Vec<String> = Vec::new();
        {
            let cli: cli_service::Client = session.interface();
            let response = session
                .run(async move {
                    let mut request = cli.list_commands_request();
                    {
                        let params = request.get();
                        let mut page = params.init_pagination();
                        page.set_offset(0);
                        page.set_limit(1024);
                    }
                    request.send().promise.await
                })
                .expect("the call answers");
            let commands = response
                .get()
                .expect("the results")
                .get_commands()
                .expect("the commands");
            for index in 0..commands.len() {
                names.push(text(
                    commands
                        .get(index)
                        .get_method_name()
                        .expect("the method"),
                ));
            }
        }
        {
            let cli: cli_service::Client = session.interface();
            let response = session
                .run(async move {
                    let mut request = cli.list_unavailable_request();
                    {
                        let params = request.get();
                        let mut page = params.init_pagination();
                        page.set_offset(0);
                        page.set_limit(1024);
                    }
                    request.send().promise.await
                })
                .expect("the call answers");
            let commands = response
                .get()
                .expect("the results")
                .get_commands()
                .expect("the commands");
            for index in 0..commands.len() {
                names.push(text(
                    commands
                        .get(index)
                        .get_method_name()
                        .expect("the method"),
                ));
            }
        }
        names.sort();
        names.dedup();
        names
    });

    assert_eq!(
        methods.len(),
        11,
        "the surface must name every contract method: {methods:?}"
    );
    for expected in ["log", "getEntries", "subscribe", "setLevel", "getLevel", "clear", "export", "setSamplingRate", "listDomains", "pull", "cancel"] {
        assert!(
            methods.iter().any(|name| name == expected),
            "the surface must name {expected}: {methods:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
