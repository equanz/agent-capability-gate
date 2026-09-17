use serde_json::json;
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn workdir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("cli-e2e-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

fn config(path: &PathBuf, executable: &str, tools: &str) {
    let source = format!(
        r#"version: 1
server:
  name: test
  transport: {{kind: stdio}}
targets:
  fake:
    kind: cli
    executable: {executable}
    cwd: /
    limits: {{timeout_ms: 1000, stdout_bytes: 65536, stderr_bytes: 65536}}
tools:
{tools}
"#,
        executable = executable,
        tools = tools
    );
    fs::write(path, source).expect("write config");
}

fn run_check(path: &PathBuf) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["check", "--config"])
        .arg(path)
        .output()
        .expect("run check")
}

#[test]
fn check_and_tools_are_target_free_and_have_stable_diagnostics() {
    let path = workdir().join("valid.yaml");
    let fake = env!("CARGO_BIN_EXE_fake-cli");
    config(
        &path,
        fake,
        r#"  z_tool:
    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: z}]}}
    output: {kind: text}
  a_tool:
    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: a}]}}
    output: {kind: text}
"#,
    );
    let binary = env!("CARGO_BIN_EXE_mcp-boundary");
    let check = Command::new(binary)
        .args(["check", "--config"])
        .arg(&path)
        .output()
        .expect("run check");
    assert!(check.status.success());
    assert!(check.stdout.is_empty());
    assert!(check.stderr.is_empty());

    let tools = Command::new(binary)
        .args(["tools", "--config"])
        .arg(&path)
        .args(["--format", "json"])
        .output()
        .expect("run tools");
    assert!(tools.status.success());
    assert!(tools.stderr.is_empty());
    let catalog: serde_json::Value = serde_json::from_slice(&tools.stdout).expect("catalog json");
    let names: Vec<_> = catalog["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a_tool", "z_tool"]);

    let relative = Command::new(binary)
        .args(["check", "--config", "relative.yaml"])
        .output()
        .expect("run relative check");
    assert_eq!(relative.status.code(), Some(3));
    assert!(relative.stdout.is_empty());
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&relative.stderr).expect("diagnostic json");
    assert_eq!(diagnostic["code"], "INVALID_ARGUMENTS");
    assert_eq!(diagnostic["path"], "");
    assert_eq!(diagnostic["config_path"], "relative.yaml");
}

#[test]
fn check_rejects_static_cli_and_mcp_paths() {
    let directory = workdir();
    let non_executable = directory.join("not-executable");
    fs::write(&non_executable, "fixture").expect("write non executable fixture");
    let cli_config = directory.join("non-executable.yaml");
    config(
        &cli_config,
        non_executable.to_str().unwrap(),
        r#"  inspect:
    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: inspect}]}}
    output: {kind: text}
"#,
    );
    let output = run_check(&cli_config);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("diagnostic json");
    assert_eq!(diagnostic["path"], "/targets/fake/executable");
    assert_eq!(diagnostic["message"], "executable is not executable");

    let mcp_config = directory.join("mcp-non-executable.yaml");
    let source = format!(
        r#"version: 1
server:
  name: test
  transport: {{kind: stdio}}
targets:
  upstream:
    kind: mcp
    transport:
      kind: stdio
      command: {command}
      cwd: /
    limits: {{timeout_ms: 1000, output_bytes: 65536, stderr_bytes: 65536}}
tools:
  inspect:
    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}
    invoke: {{target: upstream, mcp: {{tool: inspect, arguments: {{}}}}}}
    output: {{kind: text}}
"#,
        command = non_executable.display(),
    );
    fs::write(&mcp_config, source).expect("write mcp config");
    let output = run_check(&mcp_config);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("diagnostic json");
    assert_eq!(diagnostic["path"], "/targets/upstream/transport/command");
    assert_eq!(diagnostic["message"], "executable is not executable");

    let cwd_file = directory.join("cwd-file");
    fs::write(&cwd_file, "not a directory").expect("write cwd fixture");
    let cwd_config = directory.join("cwd-file.yaml");
    let source = format!(
        r#"version: 1
server:
  name: test
  transport: {{kind: stdio}}
targets:
  fake:
    kind: cli
    executable: {executable}
    cwd: {cwd}
    limits: {{timeout_ms: 1000, stdout_bytes: 65536, stderr_bytes: 65536}}
tools: {{}}
"#,
        executable = env!("CARGO_BIN_EXE_fake-cli"),
        cwd = cwd_file.display(),
    );
    fs::write(&cwd_config, source).expect("write cwd config");
    let output = run_check(&cwd_config);
    assert_eq!(output.status.code(), Some(3));
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("diagnostic json");
    assert_eq!(diagnostic["path"], "/targets/fake/cwd");
    assert_eq!(diagnostic["message"], "cwd is not a directory");
}

#[test]
fn invalid_config_rejects_serve_before_accepting_mcp_messages() {
    let path = workdir().join("invalid-before-serve.yaml");
    config(
        &path,
        "/definitely/not-a-target",
        r#"  inspect:
    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: inspect}]}}
    output: {kind: text}
"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut stdin = child.stdin.take().expect("serve stdin");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
    )
    .expect("write initialize");
    drop(stdin);
    let output = child.wait_with_output().expect("wait invalid serve");
    assert_eq!(output.status.code(), Some(3));
    assert!(
        output.stdout.is_empty(),
        "invalid config must emit no MCP message"
    );
    assert!(!output.stderr.is_empty());
}

#[test]
fn serve_executes_fake_cli_without_shell_and_returns_bounded_result() {
    let path = workdir().join("serve.yaml");
    let fake = env!("CARGO_BIN_EXE_fake-cli");
    config(
        &path,
        fake,
        r#"  inspect:
    input_schema: {type: object, properties: {value: {type: string}}, required: [value], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: inspect}, {input: /value}]}}
    output: {kind: json}
"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"inspect","arguments":{{"value":"a;$(echo canary)"}}}}}}"#).unwrap();
    stdin.flush().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut response_line = String::new();
    stdout
        .read_line(&mut response_line)
        .expect("read completed response");
    assert!(!response_line.is_empty(), "response is required before EOF");
    drop(stdin);
    let status = child.wait().expect("wait serve");
    let mut remaining_stdout = Vec::new();
    stdout
        .read_to_end(&mut remaining_stdout)
        .expect("read remaining stdout");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("serve stderr")
        .read_to_end(&mut stderr)
        .expect("read serve stderr");
    assert!(status.success());
    assert!(stderr.is_empty());
    assert!(remaining_stdout.is_empty(), "one response expected");
    let response: serde_json::Value = serde_json::from_str(&response_line).expect("response json");
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let observed: serde_json::Value = serde_json::from_str(text).expect("fake cli json");
    assert_eq!(observed["stdin_eof_observed"], true);
    assert_eq!(observed["argv_count"], 2);
}

#[test]
fn serve_observes_only_configured_cwd_and_environment() {
    let path = workdir().join("fixed-process-context.yaml");
    let cwd = workdir();
    let source = format!(
        "version: 1\nserver:\n  name: test\n  transport: {{kind: stdio}}\ntargets:\n  fake:\n    kind: cli\n    executable: {executable}\n    cwd: {cwd}\n    environment:\n      set: {{BOUNDARY_FIXED: yes}}\n    limits: {{timeout_ms: 1000, stdout_bytes: 65536, stderr_bytes: 65536}}\ntools:\n  inspect:\n    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}\n    invoke: {{target: fake, cli: {{argv: [{{literal: --fixed}}]}}}}\n    output: {{kind: json}}\n",
        executable = env!("CARGO_BIN_EXE_fake-cli"),
        cwd = cwd.display(),
    );
    fs::write(&path, source).expect("write process context config");
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut stdin = child.stdin.take().expect("serve stdin");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("serve stdout"));
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"inspect","arguments":{{}}}}}}"#
    )
    .expect("write call");
    stdin.flush().expect("flush call");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    drop(stdin);
    let output = child.wait_with_output().expect("wait serve");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
    let observed: serde_json::Value = serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("result text"),
    )
    .expect("fake cli json");
    assert_eq!(
        observed["cwd"],
        cwd.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(observed["argv"], json!(["--fixed"]));
    assert_eq!(observed["environment"], json!(["BOUNDARY_FIXED"]));
}

#[test]
fn invalid_unknown_and_nul_inputs_are_rejected_before_cli_invocation() {
    let path = workdir().join("invalid-input.yaml");
    config(
        &path,
        env!("CARGO_BIN_EXE_fake-cli"),
        r#"  inspect:
    input_schema: {type: object, properties: {value: {type: string}}, required: [value], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: inspect}, {input: /value}]}}
    output: {kind: json}
"#,
    );
    let unknown = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"inspect","arguments":{"value":"ok","unknown":true}}});
    let nul = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"inspect","arguments":{"value":"bad\u{0}value"}}});
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut stdin = child.stdin.take().expect("serve stdin");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("serve stdout"));
    for request in [&unknown, &nul] {
        serde_json::to_writer(&mut stdin, request).expect("write invalid request");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush request");
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read error response");
        let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("INVALID_ARGUMENTS:"))
        );
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait serve");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn serve_eof_cancels_active_cli_before_exit() {
    let path = workdir().join("eof-shutdown.yaml");
    config(
        &path,
        env!("CARGO_BIN_EXE_fake-cli"),
        r#"  wait:
    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: "--sleep-ms=5000"}]}}
    output: {kind: text}
"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut stdin = child.stdin.take().expect("boundary stdin");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("boundary stdout"));
    let mut stderr = child.stderr.take().expect("boundary stderr");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"wait","arguments":{{}}}}}}"#
    )
    .expect("write active request");
    stdin.flush().expect("flush active request");
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(stdin);

    let status = child.wait().expect("wait serve");
    let mut stdout_bytes = Vec::new();
    stdout
        .read_to_end(&mut stdout_bytes)
        .expect("read serve stdout");
    let mut stderr_bytes = Vec::new();
    stderr
        .read_to_end(&mut stderr_bytes)
        .expect("read serve stderr");
    assert!(status.success());
    assert!(stderr_bytes.is_empty());
    let response: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("cancellation response");
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "CANCELLED: broker is shutting down"
    );
}

#[cfg(unix)]
#[test]
fn serve_signal_cancels_active_cli_before_exit() {
    let path = workdir().join("shutdown.yaml");
    config(
        &path,
        env!("CARGO_BIN_EXE_fake-cli"),
        r#"  wait:
    input_schema: {type: object, properties: {}, required: [], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: "--sleep-ms=5000"}]}}
    output: {kind: text}
"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["serve", "--config"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut stdin = child.stdin.take().expect("boundary stdin");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("boundary stdout"));
    let mut stderr = child.stderr.take().expect("boundary stderr");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{}}}}"#
    )
    .expect("write initialize");
    let mut initialize_response = String::new();
    stdout
        .read_line(&mut initialize_response)
        .expect("read initialize response");
    assert!(
        !initialize_response.is_empty(),
        "initialize response is required"
    );
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"wait","arguments":{{}}}}}}"#
    )
    .expect("write request");
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("send SIGTERM")
            .success()
    );
    drop(stdin);
    let status = child.wait().expect("wait serve");
    let mut stdout_bytes = Vec::new();
    stdout
        .read_to_end(&mut stdout_bytes)
        .expect("read serve stdout");
    let mut stderr_bytes = Vec::new();
    stderr
        .read_to_end(&mut stderr_bytes)
        .expect("read serve stderr");
    assert!(status.success());
    assert!(stderr_bytes.is_empty());
    let response: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("cancellation response");
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "CANCELLED: broker is shutting down"
    );
}
