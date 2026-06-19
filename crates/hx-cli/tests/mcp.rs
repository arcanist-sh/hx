//! Protocol-level e2e for the `hx mcp` server.
//!
//! The unit tests in `commands::mcp` exercise `handle()` in-process. This drives
//! the real binary over stdio, covering what those can't: newline framing,
//! request multiplexing, the notification (no-reply) path, non-JSON noise
//! tolerance, and process startup/shutdown on stdin EOF.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

#[test]
fn mcp_server_speaks_jsonrpc_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hx"))
        .arg("mcp")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `hx mcp`");

    // A batch of newline-delimited messages. Two of them — the notification and
    // the non-JSON line — must produce no reply, so we expect five responses.
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, // no reply
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"bogus/method"}"#,
        // Malformed tool call: missing the required `package` arg. Exercises the
        // tools/call dispatch + error path without spawning a subprocess.
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"hx_add","arguments":{}}}"#,
        "this is not json", // ignored as noise
    ];

    // Write everything, then close stdin so the server's read loop hits EOF and
    // the process exits. Output is small (well under the pipe buffer), so
    // writing fully before reading cannot deadlock.
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for r in requests {
            writeln!(stdin, "{r}").expect("write request");
        }
    }
    drop(child.stdin.take());

    let stdout = child.stdout.take().expect("stdout");
    let responses: Vec<Value> = BufReader::new(stdout)
        .lines()
        .map(|l| l.expect("read line"))
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(&l).expect("each stdout line is valid JSON"))
        .collect();

    let status = child.wait().expect("wait for `hx mcp`");
    assert!(status.success(), "server exited with {status}");

    // The notification and the non-JSON line are silent: five replies, no more.
    assert_eq!(responses.len(), 5, "unexpected responses: {responses:#?}");

    let by_id = |id: i64| {
        responses
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("no response with id {id}"))
    };

    // initialize -> server identity + protocol version.
    let init = by_id(1);
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["result"]["serverInfo"]["name"], "hx");
    assert!(init["result"]["protocolVersion"].is_string());
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // tools/list -> every tool, each with a schema.
    let tools = by_id(2)["result"]["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert_eq!(tools.len(), 14);
    assert!(tools.iter().any(|t| t["name"] == "hx_build"));
    assert!(tools.iter().any(|t| t["name"] == "hx_outdated"));
    assert!(
        tools
            .iter()
            .all(|t| t["name"].is_string() && t["inputSchema"].is_object())
    );

    // ping -> empty result object.
    assert!(by_id(3)["result"].is_object());

    // Unknown method -> JSON-RPC method-not-found.
    assert_eq!(by_id(4)["error"]["code"], -32601);

    // Malformed tool call -> invalid-params, and no subprocess was launched.
    assert_eq!(by_id(5)["error"]["code"], -32602);
}
