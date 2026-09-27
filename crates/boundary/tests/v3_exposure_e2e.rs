use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

fn root(label: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.work")
        .join(format!("v3-exposure-{label}-{}", std::process::id()));
    fs::create_dir_all(&path).expect("create repository-local test directory");
    path
}

struct Fixture {
    config: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    events: PathBuf,
}

fn write_config(label: &str, expose_unlisted: bool) -> Fixture {
    let directory = root(label);
    let config = directory.join("config.yaml");
    let cache = directory.join("cache");
    let state = directory.join("upstream-state");
    let events = directory.join("upstream-events");
    fs::write(directory.join(".v3-fixture"), b"").expect("mark v3 fake target directory");
    let command = serde_json::to_string(env!("CARGO_BIN_EXE_fake-mcp")).unwrap();
    let cwd = serde_json::to_string(&directory.to_string_lossy()).unwrap();
    let source = format!(
        "version: 3\nserver: {{name: v3-test, transport: {{kind: stdio}}}}\ntargets:\n  time:\n    kind: mcp\n    transport:\n      kind: stdio\n      command: {command}\n      args: []\n      cwd: {cwd}\n    limits: {{timeout_ms: 2000, output_bytes: 65536, stderr_bytes: 65536}}\n    expose:\n      convert:\n        as: convert_time\n        description: Convert time within the configured boundary\n        restrict:\n          expose_unlisted_properties: {expose_unlisted}\n          properties:\n            time: {{minLength: 5, required: true, description: Time in HH:MM}}\n            timezone: {{enum: [UTC, JST], description: Approved destination}}\n            source_timezone: {{fixed: BROKER_FIXED_SENTINEL}}\n            optional_note: {{omit: true}}\n      get_current_time:\n        as: current_time\n",
    );
    fs::write(&config, source).expect("write version-3 config");
    Fixture {
        config,
        cache,
        state,
        events,
    }
}

fn request(method: &str, id: u64, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn list(id: u64) -> Value {
    request("tools/list", id, json!({}))
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    request("tools/call", id, json!({"name":name,"arguments":arguments}))
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    notifications: Vec<Value>,
}

impl Session {
    fn start(fixture: &Fixture) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-boundary"))
            .args(["serve", "--config"])
            .arg(&fixture.config)
            .env("XDG_CACHE_HOME", &fixture.cache)
            .env("MCP_TEST_CREDENTIAL", "V3_CREDENTIAL_SENTINEL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn version-3 boundary");
        Self {
            stdin: child.stdin.take().expect("boundary stdin"),
            stdout: BufReader::new(child.stdout.take().expect("boundary stdout")),
            child,
            notifications: Vec::new(),
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

fn names(response: &Value) -> Vec<String> {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list result")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn tool<'a>(response: &'a Value, name: &str) -> &'a Value {
    response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()
}

fn upstream_events(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn cached_definition_text(cache_home: &Path) -> String {
    let cache_root = cache_home.join("mcp-boundary/catalog-v3");
    let identity_dir = fs::read_dir(cache_root)
        .expect("v3 cache root")
        .next()
        .unwrap()
        .unwrap()
        .path();
    let cache_file = fs::read_dir(identity_dir)
        .expect("per-target cache directory")
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::read_to_string(cache_file).expect("read v3 cache")
}

#[test]
fn v3_restriction_proxy_cache_and_same_connection_reuse_are_enforced() {
    let fixture = write_config("policy", false);
    let mut session = Session::start(&fixture);
    let listed = session.send(list(1));
    assert_eq!(names(&listed), ["convert_time", "current_time"]);
    let convert = tool(&listed, "convert_time");
    assert_eq!(
        convert["description"],
        "Convert time within the configured boundary"
    );
    assert_eq!(convert["inputSchema"]["additionalProperties"], false);
    assert_eq!(convert["inputSchema"]["required"], json!(["time"]));
    assert_eq!(
        convert["inputSchema"]["properties"]["timezone"]["enum"],
        json!(["UTC", "JST"])
    );
    assert_eq!(
        convert["inputSchema"]["properties"]["time"]["description"],
        "Time in HH:MM"
    );
    for hidden in ["source_timezone", "optional_note", "future_optional"] {
        assert!(convert["inputSchema"]["properties"].get(hidden).is_none());
    }

    let result = session.send(call(
        2,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(
        result["result"]["structuredContent"]["received"],
        json!({
            "time":"12:30","timezone":"JST","source_timezone":"BROKER_FIXED_SENTINEL"
        })
    );
    for (id, args) in [
        (3, json!({"time":"12:30","timezone":"PST"})),
        (4, json!({"time":"12:30","timezone":"JST","extra":true})),
        (
            5,
            json!({"time":"12:30","timezone":"JST","source_timezone":"attacker"}),
        ),
    ] {
        let rejected = session.send(call(id, "convert_time", args));
        assert_eq!(rejected["result"]["isError"], true);
        assert!(
            rejected["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("INVALID_ARGUMENTS:")
        );
    }
    let proxy = session.send(call(
        6,
        "current_time",
        json!({"timezone":"UTC","proxy_extra":"passed-through"}),
    ));
    assert_eq!(
        proxy["result"]["structuredContent"]["received"]["proxy_extra"],
        "passed-through"
    );
    let unknown = session.send(call(7, "unselected", json!({})));
    assert_eq!(unknown["error"]["message"], "unknown tool");
    assert_eq!(
        names(&session.send(list(8))),
        ["convert_time", "current_time"]
    );
    session.finish();

    assert_eq!(
        upstream_events(&fixture.events),
        ["list", "call:convert", "call:get_current_time"]
    );
    let cache = cached_definition_text(&fixture.cache);
    assert!(cache.contains("upstream_tools"));
    for private_value in ["BROKER_FIXED_SENTINEL", "proxy_extra", "12:30"] {
        assert!(!cache.contains(private_value));
    }
}

#[test]
fn v3_unlisted_additions_block_or_become_inputs_only_when_explicitly_allowed() {
    let closed = write_config("new-required-blocked", false);
    fs::write(&closed.state, "new-required").unwrap();
    let mut session = Session::start(&closed);
    let listed = session.send(list(1));
    assert_eq!(
        tool(&listed, "convert_time")["inputSchema"]["properties"],
        json!({})
    );
    let result = session.send(call(2, "convert_time", json!({"time":"12:30"})));
    assert_eq!(
        result["result"]["structuredContent"]["code"],
        "ADMIN_ACTION_REQUIRED"
    );
    assert_eq!(
        result["result"]["structuredContent"]["reason"],
        "REQUIRED_PROPERTY_UNSATISFIED"
    );
    assert_eq!(result["result"]["structuredContent"]["retryable"], false);
    session.finish();
    assert_eq!(upstream_events(&closed.events), ["list"]);

    let open = write_config("new-required-exposed", true);
    fs::write(&open.state, "new-required").unwrap();
    let mut session = Session::start(&open);
    let listed = session.send(list(1));
    let convert = tool(&listed, "convert_time");
    assert!(
        convert["inputSchema"]["properties"]
            .get("future_optional")
            .is_some()
    );
    assert!(convert["inputSchema"]["properties"].get("admin").is_some());
    assert!(
        convert["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("admin"))
    );
    assert!(
        !convert["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("future_optional"))
    );
    let result = session.send(call(
        2,
        "convert_time",
        json!({
            "time":"12:30","timezone":"JST","admin":"explicitly-supplied"
        }),
    ));
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(
        result["result"]["structuredContent"]["received"]["admin"],
        "explicitly-supplied"
    );
    session.finish();
}

#[test]
fn v3_optional_properties_stay_optional_and_required_true_promotes_one() {
    let fixture = write_config("optional-properties", false);
    let config = fs::read_to_string(&fixture.config).unwrap();
    let config = config.replace(
        "optional_note: {omit: true}",
        "optional_note: {}\n            future_optional: {required: true}",
    );
    fs::write(&fixture.config, config).unwrap();

    let mut session = Session::start(&fixture);
    let listed = session.send(list(1));
    let schema = &tool(&listed, "convert_time")["inputSchema"];
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("future_optional")));
    assert!(!required.contains(&json!("optional_note")));

    let missing_required = session.send(call(
        2,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(missing_required["result"]["isError"], true);
    assert_eq!(
        missing_required["result"]["content"][0]["text"],
        "INVALID_ARGUMENTS: required"
    );

    let without_optional = session.send(call(
        3,
        "convert_time",
        json!({"time":"12:30","timezone":"JST","future_optional":true}),
    ));
    assert_eq!(without_optional["result"]["isError"], false);
    assert_eq!(
        without_optional["result"]["structuredContent"]["received"].get("optional_note"),
        None
    );

    let with_optional = session.send(call(
        4,
        "convert_time",
        json!({
            "time":"12:30",
            "timezone":"JST",
            "future_optional":true,
            "optional_note":"for the operator"
        }),
    ));
    assert_eq!(with_optional["result"]["isError"], false);
    assert_eq!(
        with_optional["result"]["structuredContent"]["received"]["optional_note"],
        "for the operator"
    );
    session.finish();

    assert_eq!(
        upstream_events(&fixture.events),
        ["list", "call:convert", "call:convert"]
    );
}

#[test]
fn v3_notification_rebuilds_then_blocks_without_proxy_fallback() {
    let fixture = write_config("list-changed", false);
    let mut session = Session::start(&fixture);
    let first = session.send(list(1));
    assert!(
        tool(&first, "convert_time")["inputSchema"]["properties"]
            .get("admin")
            .is_none()
    );
    fs::write(&fixture.state, "notify-required").unwrap();
    assert_eq!(
        session.send(call(
            2,
            "convert_time",
            json!({"time":"12:30","timezone":"JST"})
        ))["result"]["isError"],
        false
    );
    assert_eq!(session.notifications.len(), 1);
    assert_eq!(
        session.notifications[0]["method"],
        "notifications/tools/list_changed"
    );
    assert_eq!(
        upstream_events(&fixture.events),
        ["list", "call:convert", "list"]
    );

    let current = session.send(list(3));
    let blocked = tool(&current, "convert_time");
    assert_eq!(blocked["inputSchema"]["properties"], json!({}));
    assert!(
        blocked["description"]
            .as_str()
            .unwrap()
            .contains("REQUIRED_PROPERTY_UNSATISFIED")
    );
    let blocked_call = session.send(call(
        4,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(
        blocked_call["result"]["structuredContent"]["reason"],
        "REQUIRED_PROPERTY_UNSATISFIED"
    );
    assert_eq!(
        upstream_events(&fixture.events),
        ["list", "call:convert", "list"]
    );
    session.finish();
}

#[test]
fn v3_stale_cache_survives_outage_but_calls_require_a_fresh_connection() {
    let fixture = write_config("outage", false);
    let mut initial = Session::start(&fixture);
    assert!(names(&initial.send(list(1))).contains(&"convert_time".to_owned()));
    initial.finish();
    assert!(!cached_definition_text(&fixture.cache).contains("BROKER_FIXED_SENTINEL"));

    let config = fs::read_to_string(&fixture.config).unwrap();
    fs::write(
        &fixture.config,
        config.replace(
            "expose_unlisted_properties: false",
            "expose_unlisted_properties: true",
        ),
    )
    .unwrap();
    fs::write(&fixture.state, "offline").unwrap();
    let mut session = Session::start(&fixture);
    let stale = session.send(list(1));
    assert!(names(&stale).contains(&"convert_time".to_owned()));
    assert!(
        tool(&stale, "convert_time")["inputSchema"]["properties"]
            .get("future_optional")
            .is_some()
    );
    let unavailable = session.send(call(
        2,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(
        unavailable["result"]["structuredContent"]["code"],
        "TARGET_UNAVAILABLE"
    );
    assert_eq!(
        unavailable["result"]["structuredContent"]["retryable"],
        true
    );

    fs::write(&fixture.state, "online").unwrap();
    assert!(names(&session.send(list(3))).contains(&"convert_time".to_owned()));
    let recovered = session.send(call(
        4,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(recovered["result"]["isError"], false);
    session.finish();
}

#[test]
fn v3_cache_is_reused_for_policy_changes_but_not_for_a_changed_target_identity() {
    let fixture = write_config("cache-identity", false);
    let mut initial = Session::start(&fixture);
    let first = initial.send(list(1));
    assert!(names(&first).contains(&"convert_time".to_owned()));
    initial.finish();

    let config = fs::read_to_string(&fixture.config).unwrap();
    let changed_identity = config.replace("timeout_ms: 2000", "timeout_ms: 2001");
    assert_ne!(changed_identity, config);
    fs::write(&fixture.config, changed_identity).unwrap();
    fs::write(&fixture.state, "offline").unwrap();

    let mut session = Session::start(&fixture);
    let offline = session.send(list(1));
    let unavailable = tool(&offline, "convert_time");
    assert!(
        unavailable["description"]
            .as_str()
            .unwrap()
            .contains("metadata has not been retrieved")
    );
    let rejected = session.send(call(2, "convert_time", json!({"time":"12:30"})));
    assert_eq!(
        rejected["result"]["structuredContent"]["code"],
        "TARGET_UNAVAILABLE"
    );

    fs::write(&fixture.state, "online").unwrap();
    let recovered = session.send(list(3));
    assert!(names(&recovered).contains(&"convert_time".to_owned()));
    let result = session.send(call(
        4,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(result["result"]["isError"], false);
    session.finish();

    let cache_root = fixture.cache.join("mcp-boundary/catalog-v3");
    assert_eq!(fs::read_dir(cache_root).unwrap().count(), 2);
}

#[test]
fn v3_environment_values_do_not_create_persistent_cache_fingerprints() {
    let fixture = write_config("environment-cache", false);
    let config = fs::read_to_string(&fixture.config).unwrap();
    let config = config.replace(
        "      args: []\n",
        "      args: []\n      environment: {inherit: [MCP_TEST_CREDENTIAL]}\n",
    );
    assert_ne!(config, fs::read_to_string(&fixture.config).unwrap());
    fs::write(&fixture.config, config).unwrap();

    let mut session = Session::start(&fixture);
    let listed = session.send(list(1));
    assert!(names(&listed).contains(&"convert_time".to_owned()));
    let result = session.send(call(
        2,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(result["result"]["isError"], false);
    session.finish();

    assert!(!fixture.cache.exists());
}

#[test]
fn v3_credential_launch_arguments_disable_persistent_cache_but_not_calls() {
    let fixture = write_config("argument-cache", false);
    let config = fs::read_to_string(&fixture.config).unwrap();
    let config = config.replace(
        "args: []",
        &format!(
            "args: [v3-fixture, \"--token=7\", \"--marker={}\", \"--events={}\"]",
            fixture.state.display(),
            fixture.events.display(),
        ),
    );
    assert_ne!(config, fs::read_to_string(&fixture.config).unwrap());
    fs::write(&fixture.config, config).unwrap();

    for first_id in [1, 3] {
        let mut session = Session::start(&fixture);
        let listed = session.send(list(first_id));
        assert!(names(&listed).contains(&"convert_time".to_owned()));
        let result = session.send(call(
            first_id + 1,
            "convert_time",
            json!({"time":"12:30","timezone":"JST"}),
        ));
        assert_eq!(result["result"]["isError"], false);
        session.finish();
        assert!(!fixture.cache.exists());
    }

    assert_eq!(
        upstream_events(&fixture.events),
        ["list", "call:convert", "list", "call:convert"]
    );
}

#[test]
fn v3_cache_write_failure_does_not_discard_fresh_upstream_definitions() {
    let fixture = write_config("cache-write-failure", false);
    fs::write(&fixture.cache, "not a directory").unwrap();
    let mut session = Session::start(&fixture);

    let listed = session.send(list(1));
    assert!(names(&listed).contains(&"convert_time".to_owned()));
    let result = session.send(call(
        2,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(result["result"]["isError"], false);
    session.finish();

    assert_eq!(upstream_events(&fixture.events), ["list", "call:convert"]);
}

#[test]
fn v3_upstream_rejection_is_not_reported_as_a_policy_mismatch() {
    let fixture = write_config("upstream-reject", false);
    let mut session = Session::start(&fixture);
    let _ = session.send(list(1));
    fs::write(&fixture.state, "reject-call").unwrap();
    let rejected = session.send(call(
        2,
        "convert_time",
        json!({"time":"12:30","timezone":"JST"}),
    ));
    assert_eq!(rejected["result"]["isError"], true);
    assert!(
        rejected["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("TARGET_FAILED:")
    );
    let after = session.send(list(3));
    assert_eq!(
        tool(&after, "convert_time")["inputSchema"]["properties"]["time"]["type"],
        "string"
    );
    session.finish();
}

#[test]
fn v3_unsupported_schema_is_listed_as_blocked_without_proxy_fallback() {
    let fixture = write_config("unsupported-schema", false);
    fs::write(&fixture.state, "unsupported-schema").unwrap();
    let mut session = Session::start(&fixture);

    let listed = session.send(list(1));
    let blocked = tool(&listed, "convert_time");
    assert_eq!(
        blocked["inputSchema"],
        json!({
            "type":"object",
            "properties":{},
            "required":[],
            "additionalProperties":false
        })
    );
    assert!(
        blocked["description"]
            .as_str()
            .unwrap()
            .contains("UNSUPPORTED_SCHEMA")
    );

    let result = session.send(call(2, "convert_time", json!({})));
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["structuredContent"]["code"],
        "ADMIN_ACTION_REQUIRED"
    );
    assert_eq!(
        result["result"]["structuredContent"]["reason"],
        "UNSUPPORTED_SCHEMA"
    );
    assert_eq!(result["result"]["structuredContent"]["retryable"], false);
    session.finish();

    assert_eq!(upstream_events(&fixture.events), ["list"]);
}

#[test]
fn v3_removed_configured_property_blocks_without_upstream_call() {
    let fixture = write_config("missing-selected", false);
    fs::write(&fixture.state, "missing-selected").unwrap();
    let mut session = Session::start(&fixture);

    let listed = session.send(list(1));
    let blocked = tool(&listed, "convert_time");
    assert!(
        blocked["description"]
            .as_str()
            .unwrap()
            .contains("PROPERTY_NOT_IN_UPSTREAM")
    );

    let result = session.send(call(2, "convert_time", json!({"time":"12:30"})));
    assert_eq!(
        result["result"]["structuredContent"]["reason"],
        "PROPERTY_NOT_IN_UPSTREAM"
    );
    session.finish();

    assert_eq!(upstream_events(&fixture.events), ["list"]);
}

#[test]
fn v3_removed_selected_tool_blocks_without_upstream_call() {
    let fixture = write_config("missing-tool", false);
    fs::write(&fixture.state, "missing-tool").unwrap();
    let mut session = Session::start(&fixture);

    let listed = session.send(list(1));
    let blocked = tool(&listed, "convert_time");
    assert!(
        blocked["description"]
            .as_str()
            .unwrap()
            .contains("TOOL_NOT_FOUND")
    );

    let result = session.send(call(2, "convert_time", json!({"time":"12:30"})));
    assert_eq!(
        result["result"]["structuredContent"]["reason"],
        "TOOL_NOT_FOUND"
    );
    session.finish();

    assert_eq!(upstream_events(&fixture.events), ["list"]);
}
