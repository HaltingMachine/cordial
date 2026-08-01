//! Proves a real Deno plugin round-trips, is refused what it lacks, and cannot
//! reach the filesystem.
//!
//! Skipped rather than failed when Deno is absent: a contributor without it
//! should still get a green `cargo test`, and a silent pass is worse than a
//! visible skip, so it says so.

use cordial_plugins::broker::Broker;
use cordial_plugins::capability::Capability;
use cordial_plugins::host::{authorise, Plugin};
use cordial_plugins::protocol::Response;
use std::path::PathBuf;

#[test]
fn a_plugin_round_trips_and_is_held_to_its_grant() {
    if std::process::Command::new("deno").arg("--version").output().is_err() {
        eprintln!("skipping: deno is not installed");
        return;
    }

    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/roundtrip.ts");
    let mut plugin = Plugin::spawn("roundtrip", &entry).expect("deno should start");

    let mut broker = Broker::new();
    // Deliberately narrow: read but not write, so the refusal is real.
    broker.grant("roundtrip", [Capability::FlagsRead, Capability::Log]);

    let mut summary = None;
    while let Some(req) = plugin.next_request() {
        let req = req.expect("the plugin should emit valid JSON");
        match authorise(&mut broker, "roundtrip", &req) {
            Err(refusal) => plugin.reply(&refusal).unwrap(),
            Ok(()) => {
                if req.method == "log.write" {
                    summary = Some(req.params.clone());
                    break;
                }
                plugin
                    .reply(&Response::Ok { id: req.id, result: serde_json::json!([]) })
                    .unwrap();
            }
        }
    }
    plugin.kill();

    let s = summary.expect("the plugin should have reported a summary");
    assert_eq!(s["granted"], "ok", "a granted call should succeed");
    assert_eq!(s["refused"], "denied", "an ungranted call should be denied");
    assert_eq!(
        s["refusedCapability"], "flags.write",
        "the denial should name the capability that was missing"
    );
    assert_eq!(s["bogus"], "error", "an unknown method is an error, not a denial");
    assert_eq!(
        s["sandboxed"], true,
        "a plugin started with no permissions must not be able to read the filesystem"
    );
}
