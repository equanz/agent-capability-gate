#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn workdir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("process-limit-e2e-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

fn config(tools: &str, timeout_ms: u64, stdout_bytes: usize, stderr_bytes: usize) -> PathBuf {
    let path = workdir().join(format!(
        "config-{timeout_ms}-{stdout_bytes}-{stderr_bytes}.yaml"
    ));
    let source = format!(
        "version: 1\nserver:\n  name: process-limit-test\n  transport: {{kind: stdio}}\ntargets:\n  fake:\n    kind: cli\n    executable: {executable}\n    cwd: /\n    environment:\n      set: {{BOUNDARY_SECRET: SECRET_SENTINEL}}\n    limits: {{timeout_ms: {timeout_ms}, stdout_bytes: {stdout_bytes}, stderr_bytes: {stderr_bytes}}}\ntools:\n{tools}",
        executable = env!("CARGO_BIN_EXE_fake-cli"),
    );
    fs::write(&path, source).expect("write config");
    path
}

fn call(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    id: u64,
    name: &str,
) -> serde_json::Value {
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\"params\":{{\"name\":\"{name}\",\"arguments\":{{}}}}}}"
    )
    .expect("write call");
    stdin.flush().expect("flush call");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(&line).expect("parse response")
}

#[test]
fn cli_result_modes_and_limits_are_stable() {
    let path = config(
        "  text:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: '--text=hello'}]}}\n    output: {kind: text}\n  json:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: '--text={\"z\":1,\"a\":2}'}]}}\n    output: {kind: json}\n  invalid_json:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: --invalid-json}]}}\n    output: {kind: json}\n  nonzero:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: --exit-code=7}]}}\n    output: {kind: text}\n  overflow:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: --stdout-bytes=129}]}}\n    output: {kind: text}\n",
        1000,
        128,
        128,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn boundary");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let text = call(&mut stdin, &mut stdout, 1, "text");
    assert_eq!(text["result"]["content"][0]["text"], "hello");
    let json_result = call(&mut stdin, &mut stdout, 2, "json");
    assert_eq!(
        json_result["result"]["structuredContent"],
        json!({"a": 2, "z": 1})
    );
    assert_eq!(
        json_result["result"]["content"][0]["text"],
        r#"{"a":2,"z":1}"#
    );
    for (id, name, code) in [
        (3, "invalid_json", "INVALID_TARGET_OUTPUT"),
        (4, "nonzero", "TARGET_FAILED"),
        (5, "overflow", "OUTPUT_LIMIT_EXCEEDED"),
    ] {
        let response = call(&mut stdin, &mut stdout, id, name);
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with(&format!("{code}:")))
        );
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait boundary");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn cli_timeout_and_target_stderr_never_leak_secret() {
    let path = config(
        "  timeout:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: --sleep-ms=5000}]}}\n    output: {kind: text}\n  secret:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: --stderr=SECRET_SENTINEL}, {literal: --exit-code=7}]}}\n    output: {kind: text}\n",
        100,
        65536,
        16,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn boundary");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let timeout = call(&mut stdin, &mut stdout, 1, "timeout");
    assert_eq!(
        timeout["result"]["content"][0]["text"],
        "TARGET_TIMEOUT: target timed out"
    );
    let secret = call(&mut stdin, &mut stdout, 2, "secret");
    assert_eq!(secret["result"]["isError"], true);
    assert!(!secret.to_string().contains("SECRET_SENTINEL"));
    drop(stdin);
    let output = child.wait_with_output().expect("wait boundary");
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_SENTINEL"));
}

#[test]
fn seventeen_pipeline_calls_have_one_busy_result() {
    let path = config(
        "  slow:\n    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}\n    invoke: {target: fake, cli: {argv: [{literal: --sleep-ms=250}]}}\n    output: {kind: text}\n",
        2000,
        65536,
        65536,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn boundary");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    for id in 0..17 {
        writeln!(
            stdin,
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\"params\":{{\"name\":\"slow\",\"arguments\":{{}}}}}}"
        )
        .expect("write pipeline call");
    }
    stdin.flush().expect("flush pipeline calls");
    let mut responses = Vec::new();
    for _ in 0..17 {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read pipeline response");
        responses.push(serde_json::from_str::<serde_json::Value>(&line).expect("parse response"));
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait boundary");
    assert!(output.status.success());
    let busy = responses
        .iter()
        .filter(|response| response["error"]["code"] == "SERVER_BUSY")
        .count();
    assert_eq!(busy, 1, "responses: {responses:?}");
}

#[cfg(unix)]
#[test]
fn cli_termination_kills_sigterm_ignoring_leader_and_descendant() {
    let marker = workdir().join("descendant-pid");
    let _ = fs::remove_file(&marker);
    let path = config(
        &format!(
            "  descendant:\n    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}\n    invoke: {{target: fake, cli: {{argv: [{{literal: --spawn-descendant={}}}, {{literal: --ignore-term}}, {{literal: --sleep-ms=5000}}]}}}}\n    output: {{kind: text}}\n",
            marker.display()
        ),
        1000,
        65536,
        65536,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn boundary");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let response = call(&mut stdin, &mut stdout, 1, "descendant");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "TARGET_TIMEOUT: target timed out"
    );
    drop(stdin);
    let output = child.wait_with_output().expect("wait boundary");
    assert!(output.status.success());

    let pid = fs::read_to_string(&marker)
        .expect("descendant pid marker")
        .trim()
        .parse::<i32>()
        .expect("descendant pid");
    let pid = Pid::from_raw(pid);
    let mut terminated = false;
    for _ in 0..30 {
        match kill(pid, None::<Signal>) {
            Err(Errno::ESRCH) => {
                terminated = true;
                break;
            }
            Ok(()) | Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    assert!(terminated, "descendant process {pid} survived termination");
}
