//! Runs the real `plugins/flag-inspector` plugin against a real broker, with
//! real flag data, and checks it gets the answers the design says it should.
//!
//! This is the end-to-end proof for the plugin path: manifest parsed from disk,
//! grant applied, Deno process spawned with no permissions, brokered calls
//! served, and a capability it did not request refused by name.

use cordial_plugins::broker::Broker;
use cordial_plugins::capability::Capability;
use cordial_plugins::host::{authorise, Plugin};
use cordial_plugins::{grants, manifest};
use cordial_plugins::protocol::Response;
use std::path::PathBuf;

#[test]
fn the_flag_inspector_plugin_works_end_to_end() {
    if std::process::Command::new("deno").arg("--version").output().is_err() {
        eprintln!("skipping: deno is not installed");
        return;
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
    let found = manifest::discover(&root);
    let plugin = found
        .iter()
        .find(|p| p.manifest.id == "flag-inspector")
        .expect("the shipped example plugin should be discoverable");

    assert!(plugin.requested.contains(&Capability::FlagsRead));
    assert!(
        !plugin.requested.contains(&Capability::FlagsWrite),
        "the example deliberately does not request write, so the refusal is demonstrable"
    );

    // Grant exactly what it asked for — the user's decision, expressed the way
    // plugin-grants.json expresses it.
    let approved = grants::parse(r#"{"flag-inspector":["flags.read","log"]}"#).unwrap();
    let mut broker = Broker::new();
    broker.grant("flag-inspector", approved["flag-inspector"].iter().copied());

    let entry = plugin.entry_path().expect("entry should resolve inside the plugin dir");
    let mut proc = Plugin::spawn("flag-inspector", &entry).expect("deno should start");

    let mut logs: Vec<String> = Vec::new();
    while let Some(req) = proc.next_request() {
        let Ok(req) = req else { break };
        match authorise(&mut broker, "flag-inspector", &req) {
            Err(refusal) => proc.reply(&refusal).unwrap(),
            Ok(()) => {
                let result = match req.method.as_str() {
                    // Stand-in for the runtime's real resolver; shape is what matters.
                    "flags.list" => serde_json::json!([
                        {"key": "DFFlagRbxTransportUseRtcioRna", "value": "false", "source": "user"},
                        {"key": "FIntTaskSchedulerAutoThreadLimit", "value": "8", "source": "plugin:tuner"}
                    ]),
                    "log.write" => {
                        logs.push(req.params["message"].as_str().unwrap_or_default().to_string());
                        serde_json::Value::Null
                    }
                    _ => serde_json::Value::Null,
                };
                proc.reply(&Response::Ok { id: req.id, result }).unwrap();
            }
        }
        if logs.iter().any(|l| l.starts_with("writing a flag came back")) {
            break;
        }
    }
    proc.kill();

    let joined = logs.join("\n");
    assert!(joined.contains("2 flag override(s) in effect"), "got:\n{joined}");
    assert!(joined.contains("DFFlagRbxTransportUseRtcioRna = false  (from user)"), "got:\n{joined}");
    assert!(joined.contains("(from plugin:tuner)"), "got:\n{joined}");
    assert!(
        joined.contains("writing a flag came back: denied (needs flags.write)"),
        "an ungranted write must be refused by name; got:\n{joined}"
    );
}
