//! Proves a published event actually reaches another, real, separately
//! spawned plugin process — not just that the registry's bookkeeping is
//! correct in isolation (that is what `events.rs`'s unit tests already
//! cover), but that a `Session` really writes bytes down a real subscriber's
//! stdin when a different plugin publishes.
//!
//! `flag-manager` here is simulated from the Rust side rather than as its own
//! Deno process — it only ever declares and publishes, which are pure
//! `Session` calls with no process behind them, so spawning a second Deno
//! process to do it would test nothing this file does not already exercise
//! by calling `Session::handle` directly. `launcher` is the one that has to
//! be real: receiving a push over stdio, from a process that made no request
//! for it, is the part that cannot be faked without a genuine second process
//! on the other end of the pipe.

use cordial_plugins::capability::Capability;
use cordial_plugins::host::{Plugin, Session};
use cordial_plugins::protocol::{Request, Response};
use std::path::PathBuf;

#[test]
fn a_published_event_reaches_a_real_subscriber_process() {
    if std::process::Command::new("deno").arg("--version").output().is_err() {
        eprintln!("skipping: deno is not installed");
        return;
    }

    let mut session = Session::new();
    session.broker.grant("flag-manager", [Capability::EventsDeclare, Capability::EventsPublish]);
    session.broker.grant("launcher", [Capability::EventsSubscribe, Capability::Log]);

    // flag-manager declares its own type before anything can subscribe to
    // it — the ordering ADR-006 expects of a real dependency graph, not
    // something this test papers over.
    let declared = session.handle(
        "flag-manager",
        &Request { id: 1, method: "events.declare".into(), params: serde_json::json!({"name": "profile-changed"}) },
    );
    let event_type = match declared {
        Response::Ok { result, .. } => result["type"].as_str().unwrap().to_string(),
        other => panic!("flag-manager should have been able to declare its own type, got {other:?}"),
    };
    assert_eq!(event_type, "flag-manager/profile-changed");

    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events_subscriber.ts");
    let launcher = Plugin::spawn("launcher", &entry).expect("deno should start");
    session.add_plugin(launcher);

    let mut logs: Vec<String> = Vec::new();
    let mut published = false;
    while let Some(req) = session.plugin_mut("launcher").unwrap().next_request() {
        let Ok(req) = req else { break };

        if req.method == "log.write" {
            // Session has no broker for log.write (out of this crate's
            // scope, same as flags.* — see the Session doc comment), so the
            // test plays host for it the way flag_inspector.rs does.
            let message = req.params["message"].as_str().unwrap_or_default().to_string();
            session
                .plugin_mut("launcher")
                .unwrap()
                .reply(&Response::Ok { id: req.id, result: serde_json::Value::Null })
                .unwrap();
            logs.push(message);
            // Two log lines are expected: the subscribe echo and the push
            // report. Their relative order is not guaranteed — the push
            // arrives from a synchronous stdin handler while the subscribe
            // echo is queued as a microtask continuation, so the push can
            // legitimately be logged first — only that both eventually
            // arrive matters here.
            if logs.len() >= 2 {
                break;
            }
            continue;
        }

        let res = session.handle("launcher", &req);
        let subscribed_ok = req.method == "events.subscribe" && matches!(res, Response::Ok { .. });
        session.plugin_mut("launcher").unwrap().reply(&res).unwrap();

        if subscribed_ok && !published {
            published = true;
            // Now that the subscriber is actually registered, flag-manager
            // publishes — this is the call that should write a Push into
            // launcher's stdin from the other side of the process boundary.
            let pub_res = session.handle(
                "flag-manager",
                &Request {
                    id: 2,
                    method: "events.publish".into(),
                    params: serde_json::json!({"type": event_type, "payload": {"slot": 3}}),
                },
            );
            assert!(matches!(pub_res, Response::Ok { .. }), "publish should succeed: {pub_res:?}");
        }
    }
    session.plugin_mut("launcher").unwrap().kill();

    assert!(published, "the test should have reached the point of publishing");
    let joined = logs.join("\n");
    assert!(joined.contains("subscribed: ok"), "got:\n{joined}");
    assert!(joined.contains("push: flag-manager/profile-changed"), "got:\n{joined}");
    assert!(joined.contains(r#""slot":3"#), "got:\n{joined}");
}
