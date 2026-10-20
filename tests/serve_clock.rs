//! The clock domain, served through the contract — the spine's own tests.

#![cfg(all(feature = "rpc", feature = "std", feature = "domain-clock"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::clock::availability;
use smart_lib::platforms::linux::clock::LinuxClockBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::clock_capnp::{self as ccapnp, alarm_service as alarms, clock_service as clk};
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::clock::ClockDomain;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::service::Service;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{assert_honest, metadata_of, run_client, socket_path, text};

/// The service under test: the clock domain plus the foundation interfaces.
fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::CLOCK_SCHEMA_ID,
        name: "smart-protocol/clock".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = ClockDomain::new(Arc::new(LinuxClockBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-clock.sock",
    )]);
    Arc::new(service)
}

/// A contract-only client of the `ClockService` interface, in process.
fn clock_client() -> clk::Client {
    let server = smart_lib::serve::clock::ClockService::new(
        Arc::new(LinuxClockBackend::new()),
        CapabilityClass::Control,
    );
    capnp_rpc::new_client(server)
}

/// One contract call over the `ClockService` interface.
macro_rules! call {
    ($method:ident, |$params:ident| $body:block) => {{
        let client = clock_client();
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

// ═══════════════════════════════════════════════════════════════════════
// Every contract method answers, or refuses with a reason
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn every_method_of_the_clock_interface_answers_or_refuses_honestly() {
    // info @0 — the measured state of this host's clocks.
    let info = call!(info_request, |_params| {});
    assert_honest("info", info.map(|_| ()));

    // monotonicNow @1 / realtimeNow @2 — the two readings.
    let monotonic = call!(monotonic_now_request, |_params| {});
    assert_honest("monotonicNow", monotonic.map(|_| ()));
    let realtime = call!(realtime_now_request, |_params| {});
    assert_honest("realtimeNow", realtime.map(|_| ()));

    // listClockDomains @9 — the clock domains this host can serve.
    let domains = call!(list_clock_domains_request, |_params| {});
    assert_honest("listClockDomains", domains.map(|_| ()));

    // getClockSync @11 — the measured synchronization state.
    let sync = call!(get_clock_sync_request, |_params| {});
    assert_honest("getClockSync", sync.map(|_| ()));

    // getClockDrift @12 — the drift between two clock domains.
    let drift = call!(get_clock_drift_request, |params| {
        let mut params = params;
        params.set_domain1(ccapnp::ClockDomain::Realtime);
        params.set_domain2(ccapnp::ClockDomain::Monotonic);
    });
    assert_honest("getClockDrift", drift.map(|_| ()));

    // getClockQuality @15 — the quality metrics of one source.
    let quality = call!(get_clock_quality_request, |params| {
        let mut params = params;
        params.set_source_name("system");
    });
    assert_honest("getClockQuality", quality.map(|_| ()));

    // listPersistedTimers @17 — the timers this host persisted.
    let persisted = call!(list_persisted_timers_request, |params| {
        let mut params = params;
        let mut page = params.init_pagination();
        page.set_offset(0);
        page.set_limit(16);
    });
    assert_honest("listPersistedTimers", persisted.map(|_| ()));

    // createTimer @3 — the contract's own timer spec.
    let created = call!(create_timer_request, |params| {
        let mut params = params;
        let mut spec = params.init_spec();
        spec.set_label("test-timer");
        spec.set_clock_domain(ccapnp::ClockDomain::Realtime);
        spec.reborrow().init_interval().set_nanoseconds(60_000_000_000);
        spec.reborrow().init_anchor_type().set_zero(());
    });
    assert_honest("createTimer", created.map(|_| ()));

    // checkTimer @6 — did a timer fire? The handle is the contract's own.
    let checked = call!(check_timer_request, |params| {
        let mut params = params;
        params.reborrow().init_timer().reborrow().init_id().set_id(1);
    });
    assert_honest("checkTimer", checked.map(|_| ()));

    // cancelTimer @4 / resetTimer @5 — the timer verbs.
    let cancelled = call!(cancel_timer_request, |params| {
        let mut params = params;
        params.reborrow().init_timer().reborrow().init_id().set_id(1);
    });
    assert_honest("cancelTimer", cancelled.map(|_| ()));

    let reset = call!(reset_timer_request, |params| {
        let mut params = params;
        params.reborrow().init_timer().reborrow().init_id().set_id(1);
        let mut spec = params.reborrow().init_spec();
        spec.set_label("test-timer");
        spec.set_clock_domain(ccapnp::ClockDomain::Realtime);
        spec.reborrow().init_interval().set_nanoseconds(60_000_000_000);
        spec.reborrow().init_anchor_type().set_zero(());
    });
    assert_honest("resetTimer", reset.map(|_| ()));

    // sleep @8 — the caller's own wait.
    let slept = call!(sleep_request, |params| {
        let mut params = params;
        params.init_duration().set_nanoseconds(1_000_000);
    });
    assert_honest("sleep", slept.map(|_| ()));

    // sleepCancellable @10 — a cancellable sleep and its handle.
    let cancellable = call!(sleep_cancellable_request, |params| {
        let mut params = params;
        params.init_duration().set_nanoseconds(1_000_000);
    });
    assert_honest("sleepCancellable", cancellable.map(|_| ()));

    // persistTimer @16 / setTimerDependency @18.
    let persisted_one = call!(persist_timer_request, |params| {
        let mut params = params;
        params.reborrow().init_timer().reborrow().init_id().set_id(1);
        let mut persistence = params.reborrow().init_persistence();
        persistence.set_name("test-timer");
        persistence.set_re_fire_on_restart(false);
        persistence.set_max_re_fires(0);
    });
    assert_honest("persistTimer", persisted_one.map(|_| ()));

    let dependency = call!(set_timer_dependency_request, |params| {
        let mut params = params;
        params.reborrow().init_timer().reborrow().init_id().set_id(1);
        params.reborrow().init_depends_on().reborrow().init_id().set_id(2);
    });
    assert_honest("setTimerDependency", dependency.map(|_| ()));

    // createVirtualClock @13 / advanceVirtualClock @14.
    let virtual_clock = call!(create_virtual_clock_request, |params| {
        let mut params = params;
        let mut config = params.init_config();
        config.set_name("test-clock");
        config.set_domain(ccapnp::ClockDomain::Custom);
        config.set_rate(1.0);
        config.reborrow().init_initial_time().set_nanoseconds(0);
    });
    assert_honest("createVirtualClock", virtual_clock.map(|_| ()));

    let advanced = call!(advance_virtual_clock_request, |params| {
        let mut params = params;
        params.reborrow().init_clock().set_id(1);
        params.init_duration().set_nanoseconds(1_000_000);
    });
    assert_honest("advanceVirtualClock", advanced.map(|_| ()));

    // subscribeTimers @7 — the contract's own subscription.
    let subscribed = call!(subscribe_timers_request, |params| {
        let mut params = params;
        let mut filter = params.init_filter();
        let mut labels = filter.init_labels(0);
        let _ = &mut labels;
    });
    assert_honest("subscribeTimers", subscribed.map(|_| ()));

    // alarms @19 — the contract's own capability for the alarm service.
    let acquired = call!(alarms_request, |params| {
        let mut params = params;
        let mut authority = params.init_authority();
        authority.set_target("clock.alarms");
    });
    assert_honest("alarms", acquired.map(|_| ()));
}

#[test]
fn the_timer_subscription_is_the_contracts_own_capability() {
    let client = clock_client();
    let subscribed = futures::executor::block_on(async move {
        let mut request = client.subscribe_timers_request();
        {
            let mut params = request.get();
            params.init_filter().init_labels(0);
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
        let request = puller.pull_request();
        request.send().promise.await
    })
    .expect("the pull answers");
    let events = pulled
        .get()
        .expect("the results")
        .get_events()
        .expect("the events");
    assert!(events.len() <= 64, "the pull is bounded");

    // The paginated pull, the contract's own bounded window.
    let puller = subscription.clone();
    let paginated = futures::executor::block_on(async move {
        let mut request = puller.pull_paginated_request();
        {
            let mut params = request.get();
            let mut page = params.init_pagination();
            page.set_offset(0);
            page.set_limit(8);
        }
        request.send().promise.await
    })
    .expect("the paginated pull answers");
    let result = paginated
        .get()
        .expect("the results")
        .get_result()
        .expect("the result");
    let events = result.get_events().expect("the events");
    assert!(events.len() <= 8, "the window is the caller's");

    // And the subscription can be retired.
    let cancelled = futures::executor::block_on(async move {
        let request = subscription.cancel_request();
        request.send().promise.await
    });
    assert_honest("TimerSubscription.cancel", cancelled.map(|_| ()));
}

#[test]
fn the_alarm_service_is_the_contracts_own_capability() {
    let client = clock_client();
    let acquired = futures::executor::block_on(async move {
        let mut request = client.alarms_request();
        {
            let mut params = request.get();
            let mut authority = params.init_authority();
            authority.set_target("clock.alarms");
        }
        request.send().promise.await
    })
    .expect("the alarm service answers");
    let service = acquired
        .get()
        .expect("the results")
        .get_service()
        .expect("the alarm service");

    // listAlarms @4 — the alarms this process's store holds.
    let svc = service.clone();
    let svc = service.clone();
    let listed = futures::executor::block_on(async move {
        let mut request = svc.list_alarms_request();
        {
            let mut params = request.get();
            let mut page = params.init_pagination();
            page.set_offset(0);
            page.set_limit(16);
        }
        request.send().promise.await
    })
    .expect("the alarms answer");
    let results = listed.get().expect("the results");
    let list = results.get_alarms().expect("the alarms");
    assert!(results.get_total_count() >= list.len() as u64);

    // getNextAlarm @7 — the next alarm that will fire.
    let svc = service.clone();
    let svc = service.clone();
    let next = futures::executor::block_on(async move {
        let request = svc.get_next_alarm_request();
        request.send().promise.await
    });
    assert_honest("getNextAlarm", next.map(|_| ()));

    // The mutating alarm verbs.
    let svc = service.clone();
    let svc = service.clone();
    let created = futures::executor::block_on(async move {
        let mut request = svc.create_alarm_request();
        {
            let mut params = request.get();
            let mut alarm = params.init_alarm();
            alarm.set_label("test-alarm");
            alarm.set_enabled(true);
            alarm.reborrow().init_fire_time().set_nanoseconds(0);
        }
        request.send().promise.await
    });
    assert_honest("createAlarm", created.map(|_| ()));

    let svc = service.clone();
    let svc = service.clone();
    let updated = futures::executor::block_on(async move {
        let mut request = svc.update_alarm_request();
        {
            let mut params = request.get();
            let mut alarm = params.init_alarm();
            alarm.set_label("test-alarm");
            alarm.set_enabled(true);
            alarm.reborrow().init_fire_time().set_nanoseconds(0);
        }
        request.send().promise.await
    });
    assert_honest("updateAlarm", updated.map(|_| ()));

    let svc = service.clone();
    let svc = service.clone();
    let dismissed = futures::executor::block_on(async move {
        let mut request = svc.dismiss_alarm_request();
        {
            let mut params = request.get();
            params.reborrow().init_id().set_id(1);
        }
        request.send().promise.await
    });
    assert_honest("dismissAlarm", dismissed.map(|_| ()));

    let svc = service.clone();
    let svc = service.clone();
    let snoozed = futures::executor::block_on(async move {
        let mut request = svc.snooze_alarm_request();
        {
            let mut params = request.get();
            params.reborrow().init_id().set_id(1);
            params.init_duration().set_nanoseconds(60_000_000_000);
        }
        request.send().promise.await
    });
    assert_honest("snoozeAlarm", snoozed.map(|_| ()));

    let svc = service.clone();
    let svc = service.clone();
    let deleted = futures::executor::block_on(async move {
        let mut request = svc.delete_alarm_request();
        {
            let mut params = request.get();
            params.reborrow().init_id().set_id(1);
        }
        request.send().promise.await
    });
    assert_honest("deleteAlarm", deleted.map(|_| ()));

    let svc = service.clone();
    let svc = service.clone();
    let read = futures::executor::block_on(async move {
        let mut request = svc.get_alarm_request();
        {
            let mut params = request.get();
            params.reborrow().init_id().set_id(1);
        }
        request.send().promise.await
    });
    assert_honest("getAlarm", read.map(|_| ()));
}

// ═══════════════════════════════════════════════════════════════════════
// The registry
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_registry_publishes_every_method_of_the_clock_domain() {
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
            "clock info",
            "clock now",
            "clock monotonic",
            "clock uptime",
            "clock resolution",
            "clock sync",
            "clock security",
            "clock domains",
            "clock quality",
            "clock drift",
            "clock sleep",
            "clock sleep-cancellable",
            "clock timer create",
            "clock timer check",
            "clock timer reset",
            "clock timer cancel",
            "clock timers",
            "clock persist-timer",
            "clock set-timer-dependency",
            "clock watch",
            "clock timer pull",
            "clock subscription cancel",
            "clock timer pull-paginated",
            "clock virtual create",
            "clock virtual advance",
            "clock alarms",
            "clock alarm list",
            "clock alarm next",
            "clock alarm get",
            "clock alarm create",
            "clock alarm update",
            "clock alarm snooze",
            "clock alarm dismiss",
            "clock alarm delete",
        ],
        "the published surface is the domain's own"
    );
    // The subscription's methods carry the interface that declares them.
    let pull = published
        .iter()
        .find(|entry| entry.0 == "clock timer pull")
        .expect("the subscription's pull must be published");
    assert_eq!(pull.1, "TimerSubscription");
    assert_eq!(pull.2, "pull");
    // The alarm service's methods carry ITS interface.
    let list = published
        .iter()
        .find(|entry| entry.0 == "clock alarm list")
        .expect("the alarm list must be published");
    assert_eq!(list.1, "AlarmService");
    // The composition: "clock watch" needs the pull, the cancel and the
    // paginated pull.
    let watch = published
        .iter()
        .find(|entry| entry.0 == "clock watch")
        .expect("the watch must be published");
    assert_eq!(watch.4.as_deref(), Some("pull,cancel,pullPaginated"));
    // The method's identity: the ordinal the schema declares.
    assert_eq!(published[0].3.as_deref(), Some("0"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_clock_domain_has_no_unavailable_command() {
    // Every row of the clock matrix is served here: nothing of this
    // domain's surface is missing, so the unavailable list is empty.
    let (real, degraded, unavailable) = availability::status_counts();
    assert_eq!(unavailable, 0);
    assert_eq!(real + degraded, 30, "the matrix's own rows");

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
    assert_eq!(count, 0, "nothing of the clock surface is unavailable");

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The discovery descriptor
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_discovery_publishes_the_clock_domains_descriptor() {
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
                    filter.set_name("clock");
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

    assert_eq!(published.0, "clock");
    assert_eq!(published.1, "clock");
    assert_eq!(published.2, availability::CLOCK_SCHEMA_ID);
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
        31,
        "the surface must name every contract method: {methods:?}"
    );
    for expected in ["info", "monotonicNow", "realtimeNow", "createTimer", "cancelTimer", "resetTimer", "checkTimer", "subscribeTimers", "sleep", "listClockDomains", "sleepCancellable", "getClockSync", "getClockDrift", "createVirtualClock", "advanceVirtualClock", "getClockQuality", "persistTimer", "listPersistedTimers", "setTimerDependency", "alarms", "pull", "cancel", "pullPaginated", "createAlarm", "updateAlarm", "deleteAlarm", "getAlarm", "listAlarms", "snoozeAlarm", "dismissAlarm", "getNextAlarm"] {
        assert!(
            methods.iter().any(|name| name == expected),
            "the surface must name {expected}: {methods:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
