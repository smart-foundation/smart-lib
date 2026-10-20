//! The time domain, served through the contract — the spine's own tests.
//!
//! Every test drives the service the way a *contract-only* client would:
//! through the generated `Client` of `time.capnp`'s own `Time` interface.

#![cfg(all(feature = "rpc", feature = "std", feature = "domain-time"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::time::availability;
use smart_lib::platforms::linux::time::LinuxTimeBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::schema::time_capnp::{self as tcapnp, time as tm};
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::service::Service;
use smart_lib::serve::time::TimeDomain;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{
    assert_honest, metadata_of, refusal_reason, run_client, socket_path, text,
};

/// The service under test: the time domain plus the foundation interfaces.
fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::TIME_SCHEMA_ID,
        name: "smart-protocol/time".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = TimeDomain::new(Arc::new(LinuxTimeBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-time.sock",
    )]);
    Arc::new(service)
}

/// A contract-only client of the `Time` interface, in process.
fn time_client() -> tm::Client {
    let server = smart_lib::serve::time::TimeService::new(
        Arc::new(LinuxTimeBackend::new()),
        CapabilityClass::Control,
    );
    capnp_rpc::new_client(server)
}

// ═══════════════════════════════════════════════════════════════════════
// Every contract method answers, or refuses with a reason
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn every_method_of_the_time_interface_answers_or_refuses_honestly() {
    // getDateTime @0 — the host's own reading.
    let client = time_client();
    let answered = futures::executor::block_on(async move {
        let request = client.get_date_time_request();
        request.send().promise.await
    })
    .expect("the reading answers");
    let reading = answered
        .get()
        .expect("the results")
        .get_date_time()
        .expect("the reading");
    // A real reading: a year in this century, not a zero.
    assert!(
        (2000..2100).contains(&reading.get_year()),
        "the reading must be a real year: {}",
        reading.get_year()
    );

    // convertCalendar @2 — the contract's own calendar arithmetic.
    let client = time_client();
    let converted = futures::executor::block_on(async move {
        let mut request = client.convert_calendar_request();
        {
            let mut params = request.get();
            params.set_from_system(tcapnp::CalendarSystem::Gregorian);
            params.set_to_system(tcapnp::CalendarSystem::Julian);
            let mut date = params.reborrow().init_date();
            date.set_year(2024);
            date.set_month(1);
            date.set_day(31);
            date.set_calendar(tcapnp::CalendarSystem::Gregorian);
        }
        request.send().promise.await
    });
    // The backend answers or refuses with its own reason; what it must never
    // do is fall through to the generated default.
    assert_honest("convertCalendar", converted.map(|_| ()));

    // addCalendarUnits @3 / subtractCalendarUnits @4 — the contract's own
    // arithmetic.
    let client = time_client();
    let added = futures::executor::block_on(async move {
        let mut request = client.add_calendar_units_request();
        {
            let mut params = request.get();
            {
                let mut date = params.reborrow().init_date();
                date.set_year(2024);
                date.set_month(1);
                date.set_day(1);
                date.set_calendar(tcapnp::CalendarSystem::Gregorian);
            }
            {
                let mut units = params.reborrow().init_units(1);
                let mut unit = units.reborrow().get(0);
                unit.set_kind(tcapnp::CalendarUnitKind::Days);
                unit.set_amount(1);
            }
        }
        request.send().promise.await
    });
    assert_honest("addCalendarUnits", added.map(|_| ()));

    let client = time_client();
    let subtracted = futures::executor::block_on(async move {
        let mut request = client.subtract_calendar_units_request();
        {
            let mut params = request.get();
            {
                let mut date = params.reborrow().init_date();
                date.set_year(2024);
                date.set_month(3);
                date.set_day(1);
                date.set_calendar(tcapnp::CalendarSystem::Gregorian);
            }
            {
                let mut units = params.reborrow().init_units(1);
                let mut unit = units.reborrow().get(0);
                unit.set_kind(tcapnp::CalendarUnitKind::Days);
                unit.set_amount(1);
            }
        }
        request.send().promise.await
    });
    assert_honest("subtractCalendarUnits", subtracted.map(|_| ()));

    // diffCalendarDates @5 — the contract's own difference.
    let client = time_client();
    let difference = futures::executor::block_on(async move {
        let mut request = client.diff_calendar_dates_request();
        {
            let mut params = request.get();
            let mut first = params.reborrow().init_date1();
            first.set_year(2024);
            first.set_month(1);
            first.set_day(1);
            first.set_calendar(tcapnp::CalendarSystem::Gregorian);
            let mut second = params.reborrow().init_date2();
            second.set_year(2024);
            second.set_month(1);
            second.set_day(11);
            second.set_calendar(tcapnp::CalendarSystem::Gregorian);
        }
        request.send().promise.await
    })
    .expect("the difference answers");
    let diff = difference
        .get()
        .expect("the results")
        .get_diff()
        .expect("the difference");
    // The difference is the first date minus the second: -10 days.
    assert_eq!(diff.get_total_days(), -10);

    // verifyHsmTimestamp @1 — the matrix marks it unavailable on this host.
    let client = time_client();
    let verified = futures::executor::block_on(async move {
        let mut request = client.verify_hsm_timestamp_request();
        {
            let mut params = request.get();
            let mut hsm = params.init_hsm();
            hsm.reborrow().init_timestamp().set_nanoseconds(1);
            hsm.set_hardware_id("test-hsm");
        }
        request.send().promise.await
    });
    // The honest answer: the matrix's own reason, or the backend's own.
    // Never the generated default.
    assert_honest("verifyHsmTimestamp", verified.map(|_| ()));
}

#[test]
fn the_unavailable_method_answers_the_matrix_reason() {
    let client = time_client();
    let client = time_client();
    let refused = futures::executor::block_on(async move {
        let mut request = client.verify_hsm_timestamp_request();
        {
            let mut params = request.get();
            let mut hsm = params.init_hsm();
            hsm.reborrow().init_timestamp().set_nanoseconds(1);
            hsm.set_hardware_id("test-hsm");
        }
        request.send().promise.await
    });
    let reason = refusal_reason(refused.map(|_| ()))
        .expect("the matrix says this host cannot verify an HSM attestation");
    assert!(
        reason.contains("notSupported:"),
        "the refusal must ride the contract's own code token: {reason}"
    );
    assert!(
        reason.contains("not-applicable:"),
        "the refusal must carry the matrix's reason class: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// The registry
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_registry_publishes_every_method_of_the_time_domain() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = cli.list_commands_request();
                {
                    let mut params = request.get();
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
                registration.get_unavailable_reason().expect("the reason"),
                metadata_of(registration.get_metadata().ok(), "methodId"),
                metadata_of(registration.get_metadata().ok(), "composes"),
            ));
        }
        entries
    });

    // Every method of the contract's `Time` interface is published — none
    // hidden, none invented.
    let paths: Vec<&str> = published.iter().map(|entry| entry.0.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "time now",
            "time cal",
            "time add",
            "time sub",
            "time diff",
        ],
        "the available commands are the domain's own surface"
    );

    // The presentation is the contract's own: the domain, the summary, the
    // order and whether a bare call is meaningful.
    let first = &published[0];
    assert_eq!(first.1, "Time");
    assert_eq!(first.2, "getDateTime");
    assert_eq!(
        first.3,
        smart_lib::schema::cli_capnp::AvailabilityReason::Available,
        "an available command must not carry an unavailable reason"
    );
    // The method's identity: the ordinal the schema declares.
    assert_eq!(first.4.as_deref(), Some("0"));
    // "time cal" needs today's date as well as the conversion.
    assert_eq!(published[1].5.as_deref(), Some("getDateTime"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_unavailable_command_is_published_with_the_matrix_reason() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = cli.list_unavailable_request();
                {
                    let mut params = request.get();
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
                text(registration.get_method_name().expect("the method")),
                registration.get_unavailable_reason().expect("the reason"),
                metadata_of(registration.get_metadata().ok(), "reason"),
            ));
        }
        entries
    });

    assert_eq!(published.len(), 1, "one command cannot run here");
    assert_eq!(published[0].0, "time verify-hsm");
    assert_eq!(published[0].1, "verifyHsmTimestamp");
    // The contract's own reason code.
    assert_eq!(
        published[0].2,
        smart_lib::schema::cli_capnp::AvailabilityReason::UnsupportedPlatform
    );
    // And the engine's own words: the matrix's reason class and reason.
    let reason = published[0].3.clone().expect("the reason must be published");
    assert!(
        reason.starts_with("not-applicable:"),
        "the reason must be the matrix's own: {reason}"
    );

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The discovery descriptor
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_discovery_publishes_the_time_domains_descriptor() {
    let service = build_service();
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.lookup_request();
                {
                    let mut params = request.get();
                    let mut filter = params.init_filter();
                    filter.set_name("time");
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
            text(endpoints.get(0).get_transport().expect("the transport")),
            metadata_of(descriptor.get_metadata().ok(), "capabilities"),
            metadata_of(descriptor.get_metadata().ok(), "callModel"),
        )
    });

    assert_eq!(published.0, "time");
    assert_eq!(published.1, "time");
    assert_eq!(published.2, availability::TIME_SCHEMA_ID);
    // The state is the domain's OWN: one method of the six cannot run here,
    // so the service is degraded — and the missing method is named in the
    // registry.
    assert_eq!(
        published.3,
        smart_lib::schema::discovery_capnp::ServiceState::Degraded
    );
    assert_eq!(published.4, 1);
    assert_eq!(published.5, "unix");
    // The domain's own capability tokens, in the descriptor's open metadata.
    let tokens = published.6.expect("the capability tokens must be published");
    assert!(
        tokens.contains("time.datetime"),
        "the tokens must be the domain's own: {tokens}"
    );
    assert_eq!(published.7.as_deref(), Some(smart_lib::serve::CALL_MODEL));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_domains_availability_matrix_is_the_engines_own() {
    // The split the registry publishes, from the engine's own matrix.
    let (real, degraded, unavailable) = availability::status_counts();
    // The matrix carries EIGHT rows: the six contract methods of the `Time`
    // interface plus the two TAI converters this domain mirrors for callers
    // that hold only a `Time` (the contract declares those in
    // `timezone.capnp`, which this schema does not serve).
    assert_eq!(real + degraded + unavailable, 8, "the matrix's own rows");
    assert_eq!(unavailable, 1, "the HSM attestation is the one");
    assert_eq!(degraded, 1, "the calendar conversion is degraded");
    assert_eq!(real, 6);
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
        6,
        "the surface must name every contract method: {methods:?}"
    );
    for expected in ["getDateTime", "verifyHsmTimestamp", "convertCalendar", "addCalendarUnits", "subtractCalendarUnits", "diffCalendarDates"] {
        assert!(
            methods.iter().any(|name| name == expected),
            "the surface must name {expected}: {methods:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
