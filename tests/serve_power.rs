#![cfg(all(feature = "rpc", feature = "std", feature = "domain-power"))]

extern crate std;

use std::sync::Arc;

use smart_lib::domains::power::availability;
use smart_lib::platforms::linux::power::LinuxPowerBackend;
use smart_lib::schema::cli_capnp::cli_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::power::PowerDomain;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::service::Service;
use smart_lib::serve::CapabilityClass;

mod serve_common;

use serve_common::{run_client, socket_path, text};

fn build_service() -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: availability::POWER_SCHEMA_ID,
        name: "smart-protocol/power".to_string(),
        major: 0,
        minor: 1,
    });
    let domain = PowerDomain::new(Arc::new(LinuxPowerBackend::new()));
    let service = service.with_domain(Arc::new(domain));
    service.set_endpoints(vec![PublishedEndpoint::new(
        "unix",
        "/tmp/smart-lib-s1-power.sock",
    )]);
    Arc::new(service)
}

#[test]
fn registry_and_discovery_smoke() {
    let service = build_service();
    let path = socket_path();
    let count = run_client(service, &path, CapabilityClass::Control, |session| {
        let cli: cli_service::Client = session.interface();
        let response = session.run(async move {
            let mut request = cli.list_commands_request();
            {
                let params = request.get();
                let mut page = params.init_pagination();
                page.set_offset(0);
                page.set_limit(4096);
            }
            request.send().promise.await
        })
        .expect("the call answers");
        let results = response.get().expect("the results");
        results.get_commands().expect("the commands").len()
    });
    assert!(count > 0);
    let _ = std::fs::remove_file(&path);
}
