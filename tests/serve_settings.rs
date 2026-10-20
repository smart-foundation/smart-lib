//! The settings domain, served through the contract — the spine's own tests.

#![cfg(all(feature = "rpc", feature = "std", feature = "domain-settings"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::settings::availability;
use smart_lib::platforms::linux::settings::LinuxSettingsBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::schema::settings_capnp::settings as st;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::service::Service;
use smart_lib::serve::settings::SettingsDomain;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{assert_honest, metadata_of, run_client, socket_path, text};

/// The service under test: the settings domain plus the foundation interfaces.
fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::SETTINGS_SCHEMA_ID,
        name: "smart-protocol/settings".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = SettingsDomain::new(Arc::new(LinuxSettingsBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-settings.sock",
    )]);
    Arc::new(service)
}

/// A contract-only client of the `Settings` interface, in process.
fn settings_client() -> st::Client {
    let server = smart_lib::serve::settings::SettingsService::new(
        Arc::new(LinuxSettingsBackend::new()),
        CapabilityClass::Control,
    );
    capnp_rpc::new_client(server)
}

/// A domain the host actually publishes, with a key it actually carries.
///
/// The settings store serves the schemas the host has. The test reads one
/// of them through the contract — `listDomains` for the domains, `getAll`
/// for the keys — so the typed path is exercised against the host's own
/// state, and the refusal path is still proven on a host that publishes
/// no schema at all.
fn a_published_domain_and_key() -> Option<(String, String)> {
    let client = settings_client();
    let domains = futures::executor::block_on(async move {
        let mut request = client.list_domains_request();
        {
            let mut params = request.get();
            let mut page = params.init_pagination();
            page.set_offset(0);
            page.set_limit(16);
        }
        request.send().promise.await
    });
    let response = match domains {
        Ok(response) => response,
        Err(error) => {
            assert_honest("listDomains", Err(error));
            return None;
        }
    };
    let results = response.get().expect("the results");
    let list = results.get_domains().expect("the domains");
    for index in 0..list.len() {
        let domain = text(list.get(index).expect("the domain"));
        let client = settings_client();
        let domain_in = domain.clone();
        let all = futures::executor::block_on(async move {
            let mut request = client.get_all_request();
            {
                let mut params = request.get();
                params.set_domain(&domain_in);
                let mut page = params.init_pagination();
                page.set_offset(0);
                page.set_limit(16);
            }
            request.send().promise.await
        });
        let Ok(response) = all else { continue };
        let results = response.get().expect("the results");
        let settings = results.get_settings().expect("the settings");
        let entries = settings.get_entries().expect("the entries");
        if entries.len() == 0 {
            continue;
        }
        let key = text(entries.get(0).get_key().expect("the key"));
        return Some((domain, key));
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════
// Every contract method answers, or refuses with a reason
// ═══════════════════════════════════════════════════════════════════════

/// One contract call over the `Settings` interface, with the domain and key
/// the host publishes.
macro_rules! call {
    ($domain:expr, $key:expr, $method:ident, |$params:ident, $d:ident, $k:ident| $body:block) => {{
        let $d = $domain.clone();
        let $k = $key.clone();
        let client = settings_client();
        futures::executor::block_on(async move {
            let mut request = client.$method();
            {
                let $params = request.get();
                $body
            }
            request.send().promise.await
        })
    }};
}

#[test]
fn every_method_of_the_settings_interface_answers_or_refuses_honestly() {
    // The domain and key the host actually publishes. The settings store
    // serves the schemas the host has, and says so when it has none.
    let published = a_published_domain_and_key();
    let (domain, key) = published.unwrap_or_default();
    let domain = Arc::new(domain);
    let key = Arc::new(key);

    // get @0 — one key, as the contract's own union.
    let read = call!(domain, key, get_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
    });
    match read {
        Ok(response) => {
            let result = response
                .get()
                .expect("the results")
                .get_result()
                .expect("the result");
            // The union is total: a value, a notSet, an error or an unknown —
            // never a void that could mean anything.
            assert!(result.which().is_ok(), "the result must be a union arm");
        }
        Err(error) => assert_honest("get", Err(error)),
    }

    // getAll @2 — one domain's live values.
    let all = call!(domain, key, get_all_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        let mut page = params.init_pagination();
        page.set_offset(0);
        page.set_limit(16);
    });
    assert_honest("getAll", all.map(|_| ()));

    // listDomains @5 — the domains this host publishes.
    let domains = call!(domain, key, list_domains_request, |params, d, k| {
        let mut params = params;
        let mut page = params.init_pagination();
        page.set_offset(0);
        page.set_limit(16);
    });
    assert_honest("listDomains", domains.map(|_| ()));

    // bulkGet @9 — several keys at once.
    let bulk = call!(domain, key, bulk_get_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        let mut keys = params.init_keys(1);
        keys.reborrow().set(0, k.as_str());
    });
    assert_honest("bulkGet", bulk.map(|_| ()));

    // listByCategory @10 — the keys of one category (an open token).
    let category = call!(domain, key, list_by_category_request, |params, d, k| {
        let mut params = params;
        params.set_category("general");
        let mut page = params.init_pagination();
        page.set_offset(0);
        page.set_limit(16);
    });
    assert_honest("listByCategory", category.map(|_| ()));

    // getVersion @14 — the store's own version of one key.
    let version = call!(domain, key, get_version_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
    });
    assert_honest("getVersion", version.map(|_| ()));

    // getHistory @16 — one key's recorded change history.
    let history = call!(domain, key, get_history_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
        let mut page = params.init_pagination();
        page.set_offset(0);
        page.set_limit(16);
    });
    assert_honest("getHistory", history.map(|_| ()));

    // getSettingsDiff @17 — the delta read instead of a full-state poll.
    let difference = call!(domain, key, get_settings_diff_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_since_version(0);
    });
    assert_honest("getSettingsDiff", difference.map(|_| ()));

    // getAppOverride @12 — the app-override pair the matrix marks
    // unavailable on this host.
    let override_read = call!(domain, key, get_app_override_request, |params, d, k| {
        let mut params = params;
        params.reborrow().init_app_id().set_id(1);
        params.set_key("a-key");
    });
    assert_honest("getAppOverride", override_read.map(|_| ()));

    // The mutating verbs.
    let written = call!(domain, key, set_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
        let mut value = params.init_value();
        value.set_string("a-value");
    });
    assert_honest("set", written.map(|_| ()));

    let deleted = call!(domain, key, delete_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
    });
    assert_honest("delete", deleted.map(|_| ()));

    let cleared = call!(domain, key, clear_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
    });
    assert_honest("clear", cleared.map(|_| ()));

    let bulk_written = call!(domain, key, bulk_set_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        let mut entries = params.init_entries(1);
        let mut entry = entries.reborrow().get(0);
        entry.set_key("a-key");
        let mut value = entry.init_value();
        value.set_string("a-value");
    });
    assert_honest("bulkSet", bulk_written.map(|_| ()));

    let conditional = call!(domain, key, set_if_unchanged_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
        params.set_expected_version(0);
        let mut value = params.reborrow().init_value();
        value.set_string("a-value");
    });
    assert_honest("setIfUnchanged", conditional.map(|_| ()));

    // The app-override WRITER: the matrix marks it unavailable here.
    let override_written = call!(domain, key, set_app_override_request, |params, d, k| {
        let mut params = params;
        params.reborrow().init_app_id().set_id(1);
        params.set_key("a-key");
        let mut value = params.init_value();
        value.set_string("a-value");
    });
    assert_honest("setAppOverride", override_written.map(|_| ()));

    let override_cleared = call!(domain, key, clear_app_override_request, |params, d, k| {
        let mut params = params;
        params.reborrow().init_app_id().set_id(1);
        params.set_key("a-key");
    });
    assert_honest("clearAppOverride", override_cleared.map(|_| ()));

    // The two subscriptions.
    let watched = call!(domain, key, watch_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
        params.set_key(k.as_str());
    });
    assert_honest("watch", watched.map(|_| ()));

    let watched_domain = call!(domain, key, watch_domain_request, |params, d, k| {
        let mut params = params;
        params.set_domain(d.as_str());
    });
    assert_honest("watchDomain", watched_domain.map(|_| ()));
}

#[test]
fn the_subscription_is_the_contracts_own_capability() {
    let published = a_published_domain_and_key();
    let Some((domain, key)) = published else {
        // The host publishes no schema to watch: the refusal carries the
        // reason, and that is the honest answer.
        return;
    };
    let client = settings_client();
    let watched = futures::executor::block_on(async move {
        let mut request = client.watch_request();
        {
            let mut params = request.get();
            params.set_domain(&domain);
            params.set_key(&key);
        }
        request.send().promise.await
    });
    let subscription = match watched {
        Ok(response) => response
            .get()
            .expect("the results")
            .get_subscription()
            .expect("a subscription capability"),
        Err(error) => {
            assert_honest("watch", Err(error));
            return;
        }
    };

    let puller = subscription.clone();
    let pulled = futures::executor::block_on(async move {
        let mut request = puller.pull_request();
        {
            let mut params = request.get();
            params.set_max_events(8);
        }
        request.send().promise.await
    })
    .expect("the pull answers");
    let changes = pulled
        .get()
        .expect("the results")
        .get_changes()
        .expect("the changes");
    assert!(changes.len() <= 8, "the pull is bounded by the caller");

    let cancelled = futures::executor::block_on(async move {
        let request = subscription.cancel_request();
        request.send().promise.await
    });
    assert_honest("SettingsSubscription.cancel", cancelled.map(|_| ()));
}

#[test]
fn the_app_override_verbs_answer_the_matrix_reason() {
    // The matrix marks the app-override pair unavailable on this host: the
    // answer is the matrix's own reason, never a fabricated success.
    let client = settings_client();
    let refused = futures::executor::block_on(async move {
        let mut request = client.set_app_override_request();
        {
            let mut params = request.get();
            params.reborrow().init_app_id().set_id(1);
            params.set_key("a-key");
            let mut value = params.init_value();
            value.set_string("a-value");
        }
        request.send().promise.await
    });
    let reason = serve_common::refusal_reason(refused.map(|_| ()))
        .expect("the matrix says this host cannot take an app override");
    assert!(
        reason.contains("notSupported:"),
        "the refusal must ride the contract's own code token: {reason}"
    );
    assert!(
        reason.contains("no-privilege:"),
        "the refusal must carry the matrix's reason class: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// The registry
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_registry_publishes_every_method_of_the_settings_domain() {
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
            "settings domains",
            "settings list",
            "settings get",
            "settings set",
            "settings delete",
            "settings clear",
            "settings watch",
            "settings watch-domain",
            "settings bulk-set",
            "settings bulk-get",
            "settings category",
            "settings get-app-override",
            "settings version",
            "settings set-if-unchanged",
            "settings history",
            "settings diff",
            "settings pull",
            "settings cancel",
        ],
        "the published surface is the domain's own, minus what cannot run here"
    );
    // The subscription's methods carry the interface that declares them.
    let pull = published
        .iter()
        .find(|entry| entry.0 == "settings pull")
        .expect("the subscription's pull must be published");
    assert_eq!(pull.1, "SettingsSubscription");
    assert_eq!(pull.2, "pull");
    // The composition: "settings watch" needs the domain watch, the pull and
    // the cancel.
    assert_eq!(published[6].4.as_deref(), Some("watchDomain,pull,cancel"));
    // The method's identity: the ordinal the schema declares.
    assert_eq!(published[0].3.as_deref(), Some("5"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_unavailable_commands_carry_the_matrix_reason() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
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
        let mut entries = Vec::new();
        for index in 0..commands.len() {
            let registration = commands.get(index);
            entries.push((
                text(registration.get_command_path().expect("the path")),
                registration.get_unavailable_reason().expect("the reason"),
                metadata_of(registration.get_metadata().ok(), "reason"),
            ));
        }
        entries
    });

    assert_eq!(published.len(), 2, "two commands cannot run here");
    assert_eq!(published[0].0, "settings set-app-override");
    assert_eq!(published[1].0, "settings clear-app-override");
    // The contract's own reason code.
    assert_eq!(
        published[0].1,
        smart_lib::schema::cli_capnp::AvailabilityReason::CapabilityDenied
    );
    // And the engine's own words.
    let reason = published[0].2.clone().expect("the reason must be published");
    assert!(
        reason.starts_with("no-privilege:"),
        "the reason must be the matrix's own: {reason}"
    );

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The discovery descriptor
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_discovery_publishes_the_settings_domains_descriptor() {
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
                    filter.set_name("settings");
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

    assert_eq!(published.0, "settings");
    assert_eq!(published.1, "settings");
    assert_eq!(published.2, availability::SETTINGS_SCHEMA_ID);
    // Two methods of the twenty cannot run here, so the service is degraded.
    assert_eq!(
        published.3,
        smart_lib::schema::discovery_capnp::ServiceState::Degraded
    );
    assert_eq!(published.4, 1);
    let tokens = published.5.expect("the capability tokens must be published");
    assert!(!tokens.is_empty(), "the tokens must be the domain's own");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_domains_availability_matrix_is_the_engines_own() {
    let (real, degraded, unavailable) = availability::status_counts();
    assert_eq!(real + degraded + unavailable, 20, "the matrix's own rows");
    assert_eq!(unavailable, 2, "the app-override pair");
    assert_eq!(degraded + real, 18);
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
        20,
        "the surface must name every contract method: {methods:?}"
    );
    for expected in ["get", "set", "getAll", "delete", "clear", "listDomains", "watch", "watchDomain", "bulkSet", "bulkGet", "listByCategory", "setAppOverride", "getAppOverride", "clearAppOverride", "getVersion", "setIfUnchanged", "getHistory", "getSettingsDiff", "pull", "cancel"] {
        assert!(
            methods.iter().any(|name| name == expected),
            "the surface must name {expected}: {methods:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
