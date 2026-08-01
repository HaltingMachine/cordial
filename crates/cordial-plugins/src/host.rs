//! Launching a plugin and talking to it.
//!
//! A plugin is a Deno process. That gives two independent layers of containment
//! rather than one: Deno's own permission model, and Cordial's capability
//! broker. The Deno process is started with **no permissions at all** — no file,
//! network, environment or subprocess access — so a plugin cannot reach the
//! machine even if the broker had a hole in it. Everything it is allowed to do
//! arrives over stdio and is checked by the broker first.
//!
//! This is ADR-003 made concrete: plugins are isolated by process, and the only
//! channel is a named, brokered one.

use crate::broker::Broker;
use crate::protocol::{required_capability, Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct Plugin {
    pub id: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Plugin {
    /// Start a plugin from an entry module.
    ///
    /// `--no-prompt` matters as much as the absence of allow flags: without it
    /// Deno would *ask* for a permission on first use, and a plugin host has
    /// nobody to ask. With it, an attempt to touch the filesystem fails
    /// immediately instead of hanging on a prompt nothing will answer.
    pub fn spawn(id: &str, entry: &Path) -> std::io::Result<Self> {
        let mut child = Command::new("deno")
            .arg("run")
            .arg("--no-prompt")
            .arg("--quiet")
            .arg(entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
        Ok(Plugin { id: id.to_string(), child, stdin, stdout })
    }

    /// Read one request from the plugin. `None` at end of stream.
    pub fn next_request(&mut self) -> Option<Result<Request, String>> {
        let mut line = String::new();
        match self.stdout.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(serde_json::from_str(line.trim()).map_err(|e| e.to_string())),
            Err(e) => Some(Err(e.to_string())),
        }
    }

    pub fn reply(&mut self, response: &Response) -> std::io::Result<()> {
        let line = serde_json::to_string(response).expect("Response always serialises");
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Decide what a request gets, without performing any effect.
///
/// Kept separate from dispatch so the decision is testable on its own, and so
/// the check cannot be accidentally skipped by a future handler that forgets to
/// call it — a handler receives an already-authorised request or nothing.
pub fn authorise(broker: &mut Broker, plugin: &str, req: &Request) -> Result<(), Response> {
    match required_capability(&req.method) {
        None => Err(Response::Error {
            id: req.id,
            message: format!("unknown method {:?}", req.method),
        }),
        Some(cap) if !broker.allows(plugin, cap) => {
            Err(Response::Denied { id: req.id, capability: cap.name().to_string() })
        }
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    fn req(method: &str) -> Request {
        Request { id: 1, method: method.into(), params: serde_json::Value::Null }
    }

    #[test]
    fn an_authorised_call_passes() {
        let mut b = Broker::new();
        b.grant("p", [Capability::FlagsRead]);
        assert!(authorise(&mut b, "p", &req("flags.list")).is_ok());
    }

    #[test]
    fn an_unauthorised_call_is_denied_by_name() {
        let mut b = Broker::new();
        b.grant("p", [Capability::FlagsRead]);
        match authorise(&mut b, "p", &req("flags.set")) {
            Err(Response::Denied { capability, .. }) => assert_eq!(capability, "flags.write"),
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_method_is_an_error_not_a_denial() {
        // A typo must not look like a missing permission, or the author goes
        // hunting for a capability that was never the problem.
        let mut b = Broker::new();
        b.grant("p", Capability::all().iter().copied());
        match authorise(&mut b, "p", &req("flags.nonsense")) {
            Err(Response::Error { message, .. }) => assert!(message.contains("unknown method")),
            other => panic!("expected an error, got {other:?}"),
        }
    }
}
