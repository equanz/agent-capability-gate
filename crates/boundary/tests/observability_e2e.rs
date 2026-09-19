use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

fn workdir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("observability-e2e-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

fn config() -> PathBuf {
    let path = workdir().join("config.yaml");
    let source = format!(
        r#"version: 1
server:
  name: observability-test
  transport: {{kind: stdio}}
targets:
  fake:
    kind: cli
    executable: {executable}
    cwd: /
    limits: {{timeout_ms: 1000, stdout_bytes: 65536, stderr_bytes: 65536}}
  upstream:
    kind: mcp
    transport:
      kind: stdio
      command: {upstream}
      args: [modern, --stderr=UPSTREAM_SENTINEL]
      cwd: /
    limits: {{timeout_ms: 1000, output_bytes: 65536, stderr_bytes: 65536}}
tools:
  success:
    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}
    invoke: {{target: fake, cli: {{argv: [{{literal: --text=SAFE_OUTPUT}}]}}}}
    output: {{kind: text}}
  failure:
    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}
    invoke: {{target: fake, cli: {{argv: [{{literal: --stderr=SECRET_SENTINEL}}, {{literal: --exit-code=7}}]}}}}
    output: {{kind: text}}
  binary_failure:
    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}
    invoke: {{target: fake, cli: {{argv: [{{literal: --stderr-hex=00ff0a}}, {{literal: --exit-code=9}}]}}}}
    output: {{kind: text}}
  rejected:
    input_schema: {{type: object, properties: {{value: {{type: string}}}}, required: [value], additionalProperties: false}}
    invoke: {{target: fake, cli: {{argv: [{{input: /value}}]}}}}
    output: {{kind: text}}
  upstream_success:
    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}
    invoke: {{target: upstream, mcp: {{tool: echo_arguments, arguments: {{}}}}}}
    output: {{kind: text}}
"#,
        executable = env!("CARGO_BIN_EXE_fake-cli"),
        upstream = env!("CARGO_BIN_EXE_fake-mcp"),
    );
    fs::write(&path, source).expect("write config");
    path
}

fn spawn(
    config: &PathBuf,
    debug: bool,
) -> (
    Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"));
    command
        .args(["serve", "--config"])
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if debug {
        command.env("MCP_BOUNDARY_LOG", "debug");
    } else {
        command.env_remove("MCP_BOUNDARY_LOG");
    }
    let mut child = command.spawn().expect("spawn boundary");
    let stdin = child.stdin.take().expect("boundary stdin");
    let stdout = BufReader::new(child.stdout.take().expect("boundary stdout"));
    (child, stdin, stdout)
}

fn call(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    id: u64,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    });
    serde_json::to_writer(&mut *stdin, &request).expect("write request");
    stdin.write_all(b"\n").expect("write request newline");
    stdin.flush().expect("flush request");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(&line).expect("parse response")
}

#[test]
fn call_events_are_disabled_by_default() {
    let path = config();
    let (child, mut stdin, mut stdout) = spawn(&path, false);
    let response = call(&mut stdin, &mut stdout, 1, "success", json!({}));
    let failure = call(&mut stdin, &mut stdout, 2, "failure", json!({}));
    assert_eq!(response["result"]["content"][0]["text"], "SAFE_OUTPUT");
    assert_eq!(failure["result"]["isError"], true);
    drop(stdin);
    let output = child.wait_with_output().expect("wait boundary");
    let mut remaining_stdout = Vec::new();
    stdout
        .read_to_end(&mut remaining_stdout)
        .expect("read remaining stdout");
    assert!(output.status.success());
    assert!(remaining_stdout.is_empty());
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
}

#[test]
fn debug_call_events_are_jsonl_and_include_explicit_target_diagnostics() {
    let path = config();
    let (child, mut stdin, mut stdout) = spawn(&path, true);
    let success = call(&mut stdin, &mut stdout, 1, "success", json!({}));
    let failure = call(&mut stdin, &mut stdout, 2, "failure", json!({}));
    let rejected = call(
        &mut stdin,
        &mut stdout,
        3,
        "rejected",
        json!({"secret": "SECRET_SENTINEL"}),
    );
    let upstream = call(&mut stdin, &mut stdout, 4, "upstream_success", json!({}));
    let binary = call(&mut stdin, &mut stdout, 5, "binary_failure", json!({}));
    assert_eq!(success["result"]["isError"], false);
    assert_eq!(failure["result"]["isError"], true);
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(upstream["result"]["isError"], false);
    assert_eq!(binary["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["content"][0]["text"],
        "INVALID_ARGUMENTS: required"
    );
    drop(stdin);
    let output = child.wait_with_output().expect("wait boundary");
    let mut remaining_stdout = Vec::new();
    stdout
        .read_to_end(&mut remaining_stdout)
        .expect("read remaining stdout");
    assert!(output.status.success());
    assert!(remaining_stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("SECRET_SENTINEL"));
    let events: Vec<serde_json::Value> = stderr
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSONL event"))
        .collect();
    assert_eq!(events.len(), 5);
    let ids: Vec<_> = events
        .iter()
        .map(|event| {
            event["correlation_id"]
                .as_u64()
                .expect("numeric correlation id")
        })
        .collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    assert_eq!(events[0]["event"], "tool_call_finished");
    assert_eq!(events[0]["tool"], "success");
    assert_eq!(events[0]["target"], "fake");
    assert_eq!(events[0]["outcome"], "success");
    assert!(events[0].get("code").is_none());
    assert_eq!(events[1]["tool"], "failure");
    assert_eq!(events[1]["outcome"], "error");
    assert_eq!(events[1]["code"], "TARGET_FAILED");
    assert_eq!(events[1]["target_stderr"]["encoding"], "utf8");
    assert_eq!(events[1]["target_stderr"]["value"], "SECRET_SENTINEL");
    assert_eq!(events[1]["target_exit"]["code"], 7);
    assert_eq!(events[2]["tool"], "rejected");
    assert_eq!(events[2]["outcome"], "rejected");
    assert_eq!(events[2]["code"], "INVALID_ARGUMENTS");
    assert_eq!(events[3]["tool"], "upstream_success");
    assert_eq!(events[3]["target"], "upstream");
    assert_eq!(events[3]["outcome"], "success");
    assert_eq!(events[3]["target_stderr"]["encoding"], "utf8");
    assert_eq!(events[3]["target_stderr"]["value"], "UPSTREAM_SENTINEL");
    assert_eq!(events[4]["tool"], "binary_failure");
    assert_eq!(events[4]["target_stderr"]["encoding"], "hex");
    assert_eq!(events[4]["target_stderr"]["value"], "00ff0a");
    assert_eq!(events[4]["target_exit"]["code"], 9);
    for event in &events {
        assert!(event.as_object().unwrap().keys().all(|key| {
            matches!(
                key.as_str(),
                "event"
                    | "correlation_id"
                    | "tool"
                    | "target"
                    | "outcome"
                    | "code"
                    | "target_stderr"
                    | "target_exit"
            )
        }));
    }
}
