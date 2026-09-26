use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

fn root(label: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("v2-exposure-{label}-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

fn write_config(label: &str, mode: &str, changing: bool) -> (PathBuf, PathBuf) {
    write_config_with_args(label, mode, &format!("\"{mode}\""), changing)
}

fn write_config_with_args(
    label: &str,
    filename: &str,
    args: &str,
    changing: bool,
) -> (PathBuf, PathBuf) {
    let directory = root(label);
    let config = directory.join(format!("{filename}.yaml"));
    let cache = directory.join("cache");
    let command = env!("CARGO_BIN_EXE_fake-mcp");
    let expose = if changing {
        "echo_arguments:\n        as: echo_public"
    } else {
        r#"echo_arguments:
        as: echo_public
      convert:
        as: convert_public
        restrict:
          inputs:
            source: {enum: [UTC, JST]}
            format: {pattern: '^[a-z]+$'}
            offset: {minimum: -12, maximum: 14}
          fixed:
            value: fixed-source"#
    };
    let source = format!(
        r#"version: 2
server:
  name: v2-exposure-test
  transport: {{kind: stdio}}
targets:
  upstream:
    kind: mcp
    transport:
      kind: stdio
      command: {command}
      args: [{args}]
      cwd: /
    limits:
      timeout_ms: 2000
      output_bytes: 65536
      stderr_bytes: 65536
    expose:
      {expose}
"#,
        command = command,
        args = args,
        expose = expose,
    );
    fs::write(&config, source).expect("write version-2 config");
    (config, cache)
}

fn request(method: &str, id: u64, params: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
}

fn list(id: u64) -> Value {
    request("tools/list", id, json!({}))
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    request(
        "tools/call",
        id,
        json!({"name": name, "arguments": arguments}),
    )
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    notifications: Vec<Value>,
    events: Vec<Value>,
}

impl Session {
    fn start(config: &Path, cache: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
            .args(["serve", "--config"])
            .arg(config)
            .env("XDG_CACHE_HOME", cache)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn version-2 boundary");
        Self {
            stdin: child.stdin.take().expect("boundary stdin"),
            stdout: BufReader::new(child.stdout.take().expect("boundary stdout")),
            child,
            notifications: Vec::new(),
            events: Vec::new(),
        }
    }

    fn send(&mut self, message: Value) -> Value {
        let id = message["id"].clone();
        serde_json::to_writer(&mut self.stdin, &message).expect("write request");
        self.stdin.write_all(b"\n").expect("write request newline");
        self.stdin.flush().expect("flush request");
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read response");
            assert!(!line.is_empty(), "boundary exited before response");
            let value: Value = serde_json::from_str(&line).expect("JSON-RPC response");
            self.events.push(value.clone());
            if value.get("method").is_some() && value.get("id").is_none() {
                self.notifications.push(value);
                continue;
            }
            if value.get("id") == Some(&id) {
                return value;
            }
            panic!("unexpected response while waiting for id {id}: {value}");
        }
    }

    fn finish(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("wait boundary");
        assert!(status.success(), "boundary exited unsuccessfully: {status}");
    }
}

fn tool_names(response: &Value) -> Vec<String> {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list result")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect()
}

fn cache_entry(cache_home: &Path) -> PathBuf {
    let root = cache_home.join("mcp-boundary/catalog-v1");
    let digest = fs::read_dir(root)
        .expect("catalog cache root")
        .next()
        .unwrap()
        .unwrap()
        .path();
    digest.join("upstream.json")
}

#[test]
fn selected_tools_are_the_only_discoverable_and_callable_names() {
    let directory = root("selected");
    let marker = directory.join("calls");
    let args = format!("\"v2-catalog\", \"--marker={}\"", marker.display());
    let (config, cache) = write_config_with_args("selected", "selected", &args, false);
    let mut session = Session::start(&config, &cache);
    let catalog = session.send(list(1));
    let names = tool_names(&catalog);
    assert_eq!(names, vec!["convert_public", "echo_public"]);
    assert!(!names.iter().any(|name| name == "hidden"));

    let selected = session.send(call(2, "echo_public", json!({"value": "ok"})));
    assert_eq!(selected["result"]["isError"], false);

    let denied = session.send(call(3, "hidden", json!({})));
    assert_eq!(denied["error"]["code"], "INVALID_ARGUMENTS");
    assert_eq!(denied["error"]["message"], "unknown tool");
    session.finish();
    assert_eq!(fs::read_to_string(marker).unwrap(), "echo_arguments\n");
}

#[test]
fn proxy_forwards_arguments_and_catalog_change_precedes_notification() {
    let (config, cache) = write_config("changing", "v2-changing", true);
    let mut session = Session::start(&config, &cache);
    let before = session.send(list(1));
    let initial_notification_count = session.notifications.len();
    assert!(
        before["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["name"] != "new_unselected")
    );
    let initial_echo = before["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "echo_public")
        .expect("selected proxy is published");
    assert_eq!(initial_echo["description"], "Echo supplied arguments");
    assert_eq!(
        initial_echo["inputSchema"],
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}, "note": {"type": "string"}},
            "required": ["value"],
            "additionalProperties": false
        })
    );

    let call_result = session.send(call(2, "echo_public", json!({"value": "x", "extra": true})));
    assert_eq!(call_result["result"]["isError"], false);
    assert_eq!(
        call_result["result"]["structuredContent"]["received"],
        json!({"value": "x", "extra": true})
    );
    assert_eq!(session.notifications.len(), initial_notification_count + 1);

    let after = session.send(list(3));
    let echo = after["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "echo_public")
        .expect("selected proxy remains published");
    assert!(echo["inputSchema"]["properties"].get("tag").is_some());
    assert!(echo["inputSchema"]["properties"].get("note").is_some());
    assert_eq!(echo["inputSchema"]["required"], json!(["value"]));
    assert!(
        after["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["name"] != "new_unselected")
    );
    assert_eq!(session.notifications.len(), initial_notification_count + 1);
    assert_eq!(
        session.notifications.last().unwrap()["method"],
        "notifications/tools/list_changed"
    );
    let notification_position = session
        .events
        .iter()
        .position(|event| event.get("method") == Some(&json!("notifications/tools/list_changed")))
        .expect("list changed notification event");
    let call_response_position = session
        .events
        .iter()
        .position(|event| event.get("id") == Some(&json!(2)))
        .expect("downstream call response event");
    assert!(notification_position < call_response_position);

    // Proxy schemas track upstream additions and removals, including required
    // argument changes. The broker forwards each argument object without
    // applying a stale schema as an additional gate.
    let _ = session.send(call(4, "echo_public", json!({"arbitrary": [1, 2]})));
    let after_required_change = session.send(list(5));
    let echo = after_required_change["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "echo_public")
        .unwrap();
    assert_eq!(echo["inputSchema"]["required"], json!(["tag", "token"]));
    assert!(echo["inputSchema"]["properties"].get("value").is_none());
    assert!(echo["inputSchema"]["properties"].get("note").is_none());

    let _ = session.send(call(6, "echo_public", json!({"anything": false})));
    let after_required_removal = session.send(list(7));
    let echo = after_required_removal["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "echo_public")
        .unwrap();
    assert_eq!(echo["inputSchema"]["required"], json!([]));
    assert_eq!(
        echo["inputSchema"]["properties"],
        json!({"token":{"type":"string"}})
    );
    session.finish();
}

#[test]
fn restriction_enforces_enum_fixed_and_unknown_input_policy() {
    let (config, cache) = write_config("restriction", "v2-catalog", false);
    let mut session = Session::start(&config, &cache);
    let catalog = session.send(list(1));
    assert!(tool_names(&catalog).contains(&"convert_public".to_owned()));

    let valid = session.send(call(
        2,
        "convert_public",
        json!({"source": "UTC", "format": "utc", "offset": 9}),
    ));
    assert_eq!(valid["result"]["isError"], false);
    assert_eq!(
        valid["result"]["structuredContent"]["received"],
        json!({"source": "UTC", "format": "utc", "offset": 9, "value": "fixed-source"})
    );

    let invalid_enum = session.send(call(
        3,
        "convert_public",
        json!({"source": "LOCAL", "format": "utc", "offset": 9}),
    ));
    assert_eq!(invalid_enum["result"]["isError"], true);
    assert!(
        invalid_enum["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("INVALID_ARGUMENTS:")
    );

    for (id, arguments) in [
        (4, json!({"source":"UTC","format":"bad-value","offset":9})),
        (5, json!({"source":"UTC","format":"utc","offset":15})),
    ] {
        let invalid = session.send(call(id, "convert_public", arguments));
        assert_eq!(invalid["result"]["isError"], true);
        assert!(
            invalid["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("INVALID_ARGUMENTS:")
        );
    }

    let unknown = session.send(call(
        6,
        "convert_public",
        json!({"source":"UTC","format":"utc","offset":9,"value":"attacker","extra":true}),
    ));
    assert_eq!(unknown["result"]["isError"], true);
    assert!(
        unknown["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("INVALID_ARGUMENTS:")
    );
    session.finish();
}

#[test]
fn incompatible_restriction_is_removed_and_old_name_returns_explained_error() {
    let (config, cache) = write_config("breaking", "v2-breaking", false);
    let mut session = Session::start(&config, &cache);
    assert!(tool_names(&session.send(list(1))).contains(&"convert_public".to_owned()));

    // The upstream announces a definition change during this proxy call. Its
    // new required field cannot be supplied by the frozen restriction.
    let _ = session.send(call(2, "echo_public", json!({"value": "refresh"})));
    assert_eq!(session.notifications.len(), 1);
    let updated = session.send(list(3));
    assert!(!tool_names(&updated).contains(&"convert_public".to_owned()));

    // A client holding the old list receives an actionable tool error rather
    // than an unknown-name protocol error or another upstream invocation.
    let stale = session.send(call(4, "convert_public", json!({"source": "UTC"})));
    assert_eq!(stale["result"]["isError"], true);
    let message = stale["result"]["content"][0]["text"].as_str().unwrap();
    assert!(message.starts_with("TOOL_DEFINITION_CHANGED:"), "{message}");
    assert!(message.contains("refresh tools/list"), "{message}");
    session.finish();
}

#[test]
fn unsupported_and_incomplete_restrictions_are_never_exposed_as_proxies() {
    for (label, mode) in [
        ("unsupported-schema", "v2-unsupported"),
        ("missing-required", "v2-missing-required"),
    ] {
        let (config, cache) = write_config(label, mode, false);
        let mut session = Session::start(&config, &cache);
        let catalog = session.send(list(1));
        assert_eq!(tool_names(&catalog), vec!["echo_public"]);
        let unavailable = session.send(call(
            2,
            "convert_public",
            json!({
                "source":"UTC","format":"utc","offset":9
            }),
        ));
        assert_eq!(unavailable["result"]["isError"], true);
        assert!(
            unavailable["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("TARGET_UNAVAILABLE:")
        );
        session.finish();
    }
}

#[test]
fn upstream_rejection_is_reported_as_target_failure() {
    let (config, cache) = write_config("rejected", "v2-reject", true);
    let mut session = Session::start(&config, &cache);
    assert!(tool_names(&session.send(list(1))).contains(&"echo_public".to_owned()));
    let rejected = session.send(call(2, "echo_public", json!({"value": "not accepted"})));
    assert_eq!(rejected["result"]["isError"], true);
    assert!(
        rejected["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("TARGET_FAILED:"),
        "{rejected}"
    );
    session.finish();
}

#[test]
fn both_downstream_protocol_generations_keep_v2_boundaries_and_output_policy() {
    for (label, protocol) in [
        ("protocol-modern", "2026-07-28"),
        ("protocol-legacy", "2025-11-25"),
    ] {
        let (config, cache) = write_config(label, "v2-catalog", false);
        let mut session = Session::start(&config, &cache);
        let initialized = session.send(request(
            "initialize",
            1,
            json!({
                "protocolVersion": protocol,
                "capabilities": {},
                "clientInfo": {"name":"test-client","version":"1"}
            }),
        ));
        assert_eq!(initialized["result"]["protocolVersion"], protocol);
        assert_eq!(
            initialized["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
        assert_eq!(
            tool_names(&session.send(list(2))),
            vec!["convert_public", "echo_public"]
        );

        let result = session.send(call(
            3,
            "convert_public",
            json!({"source":"JST","format":"jst","offset":9}),
        ));
        assert_eq!(result["result"]["isError"], false);
        assert_eq!(
            result["result"]["structuredContent"]["received"],
            json!({"source":"JST","format":"jst","offset":9,"value":"fixed-source"})
        );
        assert!(result["result"].get("_meta").is_none());

        let denied = session.send(call(
            4,
            "convert_public",
            json!({"source":"LOCAL","format":"jst","offset":9}),
        ));
        assert_eq!(denied["result"]["isError"], true);
        assert!(
            denied["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("INVALID_ARGUMENTS:")
        );
        session.finish();
    }
}

#[test]
fn catalog_cache_is_reused_by_management_tools_after_relaunch() {
    let (config, cache) = write_config("cache", "v2-catalog", false);
    let mut first = Session::start(&config, &cache);
    let catalog = first.send(list(1));
    assert_eq!(tool_names(&catalog), vec!["convert_public", "echo_public"]);
    first.finish();

    // A cache hit makes the first post-relaunch list complete before dynamic
    // refresh. Without the snapshot, refresh would add the tools and emit a
    // list-changed notification for this request.
    let mut second = Session::start(&config, &cache);
    let relaunched = second.send(list(2));
    assert_eq!(
        tool_names(&relaunched),
        vec!["convert_public", "echo_public"]
    );
    assert!(
        second.notifications.is_empty(),
        "cache-backed relaunch should not report an unchanged catalog"
    );
    second.finish();

    let output = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
        .args(["tools", "--config"])
        .arg(&config)
        .args(["--format", "json"])
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .expect("run management tools command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let management: Value = serde_json::from_slice(&output.stdout).expect("management catalog");
    assert_eq!(
        management["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].clone())
            .collect::<Vec<_>>(),
        vec![json!("convert_public"), json!("echo_public")]
    );
    assert_eq!(management["unavailable"], json!([]));
}

#[test]
fn cache_outage_recovery_and_corrupt_snapshot_fail_closed() {
    let directory = root("outage");
    let marker = directory.join("upstream-state");
    let args = format!("\"v2-switchable\", \"--marker={}\"", marker.display());
    let (config, cache) = write_config_with_args("outage", "switchable", &args, false);

    let mut initial = Session::start(&config, &cache);
    assert_eq!(
        tool_names(&initial.send(list(1))),
        vec!["convert_public", "echo_public"]
    );
    initial.finish();

    fs::write(&marker, "offline").unwrap();
    let mut cached_offline = Session::start(&config, &cache);
    assert_eq!(
        tool_names(&cached_offline.send(list(2))),
        vec!["convert_public", "echo_public"]
    );
    let failed_call = cached_offline.send(call(3, "echo_public", json!({"value": "x"})));
    assert_eq!(failed_call["result"]["isError"], true);
    cached_offline.finish();

    let uncached_home = directory.join("uncached");
    let mut uncached_offline = Session::start(&config, &uncached_home);
    assert!(tool_names(&uncached_offline.send(list(4))).is_empty());
    let unavailable = uncached_offline.send(call(5, "echo_public", json!({"value": "x"})));
    assert_eq!(unavailable["result"]["isError"], true);
    assert!(
        unavailable["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("TARGET_UNAVAILABLE:")
    );
    fs::remove_file(&marker).unwrap();
    assert_eq!(
        tool_names(&uncached_offline.send(list(6))),
        vec!["convert_public", "echo_public"]
    );
    assert_eq!(
        tool_names(&uncached_offline.send(list(7))),
        vec!["convert_public", "echo_public"]
    );
    assert_eq!(uncached_offline.notifications.len(), 1);
    uncached_offline.finish();

    fs::write(cache_entry(&cache), b"not a catalog").unwrap();
    fs::write(&marker, "offline").unwrap();
    let mut corrupt_offline = Session::start(&config, &cache);
    assert!(tool_names(&corrupt_offline.send(list(7))).is_empty());
    let unavailable = corrupt_offline.send(call(8, "echo_public", json!({"value": "x"})));
    assert!(
        unavailable["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("TARGET_UNAVAILABLE:")
    );
    corrupt_offline.finish();
}
