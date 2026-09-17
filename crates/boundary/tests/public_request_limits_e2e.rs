use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn workdir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("public-request-limits-e2e-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

fn call(value: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "inspect", "arguments": {"value": value}}
    })
}

fn write_config(path: &Path, request_bytes: usize, json_depth: usize, marker: &Path) {
    let source = format!(
        r#"version: 1
server:
  name: request-limit-test
  transport: {{kind: stdio}}
  limits: {{request_bytes: {request_bytes}, json_depth: {json_depth}}}
targets:
  fake:
    kind: cli
    executable: {executable}
    cwd: /
    limits: {{timeout_ms: 1000, stdout_bytes: 65536, stderr_bytes: 65536}}
tools:
  inspect:
    input_schema:
      type: object
      properties: {{value: {{type: string}}}}
      required: [value]
      additionalProperties: false
    invoke:
      target: fake
      cli:
        argv: [{{literal: --pid-file={marker}}}, {{input: /value}}]
    output: {{kind: text}}
"#,
        executable = env!("CARGO_BIN_EXE_fake-cli"),
        marker = marker.display(),
    );
    fs::write(path, source).expect("write request-limit config");
}

fn run_request(config: &Path, request: &Value) -> (std::process::ExitStatus, Value, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn public boundary");
    let mut stdin = child.stdin.take().expect("boundary stdin");
    let mut request_bytes = serde_json::to_vec(request).expect("serialize request");
    request_bytes.push(b'\n');
    stdin.write_all(&request_bytes).expect("write request");
    stdin.flush().expect("flush request");

    let stdout = child.stdout.take().expect("boundary stdout");
    let mut stdout = BufReader::new(stdout);
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    assert!(!line.is_empty(), "boundary must respond to one request");
    let response = serde_json::from_str(&line).expect("parse response");
    drop(stdin);

    let status = child.wait().expect("wait public boundary");
    let mut trailing_stdout = Vec::new();
    stdout
        .read_to_end(&mut trailing_stdout)
        .expect("read remaining stdout");
    assert!(trailing_stdout.is_empty(), "unexpected extra response");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("boundary stderr")
        .read_to_end(&mut stderr)
        .expect("read stderr");
    (
        status,
        response,
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

#[test]
fn request_bytes_boundary_is_accepted_and_one_over_is_rejected_before_cli() {
    let value = Value::String("boundary".into());
    let request = call(value);
    let body_len = serde_json::to_vec(&request)
        .expect("serialize request")
        .len();

    let accepted_dir = workdir();
    let accepted_marker = accepted_dir.join("accepted-pid");
    let accepted_config = accepted_dir.join("accepted.yaml");
    write_config(&accepted_config, body_len, 8, &accepted_marker);
    let (status, response, stderr) = run_request(&accepted_config, &request);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(response["result"]["isError"], false);
    assert!(
        accepted_marker.is_file(),
        "boundary request must invoke CLI"
    );

    let rejected_dir = workdir();
    let rejected_marker = rejected_dir.join("rejected-pid");
    let rejected_config = rejected_dir.join("rejected.yaml");
    write_config(&rejected_config, body_len - 1, 8, &rejected_marker);
    let (status, response, stderr) = run_request(&rejected_config, &request);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(response["error"]["code"], "INVALID_ARGUMENTS");
    assert_eq!(
        response["error"]["message"],
        "request exceeds configured limit"
    );
    assert!(
        !rejected_marker.exists(),
        "one-over request must not invoke CLI"
    );
}

#[test]
fn json_depth_overflow_is_rejected_before_cli() {
    let request = call(json!({"nested": {"too_deep": true}}));
    let directory = workdir();
    let marker = directory.join("deep-pid");
    let config = directory.join("deep.yaml");
    let request_len = serde_json::to_vec(&request)
        .expect("serialize request")
        .len();
    write_config(&config, request_len + 100, 4, &marker);

    let (status, response, stderr) = run_request(&config, &request);
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    assert_eq!(response["error"]["code"], "INVALID_ARGUMENTS");
    assert_eq!(
        response["error"]["message"],
        "request exceeds configured JSON limits"
    );
    assert!(!marker.exists(), "deep request must not invoke CLI");
}

#[test]
fn delimiter_free_oversized_message_is_rejected_without_waiting_for_eof() {
    let directory = workdir();
    let marker = directory.join("delimiter-free-pid");
    let config = directory.join("delimiter-free.yaml");
    write_config(&config, 8, 8, &marker);

    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn public boundary");
    let mut stdin = child.stdin.take().expect("boundary stdin");
    stdin
        .write_all(b"{\"oversized\":")
        .expect("write oversized message");
    stdin.flush().expect("flush oversized message");

    let stdout = child.stdout.take().expect("boundary stdout");
    let mut stdout = BufReader::new(stdout);
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read bounded rejection");
    let response: Value = serde_json::from_str(&line).expect("bounded rejection json");
    assert_eq!(response["error"]["code"], "INVALID_ARGUMENTS");
    assert_eq!(
        response["error"]["message"],
        "request exceeds configured limit"
    );
    drop(stdin);
    let output = child.wait_with_output().expect("wait public boundary");
    assert!(output.status.success());
    assert!(!marker.exists(), "oversized message must not invoke CLI");
}
