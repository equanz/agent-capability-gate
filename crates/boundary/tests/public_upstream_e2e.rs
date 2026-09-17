use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn workdir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("public-upstream-e2e-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

fn write_config(mode: &str, output_kind: &str) -> PathBuf {
    write_config_with_limits(mode, output_kind, 2000, 65536, None)
}

fn write_config_with_limits(
    mode: &str,
    output_kind: &str,
    timeout_ms: u64,
    stderr_bytes: usize,
    marker: Option<&Path>,
) -> PathBuf {
    write_config_with_all_limits(
        mode,
        output_kind,
        timeout_ms,
        stderr_bytes,
        marker,
        1_048_576,
        32,
    )
}

fn write_config_with_all_limits(
    mode: &str,
    output_kind: &str,
    timeout_ms: u64,
    stderr_bytes: usize,
    marker: Option<&Path>,
    request_bytes: usize,
    json_depth: usize,
) -> PathBuf {
    let path = workdir().join(format!(
        "{mode}-{output_kind}-{timeout_ms}-{stderr_bytes}.yaml"
    ));
    let command = env!("CARGO_BIN_EXE_fake-mcp");
    let marker_arg = marker
        .map(|path| format!(", --marker={}", path.display()))
        .unwrap_or_default();
    let source = format!(
        r#"version: 1
server:
  name: public-upstream-test
  transport: {{kind: stdio}}
  limits: {{request_bytes: {request_bytes}, json_depth: {json_depth}}}
targets:
  upstream:
    kind: mcp
    transport:
      kind: stdio
      command: {command}
      args: [{mode}{marker_arg}]
      cwd: /
    limits:
      timeout_ms: {timeout_ms}
      output_bytes: 65536
      stderr_bytes: {stderr_bytes}
tools:
  fixed_call:
    input_schema:
      type: object
      properties:
        value: {{type: string}}
      required: [value]
      additionalProperties: false
    invoke:
      target: upstream
      mcp:
        tool: echo_arguments
        arguments:
          fixed: {{literal: fixed}}
          value: {{input: /value}}
    output:
      kind: {output_kind}
"#,
        command = command,
        mode = mode,
        marker_arg = marker_arg,
        timeout_ms = timeout_ms,
        stderr_bytes = stderr_bytes,
        output_kind = output_kind,
        request_bytes = request_bytes,
        json_depth = json_depth,
    );
    fs::write(&path, source).expect("write public upstream config");
    path
}

fn run_requests(
    config: &Path,
    requests: &[Value],
) -> (std::process::ExitStatus, Vec<Value>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn public boundary");
    let mut stdin = child.stdin.take().expect("boundary stdin");
    let stdout = child.stdout.take().expect("boundary stdout");
    let mut stdout = BufReader::new(stdout);
    let mut stderr = child.stderr.take().expect("boundary stderr");
    for request in requests {
        serde_json::to_writer(&mut stdin, request).expect("write request");
        stdin.write_all(b"\n").expect("write request newline");
    }
    stdin.flush().expect("flush requests");
    let mut responses = Vec::with_capacity(requests.len());
    for _ in requests {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read JSON-RPC response");
        responses.push(serde_json::from_str(&line).expect("JSON-RPC response"));
    }
    drop(stdin);
    let status = child.wait().expect("wait public boundary");
    let mut trailing_stdout = Vec::new();
    stdout
        .read_to_end(&mut trailing_stdout)
        .expect("read remaining public stdout");
    assert!(trailing_stdout.is_empty(), "unexpected extra responses");
    let mut stderr_bytes = Vec::new();
    stderr
        .read_to_end(&mut stderr_bytes)
        .expect("read public stderr");
    (
        status,
        responses,
        String::from_utf8_lossy(&stderr_bytes).into_owned(),
    )
}

fn run_sequential_requests(
    config: &Path,
    requests: &[Value],
) -> (std::process::ExitStatus, Vec<Value>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn public boundary");
    let mut stdin = child.stdin.take().expect("boundary stdin");
    let stdout = child.stdout.take().expect("boundary stdout");
    let mut stdout = BufReader::new(stdout);
    let mut responses = Vec::with_capacity(requests.len());
    for request in requests {
        serde_json::to_writer(&mut stdin, request).expect("write request");
        stdin.write_all(b"\n").expect("write request newline");
        stdin.flush().expect("flush request");
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read response");
        responses.push(serde_json::from_str(&line).expect("JSON-RPC response"));
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait public boundary");
    (
        output.status,
        responses,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn call(id: u64, value: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": "fixed_call", "arguments": {"value": value}}
    })
}

fn unknown_call(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": "not-published", "arguments": {}}
    })
}

fn initialize(id: u64, protocol_version: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": {"name": "raw-test-client", "version": "1"}
        }
    })
}

fn tools_list(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/list",
        "params": {}
    })
}

#[test]
fn public_stdio_calls_modern_and_legacy_upstreams_with_fixed_binding() {
    for mode in ["modern", "legacy"] {
        let config = write_config(mode, "text");
        let (status, responses, stderr) = run_requests(&config, &[call(1, "client-value")]);
        assert!(status.success(), "{mode}: {stderr}");
        assert!(stderr.is_empty(), "{mode}: unexpected stderr: {stderr}");
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text result");
        assert_eq!(text, r#"{"fixed":"fixed","value":"client-value"}"#);
        assert_eq!(responses[0]["result"]["isError"], false);
        assert!(responses[0]["result"].get("_meta").is_none());
    }
}

#[test]
fn public_stdio_reuses_upstream_target_process() {
    let config = write_config("reuse", "text");
    let (status, responses, stderr) =
        run_sequential_requests(&config, &[call(1, "same"), call(2, "same")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["result"]["isError"], false);
    assert_eq!(responses[1]["result"]["isError"], false);
    let first = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("first text result");
    let second = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("second text result");
    assert!(first.contains(r#""callNumber":1"#), "{first}");
    assert!(second.contains(r#""callNumber":2"#), "{second}");
}

#[test]
fn public_stdio_does_not_forward_unsupported_content_or_meta() {
    let config = write_config("bad-content", "text");
    let (status, responses, stderr) = run_requests(&config, &[call(1, "ignored")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().expect("error text");
    assert!(text.starts_with("INVALID_TARGET_OUTPUT:"), "{text}");
    assert!(!text.contains("not-forwarded"));
    assert!(result.get("_meta").is_none());
}

#[test]
fn public_stdio_returns_server_busy_for_concurrent_same_target_calls() {
    let config = write_config("slow", "text");
    let (status, responses, stderr) = run_requests(&config, &[call(1, "first"), call(2, "second")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(responses.len(), 2);
    let errors: Vec<_> = responses
        .iter()
        .filter(|response| response["error"]["code"] == "SERVER_BUSY")
        .collect();
    assert_eq!(errors.len(), 1, "responses: {responses:?}");
    assert!(
        responses
            .iter()
            .any(|response| response["result"]["isError"] == false)
    );
}

#[test]
fn public_upstream_preserves_multiple_text_blocks_in_wire_order() {
    let config = write_config("multiple-text", "text");
    let (status, responses, stderr) = run_requests(&config, &[call(1, "ignored")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(
        responses[0]["result"]["content"],
        json!([
            {"type":"text","text":"first"},
            {"type":"text","text":"second"},
            {"type":"text","text":"third"}
        ])
    );
}

#[test]
fn public_catalog_does_not_adopt_upstream_tools_or_arguments() {
    // The fixture advertises extra tools if asked for tools/list. The public
    // catalog is policy-owned and is produced without forwarding that request.
    let config = write_config("catalog-noise", "text");
    let (status, responses, stderr) =
        run_sequential_requests(&config, &[tools_list(1), call(2, "client")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    let tools = responses[0]["result"]["tools"].as_array().expect("catalog");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "fixed_call");
    assert_eq!(
        tools[0]["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert!(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .expect("call text")
            .contains(r#""value":"client""#)
    );
}

#[test]
fn public_modern_and_legacy_clients_resolve_the_same_call() {
    let mut observations = Vec::new();
    for protocol in ["2026-07-28", "2025-11-25"] {
        let config = write_config("modern", "text");
        let (status, responses, stderr) = run_sequential_requests(
            &config,
            &[
                initialize(1, protocol),
                tools_list(2),
                call(3, "same-input"),
            ],
        );
        assert!(status.success(), "{protocol}: {stderr}");
        assert!(stderr.is_empty(), "{protocol}: unexpected stderr: {stderr}");
        let catalog = responses[1].clone();
        let mut call_response = responses[2].clone();
        assert_eq!(
            call_response["result"]["isError"], false,
            "{protocol}: public call must succeed"
        );
        assert_eq!(
            call_response["result"]["content"][0]["text"],
            r#"{"fixed":"fixed","value":"same-input"}"#,
            "{protocol}: binding observation must be stable"
        );
        if protocol == "2026-07-28" {
            assert_eq!(call_response["result"]["resultType"], "complete");
        } else {
            assert!(call_response["result"].get("resultType").is_none());
        }
        call_response["result"]
            .as_object_mut()
            .unwrap()
            .remove("resultType");
        observations.push((catalog, call_response));
    }
    assert_eq!(observations[0], observations[1]);
}

#[test]
fn unknown_public_tool_is_a_protocol_error_without_target_start() {
    let config = write_config("modern", "text");
    let (status, responses, stderr) =
        run_sequential_requests(&config, &[initialize(1, "2026-07-28"), unknown_call(2)]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(responses[1]["error"]["code"], "INVALID_ARGUMENTS");
    assert_eq!(responses[1]["error"]["message"], "unknown tool");
    assert!(responses[1].get("result").is_none());
}

#[test]
fn modern_tool_errors_are_complete_and_legacy_tool_errors_are_adapted() {
    for protocol in ["2026-07-28", "2025-11-25"] {
        let config = write_config("protocol-error", "text");
        let (status, responses, stderr) =
            run_sequential_requests(&config, &[initialize(1, protocol), call(2, "failure")]);
        assert!(status.success(), "{protocol}: {stderr}");
        let result = &responses[1]["result"];
        assert_eq!(result["isError"], true);
        if protocol == "2026-07-28" {
            assert_eq!(result["resultType"], "complete");
        } else {
            assert!(result.get("resultType").is_none());
        }
    }
}

#[test]
fn public_legacy_discovery_restarts_once_and_calls_once() {
    let marker = workdir().join("legacy-restart-events");
    let _ = std::fs::remove_file(&marker);
    let config = write_config_with_limits("legacy-restart", "text", 2000, 65536, Some(&marker));
    let (status, responses, stderr) = run_requests(&config, &[call(1, "restart")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(responses[0]["result"]["isError"], false);
    let events = std::fs::read_to_string(&marker).expect("restart marker");
    assert_eq!(events, "first-processinitialize-restart\ntools-call\n");
}

#[test]
fn public_upstream_failures_are_stable_and_do_not_forward_raw_content() {
    let cases = [
        ("protocol-error", "TARGET_FAILED:"),
        ("mixed-content", "INVALID_TARGET_OUTPUT:"),
        ("server-request", "TARGET_FAILED:"),
        ("input-required", "TARGET_FAILED:"),
        ("structured", "INVALID_TARGET_OUTPUT:"),
    ];
    for (mode, expected) in cases {
        let config = write_config(mode, "text");
        let (status, responses, stderr) = run_requests(&config, &[call(1, "failure")]);
        assert!(status.success(), "{mode}: {stderr}");
        assert!(stderr.is_empty(), "{mode}: unexpected stderr: {stderr}");
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("stable failure text");
        assert!(text.starts_with(expected), "{mode}: {text}");
        assert!(!text.contains("not-forwarded"), "{mode}: {text}");
        assert!(!text.contains("not-json"), "{mode}: {text}");
        assert!(!text.contains("input required"), "{mode}: {text}");
        assert!(responses[0]["result"].get("_meta").is_none());
    }
}

#[test]
fn public_upstream_timeout_and_stderr_overflow_are_bounded_failures() {
    let timeout_config = write_config_with_limits("timeout", "text", 50, 65536, None);
    let (status, responses, stderr) = run_requests(&timeout_config, &[call(1, "timeout")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    let timeout_text = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("timeout text");
    assert!(
        timeout_text.starts_with("TARGET_TIMEOUT:"),
        "{timeout_text}"
    );

    let stderr_config = write_config_with_limits("stderr-overflow", "text", 2000, 1024, None);
    let (status, responses, stderr) = run_requests(&stderr_config, &[call(2, "stderr")]);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    let stderr_text = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("stderr overflow text");
    assert!(
        stderr_text.starts_with("OUTPUT_LIMIT_EXCEEDED:"),
        "{stderr_text}"
    );
    assert!(!stderr_text.contains("STDERR_SENTINEL"));
}
