//! Minimal MCP (Model Context Protocol) server exposing hx to AI agents.
//!
//! Speaks JSON-RPC 2.0 over stdio (newline-delimited messages). Each tool
//! shells out to this same `hx` binary and returns its combined stdout/stderr
//! plus a success flag, so an MCP client (Claude, editors, etc.) can drive
//! build / test / run / lock / doctor / dependency management.
//!
//! This is intentionally a small, dependency-free implementation of the core
//! MCP methods (`initialize`, `tools/list`, `tools/call`, `ping`); it can be
//! replaced with the official Rust MCP SDK later without changing the surface.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Read newline-delimited JSON-RPC from stdin, dispatch, and reply on stdout.
pub async fn run() -> Result<i32> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let stdout = std::io::stdout();

    loop {
        // A clean EOF gives `Ok(None)`; a closed stdin pipe can also surface as
        // a read error (e.g. broken pipe on Windows). Both mean "no more input",
        // so end the server loop and exit 0 rather than propagating an error.
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue, // ignore non-JSON noise
        };
        if let Some(response) = handle(&request).await {
            let mut lock = stdout.lock();
            let _ = writeln!(
                lock,
                "{}",
                serde_json::to_string(&response).unwrap_or_default()
            );
            let _ = lock.flush();
        }
    }
    Ok(0)
}

/// Dispatch one request. Returns `None` for notifications (which get no reply).
async fn handle(request: &Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

    match method {
        "initialize" => Some(reply(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "hx", "version": env!("CARGO_PKG_VERSION") }
            }),
        )),
        // Notifications carry no id and expect no response.
        m if m.starts_with("notifications/") => None,
        "ping" => Some(reply(id, json!({}))),
        "tools/list" => Some(reply(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => Some(tools_call(id, request.get("params")).await),
        _ => id.map(|id| error(Some(id), -32601, "method not found")),
    }
}

fn reply(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

fn error(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "error": { "code": code, "message": message } })
}

async fn tools_call(id: Option<Value>, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return error(id, -32602, "missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let Some(cli_args) = build_args(name, &args) else {
        return error(
            id,
            -32602,
            &format!("unknown or malformed tool call: {name}"),
        );
    };

    let (text, is_error) = run_hx(&cli_args, args.get("cwd").and_then(Value::as_str)).await;
    reply(
        id,
        json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error
        }),
    )
}

/// Translate an MCP tool name + arguments into `hx` CLI arguments.
fn build_args(name: &str, args: &Value) -> Option<Vec<String>> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(String::from);
    let b = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(false);
    let i = |k: &str| args.get(k).and_then(Value::as_i64);

    let mut a: Vec<String> = Vec::new();
    match name {
        "hx_build" => {
            a.push("build".into());
            if b("release") {
                a.push("--release".into());
            }
            if b("native") {
                a.push("--native".into());
            }
            if let Some(j) = i("jobs") {
                a.push("-j".into());
                a.push(j.to_string());
            }
        }
        "hx_check" => a.push("check".into()),
        "hx_test" => {
            a.push("test".into());
            if let Some(p) = s("pattern") {
                a.push("--pattern".into());
                a.push(p);
            }
        }
        "hx_run" => {
            a.push("run".into());
            if let Some(extra) = args.get("args").and_then(Value::as_array) {
                a.push("--".into());
                a.extend(extra.iter().filter_map(|v| v.as_str().map(String::from)));
            }
        }
        "hx_lock" => {
            a.push("lock".into());
            if b("update") {
                a.push("--update".into());
            }
        }
        "hx_sync" => {
            a.push("sync".into());
            if b("force") {
                a.push("--force".into());
            }
        }
        "hx_fmt" => {
            a.push("fmt".into());
            if b("check") {
                a.push("--check".into());
            }
        }
        "hx_lint" => {
            a.push("lint".into());
            if b("fix") {
                a.push("--fix".into());
            }
        }
        "hx_doctor" => a.push("doctor".into()),
        "hx_add" => {
            a.push("add".into());
            a.push(s("package")?);
            if let Some(c) = s("constraint") {
                a.push(c);
            }
            if b("dev") {
                a.push("--dev".into());
            }
        }
        "hx_remove" => {
            a.push("rm".into());
            a.push(s("package")?);
        }
        "hx_info" => {
            a.push("info".into());
            a.push(s("package")?);
            if b("versions") {
                a.push("--versions".into());
            }
        }
        "hx_tree" => {
            a.push("tree".into());
            if let Some(d) = i("depth") {
                a.push("--depth".into());
                a.push(d.to_string());
            }
        }
        "hx_outdated" => {
            a.push("outdated".into());
            if b("direct") {
                a.push("--direct".into());
            }
        }
        _ => return None,
    }
    Some(a)
}

/// Run `hx <args>` in `cwd` (default: current dir), returning combined
/// stdout/stderr and whether it failed (non-zero exit).
async fn run_hx(args: &[String], cwd: Option<&str>) -> (String, bool) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return (format!("failed to locate the hx binary: {e}"), true),
    };
    let mut command = tokio::process::Command::new(exe);
    command.args(args).env("NO_COLOR", "1");
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    match command.output().await {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            if text.trim().is_empty() {
                text = format!(
                    "hx {} exited with code {}",
                    args.join(" "),
                    out.status.code().unwrap_or(-1)
                );
            }
            (text, !out.status.success())
        }
        Err(e) => (format!("failed to run hx {}: {e}", args.join(" ")), true),
    }
}

/// JSON-Schema tool definitions advertised via `tools/list`.
fn tool_definitions() -> Value {
    // Helper: build an object schema from (properties, required).
    fn schema(props: Value, required: &[&str]) -> Value {
        json!({ "type": "object", "properties": props, "required": required })
    }
    let cwd = || json!({ "type": "string", "description": "Project directory to run in (default: current directory)" });

    // Built tool-by-tool into a Vec rather than as one giant `json!([...])`
    // literal: a single huge json! expression materializes all of its nested
    // temporaries on the stack at once, which overflows the smaller default
    // thread stack on Windows (debug builds especially — the server exits with
    // STATUS_STACK_OVERFLOW, 0xC00000FD). Per-tool statements keep peak stack
    // usage to one small schema at a time.
    let mut tools = Vec::with_capacity(14);
    tools.push(json!({ "name": "hx_doctor", "description": "Diagnose the Haskell toolchain and project setup. Reports missing tools and how to fix them.", "inputSchema": schema(json!({ "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_build", "description": "Build the current Haskell project.", "inputSchema": schema(json!({ "release": {"type":"boolean","description":"Optimized release build"}, "native": {"type":"boolean","description":"Native GHC build (no cabal); simple projects only"}, "jobs": {"type":"integer","description":"Parallel jobs"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_check", "description": "Fast type-check without producing a binary.", "inputSchema": schema(json!({ "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_test", "description": "Run the project's test suite.", "inputSchema": schema(json!({ "pattern": {"type":"string","description":"Only run tests matching this pattern"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_run", "description": "Build and run the project's executable.", "inputSchema": schema(json!({ "args": {"type":"array","items":{"type":"string"},"description":"Arguments passed to the program"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_lock", "description": "Generate or update hx.lock for reproducible builds.", "inputSchema": schema(json!({ "update": {"type":"boolean","description":"Update all dependencies"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_sync", "description": "Build using the locked dependency set.", "inputSchema": schema(json!({ "force": {"type":"boolean"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_fmt", "description": "Format Haskell source (fourmolu). Set check=true to verify without modifying.", "inputSchema": schema(json!({ "check": {"type":"boolean"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_lint", "description": "Run hlint. Set fix=true to apply automatic suggestions.", "inputSchema": schema(json!({ "fix": {"type":"boolean"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_add", "description": "Add a dependency to the project.", "inputSchema": schema(json!({ "package": {"type":"string"}, "constraint": {"type":"string","description":"Version constraint, e.g. \">=2.0\""}, "dev": {"type":"boolean","description":"Add as a dev dependency"}, "cwd": cwd() }), &["package"]) }));
    tools.push(json!({ "name": "hx_remove", "description": "Remove a dependency from the project.", "inputSchema": schema(json!({ "package": {"type":"string"}, "cwd": cwd() }), &["package"]) }));
    tools.push(json!({ "name": "hx_info", "description": "Show package details from Hackage.", "inputSchema": schema(json!({ "package": {"type":"string"}, "versions": {"type":"boolean","description":"Include all available versions"}, "cwd": cwd() }), &["package"]) }));
    tools.push(json!({ "name": "hx_tree", "description": "Show the dependency tree.", "inputSchema": schema(json!({ "depth": {"type":"integer"}, "cwd": cwd() }), &[]) }));
    tools.push(json!({ "name": "hx_outdated", "description": "List outdated dependencies.", "inputSchema": schema(json!({ "direct": {"type":"boolean","description":"Only direct dependencies"}, "cwd": cwd() }), &[]) }));
    Value::Array(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_args_maps_flags() {
        assert_eq!(
            build_args("hx_build", &json!({"release": true})),
            Some(vec!["build".into(), "--release".into()])
        );
        assert_eq!(
            build_args("hx_build", &json!({"native": true, "jobs": 4})),
            Some(vec![
                "build".into(),
                "--native".into(),
                "-j".into(),
                "4".into()
            ])
        );
        assert_eq!(
            build_args("hx_check", &json!({})),
            Some(vec!["check".into()])
        );
        assert_eq!(
            build_args(
                "hx_add",
                &json!({"package": "aeson", "constraint": ">=2.0", "dev": true})
            ),
            Some(vec![
                "add".into(),
                "aeson".into(),
                ">=2.0".into(),
                "--dev".into()
            ])
        );
        assert_eq!(
            build_args("hx_run", &json!({"args": ["--foo", "bar"]})),
            Some(vec![
                "run".into(),
                "--".into(),
                "--foo".into(),
                "bar".into()
            ])
        );
        // Missing required argument and unknown tools are rejected.
        assert_eq!(build_args("hx_add", &json!({})), None);
        assert_eq!(build_args("not_a_tool", &json!({})), None);
    }

    #[tokio::test]
    async fn test_handle_protocol_methods() {
        let init = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .await
            .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "hx");
        assert!(init["result"]["capabilities"]["tools"].is_object());

        // Notifications receive no reply.
        assert!(
            handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .await
                .is_none()
        );

        // tools/list advertises every tool, each with a schema.
        let list = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
            .await
            .unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 14);
        assert!(
            tools
                .iter()
                .all(|t| t["name"].is_string() && t["inputSchema"].is_object())
        );

        // Unknown methods produce a JSON-RPC "method not found" error.
        let err = handle(&json!({"jsonrpc":"2.0","id":3,"method":"bogus"}))
            .await
            .unwrap();
        assert_eq!(err["error"]["code"], -32601);
    }
}
