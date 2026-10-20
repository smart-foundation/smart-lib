//! The process domain, served through the contract — the spine's own tests.

#![cfg(all(feature = "rpc", feature = "std", feature = "domain-process"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::process::availability;
use smart_lib::platforms::linux::process::LinuxProcessBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::schema::process_capnp::process as proc;
use smart_lib::schema::types_capnp;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::process::ProcessDomain;
use smart_lib::serve::service::Service;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{assert_honest, metadata_of, run_client, socket_path, text};

/// The service under test: the process domain plus the foundation interfaces.
fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::PROCESS_SCHEMA_ID,
        name: "smart-protocol/process".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = ProcessDomain::new(Arc::new(LinuxProcessBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-process.sock",
    )]);
    Arc::new(service)
}

/// A contract-only client of the `Process` interface, in process.
fn process_client() -> proc::Client {
    let server = smart_lib::serve::process::ProcessService::new(
        Arc::new(LinuxProcessBackend::new()),
        CapabilityClass::Control,
    );
    capnp_rpc::new_client(server)
}

/// One contract call over the `Process` interface.
macro_rules! call {
    ($method:ident, |$params:ident| $body:block) => {{
        let client = process_client();
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

/// This process's own pid: a pid the host really has.
fn own_pid() -> u64 {
    u64::from(std::process::id())
}

// ═══════════════════════════════════════════════════════════════════════
// Every contract method answers, or refuses with a reason
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn every_method_of_the_process_interface_answers_or_refuses_honestly() {
    let pid = own_pid();

    // listProcesses @0 — the processes this host is running.
    let listed = call!(list_processes_request, |params| {
        let mut params = params;
        let mut filter = params.reborrow().init_filter();
        filter.set_state(types_capnp::ExecutionState::Unknown);
        let mut page = params.init_page();
        page.set_offset(0);
        page.set_limit(8);
    });
    assert_honest("listProcesses", listed.map(|_| ()));

    // getProcess @1 — one process's own record.
    let read = call!(get_process_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getProcess", read.map(|_| ()));

    // getProcessStatus @8 — one process's status.
    let status = call!(get_process_status_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getProcessStatus", status.map(|_| ()));

    // getThreads @9 — the threads of one process.
    let threads = call!(get_threads_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut page = params.init_page();
        page.set_offset(0);
        page.set_limit(8);
    });
    assert_honest("getThreads", threads.map(|_| ()));

    // getMemoryMap @10 — one process's memory map.
    let map = call!(get_memory_map_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut page = params.init_page();
        page.set_offset(0);
        page.set_limit(8);
    });
    assert_honest("getMemoryMap", map.map(|_| ()));

    // getEnvironment @4 — one process's environment.
    let environment = call!(get_environment_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getEnvironment", environment.map(|_| ()));

    // getStats @15 — the aggregate statistics.
    let stats = call!(get_stats_request, |params| {
        let mut params = params;
        params.reborrow().init_user().set_id(0);
    });
    assert_honest("getStats", stats.map(|_| ()));

    // getAccounting @16 — one process's resource accounting.
    let accounting = call!(get_accounting_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getAccounting", accounting.map(|_| ()));

    // getResourceAccounting @17 — the accounting over a window.
    let window = call!(get_resource_accounting_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.init_duration().set_nanoseconds(1_000_000);
    });
    assert_honest("getResourceAccounting", window.map(|_| ()));

    // listProcessGroups @18 / getProcessGroup @19 — the groups.
    let groups = call!(list_process_groups_request, |params| {
        let mut params = params;
        let mut page = params.init_page();
        page.set_offset(0);
        page.set_limit(8);
    });
    assert_honest("listProcessGroups", groups.map(|_| ()));

    let group = call!(get_process_group_request, |params| {
        let mut params = params;
        params.reborrow().init_group_id().set_id(1);
    });
    assert_honest("getProcessGroup", group.map(|_| ()));

    // getResourceLimits @24 — one process's declared limits.
    let limits = call!(get_resource_limits_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getResourceLimits", limits.map(|_| ()));

    // getExecutionContext @26 — one process's execution context.
    let context = call!(get_execution_context_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getExecutionContext", context.map(|_| ()));

    // getProcessTree @28 — the tree one process belongs to.
    let tree = call!(get_process_tree_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.set_max_depth(2);
    });
    assert_honest("getProcessTree", tree.map(|_| ()));

    // getProcessRespawnStatus @32 — the respawn status.
    let respawn = call!(get_process_respawn_status_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("getProcessRespawnStatus", respawn.map(|_| ()));

    // listBreakpoints @34 / listWatchpoints @38 — the recorded debug state.
    let breakpoints = call!(list_breakpoints_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("listBreakpoints", breakpoints.map(|_| ()));

    let watchpoints = call!(list_watchpoints_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("listWatchpoints", watchpoints.map(|_| ()));

    // readMemory @13 — the read, with the guarantee named.
    let memory = call!(read_memory_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.set_address(0);
        params.set_size(4);
    });
    assert_honest("readMemory", memory.map(|_| ()));

    // wait @3 — waiting for a pid that is not going to exit.
    let waited = call!(wait_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.init_timeout().set_nanoseconds(1_000_000);
    });
    assert_honest("wait", waited.map(|_| ()));

    // The verbs the matrix marks UNAVAILABLE on this host: they answer the
    // matrix's own reason, never a fabricated success.
    let attached = call!(attach_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("attach", attached.map(|_| ()));

    let written = call!(write_memory_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.set_address(0);
        params.reborrow().init_data().set_inline(b"x");
    });
    assert_honest("writeMemory", written.map(|_| ()));

    let created_group = call!(create_process_group_request, |params| {
        let mut params = params;
        params.set_name("test-group");
    });
    assert_honest("createProcessGroup", created_group.map(|_| ()));

    let added = call!(add_to_group_request, |params| {
        let mut params = params;
        params.reborrow().init_group_id().set_id(1);
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("addToGroup", added.map(|_| ()));

    let removed = call!(remove_from_group_request, |params| {
        let mut params = params;
        params.reborrow().init_group_id().set_id(1);
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("removeFromGroup", removed.map(|_| ()));

    let foreground = call!(set_foreground_group_request, |params| {
        let mut params = params;
        params.reborrow().init_group_id().set_id(1);
    });
    assert_honest("setForegroundGroup", foreground.map(|_| ()));

    let limit = call!(set_resource_limit_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut quota = params.init_limit();
        quota.set_memory_bytes(1024);
        quota.set_pids(4);
    });
    assert_honest("setResourceLimit", limit.map(|_| ()));

    let context_written = call!(set_execution_context_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut context = params.init_context();
        context.reborrow().init_pid().set_id(pid);
    });
    assert_honest("setExecutionContext", context_written.map(|_| ()));

    let supervised = call!(register_supervisor_request, |params| {
        let mut params = params;
        params.reborrow().init_supervisor().set_id(pid);
        params.reborrow().init_target().set_id(pid);
    });
    assert_honest("registerSupervisor", supervised.map(|_| ()));

    let respawned = call!(respawn_process_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("respawnProcess", respawned.map(|_| ()));

    let exited = call!(exit_process_request, |params| {
        let mut params = params;
        let mut reason = params.init_reason();
        reason.set_reason(types_capnp::FailureReason::Timeout);
    });
    assert_honest("exitProcess", exited.map(|_| ()));

    let stepped = call!(step_process_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("stepProcess", stepped.map(|_| ()));

    let continued = call!(continue_process_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("continueProcess", continued.map(|_| ()));

    // The verbs that are served here.
    let spawned = call!(spawn_request, |params| {
        let mut params = params;
        let mut spec = params.init_spec();
        spec.set_image_ref("/bin/true");
    });
    assert_honest("spawn", spawned.map(|_| ()));

    let environment_written = call!(set_environment_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut env = params.init_env();
        let mut entries = env.init_entries(1);
        let mut entry = entries.reborrow().get(0);
        entry.set_key("SMART_LIB_TEST");
        entry.set_value("1");
    });
    assert_honest("setEnvironment", environment_written.map(|_| ()));

    let detached = call!(detach_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("detach", detached.map(|_| ()));

    let reported = call!(report_process_failure_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut report = params.init_report();
        report.set_reason(types_capnp::FailureReason::Timeout);
        report.set_severity(types_capnp::FailureSeverity::Recoverable);
    });
    assert_honest("reportProcessFailure", reported.map(|_| ()));

    let breakpoint_set = call!(set_breakpoint_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut breakpoint = params.init_breakpoint();
        breakpoint.set_address(0);
        breakpoint.set_enabled(true);
    });
    assert_honest("setBreakpoint", breakpoint_set.map(|_| ()));

    let breakpoint_removed = call!(remove_breakpoint_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.reborrow().init_id().set_id(1);
    });
    assert_honest("removeBreakpoint", breakpoint_removed.map(|_| ()));

    let watchpoint_set = call!(set_watchpoint_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        let mut watchpoint = params.init_watchpoint();
        watchpoint.set_address(0);
        watchpoint.set_size(4);
        watchpoint.set_enabled(true);
    });
    assert_honest("setWatchpoint", watchpoint_set.map(|_| ()));

    let watchpoint_removed = call!(remove_watchpoint_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
        params.reborrow().init_id().set_id(1);
    });
    assert_honest("removeWatchpoint", watchpoint_removed.map(|_| ()));

    // The two subscriptions.
    let subscribed = call!(subscribe_process_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("subscribeProcess", subscribed.map(|_| ()));

    let death = call!(subscribe_death_request, |params| {
        let mut params = params;
        params.reborrow().init_pid().set_id(pid);
    });
    assert_honest("subscribeDeath", death.map(|_| ()));
}

#[test]
fn the_subscriptions_are_the_contracts_own_capabilities() {
    let pid = own_pid();
    let client = process_client();
    let subscribed = futures::executor::block_on(async move {
        let mut request = client.subscribe_process_request();
        {
            let mut params = request.get();
            params.reborrow().init_pid().set_id(pid);
        }
        request.send().promise.await
    })
    .expect("the subscription answers");
    let subscription = subscribed
        .get()
        .expect("the results")
        .get_subscription()
        .expect("a subscription capability");

    // The reference IS the authority to pull.
    let puller = subscription.clone();
    let pulled = futures::executor::block_on(async move {
        let mut request = puller.pull_request();
        {
            let mut params = request.get();
            params.set_max_infos(4);
        }
        request.send().promise.await
    })
    .expect("the pull answers");
    let info = pulled
        .get()
        .expect("the results")
        .get_info()
        .expect("the info");
    assert!(
        info.get_pid().expect("the pid").get_id() == pid,
        "the pull answers the process it was subscribed to"
    );

    let cancelled = futures::executor::block_on(async move {
        let request = subscription.cancel_request();
        request.send().promise.await
    });
    assert_honest("ProcessSubscription.cancel", cancelled.map(|_| ()));

    // The exit-notification subscription.
    let client = process_client();
    let death = futures::executor::block_on(async move {
        let mut request = client.subscribe_death_request();
        {
            let mut params = request.get();
            params.reborrow().init_pid().set_id(pid);
        }
        request.send().promise.await
    })
    .expect("the subscription answers");
    let subscription = death
        .get()
        .expect("the results")
        .get_subscription()
        .expect("a subscription capability");
    let puller = subscription.clone();
    let pulled = futures::executor::block_on(async move {
        let mut request = puller.pull_request();
        {
            let mut params = request.get();
            params.set_max_notifications(4);
        }
        request.send().promise.await
    });
    assert_honest("ExitNotificationSubscription.pull", pulled.map(|_| ()));

    let cancelled = futures::executor::block_on(async move {
        let request = subscription.cancel_request();
        request.send().promise.await
    });
    assert_honest("ExitNotificationSubscription.cancel", cancelled.map(|_| ()));
}

#[test]
fn the_unavailable_verbs_answer_the_matrix_reason() {
    let pid = own_pid();
    let client = process_client();
    let refused = futures::executor::block_on(async move {
        let mut request = client.attach_request();
        {
            let mut params = request.get();
            params.reborrow().init_pid().set_id(pid);
        }
        request.send().promise.await
    });
    let reason = serve_common::refusal_reason(refused.map(|_| ()))
        .expect("the matrix says this host cannot attach to a process");
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
fn the_registry_publishes_every_method_of_the_process_domain() {
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

    // Every method of the contract's `Process` interface that can run here
    // is published. The thirteen the matrix marks unavailable are published
    // in the unavailable list instead.
    let paths: Vec<&str> = published.iter().map(|entry| entry.0.as_str()).collect();
    for expected in [
        "process list",
        "process inspect",
        "process status",
        "process tree",
        "process threads",
        "process env",
        "process maps",
        "process limits",
        "process accounting",
        "process resource-accounting",
        "process groups",
        "process group",
        "process stats",
        "process context",
        "process spawn",
        "process wait",
        "process set-env",
        "process detach",
        "process read-memory",
        "process watch",
        "process watch-exit",
        "process report-failure",
        "process respawn-status",
        "process breakpoint set",
        "process breakpoint remove",
        "process breakpoint list",
        "process watchpoint set",
        "process watchpoint remove",
        "process watchpoint list",
        "process pull",
        "process subscription cancel",
        "process exit pull",
        "process exit cancel",
    ] {
        assert!(
            paths.contains(&expected),
            "the surface must not hide {expected}: {paths:?}"
        );
    }
    // The composition: "process inspect" is the whole read surface of one
    // process.
    let inspect = published
        .iter()
        .find(|entry| entry.0 == "process inspect")
        .expect("the inspect must be published");
    assert_eq!(inspect.4.as_deref(), Some("getProcessStatus,getAccounting"));
    // The subscription's methods carry the interface that declares them.
    let pull = published
        .iter()
        .find(|entry| entry.0 == "process pull")
        .expect("the pull must be published");
    assert_eq!(pull.1, "ProcessSubscription");
    // The method's identity: the ordinal the schema declares.
    assert_eq!(published[0].3.as_deref(), Some("0"));

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

    // The thirteen verbs the process matrix marks UNAVAILABLE on this host.
    assert_eq!(published.len(), 13, "thirteen commands cannot run here");
    for entry in &published {
        assert_eq!(
            entry.1,
            smart_lib::schema::cli_capnp::AvailabilityReason::CapabilityDenied
        );
        assert!(
            entry
                .2
                .as_deref()
                .expect("the reason must be published")
                .starts_with("no-privilege:"),
            "the reason must be the matrix's own: {:?}",
            entry.2
        );
    }
    let paths: Vec<&str> = published.iter().map(|entry| entry.0.as_str()).collect();
    assert!(paths.contains(&"process attach"));
    assert!(paths.contains(&"process group create"));

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The discovery descriptor
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_discovery_publishes_the_process_domains_descriptor() {
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
                    filter.set_name("process");
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

    assert_eq!(published.0, "process");
    assert_eq!(published.1, "process");
    assert_eq!(published.2, availability::PROCESS_SCHEMA_ID);
    // Thirteen of the forty-six cannot run here, so the service is degraded.
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
    assert_eq!(real + degraded + unavailable, 46, "the matrix's own rows");
    assert_eq!(unavailable, 13, "the ptrace and setpgid walls");
    assert_eq!(real + degraded, 33);
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
        44,
        "the surface must name every contract method: {methods:?}"
    );
    for expected in ["listProcesses", "getProcess", "spawn", "wait", "getEnvironment", "setEnvironment", "subscribeProcess", "subscribeDeath", "getProcessStatus", "getThreads", "getMemoryMap", "attach", "detach", "readMemory", "writeMemory", "getStats", "getAccounting", "getResourceAccounting", "listProcessGroups", "getProcessGroup", "createProcessGroup", "addToGroup", "removeFromGroup", "setForegroundGroup", "getResourceLimits", "setResourceLimit", "getExecutionContext", "setExecutionContext", "getProcessTree", "registerSupervisor", "reportProcessFailure", "respawnProcess", "getProcessRespawnStatus", "exitProcess", "setBreakpoint", "removeBreakpoint", "listBreakpoints", "setWatchpoint", "removeWatchpoint", "listWatchpoints", "stepProcess", "continueProcess", "pull", "cancel"] {
        assert!(
            methods.iter().any(|name| name == expected),
            "the surface must name {expected}: {methods:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
