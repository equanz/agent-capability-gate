//! Process adapters for validated core invocations.

use mcp_boundary_core::{
    BrokerErrorCode, MAX_ARRAY_ITEMS, MAX_JSON_DEPTH, MAX_STRING_BYTES, OutputKind,
    ResolvedCliInvocation, ResolvedMcpInvocation, canonicalize,
};
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::{Pid, setpgid};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
#[cfg(unix)]
use std::{io, os::unix::process::CommandExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

/// Maximum size of one newline-delimited message on the upstream MCP wire.
/// This is a transport bound, independent of the configured public result
/// limit.  The latter applies to the adapted target result only.
const UPSTREAM_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub struct ExecutionResult {
    pub code: BrokerErrorCode,
    pub is_error: bool,
    pub text: Option<String>,
    /// Text blocks as received from an upstream MCP result, in wire order.
    /// `text` remains the compatibility projection used by the runtime tests
    /// and the CLI adapter; public MCP serialization uses this vector.
    pub text_blocks: Vec<String>,
    pub structured: Option<Value>,
}

#[derive(Debug)]
pub struct RuntimeError {
    pub code: BrokerErrorCode,
    pub message: &'static str,
}

/// A target-scoped MCP actor collection.  The process, negotiated revision,
/// and in-flight request are kept behind the actor's mutex; callers only ever
/// pass an immutable, already-resolved invocation into this API.
const GLOBAL_CALL_SLOTS: usize = 16;

/// Admission shared by every target adapter in one broker process.
#[derive(Clone)]
pub struct Admission {
    slots: Arc<Semaphore>,
}

impl Admission {
    pub fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(GLOBAL_CALL_SLOTS)),
        }
    }

    fn try_acquire(&self) -> Result<OwnedSemaphorePermit, RuntimeError> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| RuntimeError::new(BrokerErrorCode::ServerBusy, "broker is busy"))
    }
}

impl Default for Admission {
    fn default() -> Self {
        Self::new()
    }
}

/// Broadcast cancellation used during broker shutdown. Active target calls
/// translate it to their normal process-group termination path.
#[derive(Clone)]
pub struct Cancellation {
    signal: Arc<watch::Sender<bool>>,
}

impl Cancellation {
    pub fn new() -> Self {
        let (signal, _) = watch::channel(false);
        Self {
            signal: Arc::new(signal),
        }
    }

    pub fn cancel(&self) {
        let _ = self.signal.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.signal.borrow()
    }

    fn receiver(&self) -> watch::Receiver<bool> {
        self.signal.subscribe()
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct McpExecutor {
    targets: Arc<Mutex<HashMap<String, Arc<McpTarget>>>>,
    admission: Admission,
}

impl Default for McpExecutor {
    fn default() -> Self {
        Self::with_admission(Admission::new())
    }
}

impl McpExecutor {
    pub fn with_admission(admission: Admission) -> Self {
        Self {
            targets: Arc::new(Mutex::new(HashMap::new())),
            admission,
        }
    }
}

struct McpTarget {
    actor: Mutex<McpSession>,
}

struct McpSession {
    ready: Option<ReadyProcess>,
    starting: Option<McpProcess>,
    active_request: Option<ActiveRequest>,
}

struct ReadyProcess {
    process: McpProcess,
    revision: Revision,
    next_id: u64,
}

struct ActiveRequest {
    revision: Revision,
    id: u64,
}

#[derive(Clone, Copy)]
enum Revision {
    Modern,
    Legacy,
}

struct McpProcess {
    child: Child,
    #[cfg(unix)]
    process_group: Pid,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr_status: watch::Receiver<StderrStatus>,
    stderr_task: JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StderrStatus {
    Draining,
    Done,
    Overflow,
    Failed,
}

#[derive(Debug)]
enum McpCallError {
    Runtime(RuntimeError),
    StderrOverflow,
    StderrFailed,
    ConnectionClosed,
    Protocol,
    RemoteError,
    Output,
    MessageLimit,
}

impl McpCallError {
    fn runtime(code: BrokerErrorCode, message: &'static str) -> Self {
        Self::Runtime(RuntimeError::new(code, message))
    }
}

impl From<RuntimeError> for McpCallError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

impl McpExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute one resolved upstream invocation.  A target has no waiting
    /// queue: a second call while its actor is active receives SERVER_BUSY.
    pub async fn execute(
        &self,
        invocation: ResolvedMcpInvocation,
    ) -> Result<ExecutionResult, RuntimeError> {
        self.execute_with_cancellation(invocation, &Cancellation::new())
            .await
    }

    pub async fn execute_with_cancellation(
        &self,
        invocation: ResolvedMcpInvocation,
        cancellation: &Cancellation,
    ) -> Result<ExecutionResult, RuntimeError> {
        if cancellation.is_cancelled() {
            return Err(RuntimeError::new(
                BrokerErrorCode::Cancelled,
                "broker is shutting down",
            ));
        }
        let _global_slot = self.admission.try_acquire()?;
        let target = {
            let mut targets = self.targets.lock().await;
            targets
                .entry(invocation.target_id.clone())
                .or_insert_with(|| {
                    Arc::new(McpTarget {
                        actor: Mutex::new(McpSession {
                            ready: None,
                            starting: None,
                            active_request: None,
                        }),
                    })
                })
                .clone()
        };
        let mut actor = target.actor.try_lock().map_err(|_| {
            RuntimeError::new(BrokerErrorCode::ServerBusy, "upstream target is busy")
        })?;
        let deadline = Duration::from_millis(invocation.limits.timeout_ms);
        let mut shutdown = cancellation.receiver();
        let result = tokio::select! {
            result = timeout(deadline, actor.call(&invocation)) => result,
            _ = wait_for_cancellation(&mut shutdown) => {
                actor.cancel_and_terminate().await;
                return Err(RuntimeError::new(
                    BrokerErrorCode::Cancelled,
                    "broker is shutting down",
                ));
            }
        };
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                actor.terminate().await;
                Err(match error {
                    McpCallError::Runtime(value) => value,
                    McpCallError::StderrOverflow => RuntimeError::new(
                        BrokerErrorCode::OutputLimitExceeded,
                        "upstream stderr exceeded configured limit",
                    ),
                    McpCallError::StderrFailed => RuntimeError::new(
                        BrokerErrorCode::TargetFailed,
                        "upstream stderr could not be read",
                    ),
                    McpCallError::ConnectionClosed => RuntimeError::new(
                        BrokerErrorCode::TargetFailed,
                        "upstream connection failed",
                    ),
                    McpCallError::Protocol => RuntimeError::new(
                        BrokerErrorCode::TargetFailed,
                        "upstream protocol response was invalid",
                    ),
                    McpCallError::RemoteError => RuntimeError::new(
                        BrokerErrorCode::TargetFailed,
                        "upstream returned an error",
                    ),
                    McpCallError::Output => RuntimeError::new(
                        BrokerErrorCode::InvalidTargetOutput,
                        "upstream output was invalid",
                    ),
                    McpCallError::MessageLimit => RuntimeError::new(
                        BrokerErrorCode::OutputLimitExceeded,
                        "upstream message exceeded configured limit",
                    ),
                })
            }
            Err(_) => {
                actor.cancel_and_terminate().await;
                Err(RuntimeError::new(
                    BrokerErrorCode::TargetTimeout,
                    "upstream target timed out",
                ))
            }
        }
    }

    pub async fn shutdown(&self) {
        let targets = self
            .targets
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for target in targets {
            let mut actor = target.actor.lock().await;
            actor.terminate().await;
        }
    }
}

async fn wait_for_cancellation(signal: &mut watch::Receiver<bool>) {
    if *signal.borrow() {
        return;
    }
    let _ = signal.changed().await;
}

impl McpSession {
    async fn call(
        &mut self,
        invocation: &ResolvedMcpInvocation,
    ) -> Result<ExecutionResult, McpCallError> {
        if let Some(ready) = self.ready.as_mut()
            && ready
                .process
                .child
                .try_wait()
                .map_err(|_| McpCallError::ConnectionClosed)?
                .is_some()
        {
            self.terminate().await;
        }
        if self.ready.is_none() {
            self.ready = Some(self.negotiate(invocation).await?);
        }
        let (id, revision, request) = {
            let ready = self.ready.as_mut().expect("negotiation installed process");
            let id = ready.next_id;
            ready.next_id = ready.next_id.saturating_add(1);
            let revision = ready.revision;
            let request =
                revision.tool_request(id, &invocation.upstream_tool, &invocation.arguments);
            (id, revision, request)
        };
        send_message(
            &mut self.ready.as_mut().expect("ready process").process.stdin,
            &request,
        )
        .await
        .map_err(McpCallError::from)?;
        self.active_request = Some(ActiveRequest { revision, id });
        let reply = self
            .ready
            .as_mut()
            .expect("ready process")
            .process
            .read_reply(id)
            .await;
        self.active_request = None;
        let reply = reply?;
        let revision = self.ready.as_ref().expect("ready process").revision;
        adapt_result(
            reply,
            invocation.output_kind,
            revision,
            invocation.limits.output_bytes,
        )
    }

    async fn negotiate(
        &mut self,
        invocation: &ResolvedMcpInvocation,
    ) -> Result<ReadyProcess, McpCallError> {
        self.starting = Some(spawn_process(invocation).await?);
        let discover_id = 1;
        let discover =
            json!({"jsonrpc":"2.0","id":discover_id,"method":"server/discover","params":{}});
        let discovery = {
            let process = self.starting.as_mut().expect("starting process");
            if let Err(error) = send_message(&mut process.stdin, &discover).await {
                Err(error.into())
            } else {
                process.read_reply(discover_id).await
            }
        };
        match discovery {
            Ok(reply) if is_modern_discovery(&reply) => Ok(ReadyProcess {
                process: self.starting.take().expect("starting process"),
                revision: Revision::Modern,
                next_id: 2,
            }),
            Ok(_) | Err(McpCallError::RemoteError) => self.legacy_initialize(invocation, 2).await,
            Err(McpCallError::ConnectionClosed) => {
                self.terminate_starting().await;
                self.starting = Some(spawn_process(invocation).await?);
                self.legacy_initialize(invocation, 1).await
            }
            Err(error) => {
                self.terminate_starting().await;
                Err(error)
            }
        }
    }

    async fn legacy_initialize(
        &mut self,
        _invocation: &ResolvedMcpInvocation,
        id: u64,
    ) -> Result<ReadyProcess, McpCallError> {
        let initialize = json!({
            "jsonrpc":"2.0", "id":id, "method":"initialize",
            "params": {
                "protocolVersion":"2025-11-25", "capabilities": {},
                "clientInfo": {"name":"mcp-capability-boundary", "version":"0.1.0"}
            }
        });
        let reply = {
            let process = self.starting.as_mut().expect("starting process");
            if let Err(error) = send_message(&mut process.stdin, &initialize).await {
                Err(error.into())
            } else {
                process.read_reply(id).await
            }
        };
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                self.terminate_starting().await;
                return Err(error);
            }
        };
        if !is_legacy_initialize(&reply) {
            self.terminate_starting().await;
            return Err(McpCallError::Protocol);
        }
        let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
        let send_result = {
            let process = self.starting.as_mut().expect("starting process");
            send_message(&mut process.stdin, &initialized).await
        };
        if let Err(error) = send_result {
            self.terminate_starting().await;
            return Err(error.into());
        }
        Ok(ReadyProcess {
            process: self.starting.take().expect("starting process"),
            revision: Revision::Legacy,
            next_id: id.saturating_add(1),
        })
    }

    async fn terminate_starting(&mut self) {
        if let Some(process) = self.starting.take() {
            terminate_process(process).await;
        }
    }

    async fn cancel_and_terminate(&mut self) {
        if let Some(active) = self.active_request.take()
            && let Some(ready) = self.ready.as_mut()
        {
            let notification = active.revision.cancel_notification(active.id);
            let _ = send_message(&mut ready.process.stdin, &notification).await;
        }
        self.terminate().await;
    }

    async fn terminate(&mut self) {
        self.active_request = None;
        if let Some(ready) = self.ready.take() {
            terminate_process(ready.process).await;
        }
        if let Some(process) = self.starting.take() {
            terminate_process(process).await;
        }
    }
}

impl Revision {
    // Both supported revisions use the same tools/call envelope. Keeping the
    // selection here makes it impossible for a future revision-specific
    // transport change to accidentally bypass the negotiated session.
    fn tool_request(&self, id: u64, tool: &str, arguments: &Value) -> Value {
        match self {
            Self::Modern | Self::Legacy => json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            }),
        }
    }

    fn cancel_notification(&self, id: u64) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": id, "reason": "cancelled"},
        })
    }
}

async fn spawn_process(invocation: &ResolvedMcpInvocation) -> Result<McpProcess, McpCallError> {
    let mut command = Command::new(&invocation.command);
    command
        .args(&invocation.args)
        .current_dir(&invocation.cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    unsafe {
        command.as_std_mut().pre_exec(|| {
            setpgid(Pid::from_raw(0), Pid::from_raw(0))
                .map_err(|error| io::Error::other(format!("setpgid failed: {error}")))
        });
    }
    for (name, value) in &invocation.environment.inherited_values {
        command.env(name, value);
    }
    for (name, value) in &invocation.environment.set {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|_| {
        McpCallError::runtime(
            BrokerErrorCode::TargetUnavailable,
            "upstream target could not be started",
        )
    })?;
    #[cfg(unix)]
    let process_group = match child.id() {
        Some(pid) => Pid::from_raw(pid as i32),
        None => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(McpCallError::runtime(
                BrokerErrorCode::TargetUnavailable,
                "upstream process had no pid",
            ));
        }
    };
    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate_cli_process(&mut child, process_group).await;
            return Err(McpCallError::runtime(
                BrokerErrorCode::TargetUnavailable,
                "upstream stdin unavailable",
            ));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_cli_process(&mut child, process_group).await;
            return Err(McpCallError::runtime(
                BrokerErrorCode::TargetUnavailable,
                "upstream stdout unavailable",
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_cli_process(&mut child, process_group).await;
            return Err(McpCallError::runtime(
                BrokerErrorCode::TargetUnavailable,
                "upstream stderr unavailable",
            ));
        }
    };
    let (status_tx, status_rx) = watch::channel(StderrStatus::Draining);
    let stderr_limit = invocation.limits.stderr_bytes;
    let stderr_task = tokio::spawn(async move {
        let mut reader = stderr;
        let mut total = 0usize;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => {
                    let _ = status_tx.send(StderrStatus::Done);
                    return;
                }
                Ok(n) => {
                    total = total.saturating_add(n);
                    if total > stderr_limit {
                        let _ = status_tx.send(StderrStatus::Overflow);
                        return;
                    }
                }
                Err(_) => {
                    let _ = status_tx.send(StderrStatus::Failed);
                    return;
                }
            }
        }
    });
    Ok(McpProcess {
        child,
        #[cfg(unix)]
        process_group,
        stdin,
        stdout: BufReader::new(stdout),
        stderr_status: status_rx,
        stderr_task,
    })
}

async fn terminate_process(process: McpProcess) {
    let McpProcess {
        child,
        #[cfg(unix)]
        process_group,
        stderr_task,
        ..
    } = process;
    let mut child = child;
    terminate_cli_process(&mut child, process_group).await;
    let mut stderr_task = stderr_task;
    if timeout(Duration::from_millis(500), &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
        let _ = stderr_task.await;
    }
}

impl McpProcess {
    async fn read_reply(&mut self, id: u64) -> Result<Value, McpCallError> {
        loop {
            match *self.stderr_status.borrow() {
                StderrStatus::Overflow => return Err(McpCallError::StderrOverflow),
                StderrStatus::Failed => return Err(McpCallError::StderrFailed),
                StderrStatus::Draining | StderrStatus::Done => {}
            }
            let mut overflow = self.stderr_status.clone();
            let line = tokio::select! {
                reply = read_message(&mut self.stdout) => reply,
                changed = overflow.changed() => {
                    if changed.is_ok() {
                        match *overflow.borrow() {
                            StderrStatus::Overflow => return Err(McpCallError::StderrOverflow),
                            StderrStatus::Failed => return Err(McpCallError::StderrFailed),
                            StderrStatus::Draining | StderrStatus::Done => {}
                        }
                    }
                    continue;
                }
            }?;
            match *self.stderr_status.borrow() {
                StderrStatus::Overflow => return Err(McpCallError::StderrOverflow),
                StderrStatus::Failed => return Err(McpCallError::StderrFailed),
                StderrStatus::Draining | StderrStatus::Done => {}
            }
            // The raw line was bounded by `read_message` before this decode.
            // rmcp handles the MCP JSON-RPC model; the actor below still
            // applies its explicit correlation and notification policy.
            let message: ServerJsonRpcMessage =
                serde_json::from_slice(&line).map_err(|_| McpCallError::Protocol)?;
            let value = serde_json::to_value(message).map_err(|_| McpCallError::Protocol)?;
            if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                return Err(McpCallError::Protocol);
            }
            if value.get("method").is_some() {
                // Notifications do not complete a request. Server requests
                // (method plus id) are deliberately not answered in MVP.
                if value.get("id").is_none() {
                    continue;
                }
                return Err(McpCallError::Protocol);
            }
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(McpCallError::Protocol);
            }
            match (value.get("result"), value.get("error")) {
                (Some(_), Some(_)) => return Err(McpCallError::Protocol),
                (None, Some(error)) if error.is_object() => {
                    return Err(McpCallError::RemoteError);
                }
                (None, Some(_)) => return Err(McpCallError::Protocol),
                (Some(result), None) => return Ok(result.clone()),
                (None, None) => return Err(McpCallError::Protocol),
            }
        }
    }
}

fn is_modern_discovery(value: &Value) -> bool {
    value
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_some_and(|v| v == "2026-07-28")
        || value
            .get("protocolVersions")
            .and_then(Value::as_array)
            .is_some_and(|versions| versions.iter().any(|v| v.as_str() == Some("2026-07-28")))
}

fn is_legacy_initialize(value: &Value) -> bool {
    value
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_some_and(|v| v == "2025-11-25")
}

async fn send_message(writer: &mut ChildStdin, value: &Value) -> Result<(), RuntimeError> {
    // Let rmcp own MCP message shape and model evolution. Framing and the
    // byte bound remain ours: serialization happens before the delimiter is
    // written, and no SDK transport is allowed to bypass this check.
    let message: ClientJsonRpcMessage = serde_json::from_value(value.clone()).map_err(|_| {
        RuntimeError::new(
            BrokerErrorCode::TargetFailed,
            "upstream request could not be encoded",
        )
    })?;
    let bytes = serde_json::to_vec(&message).map_err(|_| {
        RuntimeError::new(
            BrokerErrorCode::TargetFailed,
            "upstream request could not be encoded",
        )
    })?;
    if bytes.len().saturating_add(1) > UPSTREAM_MESSAGE_BYTES {
        return Err(RuntimeError::new(
            BrokerErrorCode::OutputLimitExceeded,
            "upstream message exceeded configured limit",
        ));
    }
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| RuntimeError::new(BrokerErrorCode::TargetFailed, "upstream request failed"))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|_| RuntimeError::new(BrokerErrorCode::TargetFailed, "upstream request failed"))?;
    writer
        .flush()
        .await
        .map_err(|_| RuntimeError::new(BrokerErrorCode::TargetFailed, "upstream request failed"))
}

async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, McpCallError> {
    let mut bytes = Vec::new();
    let mut one = [0u8; 1];
    loop {
        let count = reader
            .read(&mut one)
            .await
            .map_err(|_| McpCallError::ConnectionClosed)?;
        if count == 0 {
            return Err(McpCallError::ConnectionClosed);
        }
        if one[0] == b'\n' {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            if bytes.is_empty() {
                return Err(McpCallError::Protocol);
            }
            return Ok(bytes);
        }
        bytes.push(one[0]);
        if bytes.len() > UPSTREAM_MESSAGE_BYTES {
            return Err(McpCallError::MessageLimit);
        }
    }
}

fn adapt_result(
    value: Value,
    kind: OutputKind,
    _revision: Revision,
    output_limit: usize,
) -> Result<ExecutionResult, McpCallError> {
    let object = value.as_object().ok_or(McpCallError::Output)?;
    let is_error = object
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut text = String::new();
    let mut text_blocks = Vec::new();
    if let Some(content) = object.get("content") {
        let blocks = content.as_array().ok_or(McpCallError::Output)?;
        for block in blocks {
            let block = block.as_object().ok_or(McpCallError::Output)?;
            if block.get("type").and_then(Value::as_str) != Some("text") {
                return Err(McpCallError::Output);
            }
            let part = block
                .get("text")
                .and_then(Value::as_str)
                .ok_or(McpCallError::Output)?;
            if part.len() > MAX_STRING_BYTES {
                return Err(McpCallError::Output);
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
            text_blocks.push(part.to_owned());
        }
    }
    let structured = object
        .get("structuredContent")
        .cloned()
        .map(|value| bounded_json(value, 1))
        .transpose()?;
    if kind == OutputKind::Text && (structured.is_some() || text_blocks.is_empty()) {
        return Err(McpCallError::Output);
    }
    if kind == OutputKind::Structured && structured.is_none() {
        return Err(McpCallError::Output);
    }
    let structured_bytes = structured
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| McpCallError::Output)?
        .map_or(0, |bytes| bytes.len());
    if text.len().saturating_add(structured_bytes) > output_limit {
        return Err(McpCallError::MessageLimit);
    }
    if is_error {
        return Ok(ExecutionResult {
            code: BrokerErrorCode::TargetFailed,
            is_error: true,
            text: None,
            text_blocks: Vec::new(),
            structured: None,
        });
    }
    Ok(ExecutionResult {
        code: BrokerErrorCode::TargetFailed,
        is_error: false,
        text: (!text_blocks.is_empty()).then_some(text),
        text_blocks,
        structured,
    })
}

fn bounded_json(value: Value, depth: usize) -> Result<Value, McpCallError> {
    if depth > MAX_JSON_DEPTH {
        return Err(McpCallError::Output);
    }
    match value {
        Value::Null | Value::Bool(_) => Ok(value),
        Value::String(s) => {
            if s.len() > MAX_STRING_BYTES {
                Err(McpCallError::Output)
            } else {
                Ok(Value::String(s))
            }
        }
        Value::Number(n) => {
            if n.as_i64().is_none()
                && n.as_u64().is_none()
                && !n.as_f64().is_some_and(f64::is_finite)
            {
                Err(McpCallError::Output)
            } else {
                Ok(Value::Number(n))
            }
        }
        Value::Array(items) => {
            if items.len() > MAX_ARRAY_ITEMS {
                return Err(McpCallError::Output);
            }
            Ok(Value::Array(
                items
                    .into_iter()
                    .map(|item| bounded_json(item, depth + 1))
                    .collect::<Result<_, _>>()?,
            ))
        }
        Value::Object(object) => Ok(Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    if key.len() > MAX_STRING_BYTES {
                        return Err(McpCallError::Output);
                    }
                    Ok((key, bounded_json(value, depth + 1)?))
                })
                .collect::<Result<_, McpCallError>>()?,
        )),
    }
}

impl RuntimeError {
    fn new(code: BrokerErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }
}

/// Execute one already-resolved CLI invocation. No shell is involved, stdin
/// is connected to the null device, and inherited environment is rebuilt from
/// the allowlist carried by the invocation.
pub async fn execute_cli(
    invocation: ResolvedCliInvocation,
) -> Result<ExecutionResult, RuntimeError> {
    execute_cli_with_admission(invocation, &Admission::new()).await
}

/// Execute a CLI invocation while holding the broker-wide admission slot.
/// Public composition uses this alongside `McpExecutor::with_admission` so
/// CLI and upstream MCP calls share the same sixteen-call ceiling.
pub async fn execute_cli_with_admission(
    invocation: ResolvedCliInvocation,
    admission: &Admission,
) -> Result<ExecutionResult, RuntimeError> {
    execute_cli_with_admission_and_cancellation(invocation, admission, &Cancellation::new()).await
}

pub async fn execute_cli_with_admission_and_cancellation(
    invocation: ResolvedCliInvocation,
    admission: &Admission,
    cancellation: &Cancellation,
) -> Result<ExecutionResult, RuntimeError> {
    if cancellation.is_cancelled() {
        return Err(RuntimeError::new(
            BrokerErrorCode::Cancelled,
            "broker is shutting down",
        ));
    }
    let _admission = admission.try_acquire()?;
    execute_cli_inner(invocation, cancellation).await
}

async fn execute_cli_inner(
    invocation: ResolvedCliInvocation,
    cancellation: &Cancellation,
) -> Result<ExecutionResult, RuntimeError> {
    let mut command = Command::new(&invocation.executable);
    command
        .args(&invocation.argv)
        .current_dir(&invocation.cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    unsafe {
        command.as_std_mut().pre_exec(|| {
            setpgid(Pid::from_raw(0), Pid::from_raw(0))
                .map_err(|error| io::Error::other(format!("setpgid failed: {error}")))
        });
    }
    for (name, value) in &invocation.environment.inherited_values {
        command.env(name, value);
    }
    for (name, value) in &invocation.environment.set {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|_| {
        RuntimeError::new(
            BrokerErrorCode::TargetUnavailable,
            "target could not be started",
        )
    })?;
    #[cfg(unix)]
    let process_group =
        Pid::from_raw(child.id().ok_or_else(|| {
            RuntimeError::new(BrokerErrorCode::TargetUnavailable, "target had no pid")
        })? as i32);
    #[cfg(not(unix))]
    let process_group = ();
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_cli_process(&mut child, process_group).await;
            return Err(RuntimeError::new(
                BrokerErrorCode::TargetUnavailable,
                "target stdout unavailable",
            ));
        }
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_cli_process(&mut child, process_group).await;
            return Err(RuntimeError::new(
                BrokerErrorCode::TargetUnavailable,
                "target stderr unavailable",
            ));
        }
    };
    let stdout_limit = invocation.limits.stdout_bytes;
    let stderr_limit = invocation.limits.stderr_bytes;
    let mut stdout_task = Some(tokio::spawn(async move {
        read_bounded(&mut stdout, stdout_limit).await
    }));
    let mut stderr_task = Some(tokio::spawn(async move {
        read_bounded(&mut stderr, stderr_limit).await
    }));
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(invocation.limits.timeout_ms);
    let mut shutdown = cancellation.receiver();
    let mut status: Option<Result<ExitStatus, ()>> = None;
    let mut stdout_result = None;
    let mut stderr_result = None;
    while status.is_none() || stdout_result.is_none() || stderr_result.is_none() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            terminate_cli_process(&mut child, process_group).await;
            reap_cli_reader(&mut stdout_task).await;
            reap_cli_reader(&mut stderr_task).await;
            return Err(RuntimeError::new(
                BrokerErrorCode::TargetTimeout,
                "target timed out",
            ));
        }
        tokio::select! {
            result = child.wait(), if status.is_none() => {
                status = Some(result.map_err(|_| ()));
            }
            result = stdout_task.as_mut().expect("stdout task exists"), if stdout_result.is_none() => {
                stdout_result = Some(join_reader(result));
                if stdout_result.as_ref().is_some_and(Result::is_err) {
                    terminate_cli_process(&mut child, process_group).await;
                    reap_cli_reader(&mut stderr_task).await;
                    return Err(reader_error(stdout_result.as_ref().unwrap().as_ref().unwrap_err()));
                }
            }
            result = stderr_task.as_mut().expect("stderr task exists"), if stderr_result.is_none() => {
                stderr_result = Some(join_reader(result));
                if stderr_result.as_ref().is_some_and(Result::is_err) {
                    terminate_cli_process(&mut child, process_group).await;
                    reap_cli_reader(&mut stdout_task).await;
                    return Err(reader_error(stderr_result.as_ref().unwrap().as_ref().unwrap_err()));
                }
            }
            _ = tokio::time::sleep(remaining) => {
                terminate_cli_process(&mut child, process_group).await;
                reap_cli_reader(&mut stdout_task).await;
                reap_cli_reader(&mut stderr_task).await;
                return Err(RuntimeError::new(
                    BrokerErrorCode::TargetTimeout,
                    "target timed out",
                ));
            }
            _ = wait_for_cancellation(&mut shutdown) => {
                terminate_cli_process(&mut child, process_group).await;
                reap_cli_reader(&mut stdout_task).await;
                reap_cli_reader(&mut stderr_task).await;
                return Err(RuntimeError::new(
                    BrokerErrorCode::Cancelled,
                    "broker is shutting down",
                ));
            }
        }
    }
    let status = match status.expect("child status exists") {
        Ok(status) => status,
        Err(()) => {
            terminate_cli_process(&mut child, process_group).await;
            return Err(RuntimeError::new(
                BrokerErrorCode::TargetFailed,
                "target wait failed",
            ));
        }
    };
    let stdout = stdout_result
        .expect("stdout result exists")
        .map_err(|error| reader_error(&error))?;
    let _stderr = stderr_result
        .expect("stderr result exists")
        .map_err(|error| reader_error(&error))?;
    let success = status.success();
    if !success {
        return Ok(ExecutionResult {
            code: BrokerErrorCode::TargetFailed,
            is_error: true,
            text: None,
            text_blocks: Vec::new(),
            structured: None,
        });
    }
    match invocation.output_kind {
        OutputKind::Text => {
            let text = String::from_utf8(stdout).map_err(|_| {
                RuntimeError::new(
                    BrokerErrorCode::InvalidTargetOutput,
                    "target output was not UTF-8",
                )
            })?;
            Ok(ExecutionResult {
                code: BrokerErrorCode::TargetFailed,
                is_error: false,
                text: Some(text.clone()),
                text_blocks: vec![text],
                structured: None,
            })
        }
        OutputKind::Json => {
            let value: Value = serde_json::from_slice(&stdout).map_err(|_| {
                RuntimeError::new(
                    BrokerErrorCode::InvalidTargetOutput,
                    "target output was not valid JSON",
                )
            })?;
            let value = bounded_json(value, 1).map_err(|_| {
                RuntimeError::new(
                    BrokerErrorCode::InvalidTargetOutput,
                    "target JSON output exceeded structural limits",
                )
            })?;
            let value = canonicalize(value);
            let text = serde_json::to_string(&value).map_err(|_| {
                RuntimeError::new(
                    BrokerErrorCode::InvalidTargetOutput,
                    "target output could not be normalized",
                )
            })?;
            Ok(ExecutionResult {
                code: BrokerErrorCode::TargetFailed,
                is_error: false,
                text: Some(text.clone()),
                text_blocks: vec![text],
                structured: Some(value),
            })
        }
        OutputKind::Structured => Err(RuntimeError::new(
            BrokerErrorCode::InvalidTargetOutput,
            "CLI targets do not support structured upstream output",
        )),
    }
}

#[derive(Debug)]
enum ReadError {
    LimitExceeded,
    Io,
}

fn join_reader(
    result: Result<Result<Vec<u8>, ReadError>, tokio::task::JoinError>,
) -> Result<Vec<u8>, ReadError> {
    result.map_err(|_| ReadError::Io).and_then(|result| result)
}

fn reader_error(error: &ReadError) -> RuntimeError {
    match error {
        ReadError::LimitExceeded => RuntimeError::new(
            BrokerErrorCode::OutputLimitExceeded,
            "target output exceeded configured limit",
        ),
        ReadError::Io => {
            RuntimeError::new(BrokerErrorCode::TargetFailed, "target output read failed")
        }
    }
}

async fn reap_cli_reader(task: &mut Option<tokio::task::JoinHandle<Result<Vec<u8>, ReadError>>>) {
    if let Some(mut task_handle) = task.take()
        && timeout(Duration::from_millis(500), &mut task_handle)
            .await
            .is_err()
    {
        task_handle.abort();
        let _ = task_handle.await;
    }
}

#[cfg(unix)]
async fn terminate_cli_process(child: &mut Child, process_group: Pid) {
    let grace = Duration::from_millis(500);
    let started = tokio::time::Instant::now();
    let leader_reaped = if killpg(process_group, Signal::SIGTERM).is_ok() {
        matches!(timeout(grace, child.wait()).await, Ok(Ok(_)))
    } else {
        false
    };

    // A leader may exit while a descendant still owns stdout/stderr. Keep
    // the termination window fixed, then kill the known process group even
    // when child.wait() already completed.
    let elapsed = started.elapsed();
    if elapsed < grace {
        tokio::time::sleep(grace - elapsed).await;
    }
    let _ = killpg(process_group, Signal::SIGKILL);
    if !leader_reaped {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

#[cfg(not(unix))]
async fn terminate_cli_process(child: &mut Child, _process_group: ()) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await.map_err(|_| ReadError::Io)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(read) > limit {
            return Err(ReadError::LimitExceeded);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

#[derive(Debug, Default)]
pub struct Runtime;
impl Runtime {
    pub const fn new() -> Self {
        Self
    }
    pub fn unavailable(&self) -> BrokerErrorCode {
        BrokerErrorCode::TargetUnavailable
    }
}
