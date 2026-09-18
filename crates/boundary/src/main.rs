//! Management CLI and the public STDIO boundary.

use clap::{Parser, Subcommand, ValueEnum};
use mcp_boundary_core::{
    BrokerErrorCode, Diagnostic, MAX_ARRAY_ITEMS, MAX_STRING_BYTES, ResolvedCliInvocation,
    ResolvedMcpInvocation, ValidatedConfig, parse_config, resolve_cli, resolve_mcp, serde_json,
    tools_json,
};
use mcp_boundary_runtime::{
    Admission, Cancellation, ExecutionResult, McpExecutor, RuntimeError,
    execute_cli_with_admission_and_cancellation,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

const EXIT_SUCCESS: i32 = 0;
const EXIT_USAGE: i32 = 2;
const EXIT_CONFIG: i32 = 3;
const EXIT_RUNTIME: i32 = 4;

#[derive(Debug, Parser)]
#[command(name = "mcp-boundary", version, about = "MCP capability boundary")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a configuration without starting a target.
    Check {
        #[arg(long)]
        config: String,
    },
    /// Print the normalized public tools/list catalog.
    Tools {
        #[arg(long)]
        config: String,
        #[arg(long, value_enum)]
        format: OutputFormat,
    },
    /// Start the public STDIO MCP boundary.
    Serve {
        #[arg(long)]
        config: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicRevision {
    Modern,
    Legacy,
}

struct LoadedConfig {
    config: ValidatedConfig,
}

/// Opt-in operator diagnostics for completed public tool calls.  The logger
/// deliberately owns only stable, configuration-derived labels: request
/// arguments and target output never cross this boundary.
#[derive(Clone)]
struct DebugLogger {
    enabled: bool,
    next_correlation_id: Arc<AtomicU64>,
    stderr: Arc<Mutex<io::Stderr>>,
}

impl DebugLogger {
    fn from_environment() -> Self {
        Self {
            enabled: std::env::var("MCP_BOUNDARY_LOG").as_deref() == Ok("debug"),
            next_correlation_id: Arc::new(AtomicU64::new(1)),
            stderr: Arc::new(Mutex::new(io::stderr())),
        }
    }

    fn begin(&self, tool: &str, target: Option<&str>) -> DebugCall {
        DebugCall {
            logger: self.clone(),
            correlation_id: self.next_correlation_id.fetch_add(1, Ordering::Relaxed),
            tool: tool.to_owned(),
            target: target.map(str::to_owned),
            outcome: "rejected",
            code: Some("INVALID_ARGUMENTS"),
            finished: false,
        }
    }

    fn inactive(&self) -> DebugCall {
        DebugCall {
            logger: self.clone(),
            correlation_id: 0,
            tool: String::new(),
            target: None,
            outcome: "rejected",
            code: Some("INVALID_ARGUMENTS"),
            finished: true,
        }
    }

    fn emit(&self, call: &DebugCall) {
        if !self.enabled {
            return;
        }
        let mut event = serde_json::Map::new();
        event.insert(
            "event".to_owned(),
            Value::String("tool_call_finished".to_owned()),
        );
        event.insert(
            "correlation_id".to_owned(),
            Value::Number(call.correlation_id.into()),
        );
        event.insert("tool".to_owned(), Value::String(call.tool.clone()));
        if let Some(target) = &call.target {
            event.insert("target".to_owned(), Value::String(target.clone()));
        }
        event.insert("outcome".to_owned(), Value::String(call.outcome.to_owned()));
        if let Some(code) = call.code {
            event.insert("code".to_owned(), Value::String(code.to_owned()));
        }
        let Ok(line) = serde_json::to_vec(&Value::Object(event)) else {
            return;
        };
        let Ok(mut stderr) = self.stderr.lock() else {
            return;
        };
        let _ = stderr.write_all(&line);
        let _ = stderr.write_all(b"\n");
        let _ = stderr.flush();
    }
}

struct DebugCall {
    logger: DebugLogger,
    correlation_id: u64,
    tool: String,
    target: Option<String>,
    outcome: &'static str,
    code: Option<&'static str>,
    finished: bool,
}

impl DebugCall {
    fn success(&mut self) {
        self.outcome = "success";
        self.code = None;
        self.finish();
    }

    fn error(&mut self, code: BrokerErrorCode) {
        self.outcome = "error";
        self.code = Some(code.as_str());
        self.finish();
    }

    fn rejected(&mut self, code: BrokerErrorCode) {
        self.outcome = "rejected";
        self.code = Some(code.as_str());
        self.finish();
    }

    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            self.logger.emit(self);
        }
    }
}

impl Drop for DebugCall {
    fn drop(&mut self) {
        self.finish();
    }
}

fn main() {
    let cli = Cli::parse();
    let exit = match cli.command {
        Command::Check { config } => run_check(&config),
        Command::Tools { config, format } => run_tools(&config, format),
        Command::Serve { config } => run_serve(&config),
    };
    std::process::exit(exit);
}

fn run_check(config_path: &str) -> i32 {
    match load_config(config_path) {
        Ok(_) => EXIT_SUCCESS,
        Err(error) => {
            emit_diagnostic(config_path, &error);
            EXIT_CONFIG
        }
    }
}

fn run_tools(config_path: &str, _format: OutputFormat) -> i32 {
    match load_config(config_path) {
        Ok(loaded) => match serde_json::to_string(&tools_json(&loaded.config)) {
            Ok(json) => {
                println!("{json}");
                EXIT_SUCCESS
            }
            Err(_) => {
                emit_diagnostic(
                    config_path,
                    &Diagnostic::internal("/tools", "catalog serialization failed"),
                );
                EXIT_RUNTIME
            }
        },
        Err(error) => {
            emit_diagnostic(config_path, &error);
            EXIT_CONFIG
        }
    }
}

fn run_serve(config_path: &str) -> i32 {
    let loaded = match with_loaded_config(config_path, |loaded| loaded) {
        Ok(config) => config,
        Err(error) => {
            emit_diagnostic(config_path, &error);
            return EXIT_CONFIG;
        }
    };
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => match runtime.block_on(serve_stdio(loaded)) {
            Ok(()) => EXIT_SUCCESS,
            Err(error) => {
                emit_diagnostic(config_path, &error);
                EXIT_RUNTIME
            }
        },
        Err(_) => {
            emit_diagnostic(
                config_path,
                &Diagnostic::internal("", "server runtime could not start"),
            );
            EXIT_RUNTIME
        }
    }
}

fn load_config(config_path: &str) -> Result<LoadedConfig, Diagnostic> {
    let path = Path::new(config_path);
    if !path.is_absolute() {
        return Err(Diagnostic::invalid("", "--config must be an absolute path"));
    }
    // Read one byte beyond the policy limit so an oversized file is rejected
    // before the YAML decoder sees it, without allocating the whole file.
    let file = fs::File::open(path)
        .map_err(|_| Diagnostic::invalid("", "configuration could not be read"))?;
    let mut bytes = Vec::with_capacity(mcp_boundary_core::MAX_CONFIG_BYTES.min(8192));
    file.take((mcp_boundary_core::MAX_CONFIG_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Diagnostic::invalid("", "configuration could not be read"))?;
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let config = parse_config(config_path.to_owned(), &bytes, &environment)?;
    validate_static_paths(&config)?;
    Ok(LoadedConfig { config })
}

/// Keep configuration adoption separate from execution.  The callback is
/// entered only after the complete configuration has parsed and passed static
/// validation; this is also the small seam used by the no-invocation tests.
fn with_loaded_config<F, R>(config_path: &str, execute: F) -> Result<R, Diagnostic>
where
    F: FnOnce(LoadedConfig) -> R,
{
    Ok(execute(load_config(config_path)?))
}

/// Resolve before handing an invocation to an executor.  In particular, a
/// schema or binding error cannot call the callback, even if the caller later
/// adapts that callback to an asynchronous process launch.
fn resolve_cli_then_execute<F, R>(
    config: &ValidatedConfig,
    name: &str,
    arguments: &Value,
    execute: F,
) -> Result<R, Diagnostic>
where
    F: FnOnce(ResolvedCliInvocation) -> R,
{
    Ok(execute(resolve_cli(config, name, arguments)?))
}

fn resolve_mcp_then_execute<F, R>(
    config: &ValidatedConfig,
    name: &str,
    arguments: &Value,
    execute: F,
) -> Result<R, Diagnostic>
where
    F: FnOnce(ResolvedMcpInvocation) -> R,
{
    Ok(execute(resolve_mcp(config, name, arguments)?))
}

fn validate_static_paths(config: &ValidatedConfig) -> Result<(), Diagnostic> {
    for (id, target) in &config.targets {
        validate_executable(&target.executable, &format!("/targets/{id}/executable"))?;
        validate_directory(&target.cwd, &format!("/targets/{id}/cwd"))?;
    }
    for (id, target) in &config.upstream_targets {
        validate_executable(&target.command, &format!("/targets/{id}/transport/command"))?;
        validate_directory(&target.cwd, &format!("/targets/{id}/transport/cwd"))?;
    }
    Ok(())
}

fn validate_executable(path: &Path, config_path: &str) -> Result<(), Diagnostic> {
    let metadata = fs::metadata(path)
        .map_err(|_| Diagnostic::invalid(config_path, "executable does not exist"))?;
    if !metadata.is_file() {
        return Err(Diagnostic::invalid(
            config_path,
            "executable is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Diagnostic::invalid(
                config_path,
                "executable is not executable",
            ));
        }
    }
    Ok(())
}

fn validate_directory(path: &Path, config_path: &str) -> Result<(), Diagnostic> {
    let metadata =
        fs::metadata(path).map_err(|_| Diagnostic::invalid(config_path, "cwd does not exist"))?;
    if !metadata.is_dir() {
        return Err(Diagnostic::invalid(config_path, "cwd is not a directory"));
    }
    Ok(())
}

fn emit_diagnostic(config_path: &str, diagnostic: &Diagnostic) {
    let output = json!({
        "code": diagnostic.code.as_str(),
        "message": diagnostic.message,
        "config_path": config_path,
        "path": diagnostic.path,
    });
    match serde_json::to_string(&output) {
        Ok(value) => eprintln!("{value}"),
        Err(_) => {
            eprintln!(
                "{{\"code\":\"TARGET_FAILED\",\"message\":\"diagnostic serialization failed\"}}"
            )
        }
    }
}

async fn serve_stdio(loaded: LoadedConfig) -> Result<(), Diagnostic> {
    let stdout = Arc::new(AsyncMutex::new(io::BufWriter::new(io::stdout())));
    let config = Arc::new(loaded.config);
    let debug = DebugLogger::from_environment();
    let admission = Admission::new();
    let cancellation = Cancellation::new();
    let mcp = McpExecutor::with_admission(admission.clone());
    let revision = Arc::new(AsyncMutex::new(PublicRevision::Legacy));
    let mut tasks = Vec::new();
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut shutdown_signal = Box::pin(wait_for_shutdown_signal());
    let mut input_error = false;
    loop {
        let message = tokio::select! {
            message = read_bounded_message(&mut stdin, config.request_bytes) => {
                match message {
                    Ok(message) => message,
                    Err(_) => {
                        input_error = true;
                        None
                    }
                }
            }
            _ = &mut shutdown_signal => {
                break;
            },
        };
        let Some(message) = message else { break };
        let line = match message {
            BoundedMessage::Complete(line) => line,
            BoundedMessage::TooLarge => {
                let stdout = Arc::clone(&stdout);
                tasks.push(tokio::spawn(async move {
                    let mut stdout = stdout.lock().await;
                    write_json_error(
                        &mut stdout,
                        None,
                        "INVALID_ARGUMENTS",
                        "request exceeds configured limit",
                    )
                }));
                // Once a delimiter-free message has crossed the limit, stop
                // consuming the connection.  Continuing would wait forever
                // for a newline (or reinterpret the tail as another request)
                // and would leave the framing boundary ambiguous.
                break;
            }
        };
        if line.len() > config.request_bytes {
            let stdout = Arc::clone(&stdout);
            tasks.push(tokio::spawn(async move {
                let mut stdout = stdout.lock().await;
                write_json_error(
                    &mut stdout,
                    None,
                    "INVALID_ARGUMENTS",
                    "request exceeds configured limit",
                )
            }));
            continue;
        }
        let request: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                let stdout = Arc::clone(&stdout);
                tasks.push(tokio::spawn(async move {
                    let mut stdout = stdout.lock().await;
                    write_json_error(
                        &mut stdout,
                        None,
                        "INVALID_ARGUMENTS",
                        "invalid JSON request",
                    )
                }));
                continue;
            }
        };
        if !request_within_limits(&request, 1, config.json_depth) {
            let stdout = Arc::clone(&stdout);
            tasks.push(tokio::spawn(async move {
                let mut stdout = stdout.lock().await;
                write_json_error(
                    &mut stdout,
                    None,
                    "INVALID_ARGUMENTS",
                    "request exceeds configured JSON limits",
                )
            }));
            continue;
        }
        let context = RequestContext {
            config: Arc::clone(&config),
            stdout: Arc::clone(&stdout),
            mcp: mcp.clone(),
            admission: admission.clone(),
            cancellation: cancellation.clone(),
            revision: Arc::clone(&revision),
            debug: debug.clone(),
        };
        tasks.push(tokio::spawn(async move {
            handle_request(context, request).await
        }));
    }
    // Public EOF and process signals both start broker shutdown. Active
    // requests use the same cancellation path, which terminates and reaps
    // their target processes before the broker exits. A client that wants to
    // close stdin after a completed response can read that response first.
    cancellation.cancel();
    let mut task_error = None;
    for task in tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if task_error.is_none() => task_error = Some(error),
            Ok(Err(_)) => {}
            Err(_) if task_error.is_none() => {
                task_error = Some(Diagnostic::internal("", "request task failed"));
            }
            Err(_) => {}
        }
    }
    mcp.shutdown().await;
    if let Some(error) = task_error {
        return Err(error);
    }
    if input_error {
        Err(Diagnostic::internal("", "stdin read failed"))
    } else {
        Ok(())
    }
}

enum BoundedMessage {
    Complete(Vec<u8>),
    TooLarge,
}

/// Read one newline-delimited JSON message without allowing the framing
/// adapter to allocate an unbounded buffer. The delimiter is not part of the
/// message size, and CRLF is normalized to the same bytes as `lines()`.
async fn read_bounded_message<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<BoundedMessage>> {
    let mut message = Vec::with_capacity(limit.saturating_add(1).min(8192));
    let mut length = 0usize;
    let mut too_large = false;

    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            if length == 0 {
                return Ok(None);
            }
            if too_large {
                return Ok(Some(BoundedMessage::TooLarge));
            }
            if message.last() == Some(&b'\r') {
                message.pop();
            }
            return Ok(Some(BoundedMessage::Complete(message)));
        }

        let delimiter = buffer.iter().position(|byte| *byte == b'\n');
        let body_len = delimiter.unwrap_or(buffer.len());
        if body_len > limit.saturating_sub(length) {
            too_large = true;
        }
        if !too_large {
            message.extend_from_slice(&buffer[..body_len]);
        }
        length = length.saturating_add(body_len);

        let consumed = delimiter.map_or(body_len, |position| position + 1);
        reader.consume(consumed);
        if delimiter.is_some() {
            if too_large {
                return Ok(Some(BoundedMessage::TooLarge));
            }
            if message.last() == Some(&b'\r') {
                message.pop();
            }
            return Ok(Some(BoundedMessage::Complete(message)));
        }
        if too_large {
            return Ok(Some(BoundedMessage::TooLarge));
        }
    }
}

fn request_within_limits(value: &Value, depth: usize, max_depth: usize) -> bool {
    if depth > max_depth {
        return false;
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(value) => value.len() <= MAX_STRING_BYTES,
        Value::Array(values) => {
            values.len() <= MAX_ARRAY_ITEMS
                && values
                    .iter()
                    .all(|value| request_within_limits(value, depth + 1, max_depth))
        }
        Value::Object(values) => {
            values.keys().all(|key| key.len() <= MAX_STRING_BYTES)
                && values
                    .values()
                    .all(|value| request_within_limits(value, depth + 1, max_depth))
        }
    }
}

#[cfg(unix)]
fn wait_for_shutdown_signal() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(mut sigint) = signal(SignalKind::interrupt()) else {
        return Box::pin(std::future::pending());
    };
    let Ok(mut sigterm) = signal(SignalKind::terminate()) else {
        return Box::pin(std::future::pending());
    };
    Box::pin(async move {
        tokio::select! {
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
    })
}

#[cfg(not(unix))]
fn wait_for_shutdown_signal() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(std::future::pending())
}

fn validate_envelope(request: &Value) -> Option<&'static str> {
    let Some(object) = request.as_object() else {
        return Some("request must be an object");
    };
    for key in object.keys() {
        if !matches!(key.as_str(), "jsonrpc" | "id" | "method" | "params") {
            return Some("unknown JSON-RPC envelope field");
        }
    }
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some("jsonrpc must be exactly 2.0");
    }
    if object.get("method").and_then(Value::as_str).is_none() {
        return Some("method is required");
    }
    if let Some(id) = object.get("id")
        && !matches!(id, Value::Null | Value::String(_) | Value::Number(_))
    {
        return Some("id must be a string, number, or null");
    }
    if let Some(params) = object.get("params")
        && !params.is_object()
    {
        return Some("params must be an object");
    }
    None
}

fn validate_params(method: &str, params: &Value) -> Option<&'static str> {
    let Some(object) = params.as_object() else {
        return Some("params must be an object");
    };
    let allowed = match method {
        "initialize" => &["protocolVersion", "capabilities", "clientInfo", "_meta"][..],
        "server/discover" | "notifications/initialized" => &["_meta"][..],
        "tools/list" => &["cursor", "_meta"][..],
        "tools/call" => &["name", "arguments", "_meta"][..],
        _ => &[][..],
    };
    if object
        .keys()
        .any(|key| !allowed.iter().any(|candidate| candidate == key))
    {
        return Some("unknown method parameter");
    }
    match method {
        "initialize" => {
            if object
                .get("protocolVersion")
                .is_some_and(|value| !value.is_string())
                || object
                    .get("capabilities")
                    .is_some_and(|value| !value.is_object())
                || object
                    .get("clientInfo")
                    .is_some_and(|value| !value.is_object())
            {
                return Some("invalid initialize parameters");
            }
        }
        "tools/list" => {
            if object.get("cursor").is_some_and(|value| !value.is_string()) {
                return Some("cursor must be a string");
            }
        }
        "tools/call" => {
            if object.get("name").and_then(Value::as_str).is_none() && object.contains_key("name") {
                return Some("tool name must be a string");
            }
            if object
                .get("arguments")
                .is_some_and(|value| !value.is_object())
            {
                return Some("tool arguments must be an object");
            }
        }
        _ => {}
    }
    None
}

struct RequestContext {
    config: Arc<ValidatedConfig>,
    stdout: Arc<AsyncMutex<io::BufWriter<io::Stdout>>>,
    mcp: McpExecutor,
    admission: Admission,
    cancellation: Cancellation,
    revision: Arc<AsyncMutex<PublicRevision>>,
    debug: DebugLogger,
}

async fn handle_request(context: RequestContext, request: Value) -> Result<(), Diagnostic> {
    let RequestContext {
        config,
        stdout,
        mcp,
        admission,
        cancellation,
        revision,
        debug,
    } = context;
    let id = request.get("id").cloned();
    if let Some(message) = validate_envelope(&request) {
        write_error(&stdout, id, "INVALID_ARGUMENTS", message).await?;
        return Ok(());
    }
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let mut debug_call = if method == "tools/call" {
        let name = params.get("name").and_then(Value::as_str);
        let known_tool = name.filter(|name| config.tools.contains_key(*name));
        let target = known_tool
            .and_then(|name| config.tools.get(name))
            .map(|tool| tool.target.as_str());
        debug.begin(known_tool.unwrap_or("unknown"), target)
    } else {
        // This call is never emitted because it is not a public tools/call.
        debug.inactive()
    };
    if let Some(message) = validate_params(method, &params) {
        if method == "tools/call" {
            debug_call.rejected(BrokerErrorCode::InvalidArguments);
        }
        write_error(&stdout, id, "INVALID_ARGUMENTS", message).await?;
        return Ok(());
    }
    match method {
        "initialize" => {
            if id.is_none() {
                write_error(
                    &stdout,
                    id,
                    "INVALID_ARGUMENTS",
                    "initialize requires an id",
                )
                .await?;
                return Ok(());
            }
            let selected = match params.get("protocolVersion").and_then(Value::as_str) {
                None | Some("2025-11-25") => PublicRevision::Legacy,
                Some("2026-07-28") => PublicRevision::Modern,
                Some(_) => {
                    write_error(
                        &stdout,
                        id,
                        "INVALID_ARGUMENTS",
                        "unsupported protocol version",
                    )
                    .await?;
                    return Ok(());
                }
            };
            *revision.lock().await = selected;
            write_result(
                &stdout,
                id,
                json!({
                    "protocolVersion": match selected {
                        PublicRevision::Modern => "2026-07-28",
                        PublicRevision::Legacy => "2025-11-25",
                    },
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": config.server_name, "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?
        }
        "server/discover" => {
            if id.is_none() {
                write_error(
                    &stdout,
                    id,
                    "INVALID_ARGUMENTS",
                    "server/discover requires an id",
                )
                .await?;
                return Ok(());
            }
            *revision.lock().await = PublicRevision::Modern;
            write_result(
                &stdout,
                id,
                json!({
                    "protocolVersion": "2026-07-28",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": config.server_name, "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?
        }
        "notifications/initialized" => {
            if id.is_some() {
                write_error(
                    &stdout,
                    id,
                    "INVALID_ARGUMENTS",
                    "notification must not have an id",
                )
                .await?;
            }
        }
        "tools/list" => {
            if id.is_none() {
                write_error(
                    &stdout,
                    id,
                    "INVALID_ARGUMENTS",
                    "tools/list requires an id",
                )
                .await?;
                return Ok(());
            }
            if params.get("cursor").is_some() {
                write_error(
                    &stdout,
                    id,
                    "INVALID_ARGUMENTS",
                    "pagination is unsupported",
                )
                .await?;
            } else {
                write_result(&stdout, id, tools_json(&config)).await?;
            }
        }
        "tools/call" => {
            if id.is_none() {
                write_error(
                    &stdout,
                    id,
                    "INVALID_ARGUMENTS",
                    "tools/call requires an id",
                )
                .await?;
                return Ok(());
            }
            let name = params.get("name").and_then(Value::as_str);
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let Some(name) = name else {
                debug_call.rejected(BrokerErrorCode::InvalidArguments);
                write_error(&stdout, id, "INVALID_ARGUMENTS", "tool name is required").await?;
                return Ok(());
            };
            if cancellation.is_cancelled() {
                debug_call.rejected(BrokerErrorCode::TargetUnavailable);
                write_error(&stdout, id, "TARGET_UNAVAILABLE", "broker is shutting down").await?;
                return Ok(());
            }
            if !config.tools.contains_key(name) {
                debug_call.rejected(BrokerErrorCode::InvalidArguments);
                write_error(&stdout, id, "INVALID_ARGUMENTS", "unknown tool").await?;
                return Ok(());
            }
            if config
                .tools
                .get(name)
                .and_then(|tool| tool.mcp.as_ref())
                .is_some()
            {
                match resolve_mcp_then_execute(&config, name, &arguments, |invocation| {
                    mcp.execute_with_cancellation(invocation, &cancellation)
                }) {
                    Ok(execution) => match execution.await {
                        Ok(result) => {
                            if result.is_error {
                                debug_call.error(result.code);
                            } else {
                                debug_call.success();
                            }
                            write_execution(&stdout, id, result, *revision.lock().await).await?
                        }
                        Err(error) => {
                            debug_call.error(error.code);
                            write_runtime_error_async(&stdout, id, error, *revision.lock().await)
                                .await?
                        }
                    },
                    Err(error) => {
                        debug_call.rejected(error.code);
                        write_tool_error_async(
                            &stdout,
                            id,
                            error.code.as_str(),
                            &error.message,
                            *revision.lock().await,
                        )
                        .await?
                    }
                }
            } else {
                match resolve_cli_then_execute(&config, name, &arguments, |invocation| {
                    execute_cli_with_admission_and_cancellation(
                        invocation,
                        &admission,
                        &cancellation,
                    )
                }) {
                    Ok(execution) => match execution.await {
                        Ok(result) => {
                            if result.is_error {
                                debug_call.error(result.code);
                            } else {
                                debug_call.success();
                            }
                            write_execution(&stdout, id, result, *revision.lock().await).await?
                        }
                        Err(error) => {
                            debug_call.error(error.code);
                            write_runtime_error_async(&stdout, id, error, *revision.lock().await)
                                .await?
                        }
                    },
                    Err(error) => {
                        debug_call.rejected(error.code);
                        write_tool_error_async(
                            &stdout,
                            id,
                            error.code.as_str(),
                            &error.message,
                            *revision.lock().await,
                        )
                        .await?
                    }
                }
            }
        }
        _ => write_error(&stdout, id, "INVALID_ARGUMENTS", "unsupported method").await?,
    }
    Ok(())
}

async fn write_result(
    stdout: &Arc<AsyncMutex<io::BufWriter<io::Stdout>>>,
    id: Option<Value>,
    result: Value,
) -> Result<(), Diagnostic> {
    let mut stdout = stdout.lock().await;
    write_json_result(&mut stdout, id, result)
}

async fn write_error(
    stdout: &Arc<AsyncMutex<io::BufWriter<io::Stdout>>>,
    id: Option<Value>,
    code: &str,
    message: &str,
) -> Result<(), Diagnostic> {
    let mut stdout = stdout.lock().await;
    write_json_error(&mut stdout, id, code, message)
}

async fn write_execution(
    stdout: &Arc<AsyncMutex<io::BufWriter<io::Stdout>>>,
    id: Option<Value>,
    result: ExecutionResult,
    revision: PublicRevision,
) -> Result<(), Diagnostic> {
    let mut stdout = stdout.lock().await;
    write_execution_result(&mut stdout, id, result, revision)
}

async fn write_tool_error_async(
    stdout: &Arc<AsyncMutex<io::BufWriter<io::Stdout>>>,
    id: Option<Value>,
    code: &str,
    message: &str,
    revision: PublicRevision,
) -> Result<(), Diagnostic> {
    let mut stdout = stdout.lock().await;
    write_tool_error(&mut stdout, id, code, message, revision)
}

async fn write_runtime_error_async(
    stdout: &Arc<AsyncMutex<io::BufWriter<io::Stdout>>>,
    id: Option<Value>,
    error: RuntimeError,
    revision: PublicRevision,
) -> Result<(), Diagnostic> {
    let mut stdout = stdout.lock().await;
    if error.code == BrokerErrorCode::ServerBusy {
        write_json_error(&mut stdout, id, error.code.as_str(), error.message)
    } else {
        write_tool_error(
            &mut stdout,
            id,
            error.code.as_str(),
            error.message,
            revision,
        )
    }
}

fn write_execution_result(
    stdout: &mut io::BufWriter<io::Stdout>,
    id: Option<Value>,
    result: ExecutionResult,
    revision: PublicRevision,
) -> Result<(), Diagnostic> {
    if result.is_error {
        write_tool_error(stdout, id, result.code.as_str(), "target failed", revision)
    } else {
        let text_blocks = if result.text_blocks.is_empty() {
            result.text.into_iter().collect::<Vec<_>>()
        } else {
            result.text_blocks
        };
        let content = text_blocks
            .into_iter()
            .map(|text| json!({"type":"text","text":text}))
            .collect::<Vec<_>>();
        let response = if let Some(structured) = result.structured {
            json!({"content": content, "structuredContent": structured, "isError": false, "resultType": "complete"})
        } else {
            json!({"content": content, "isError": false, "resultType": "complete"})
        };
        write_json_result(stdout, id, add_revision_fields(response, revision))
    }
}

fn write_tool_error(
    stdout: &mut io::BufWriter<io::Stdout>,
    id: Option<Value>,
    code: &str,
    message: &str,
    revision: PublicRevision,
) -> Result<(), Diagnostic> {
    let result = add_revision_fields(
        json!({"content":[{"type":"text","text":format!("{code}: {message}")}],"isError":true,"resultType":"complete"}),
        revision,
    );
    write_json_result(stdout, id, result)
}

fn add_revision_fields(mut result: Value, revision: PublicRevision) -> Value {
    if revision == PublicRevision::Legacy
        && let Some(object) = result.as_object_mut()
    {
        object.remove("resultType");
    }
    result
}

fn write_json_result(
    stdout: &mut io::BufWriter<io::Stdout>,
    id: Option<Value>,
    result: Value,
) -> Result<(), Diagnostic> {
    write_json_line(stdout, &json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn write_json_error(
    stdout: &mut io::BufWriter<io::Stdout>,
    id: Option<Value>,
    code: &str,
    message: &str,
) -> Result<(), Diagnostic> {
    write_json_line(
        stdout,
        &json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    )
}

fn write_json_line(
    stdout: &mut io::BufWriter<io::Stdout>,
    value: &Value,
) -> Result<(), Diagnostic> {
    let line = serde_json::to_string(value)
        .map_err(|_| Diagnostic::internal("", "response serialization failed"))?;
    stdout
        .write_all(line.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
        .map_err(|_| Diagnostic::internal("", "stdout write failed"))
}

#[allow(dead_code)]
const _: i32 = EXIT_USAGE;
#[allow(dead_code)]
fn _runtime_error_code(error: &RuntimeError) -> BrokerErrorCode {
    error.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    fn test_config(tool: &str, schema: &str, argv: &str) -> ValidatedConfig {
        let source = format!(
            r#"version: 1
server:
  name: test
  transport: {{kind: stdio}}
targets:
  fake:
    kind: cli
    executable: /bin/echo
    cwd: /
    limits: {{timeout_ms: 1000, stdout_bytes: 1024, stderr_bytes: 1024}}
tools:
  {tool}:
    input_schema: {schema}
    invoke: {{target: fake, cli: {{argv: {argv}}}}}
    output: {{kind: text}}
"#,
            tool = tool,
            schema = schema,
            argv = argv,
        );
        parse_config("test", source.as_bytes(), &BTreeMap::new()).expect("valid test config")
    }

    #[test]
    fn parse_schema_and_binding_failures_do_not_enter_executor() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.work")
            .join(format!("boundary-unit-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create unit test directory");
        let invalid_path = root.join("invalid.yaml");
        fs::write(&invalid_path, "not: [valid").expect("write invalid config");
        let parse_calls = Cell::new(0usize);
        assert!(
            with_loaded_config(invalid_path.to_str().unwrap(), |_| {
                parse_calls.set(parse_calls.get() + 1);
            })
            .is_err()
        );
        assert_eq!(parse_calls.get(), 0);

        let schema_config = test_config(
            "schema",
            "{type: object, properties: {value: {type: string}}, required: [value], additionalProperties: false}",
            "[{literal: fixed}, {input: /value}]",
        );
        let schema_calls = Cell::new(0usize);
        assert!(
            resolve_cli_then_execute(
                &schema_config,
                "schema",
                &serde_json::json!({"value": 7}),
                |_| schema_calls.set(schema_calls.get() + 1),
            )
            .is_err()
        );
        assert_eq!(schema_calls.get(), 0);

        let binding_config = test_config(
            "binding",
            "{type: object, properties: {value: {type: string}}, required: [value], additionalProperties: false}",
            "[{input: /value}]",
        );
        let binding_calls = Cell::new(0usize);
        assert!(
            resolve_cli_then_execute(
                &binding_config,
                "binding",
                &serde_json::json!({"value": "bad\u{0}value"}),
                |_| binding_calls.set(binding_calls.get() + 1),
            )
            .is_err()
        );
        assert_eq!(binding_calls.get(), 0);
    }
}
