//! Pure configuration, schema, binding, and invocation policy.
//!
//! This crate deliberately has no process, filesystem, clock, or MCP
//! dependencies. Runtime adapters consume only validated values constructed
//! here.

use regex::Regex;
use serde::Deserialize;
pub use serde_json;
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub const MAX_JSON_DEPTH: usize = 32;
pub const MAX_STRING_BYTES: usize = 256 * 1024;
pub const MAX_ARRAY_ITEMS: usize = 1024;
pub const MAX_PATTERN_COMPILED_BYTES: usize = 1024 * 1024;
pub const MAX_TOOLS: usize = 256;
pub const MAX_DIAGNOSTICS: usize = 32;
/// Schema and binding trees use the same finite structural budget as JSON
/// messages.  This is checked while adopting configuration, before any
/// recursive value can reach the runtime.
pub const MAX_SCHEMA_DEPTH: usize = MAX_JSON_DEPTH;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BrokerErrorCode {
    InvalidArguments,
    PolicyDenied,
    TargetUnavailable,
    TargetTimeout,
    TargetFailed,
    OutputLimitExceeded,
    InvalidTargetOutput,
    Cancelled,
    ServerBusy,
}
impl BrokerErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "INVALID_ARGUMENTS",
            Self::PolicyDenied => "POLICY_DENIED",
            Self::TargetUnavailable => "TARGET_UNAVAILABLE",
            Self::TargetTimeout => "TARGET_TIMEOUT",
            Self::TargetFailed => "TARGET_FAILED",
            Self::OutputLimitExceeded => "OUTPUT_LIMIT_EXCEEDED",
            Self::InvalidTargetOutput => "INVALID_TARGET_OUTPUT",
            Self::Cancelled => "CANCELLED",
            Self::ServerBusy => "SERVER_BUSY",
        }
    }
}
impl fmt::Display for BrokerErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub code: BrokerErrorCode,
    pub path: String,
    pub message: String,
}
impl Diagnostic {
    pub fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: BrokerErrorCode::InvalidArguments,
            path: path.into(),
            message: message.into(),
        }
    }
    pub fn internal(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: BrokerErrorCode::TargetFailed,
            path: path.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}
impl std::error::Error for Diagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigBytes {
    pub source: String,
    pub bytes: Vec<u8>,
}
impl ConfigBytes {
    pub const MAX_BYTES: usize = MAX_CONFIG_BYTES;
    pub fn new(source: impl Into<String>, bytes: Vec<u8>) -> Result<Self, BrokerErrorCode> {
        if bytes.len() > Self::MAX_BYTES {
            Err(BrokerErrorCode::InvalidArguments)
        } else {
            Ok(Self {
                source: source.into(),
                bytes,
            })
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: u64,
    server: RawServer,
    targets: BTreeMap<String, RawTarget>,
    tools: BTreeMap<String, RawTool>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    name: String,
    transport: RawTransport,
    #[serde(default)]
    limits: RawServerLimits,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTransport {
    kind: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServerLimits {
    #[serde(default = "default_request_bytes")]
    request_bytes: usize,
    #[serde(default = "default_json_depth")]
    json_depth: usize,
}
fn default_request_bytes() -> usize {
    MAX_CONFIG_BYTES
}
fn default_json_depth() -> usize {
    MAX_JSON_DEPTH
}
impl Default for RawServerLimits {
    fn default() -> Self {
        Self {
            request_bytes: default_request_bytes(),
            json_depth: default_json_depth(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    kind: String,
    executable: Option<String>,
    #[serde(default)]
    transport: Option<RawTargetTransport>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    environment: RawEnvironment,
    limits: RawTargetLimits,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTargetTransport {
    kind: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: String,
    #[serde(default)]
    environment: RawEnvironment,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvironment {
    #[serde(default)]
    inherit: Vec<String>,
    #[serde(default)]
    set: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTargetLimits {
    timeout_ms: u64,
    stdout_bytes: Option<usize>,
    output_bytes: Option<usize>,
    stderr_bytes: usize,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnnotations {
    #[serde(default)]
    read_only_hint: Option<bool>,
    #[serde(default)]
    destructive_hint: Option<bool>,
    #[serde(default)]
    idempotent_hint: Option<bool>,
    #[serde(default)]
    open_world_hint: Option<bool>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTool {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    annotations: RawAnnotations,
    input_schema: Value,
    invoke: RawInvoke,
    output: RawOutput,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInvoke {
    target: String,
    #[serde(default)]
    cli: Option<RawCliInvoke>,
    #[serde(default)]
    mcp: Option<RawMcpInvoke>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCliInvoke {
    argv: Vec<Value>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMcpInvoke {
    tool: String,
    arguments: BTreeMap<String, RawMcpBinding>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum RawMcpBinding {
    Literal(RawLiteralBinding),
    Input(RawInputBinding),
    Environment(RawEnvironmentBinding),
    Object(RawObjectBinding),
    Array(RawArrayBinding),
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLiteralBinding {
    literal: Value,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInputBinding {
    input: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvironmentBinding {
    environment: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawObjectBinding {
    object: BTreeMap<String, RawMcpBinding>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArrayBinding {
    array: Vec<RawMcpBinding>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub timeout_ms: u64,
    pub stdout_bytes: usize,
    pub output_bytes: usize,
    pub stderr_bytes: usize,
}
#[derive(Clone, Eq, PartialEq)]
pub struct EnvironmentSpec {
    pub inherit: Vec<String>,
    /// Values captured when the configuration was adopted.  Runtime adapters
    /// must use this snapshot instead of looking up the broker environment
    /// again after validation.
    pub inherited_values: BTreeMap<String, String>,
    pub set: BTreeMap<String, String>,
}
impl fmt::Debug for EnvironmentSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentSpec")
            .field("inherit", &self.inherit)
            .field(
                "inherited_names",
                &self.inherited_values.keys().collect::<Vec<_>>(),
            )
            .field("set_names", &self.set.keys().collect::<Vec<_>>())
            .finish()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliTarget {
    pub id: String,
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub environment: EnvironmentSpec,
    pub limits: Limits,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamMcpTarget {
    pub id: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: EnvironmentSpec,
    pub limits: Limits,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaKind {
    Object,
    Array,
    String,
    Integer,
    Number,
    Boolean,
    Null,
}
#[derive(Clone, Debug)]
pub struct CompiledSchema {
    pub kind: SchemaKind,
    pub properties: BTreeMap<String, CompiledSchema>,
    pub items: Option<Box<CompiledSchema>>,
    pub required: BTreeSet<String>,
    pub min_items: Option<usize>,
    pub max_items: Option<usize>,
    pub unique_items: bool,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub minimum: Option<Number>,
    pub maximum: Option<Number>,
    pub enum_values: Vec<Value>,
    pub pattern: Option<Regex>,
    pub public_json: Value,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliBinding {
    Literal(String),
    Input(String),
    Each(String),
}
#[derive(Clone, Debug)]
pub struct CompiledTool {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub annotations: Annotations,
    pub schema: CompiledSchema,
    pub target: String,
    pub argv: Vec<CliBinding>,
    pub mcp: Option<McpInvoke>,
    pub output_kind: OutputKind,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Annotations {
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(rename = "idempotentHint", skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKind {
    Text,
    Json,
    Structured,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum McpBinding {
    Literal(Value),
    Input(String),
    Environment(String),
    Object(BTreeMap<String, McpBinding>),
    Array(Vec<McpBinding>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpInvoke {
    pub tool: String,
    pub arguments: BTreeMap<String, McpBinding>,
}
#[derive(Clone)]
pub struct ValidatedConfig {
    pub server_name: String,
    pub request_bytes: usize,
    pub json_depth: usize,
    pub targets: BTreeMap<String, CliTarget>,
    pub upstream_targets: BTreeMap<String, UpstreamMcpTarget>,
    pub tools: BTreeMap<String, CompiledTool>,
    server_environment: BTreeMap<String, String>,
}
impl fmt::Debug for ValidatedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedConfig")
            .field("server_name", &self.server_name)
            .field("request_bytes", &self.request_bytes)
            .field("json_depth", &self.json_depth)
            .field("target_ids", &self.targets.keys().collect::<Vec<_>>())
            .field(
                "upstream_target_ids",
                &self.upstream_targets.keys().collect::<Vec<_>>(),
            )
            .field("tool_names", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCliInvocation {
    pub target_id: String,
    pub executable: PathBuf,
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    pub environment: EnvironmentSpec,
    pub limits: Limits,
    pub output_kind: OutputKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedMcpInvocation {
    pub target_id: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: EnvironmentSpec,
    pub upstream_tool: String,
    pub arguments: Value,
    pub limits: Limits,
    pub output_kind: OutputKind,
}
#[derive(Clone, Debug)]
pub struct ValidationIssue {
    pub path: String,
    pub constraint: String,
}

pub fn parse_config(
    source: impl Into<String>,
    bytes: &[u8],
    environment: &BTreeMap<String, String>,
) -> Result<ValidatedConfig, Diagnostic> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(Diagnostic::invalid("", "configuration exceeds 1 MiB"));
    }
    let options = serde_saphyr::options! {
        strict_booleans: true,
        reject_unsupported_tags: true,
        // Parser errors may otherwise carry a source snippet.  The YAML is a
        // credential-bearing configuration, so never copy its bytes into a
        // broker diagnostic.
        with_snippet: false,
        crop_radius: 0,
        merge_keys: serde_saphyr::MergeKeyPolicy::Error,
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: 0,
            max_replay_stack_depth: 0,
            max_alias_expansions_per_anchor: 0,
        },
    };
    let raw: RawConfig = serde_saphyr::from_slice_with_options(bytes, options)
        .map_err(|e| Diagnostic::invalid("", format!("invalid YAML: {e}")))?;
    validate_raw(raw, source.into(), environment)
}
fn validate_raw(
    raw: RawConfig,
    _source: String,
    environment: &BTreeMap<String, String>,
) -> Result<ValidatedConfig, Diagnostic> {
    if raw.version != 1 {
        return Err(Diagnostic::invalid("/version", "unsupported version"));
    }
    if raw.server.transport.kind != "stdio" {
        return Err(Diagnostic::invalid(
            "/server/transport/kind",
            "only stdio is supported",
        ));
    }
    if raw.server.limits.request_bytes == 0 || raw.server.limits.request_bytes > MAX_CONFIG_BYTES {
        return Err(Diagnostic::invalid(
            "/server/limits/request_bytes",
            "must be 1..1048576",
        ));
    }
    if raw.server.limits.json_depth == 0 || raw.server.limits.json_depth > MAX_JSON_DEPTH {
        return Err(Diagnostic::invalid(
            "/server/limits/json_depth",
            "must be 1..32",
        ));
    }
    if raw.tools.len() > MAX_TOOLS {
        return Err(Diagnostic::invalid("/tools", "too many tools"));
    }
    if raw.server.name.is_empty() {
        return Err(Diagnostic::invalid("/server/name", "must not be empty"));
    }
    let mut targets = BTreeMap::new();
    let mut upstream_targets = BTreeMap::new();
    for (id, target) in raw.targets {
        validate_id(&id, "/targets")?;
        match target.kind.as_str() {
            "cli" => {
                let executable = absolute_path(
                    target.executable.as_deref().ok_or_else(|| {
                        Diagnostic::invalid(format!("/targets/{id}/executable"), "required for cli")
                    })?,
                    &format!("/targets/{id}/executable"),
                )?;
                let cwd = absolute_path(
                    target.cwd.as_deref().ok_or_else(|| {
                        Diagnostic::invalid(format!("/targets/{id}/cwd"), "required for cli")
                    })?,
                    &format!("/targets/{id}/cwd"),
                )?;
                validate_limits(&target.limits, &format!("/targets/{id}/limits"), false)?;
                let env = validate_environment(
                    target.environment,
                    environment,
                    &format!("/targets/{id}/environment"),
                )?;
                targets.insert(
                    id.clone(),
                    CliTarget {
                        id,
                        executable,
                        cwd,
                        environment: env,
                        limits: Limits {
                            timeout_ms: target.limits.timeout_ms,
                            stdout_bytes: target
                                .limits
                                .stdout_bytes
                                .or(target.limits.output_bytes)
                                .unwrap(),
                            output_bytes: target
                                .limits
                                .stdout_bytes
                                .or(target.limits.output_bytes)
                                .unwrap(),
                            stderr_bytes: target.limits.stderr_bytes,
                        },
                    },
                );
            }
            "mcp" => {
                let transport = target.transport.ok_or_else(|| {
                    Diagnostic::invalid(format!("/targets/{id}/transport"), "required for mcp")
                })?;
                if transport.kind != "stdio" {
                    return Err(Diagnostic::invalid(
                        format!("/targets/{id}/transport/kind"),
                        "only stdio is supported",
                    ));
                }
                let command = absolute_path(
                    &transport.command,
                    &format!("/targets/{id}/transport/command"),
                )?;
                let cwd = absolute_path(&transport.cwd, &format!("/targets/{id}/transport/cwd"))?;
                if transport.args.iter().any(|v| v.contains('\0')) {
                    return Err(Diagnostic::invalid(
                        format!("/targets/{id}/transport/args"),
                        "argument contains NUL",
                    ));
                }
                validate_limits(&target.limits, &format!("/targets/{id}/limits"), true)?;
                let env = validate_environment(
                    transport.environment,
                    environment,
                    &format!("/targets/{id}/transport/environment"),
                )?;
                upstream_targets.insert(
                    id.clone(),
                    UpstreamMcpTarget {
                        id,
                        command,
                        args: transport.args,
                        cwd,
                        environment: env,
                        limits: Limits {
                            timeout_ms: target.limits.timeout_ms,
                            stdout_bytes: target.limits.output_bytes.unwrap(),
                            output_bytes: target.limits.output_bytes.unwrap(),
                            stderr_bytes: target.limits.stderr_bytes,
                        },
                    },
                );
            }
            _ => {
                return Err(Diagnostic::invalid(
                    format!("/targets/{id}/kind"),
                    "kind must be cli or mcp",
                ));
            }
        }
    }
    let mut tools = BTreeMap::new();
    for (name, tool) in raw.tools {
        validate_id(&name, "/tools")?;
        if !targets.contains_key(&tool.invoke.target)
            && !upstream_targets.contains_key(&tool.invoke.target)
        {
            return Err(Diagnostic::invalid(
                format!("/tools/{name}/invoke/target"),
                "unknown target",
            ));
        }
        let schema = compile_schema(
            &tool.input_schema,
            &format!("/tools/{name}/input_schema"),
            true,
            1,
        )?;
        let argv = if let Some(cli) = &tool.invoke.cli {
            Some(compile_bindings(
                &cli.argv,
                &schema,
                &format!("/tools/{name}/invoke/cli/argv"),
            )?)
        } else {
            None
        };
        let mcp = if let Some(mcp) = &tool.invoke.mcp {
            if !upstream_targets.contains_key(&tool.invoke.target) {
                return Err(Diagnostic::invalid(
                    format!("/tools/{name}/invoke/mcp"),
                    "target is not mcp",
                ));
            }
            Some(McpInvoke {
                tool: validate_mcp_tool_name(&mcp.tool, &format!("/tools/{name}/invoke/mcp/tool"))?,
                arguments: compile_mcp_bindings(
                    &mcp.arguments,
                    &schema,
                    &format!("/tools/{name}/invoke/mcp/arguments"),
                    environment,
                )?,
            })
        } else {
            None
        };
        if argv.is_none() == mcp.is_none() {
            return Err(Diagnostic::invalid(
                format!("/tools/{name}/invoke"),
                "exactly one cli or mcp invocation is required",
            ));
        }
        let output_kind = match (tool.output.kind.as_str(), mcp.is_some()) {
            ("text", _) => OutputKind::Text,
            ("json", false) => OutputKind::Json,
            ("structured", true) => OutputKind::Structured,
            _ => {
                return Err(Diagnostic::invalid(
                    format!("/tools/{name}/output/kind"),
                    "output kind is incompatible with target",
                ));
            }
        };
        tools.insert(
            name.clone(),
            CompiledTool {
                name,
                title: tool.title,
                description: tool.description,
                annotations: Annotations {
                    read_only_hint: tool.annotations.read_only_hint,
                    destructive_hint: tool.annotations.destructive_hint,
                    idempotent_hint: tool.annotations.idempotent_hint,
                    open_world_hint: tool.annotations.open_world_hint,
                },
                schema,
                target: tool.invoke.target,
                argv: argv.unwrap_or_default(),
                mcp,
                output_kind,
            },
        );
    }
    Ok(ValidatedConfig {
        server_name: raw.server.name,
        request_bytes: raw.server.limits.request_bytes,
        json_depth: raw.server.limits.json_depth,
        targets,
        upstream_targets,
        tools,
        server_environment: environment.clone(),
    })
}
fn validate_id(id: &str, path: &str) -> Result<(), Diagnostic> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
    {
        return Err(Diagnostic::invalid(
            path,
            "must be 1..128 ASCII identifier characters",
        ));
    }
    Ok(())
}
fn absolute_path(value: &str, path: &str) -> Result<PathBuf, Diagnostic> {
    if value.is_empty() || value.contains('\0') || !value.starts_with('/') {
        return Err(Diagnostic::invalid(
            path,
            "must be a non-empty absolute path without NUL",
        ));
    }
    Ok(PathBuf::from(value))
}
fn validate_limits(l: &RawTargetLimits, path: &str, mcp: bool) -> Result<(), Diagnostic> {
    if !(1..=300_000).contains(&l.timeout_ms) {
        return Err(Diagnostic::invalid(
            format!("{path}/timeout_ms"),
            "must be 1..300000",
        ));
    }
    if mcp && l.stdout_bytes.is_some() {
        return Err(Diagnostic::invalid(
            format!("{path}/stdout_bytes"),
            "not allowed for mcp targets; use output_bytes",
        ));
    }
    if !mcp && l.output_bytes.is_some() {
        return Err(Diagnostic::invalid(
            format!("{path}/output_bytes"),
            "not allowed for cli targets; use stdout_bytes",
        ));
    }
    let out = if mcp { l.output_bytes } else { l.stdout_bytes }.ok_or_else(|| {
        Diagnostic::invalid(
            format!(
                "{path}/{}",
                if mcp { "output_bytes" } else { "stdout_bytes" }
            ),
            "required",
        )
    })?;
    if !(1..=16 * 1024 * 1024).contains(&out) {
        return Err(Diagnostic::invalid(
            format!("{path}/stdout_bytes"),
            "must be 1..16777216",
        ));
    }
    if !(1..=1024 * 1024).contains(&l.stderr_bytes) {
        return Err(Diagnostic::invalid(
            format!("{path}/stderr_bytes"),
            "must be 1..1048576",
        ));
    }
    Ok(())
}

fn validate_mcp_tool_name(value: &str, path: &str) -> Result<String, Diagnostic> {
    if value.is_empty() || value.len() > 128 || value.contains('\0') {
        return Err(Diagnostic::invalid(
            path,
            "upstream tool name must be 1..128 bytes without NUL",
        ));
    }
    Ok(value.to_owned())
}
fn validate_environment(
    raw: RawEnvironment,
    current: &BTreeMap<String, String>,
    path: &str,
) -> Result<EnvironmentSpec, Diagnostic> {
    let mut inherit = raw.inherit;
    inherit.sort();
    let mut names = BTreeSet::new();
    let mut inherited_values = BTreeMap::new();
    for name in &inherit {
        validate_env_name(name, path)?;
        if !names.insert(name.clone()) || raw.set.contains_key(name) {
            return Err(Diagnostic::invalid(path, "duplicate environment source"));
        }
        if !current.contains_key(name) {
            return Err(Diagnostic::invalid(
                path,
                "inherited variable is not present",
            ));
        }
        inherited_values.insert(name.clone(), current[name].clone());
    }
    for (name, value) in &raw.set {
        validate_env_name(name, path)?;
        if value.contains('\0') {
            return Err(Diagnostic::invalid(path, "environment value contains NUL"));
        }
    }
    Ok(EnvironmentSpec {
        inherit,
        inherited_values,
        set: raw.set,
    })
}
fn validate_env_name(name: &str, path: &str) -> Result<(), Diagnostic> {
    if name.is_empty()
        || !name.bytes().enumerate().all(|(i, b)| {
            if i == 0 {
                b.is_ascii_alphabetic() || b == b'_'
            } else {
                b.is_ascii_alphanumeric() || b == b'_'
            }
        })
    {
        return Err(Diagnostic::invalid(
            path,
            "invalid environment variable name",
        ));
    }
    Ok(())
}

fn validate_schema_keyword_applicability(
    obj: &Map<String, Value>,
    kind: &SchemaKind,
    path: &str,
) -> Result<(), Diagnostic> {
    let object_keywords = ["properties", "required", "additionalProperties"];
    if *kind != SchemaKind::Object
        && let Some(keyword) = object_keywords.iter().find(|key| obj.contains_key(**key))
    {
        return Err(Diagnostic::invalid(
            format!("{path}/{keyword}"),
            "keyword is only valid for object schemas",
        ));
    }
    let array_keywords = ["items", "minItems", "maxItems", "uniqueItems"];
    if *kind != SchemaKind::Array
        && let Some(keyword) = array_keywords.iter().find(|key| obj.contains_key(**key))
    {
        return Err(Diagnostic::invalid(
            format!("{path}/{keyword}"),
            "keyword is only valid for array schemas",
        ));
    }
    let string_keywords = ["minLength", "maxLength", "pattern"];
    if *kind != SchemaKind::String
        && let Some(keyword) = string_keywords.iter().find(|key| obj.contains_key(**key))
    {
        return Err(Diagnostic::invalid(
            format!("{path}/{keyword}"),
            "keyword is only valid for string schemas",
        ));
    }
    let numeric_keywords = ["minimum", "maximum"];
    if !matches!(kind, SchemaKind::Integer | SchemaKind::Number)
        && let Some(keyword) = numeric_keywords.iter().find(|key| obj.contains_key(**key))
    {
        return Err(Diagnostic::invalid(
            format!("{path}/{keyword}"),
            "keyword is only valid for numeric schemas",
        ));
    }
    if *kind == SchemaKind::Object {
        if obj
            .get("properties")
            .is_some_and(|value| !value.is_object())
        {
            return Err(Diagnostic::invalid(
                format!("{path}/properties"),
                "properties must be an object",
            ));
        }
        if obj.get("required").is_some_and(|value| !value.is_array()) {
            return Err(Diagnostic::invalid(
                format!("{path}/required"),
                "required must be an array",
            ));
        }
        if obj.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err(Diagnostic::invalid(
                format!("{path}/additionalProperties"),
                "must be false",
            ));
        }
    }
    if *kind == SchemaKind::Array {
        if obj.get("items").is_some_and(|value| !value.is_object()) {
            return Err(Diagnostic::invalid(
                format!("{path}/items"),
                "items must be a schema object",
            ));
        }
        if obj
            .get("uniqueItems")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(Diagnostic::invalid(
                format!("{path}/uniqueItems"),
                "uniqueItems must be boolean",
            ));
        }
    }
    Ok(())
}

fn schema_value_compatible(kind: &SchemaKind, value: &Value) -> bool {
    match kind {
        SchemaKind::Object => value.is_object(),
        SchemaKind::Array => value.is_array(),
        SchemaKind::String => value.is_string(),
        SchemaKind::Integer => value.as_i64().is_some(),
        SchemaKind::Number => value.as_f64().is_some_and(f64::is_finite),
        SchemaKind::Boolean => value.is_boolean(),
        SchemaKind::Null => value.is_null(),
    }
}

#[derive(Clone, Copy, Debug)]
enum NumericValue {
    Integer(i128),
    Float(f64),
}

fn numeric_value(number: &Number) -> Option<NumericValue> {
    number.as_i128().map(NumericValue::Integer).or_else(|| {
        number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(NumericValue::Float)
    })
}

/// Compare an i128 with a finite f64 without converting the integer through
/// f64.  The latter would make adjacent integers above 2^53 indistinguishable.
fn compare_integer_float(integer: i128, float: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    debug_assert!(float.is_finite());
    if integer == 0 {
        return float.partial_cmp(&0.0).unwrap();
    }
    let integer_negative = integer < 0;
    let float_negative = float.is_sign_negative() && float != 0.0;
    if integer_negative != float_negative {
        return if integer_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    let integer_magnitude = integer.unsigned_abs();
    let float_magnitude = float.abs();
    // Every i128 has magnitude below 2^127.  Values at or above that point
    // are therefore strictly larger than any positive i128 magnitude.
    let i128_limit = 2f64.powi(127);
    let magnitude_order = if float_magnitude >= i128_limit {
        Ordering::Less
    } else {
        let truncated = float_magnitude.trunc() as u128;
        integer_magnitude.cmp(&truncated).then_with(|| {
            if float_magnitude.fract() == 0.0 {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        })
    };
    if integer_negative {
        magnitude_order.reverse()
    } else {
        magnitude_order
    }
}

fn numeric_compare(left: &Number, right: &Number) -> Option<std::cmp::Ordering> {
    match (numeric_value(left)?, numeric_value(right)?) {
        (NumericValue::Integer(a), NumericValue::Integer(b)) => Some(a.cmp(&b)),
        (NumericValue::Integer(a), NumericValue::Float(b)) => Some(compare_integer_float(a, b)),
        (NumericValue::Float(a), NumericValue::Integer(b)) => {
            Some(compare_integer_float(b, a).reverse())
        }
        (NumericValue::Float(a), NumericValue::Float(b)) => a.partial_cmp(&b),
    }
}

fn schema_values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            numeric_compare(left, right) == Some(std::cmp::Ordering::Equal)
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| schema_values_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| schema_values_equal(value, other))
                })
        }
        _ => left == right,
    }
}

fn compile_schema(
    value: &Value,
    path: &str,
    root: bool,
    depth: usize,
) -> Result<CompiledSchema, Diagnostic> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(Diagnostic::invalid(
            path,
            format!("schema depth exceeds {MAX_SCHEMA_DEPTH}"),
        ));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| Diagnostic::invalid(path, "schema must be object"))?;
    let allowed = [
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "minItems",
        "maxItems",
        "uniqueItems",
        "enum",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "pattern",
    ];
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(Diagnostic::invalid(
                format!("{path}/{key}"),
                "unsupported schema keyword",
            ));
        }
    }
    let kind_s = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Diagnostic::invalid(format!("{path}/type"), "type is required"))?;
    let kind = match kind_s {
        "object" => SchemaKind::Object,
        "array" => SchemaKind::Array,
        "string" => SchemaKind::String,
        "integer" => SchemaKind::Integer,
        "number" => SchemaKind::Number,
        "boolean" => SchemaKind::Boolean,
        "null" => SchemaKind::Null,
        _ => {
            return Err(Diagnostic::invalid(
                format!("{path}/type"),
                "unsupported type",
            ));
        }
    };
    if root && kind != SchemaKind::Object {
        return Err(Diagnostic::invalid(path, "root schema must be object"));
    }
    validate_schema_keyword_applicability(obj, &kind, path)?;
    let mut properties = BTreeMap::new();
    let required;
    if kind == SchemaKind::Object {
        let props = obj
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                Diagnostic::invalid(
                    format!("{path}/properties"),
                    "object properties are required",
                )
            })?;
        let required_values = obj
            .get("required")
            .and_then(Value::as_array)
            .ok_or_else(|| Diagnostic::invalid(format!("{path}/required"), "required is required"))?
            .iter();
        let required_vec = required_values
            .map(|v| {
                v.as_str().map(String::from).ok_or_else(|| {
                    Diagnostic::invalid(
                        format!("{path}/required"),
                        "required names must be strings",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        required = required_vec
            .iter()
            .map(|v| Ok(v.clone()))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if required.len() != required_vec.len()
            || required.len() != props.len()
            || props.keys().any(|k| !required.contains(k))
        {
            return Err(Diagnostic::invalid(
                format!("{path}/required"),
                "all properties must be required",
            ));
        }
        if obj.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err(Diagnostic::invalid(
                format!("{path}/additionalProperties"),
                "must be false",
            ));
        }
        for (k, v) in props {
            properties.insert(
                k.clone(),
                compile_schema(
                    v,
                    &format!("{path}/properties/{}", escape_ptr(k)),
                    false,
                    depth + 1,
                )?,
            );
        }
    } else {
        required = BTreeSet::new();
    }
    let min_items = usize_value(obj.get("minItems"), path, "minItems")?;
    let max_items = usize_value(obj.get("maxItems"), path, "maxItems")?;
    if min_items.zip(max_items).is_some_and(|(a, b)| a > b) {
        return Err(Diagnostic::invalid(path, "minItems exceeds maxItems"));
    }
    if min_items.is_some_and(|x| x > MAX_ARRAY_ITEMS)
        || max_items.is_some_and(|x| x > MAX_ARRAY_ITEMS)
    {
        return Err(Diagnostic::invalid(path, "array limit exceeds 1024"));
    }
    let items = if kind == SchemaKind::Array {
        Some(Box::new(compile_schema(
            obj.get("items")
                .ok_or_else(|| Diagnostic::invalid(format!("{path}/items"), "items is required"))?,
            &format!("{path}/items"),
            false,
            depth + 1,
        )?))
    } else {
        None
    };
    let min_length = usize_value(obj.get("minLength"), path, "minLength")?;
    let max_length = usize_value(obj.get("maxLength"), path, "maxLength")?;
    if min_length.zip(max_length).is_some_and(|(a, b)| a > b) {
        return Err(Diagnostic::invalid(path, "minLength exceeds maxLength"));
    }
    let minimum = numeric_bound(obj.get("minimum"), path, "minimum", &kind)?;
    let maximum = numeric_bound(obj.get("maximum"), path, "maximum", &kind)?;
    if minimum
        .as_ref()
        .zip(maximum.as_ref())
        .is_some_and(|(a, b)| numeric_compare(a, b) == Some(std::cmp::Ordering::Greater))
    {
        return Err(Diagnostic::invalid(path, "minimum exceeds maximum"));
    }
    let enum_values = if let Some(value) = obj.get("enum") {
        let values = value
            .as_array()
            .ok_or_else(|| Diagnostic::invalid(format!("{path}/enum"), "enum must be an array"))?;
        if values.is_empty() {
            return Err(Diagnostic::invalid(
                format!("{path}/enum"),
                "enum must not be empty",
            ));
        }
        for (index, value) in values.iter().enumerate() {
            if !schema_value_compatible(&kind, value) {
                return Err(Diagnostic::invalid(
                    format!("{path}/enum/{index}"),
                    "enum value is incompatible with schema type",
                ));
            }
            if values[..index]
                .iter()
                .any(|previous| schema_values_equal(previous, value))
            {
                return Err(Diagnostic::invalid(
                    format!("{path}/enum/{index}"),
                    "enum must not contain duplicate values",
                ));
            }
        }
        values.clone()
    } else {
        Vec::new()
    };
    let pattern = if let Some(v) = obj.get("pattern") {
        let p = v.as_str().ok_or_else(|| {
            Diagnostic::invalid(format!("{path}/pattern"), "pattern must be string")
        })?;
        if p.len() > 16 * 1024 {
            return Err(Diagnostic::invalid(
                format!("{path}/pattern"),
                "pattern exceeds 16 KiB",
            ));
        }
        Some(
            regex::RegexBuilder::new(p)
                .size_limit(MAX_PATTERN_COMPILED_BYTES)
                .build()
                .map_err(|_| {
            Diagnostic::invalid(
                format!("{path}/pattern"),
                "pattern is not supported by linear regex engine or exceeds compiled size limit",
            )
                })?,
        )
    } else {
        None
    };
    let mut public = obj.clone();
    public.insert("type".into(), Value::String(kind_s.into()));
    if kind == SchemaKind::Object {
        public.insert("additionalProperties".into(), Value::Bool(false));
        public.insert(
            "required".into(),
            Value::Array(required.iter().cloned().map(Value::String).collect()),
        );
    }
    Ok(CompiledSchema {
        kind,
        properties,
        items,
        required,
        min_items,
        max_items,
        unique_items: obj
            .get("uniqueItems")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        min_length,
        max_length,
        minimum,
        maximum,
        enum_values,
        pattern,
        public_json: canonicalize(Value::Object(public)),
    })
}
fn usize_value(value: Option<&Value>, path: &str, name: &str) -> Result<Option<usize>, Diagnostic> {
    value
        .map(|v| {
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    Diagnostic::invalid(format!("{path}/{name}"), "must be non-negative integer")
                })
        })
        .transpose()
}
fn numeric_bound(
    value: Option<&Value>,
    path: &str,
    name: &str,
    kind: &SchemaKind,
) -> Result<Option<Number>, Diagnostic> {
    value
        .map(|v| {
            let number = v.as_number().ok_or_else(|| {
                Diagnostic::invalid(format!("{path}/{name}"), "must be finite number")
            })?;
            if *kind == SchemaKind::Integer {
                if number.as_i64().is_none() {
                    return Err(Diagnostic::invalid(
                        format!("{path}/{name}"),
                        "integer bound must fit signed 64 bit range",
                    ));
                }
            } else if number.as_f64().filter(|n| n.is_finite()).is_none() {
                return Err(Diagnostic::invalid(
                    format!("{path}/{name}"),
                    "must be finite number",
                ));
            }
            Ok(number.clone())
        })
        .transpose()
}
fn escape_ptr(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn decode_ptr(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    if !pointer.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in pointer[1..].split('/') {
        let mut decoded = String::with_capacity(part.len());
        let mut chars = part.chars();
        while let Some(ch) = chars.next() {
            if ch != '~' {
                decoded.push(ch);
                continue;
            }
            match chars.next() {
                Some('0') => decoded.push('~'),
                Some('1') => decoded.push('/'),
                _ => return None,
            }
        }
        parts.push(decoded);
    }
    Some(parts)
}
fn compile_bindings(
    values: &[Value],
    schema: &CompiledSchema,
    path: &str,
) -> Result<Vec<CliBinding>, Diagnostic> {
    let mut result = Vec::new();
    for (i, value) in values.iter().enumerate() {
        let p = format!("{path}/{i}");
        let obj = value
            .as_object()
            .ok_or_else(|| Diagnostic::invalid(&p, "binding node must be object"))?;
        if obj.len() != 1 {
            return Err(Diagnostic::invalid(&p, "binding node must have one key"));
        }
        let (kind, val) = obj.iter().next().unwrap();
        let pointer = val
            .as_str()
            .ok_or_else(|| Diagnostic::invalid(&p, "binding pointer/value must be string"))?;
        match kind.as_str() {
            "literal" => {
                if pointer.contains('\0') {
                    return Err(Diagnostic::invalid(&p, "literal contains NUL"));
                }
                result.push(CliBinding::Literal(pointer.into()));
            }
            "input" => {
                let s = schema_at_pointer(schema, pointer).ok_or_else(|| {
                    Diagnostic::invalid(&p, "input pointer does not reference schema")
                })?;
                if matches!(
                    s.kind,
                    SchemaKind::Object | SchemaKind::Array | SchemaKind::Boolean | SchemaKind::Null
                ) {
                    return Err(Diagnostic::invalid(&p, "input binding requires scalar"));
                }
                result.push(CliBinding::Input(pointer.into()));
            }
            "each" => {
                let s = schema_at_pointer(schema, pointer).ok_or_else(|| {
                    Diagnostic::invalid(&p, "each pointer does not reference schema")
                })?;
                if s.kind != SchemaKind::Array
                    || s.items.as_ref().is_none_or(|i| {
                        matches!(
                            i.kind,
                            SchemaKind::Object
                                | SchemaKind::Array
                                | SchemaKind::Boolean
                                | SchemaKind::Null
                        )
                    })
                {
                    return Err(Diagnostic::invalid(
                        &p,
                        "each binding requires scalar array",
                    ));
                }
                result.push(CliBinding::Each(pointer.into()));
            }
            _ => return Err(Diagnostic::invalid(&p, "unsupported binding node")),
        }
    }
    Ok(result)
}

fn compile_mcp_bindings(
    values: &BTreeMap<String, RawMcpBinding>,
    schema: &CompiledSchema,
    path: &str,
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, McpBinding>, Diagnostic> {
    values
        .iter()
        .map(|(key, value)| {
            validate_id(key, path)?;
            Ok((
                key.clone(),
                compile_mcp_binding(
                    value,
                    schema,
                    &format!("{path}/{}", escape_ptr(key)),
                    environment,
                    1,
                )?,
            ))
        })
        .collect()
}

fn compile_mcp_binding(
    value: &RawMcpBinding,
    schema: &CompiledSchema,
    path: &str,
    environment: &BTreeMap<String, String>,
    depth: usize,
) -> Result<McpBinding, Diagnostic> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(Diagnostic::invalid(
            path,
            format!("binding depth exceeds {MAX_SCHEMA_DEPTH}"),
        ));
    }
    match value {
        RawMcpBinding::Literal(binding) => {
            if value_exceeds_depth(&binding.literal, 1) {
                return Err(Diagnostic::invalid(
                    path,
                    format!("binding literal depth exceeds {MAX_SCHEMA_DEPTH}"),
                ));
            }
            if contains_nul(&binding.literal) {
                return Err(Diagnostic::invalid(path, "literal contains NUL"));
            }
            Ok(McpBinding::Literal(canonicalize(binding.literal.clone())))
        }
        RawMcpBinding::Input(binding) => {
            if schema_at_pointer(schema, &binding.input).is_none() {
                return Err(Diagnostic::invalid(
                    path,
                    "input pointer does not reference schema",
                ));
            }
            Ok(McpBinding::Input(binding.input.clone()))
        }
        RawMcpBinding::Environment(binding) => {
            validate_env_name(&binding.environment, path)?;
            if !environment.contains_key(&binding.environment) {
                return Err(Diagnostic::invalid(
                    path,
                    "environment variable is not present",
                ));
            }
            Ok(McpBinding::Environment(binding.environment.clone()))
        }
        RawMcpBinding::Object(binding) => Ok(McpBinding::Object(
            binding
                .object
                .iter()
                .map(|(key, child)| {
                    Ok((
                        key.clone(),
                        compile_mcp_binding(
                            child,
                            schema,
                            &format!("{path}/{}", escape_ptr(key)),
                            environment,
                            depth + 1,
                        )?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
        RawMcpBinding::Array(binding) => Ok(McpBinding::Array(
            binding
                .array
                .iter()
                .enumerate()
                .map(|(i, child)| {
                    compile_mcp_binding(
                        child,
                        schema,
                        &format!("{path}/{i}"),
                        environment,
                        depth + 1,
                    )
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
    }
}

fn value_exceeds_depth(value: &Value, depth: usize) -> bool {
    if depth > MAX_SCHEMA_DEPTH {
        return true;
    }
    match value {
        Value::Array(items) => items
            .iter()
            .any(|item| value_exceeds_depth(item, depth + 1)),
        Value::Object(object) => object
            .values()
            .any(|item| value_exceeds_depth(item, depth + 1)),
        _ => false,
    }
}

fn contains_nul(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains('\0'),
        Value::Array(items) => items.iter().any(contains_nul),
        Value::Object(obj) => obj.values().any(contains_nul),
        _ => false,
    }
}
fn schema_at_pointer<'a>(root: &'a CompiledSchema, pointer: &str) -> Option<&'a CompiledSchema> {
    let parts = decode_ptr(pointer)?;
    let mut current = root;
    for part in parts {
        current = current.properties.get(&part)?;
    }
    Some(current)
}

pub fn validate_input(schema: &CompiledSchema, input: &Value) -> Result<(), Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    validate_value(schema, input, "", &mut issues);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}
fn validate_value(
    schema: &CompiledSchema,
    value: &Value,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if issues.len() >= MAX_DIAGNOSTICS {
        return;
    }
    let compatible = match schema.kind {
        SchemaKind::Object => value.is_object(),
        SchemaKind::Array => value.is_array(),
        SchemaKind::String => value.is_string(),
        SchemaKind::Integer => {
            value.as_i64().is_some() && value.as_f64().is_some_and(|x| x.fract() == 0.0)
        }
        SchemaKind::Number => value.as_f64().is_some_and(f64::is_finite),
        SchemaKind::Boolean => value.is_boolean(),
        SchemaKind::Null => value.is_null(),
    };
    if !compatible {
        issues.push(ValidationIssue {
            path: path.into(),
            constraint: format!("type:{:?}", schema.kind),
        });
        return;
    }
    if !schema.enum_values.is_empty()
        && !schema
            .enum_values
            .iter()
            .any(|candidate| schema_values_equal(candidate, value))
    {
        issues.push(ValidationIssue {
            path: path.into(),
            constraint: "enum".into(),
        });
    }
    match schema.kind {
        SchemaKind::Object => {
            let obj = value.as_object().unwrap();
            for k in &schema.required {
                if !obj.contains_key(k) {
                    issues.push(ValidationIssue {
                        path: format!("{path}/{}", escape_ptr(k)),
                        constraint: "required".into(),
                    });
                }
            }
            for k in obj.keys() {
                if !schema.properties.contains_key(k) {
                    issues.push(ValidationIssue {
                        path: format!("{path}/{}", escape_ptr(k)),
                        constraint: "additionalProperties:false".into(),
                    });
                }
            }
            for (k, s) in &schema.properties {
                if let Some(v) = obj.get(k) {
                    validate_value(s, v, &format!("{path}/{}", escape_ptr(k)), issues);
                }
            }
        }
        SchemaKind::Array => {
            let arr = value.as_array().unwrap();
            if schema.min_items.is_some_and(|n| arr.len() < n) {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "minItems".into(),
                });
            }
            if schema.max_items.is_some_and(|n| arr.len() > n) {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "maxItems".into(),
                });
            }
            if schema.unique_items {
                for i in 0..arr.len() {
                    if arr[..i]
                        .iter()
                        .any(|candidate| schema_values_equal(candidate, &arr[i]))
                    {
                        issues.push(ValidationIssue {
                            path: path.into(),
                            constraint: "uniqueItems".into(),
                        });
                        break;
                    }
                }
            }
            if let Some(s) = &schema.items {
                for (i, v) in arr.iter().enumerate() {
                    validate_value(s, v, &format!("{path}/{i}"), issues);
                }
            }
        }
        SchemaKind::String => {
            let s = value.as_str().unwrap();
            if s.len() > MAX_STRING_BYTES {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "stringBytes".into(),
                });
            }
            let n = s.chars().count();
            if schema.min_length.is_some_and(|x| n < x) {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "minLength".into(),
                });
            }
            if schema.max_length.is_some_and(|x| n > x) {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "maxLength".into(),
                });
            }
            if schema.pattern.as_ref().is_some_and(|p| !p.is_match(s)) {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "pattern".into(),
                });
            }
        }
        SchemaKind::Integer | SchemaKind::Number => {
            let n = value.as_number().unwrap();
            if schema
                .minimum
                .as_ref()
                .is_some_and(|bound| numeric_compare(n, bound) == Some(std::cmp::Ordering::Less))
            {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "minimum".into(),
                });
            }
            if schema
                .maximum
                .as_ref()
                .is_some_and(|bound| numeric_compare(n, bound) == Some(std::cmp::Ordering::Greater))
            {
                issues.push(ValidationIssue {
                    path: path.into(),
                    constraint: "maximum".into(),
                });
            }
        }
        _ => {}
    }
}
fn value_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    let parts = decode_ptr(pointer)?;
    let mut v = value;
    for p in parts {
        v = v.as_object()?.get(&p)?;
    }
    Some(v)
}
pub fn resolve_cli(
    config: &ValidatedConfig,
    tool_name: &str,
    input: &Value,
) -> Result<ResolvedCliInvocation, Diagnostic> {
    let tool = config
        .tools
        .get(tool_name)
        .ok_or_else(|| Diagnostic::invalid("/tool", "unknown tool"))?;
    if let Err(issues) = validate_input(&tool.schema, input) {
        return Err(Diagnostic::invalid(
            issues[0].path.clone(),
            issues[0].constraint.clone(),
        ));
    }
    let target = config
        .targets
        .get(&tool.target)
        .ok_or_else(|| Diagnostic::invalid("/tool", "tool is not a CLI target"))?;
    let mut argv = Vec::new();
    for node in &tool.argv {
        match node {
            CliBinding::Literal(v) => argv.push(OsString::from(v)),
            CliBinding::Input(p) => argv.push(value_to_arg(value_at(input, p).unwrap())?),
            CliBinding::Each(p) => {
                for v in value_at(input, p).unwrap().as_array().unwrap() {
                    argv.push(value_to_arg(v)?)
                }
            }
        }
    }
    Ok(ResolvedCliInvocation {
        target_id: tool.target.clone(),
        executable: target.executable.clone(),
        argv,
        cwd: target.cwd.clone(),
        environment: target.environment.clone(),
        limits: target.limits.clone(),
        output_kind: tool.output_kind,
    })
}

pub fn resolve_mcp(
    config: &ValidatedConfig,
    tool_name: &str,
    input: &Value,
) -> Result<ResolvedMcpInvocation, Diagnostic> {
    let tool = config
        .tools
        .get(tool_name)
        .ok_or_else(|| Diagnostic::invalid("/tool", "unknown tool"))?;
    if let Err(issues) = validate_input(&tool.schema, input) {
        return Err(Diagnostic::invalid(
            issues[0].path.clone(),
            issues[0].constraint.clone(),
        ));
    }
    let invoke = tool
        .mcp
        .as_ref()
        .ok_or_else(|| Diagnostic::invalid("/tool", "tool is not an MCP target"))?;
    let target = config
        .upstream_targets
        .get(&tool.target)
        .ok_or_else(|| Diagnostic::invalid("/tool", "unknown MCP target"))?;
    let arguments = invoke
        .arguments
        .iter()
        .map(|(key, binding)| {
            Ok((
                key.clone(),
                resolve_mcp_binding(binding, input, &config.server_environment)?,
            ))
        })
        .collect::<Result<Map<String, Value>, Diagnostic>>()?;
    Ok(ResolvedMcpInvocation {
        target_id: target.id.clone(),
        command: target.command.clone(),
        args: target.args.clone(),
        cwd: target.cwd.clone(),
        environment: target.environment.clone(),
        upstream_tool: invoke.tool.clone(),
        arguments: Value::Object(arguments),
        limits: target.limits.clone(),
        output_kind: tool.output_kind,
    })
}

fn resolve_mcp_binding(
    binding: &McpBinding,
    input: &Value,
    environment: &BTreeMap<String, String>,
) -> Result<Value, Diagnostic> {
    match binding {
        McpBinding::Literal(value) => Ok(value.clone()),
        McpBinding::Input(pointer) => value_at(input, pointer)
            .cloned()
            .ok_or_else(|| Diagnostic::invalid(pointer, "input pointer is missing")),
        McpBinding::Environment(name) => environment
            .get(name)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| Diagnostic::invalid("", "environment variable is not present")),
        McpBinding::Object(object) => Ok(Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    Ok((key.clone(), resolve_mcp_binding(value, input, environment)?))
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
        McpBinding::Array(array) => Ok(Value::Array(
            array
                .iter()
                .map(|value| resolve_mcp_binding(value, input, environment))
                .collect::<Result<_, Diagnostic>>()?,
        )),
    }
}
fn value_to_arg(value: &Value) -> Result<OsString, Diagnostic> {
    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if n.as_f64().is_some_and(|value| value == 0.0) {
                String::from("0")
            } else {
                n.to_string()
            }
        }
        _ => {
            return Err(Diagnostic::invalid(
                "",
                "binding requires scalar string or number",
            ));
        }
    };
    if s.contains('\0') {
        return Err(Diagnostic::invalid("", "argument contains NUL"));
    }
    Ok(OsString::from(s))
}
pub fn tools_json(config: &ValidatedConfig) -> Value {
    let tools = config
        .tools
        .values()
        .map(|tool| {
            let mut m = Map::new();
            m.insert("name".into(), Value::String(tool.name.clone()));
            if let Some(v) = &tool.title {
                m.insert("title".into(), Value::String(v.clone()));
            }
            if let Some(v) = &tool.description {
                m.insert("description".into(), Value::String(v.clone()));
            }
            m.insert("inputSchema".into(), tool.schema.public_json.clone());
            m.insert(
                "annotations".into(),
                serde_json::to_value(&tool.annotations).unwrap(),
            );
            Value::Object(m)
        })
        .collect();
    let mut root = Map::new();
    root.insert("tools".into(), Value::Array(tools));
    Value::Object(root)
}
pub fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(obj) => {
            Value::Object(obj.into_iter().map(|(k, v)| (k, canonicalize(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(canonicalize).collect()),
        Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && f == 0.0
            {
                return Value::Number(Number::from(0));
            }
            Value::Number(n)
        }
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::{Config as ProptestConfig, RngSeed};

    fn property_config() -> ProptestConfig {
        let mut config = ProptestConfig::with_cases(24);
        config.rng_seed = RngSeed::Fixed(0x4d43_5042_4f55_4e44);
        config.failure_persistence = None;
        config
    }

    fn env() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn mcp_env() -> BTreeMap<String, String> {
        BTreeMap::from([(String::from("TOKEN"), String::from("secret-sentinel"))])
    }

    fn valid() -> String {
        r#"version: 1
server: {name: test, transport: {kind: stdio}}
targets:
  fake:
    kind: cli
    executable: /bin/echo
    cwd: /
    limits: {timeout_ms: 1000, stdout_bytes: 1024, stderr_bytes: 1024}
tools:
  read:
    input_schema: {type: object, properties: {name: {type: string, minLength: 1}}, required: [name], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: hi}, {input: /name}]}}
    output: {kind: text}
"#.into()
    }

    fn mcp_valid() -> String {
        r#"version: 1
server: {name: test, transport: {kind: stdio}}
targets:
  upstream:
    kind: mcp
    transport: {kind: stdio, command: /bin/fake-mcp, cwd: /, args: [--fixed]}
    limits: {timeout_ms: 1000, output_bytes: 1024, stderr_bytes: 1024}
tools:
  query:
    input_schema:
      type: object
      properties:
        owner: {type: string, minLength: 1}
        limit: {type: integer, minimum: 1, maximum: 100}
      required: [owner, limit]
      additionalProperties: false
    invoke:
      target: upstream
      mcp:
        tool: private-query
        arguments:
          query:
            object:
              owner: {input: /owner}
              visibility: {literal: private}
          limit: {input: /limit}
          credential: {environment: TOKEN}
    output: {kind: structured}
"#
        .into()
    }

    fn config_with_property_schema(schema: &str) -> String {
        valid().replace("{type: string, minLength: 1}", schema)
    }

    fn limits_config(
        request_bytes: usize,
        json_depth: usize,
        timeout_ms: u64,
        output_bytes: usize,
        stderr_bytes: usize,
    ) -> String {
        format!(
            r#"version: 1
server:
  name: test
  transport: {{kind: stdio}}
  limits: {{request_bytes: {request_bytes}, json_depth: {json_depth}}}
targets:
  fake:
    kind: cli
    executable: /bin/echo
    cwd: /
    limits: {{timeout_ms: {timeout_ms}, stdout_bytes: {output_bytes}, stderr_bytes: {stderr_bytes}}}
tools:
  read:
    input_schema: {{type: object, properties: {{name: {{type: string}}}}, required: [name], additionalProperties: false}}
    invoke: {{target: fake, cli: {{argv: [{{input: /name}}]}}}}
    output: {{kind: text}}
"#
        )
    }
    #[test]
    fn parses_and_resolves() {
        let c = parse_config("test", valid().as_bytes(), &env()).unwrap();
        let i = resolve_cli(&c, "read", &serde_json::json!({"name":"a b"})).unwrap();
        assert_eq!(i.argv, vec![OsString::from("hi"), OsString::from("a b")]);
    }
    #[test]
    fn rejects_unknown_input() {
        let c = parse_config("test", valid().as_bytes(), &env()).unwrap();
        assert!(resolve_cli(&c, "read", &serde_json::json!({"name":"a","x":1})).is_err());
    }
    #[test]
    fn rejects_optional_schema() {
        let s = valid().replace("required: [name]", "required: []");
        assert!(parse_config("test", s.as_bytes(), &env()).is_err());
    }

    #[test]
    fn mcp_binding_keeps_literal_environment_and_target_fixed() {
        let config = parse_config("test", mcp_valid().as_bytes(), &mcp_env()).unwrap();
        let invocation = resolve_mcp(
            &config,
            "query",
            &serde_json::json!({"owner": "team-a", "limit": 20}),
        )
        .unwrap();

        assert_eq!(invocation.target_id, "upstream");
        assert_eq!(invocation.command, PathBuf::from("/bin/fake-mcp"));
        assert_eq!(invocation.args, vec!["--fixed"]);
        assert_eq!(invocation.cwd, PathBuf::from("/"));
        assert_eq!(invocation.upstream_tool, "private-query");
        assert_eq!(
            invocation.arguments,
            serde_json::json!({
                "query": {"owner": "team-a", "visibility": "private"},
                "limit": 20,
                "credential": "secret-sentinel"
            })
        );

        let changed = resolve_mcp(
            &config,
            "query",
            &serde_json::json!({"owner": "other", "limit": 99}),
        )
        .unwrap();
        assert_eq!(changed.target_id, invocation.target_id);
        assert_eq!(changed.command, invocation.command);
        assert_eq!(changed.args, invocation.args);
        assert_eq!(changed.cwd, invocation.cwd);
        assert_eq!(changed.upstream_tool, invocation.upstream_tool);
        assert_eq!(changed.arguments["query"]["visibility"], "private");
        assert_eq!(changed.arguments["credential"], "secret-sentinel");
    }

    #[test]
    fn mcp_unknown_input_is_rejected_before_resolution() {
        let config = parse_config("test", mcp_valid().as_bytes(), &mcp_env()).unwrap();
        let result = resolve_mcp(
            &config,
            "query",
            &serde_json::json!({"owner": "team-a", "limit": 20, "credential": "attacker"}),
        );
        let diagnostic = result.unwrap_err();
        assert_eq!(diagnostic.code, BrokerErrorCode::InvalidArguments);
        assert!(diagnostic.path.contains("credential"));
    }

    #[test]
    fn mcp_binding_nodes_reject_unknown_fields_at_every_level() {
        let unknown = mcp_valid().replace(
            "credential: {environment: TOKEN}",
            "credential: {environment: TOKEN, extra: leaked}",
        );
        assert!(parse_config("test", unknown.as_bytes(), &mcp_env()).is_err());

        let nested = mcp_valid().replace(
            "visibility: {literal: private}",
            "visibility: {literal: private, extra: leaked}",
        );
        assert!(parse_config("test", nested.as_bytes(), &mcp_env()).is_err());
    }

    #[test]
    fn target_limit_fields_are_specific_to_target_kind() {
        let cli_with_mcp_limit = valid().replace(
            "stdout_bytes: 1024",
            "stdout_bytes: 1024, output_bytes: 1024",
        );
        assert!(parse_config("test", cli_with_mcp_limit.as_bytes(), &env()).is_err());

        let mcp_with_cli_limit = mcp_valid().replace(
            "output_bytes: 1024",
            "output_bytes: 1024, stdout_bytes: 1024",
        );
        assert!(parse_config("test", mcp_with_cli_limit.as_bytes(), &mcp_env()).is_err());
    }

    #[test]
    fn environment_debug_does_not_expose_values() {
        let source = valid().replace(
            "    cwd: /\n",
            "    cwd: /\n    environment:\n      set: {TOKEN: secret-sentinel}\n",
        );
        let config = parse_config("test", source.as_bytes(), &env()).unwrap();
        let rendered = format!("{:?}", config.targets["fake"].environment);
        assert!(!rendered.contains("secret-sentinel"));
        assert!(rendered.contains("TOKEN"));
    }

    #[test]
    fn environment_names_accept_lowercase_ascii_letters() {
        let source = valid().replace(
            "    cwd: /\n",
            "    cwd: /\n    environment:\n      set: {lowercase_name_2: fixed}\n",
        );
        assert!(parse_config("test", source.as_bytes(), &env()).is_ok());

        let invalid = source.replace("lowercase_name_2", "2lowercase_name");
        assert!(parse_config("test", invalid.as_bytes(), &env()).is_err());
    }

    #[test]
    fn catalog_schema_is_the_schema_used_by_validation() {
        let config = parse_config("test", valid().as_bytes(), &env()).unwrap();
        let catalog = tools_json(&config);
        let published = &catalog["tools"][0]["inputSchema"];
        assert_eq!(published, &config.tools["read"].schema.public_json);

        assert!(
            validate_input(
                &config.tools["read"].schema,
                &serde_json::json!({"name": "ok"})
            )
            .is_ok()
        );
        let invalid = serde_json::json!({"name": "", "unexpected": true});
        let issues = validate_input(&config.tools["read"].schema, &invalid).unwrap_err();
        assert!(issues.iter().any(|i| i.path == "/unexpected"));
        assert!(issues.iter().any(|i| i.path == "/name"));
    }

    #[test]
    fn normalization_is_deterministic_for_map_order() {
        let first = r#"version: 1
server: {name: test, transport: {kind: stdio}}
targets:
  fake:
    kind: cli
    executable: /bin/echo
    cwd: /
    limits: {timeout_ms: 1000, stdout_bytes: 1024, stderr_bytes: 1024}
tools:
  zed:
    input_schema: {type: object, properties: {z: {type: string}, a: {type: integer}}, required: [z, a], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: fixed}, {input: /z}, {input: /a}]}}
    output: {kind: text}
  alpha:
    input_schema: {type: object, properties: {value: {type: string}}, required: [value], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{input: /value}]}}
    output: {kind: text}
"#;
        let second = r#"version: 1
server: {transport: {kind: stdio}, name: test}
targets:
  fake:
    limits: {stderr_bytes: 1024, stdout_bytes: 1024, timeout_ms: 1000}
    cwd: /
    executable: /bin/echo
    kind: cli
tools:
  alpha:
    output: {kind: text}
    invoke: {cli: {argv: [{input: /value}]}, target: fake}
    input_schema: {required: [value], additionalProperties: false, properties: {value: {type: string}}, type: object}
  zed:
    output: {kind: text}
    invoke: {cli: {argv: [{literal: fixed}, {input: /z}, {input: /a}]}, target: fake}
    input_schema: {required: [a, z], additionalProperties: false, properties: {a: {type: integer}, z: {type: string}}, type: object}
"#;
        let left = parse_config("test", first.as_bytes(), &env()).unwrap();
        let right = parse_config("test", second.as_bytes(), &env()).unwrap();
        assert_eq!(tools_json(&left), tools_json(&right));

        let left_call =
            resolve_cli(&left, "zed", &serde_json::json!({"z": "name", "a": 4})).unwrap();
        let right_call =
            resolve_cli(&right, "zed", &serde_json::json!({"a": 4, "z": "name"})).unwrap();
        assert_eq!(left_call.target_id, right_call.target_id);
        assert_eq!(left_call.executable, right_call.executable);
        assert_eq!(left_call.cwd, right_call.cwd);
        assert_eq!(left_call.argv, right_call.argv);
    }

    #[test]
    fn invalid_config_is_rejected_atomically() {
        let mut invalid = valid();
        invalid = invalid.replace(
            "output: {kind: text}",
            "output: {kind: text, unexpected: true}",
        );
        let error = parse_config("test", invalid.as_bytes(), &env()).unwrap_err();
        assert_eq!(error.code, BrokerErrorCode::InvalidArguments);

        let valid_config = parse_config("test", valid().as_bytes(), &env()).unwrap();
        assert_eq!(valid_config.tools.len(), 1);
        assert_eq!(valid_config.targets.len(), 1);
    }

    #[test]
    fn malformed_yaml_diagnostic_does_not_include_configuration_bytes() {
        let malformed = r#"version: 1
server:
  name: SECRET_SENTINEL
  transport: {kind: stdio
targets: {}
tools: {}
"#;
        let error = parse_config("secret-config.yaml", malformed.as_bytes(), &env())
            .expect_err("malformed YAML must be rejected");
        assert_eq!(error.code, BrokerErrorCode::InvalidArguments);
        assert!(!error.message.contains("SECRET_SENTINEL"), "{error:?}");
        assert!(!error.to_string().contains("SECRET_SENTINEL"), "{error}");
    }

    #[test]
    fn finite_limit_boundaries_are_explicit() {
        assert!(ConfigBytes::new("test", vec![0; MAX_CONFIG_BYTES]).is_ok());
        assert_eq!(
            ConfigBytes::new("test", vec![0; MAX_CONFIG_BYTES + 1]),
            Err(BrokerErrorCode::InvalidArguments)
        );

        let at_limit =
            parse_config("test", limits_config(1, 1, 1, 1, 1).as_bytes(), &env()).unwrap();
        assert_eq!(at_limit.request_bytes, 1);
        assert_eq!(at_limit.json_depth, 1);
        assert_eq!(at_limit.targets["fake"].limits.timeout_ms, 1);
        assert_eq!(at_limit.targets["fake"].limits.output_bytes, 1);
        assert_eq!(at_limit.targets["fake"].limits.stderr_bytes, 1);

        assert!(
            parse_config(
                "test",
                limits_config(MAX_CONFIG_BYTES + 1, 1, 1, 1, 1).as_bytes(),
                &env(),
            )
            .is_err()
        );
        assert!(
            parse_config(
                "test",
                limits_config(1, MAX_JSON_DEPTH + 1, 1, 1, 1).as_bytes(),
                &env(),
            )
            .is_err()
        );
        assert!(
            parse_config(
                "test",
                limits_config(1, 1, 300_001, 1, 1).as_bytes(),
                &env(),
            )
            .is_err()
        );
        assert!(
            parse_config(
                "test",
                limits_config(1, 1, 1, 16 * 1024 * 1024 + 1, 1).as_bytes(),
                &env(),
            )
            .is_err()
        );
        assert!(
            parse_config(
                "test",
                limits_config(1, 1, 1, 1, 1024 * 1024 + 1).as_bytes(),
                &env(),
            )
            .is_err()
        );
    }

    #[test]
    fn schema_resource_boundaries_are_enforced_before_resolution() {
        let config_text = r#"version: 1
server: {name: test, transport: {kind: stdio}}
targets:
  fake:
    kind: cli
    executable: /bin/echo
    cwd: /
    limits: {timeout_ms: 1000, stdout_bytes: 1024, stderr_bytes: 1024}
tools:
  read:
    input_schema:
      type: object
      properties:
        text: {type: string}
        values: {type: array, items: {type: integer}, maxItems: 1024}
      required: [text, values]
      additionalProperties: false
    invoke: {target: fake, cli: {argv: [{literal: fixed}]}}
    output: {kind: text}
"#;
        let config = parse_config("test", config_text.as_bytes(), &env()).unwrap();
        let text_at_limit = "x".repeat(MAX_STRING_BYTES);
        assert!(
            resolve_cli(
                &config,
                "read",
                &serde_json::json!({
                    "text": text_at_limit,
                    "values": Value::Array(vec![Value::from(0); 1024])
                })
            )
            .is_ok()
        );

        let too_long = "x".repeat(MAX_STRING_BYTES + 1);
        assert!(
            resolve_cli(
                &config,
                "read",
                &serde_json::json!({
                    "text": too_long,
                    "values": Value::Array(vec![Value::from(0); 1024])
                })
            )
            .is_err()
        );
        assert!(
            resolve_cli(
                &config,
                "read",
                &serde_json::json!({
                    "text": "ok",
                    "values": Value::Array(vec![Value::from(0); 1025])
                })
            )
            .is_err()
        );
    }

    #[test]
    fn unsupported_regex_is_rejected_at_configuration_boundary() {
        let invalid = valid().replace(
            "{type: string, minLength: 1}",
            "{type: string, pattern: '(?=x)'}",
        );
        assert!(parse_config("test", invalid.as_bytes(), &env()).is_err());
    }

    #[test]
    fn regex_compiled_size_is_bounded_separately_from_pattern_bytes() {
        let pattern = String::from("(?:a|b){1000000}");
        assert!(pattern.len() <= 16 * 1024);
        let schema = serde_json::json!({"type": "string", "pattern": pattern});
        assert!(compile_schema(&schema, "/schema", false, 1).is_err());
    }

    #[test]
    fn schema_keywords_are_strictly_typed_and_scoped() {
        let invalid = [
            config_with_property_schema("{type: string, minItems: 1}"),
            config_with_property_schema("{type: string, pattern: 1}"),
            config_with_property_schema("{type: array, items: [], uniqueItems: true}"),
            config_with_property_schema("{type: array, items: {type: string}, uniqueItems: yes}"),
            config_with_property_schema("{type: string, enum: value}"),
            config_with_property_schema("{type: string, enum: [1]}"),
        ];
        for text in invalid {
            assert!(
                parse_config("test", text.as_bytes(), &env()).is_err(),
                "schema unexpectedly accepted: {text}"
            );
        }
    }

    #[test]
    fn enum_rejects_semantic_duplicates() {
        let text = config_with_property_schema("{type: number, enum: [1, 1.0]}");
        assert!(parse_config("test", text.as_bytes(), &env()).is_err());
    }

    #[test]
    fn numeric_limits_and_runtime_equality_preserve_large_integers() {
        let bounded = config_with_property_schema(
            "{type: integer, minimum: 9007199254740993, maximum: 9007199254740993}",
        );
        let config = parse_config("test", bounded.as_bytes(), &env()).unwrap();
        assert!(
            resolve_cli(
                &config,
                "read",
                &serde_json::json!({"name": 9007199254740992_i64})
            )
            .is_err()
        );
        assert!(
            resolve_cli(
                &config,
                "read",
                &serde_json::json!({"name": 9007199254740993_i64})
            )
            .is_ok()
        );

        let unique =
            config_with_property_schema("{type: array, items: {type: number}, uniqueItems: true}")
                .replace("{input: /name}", "{literal: fixed}");
        let config = parse_config("test", unique.as_bytes(), &env()).unwrap();
        assert!(
            validate_input(
                &config.tools["read"].schema,
                &serde_json::json!({"name": [9007199254740992_i64, 9007199254740993_i64]})
            )
            .is_ok()
        );
        assert!(
            validate_input(
                &config.tools["read"].schema,
                &serde_json::json!({"name": [1, 1.0]}),
            )
            .is_err()
        );

        let nested_number = config_with_property_schema(
            "{type: array, items: {type: number}, enum: [[9007199254740993]]}",
        )
        .replace("{input: /name}", "{literal: fixed}");
        let config = parse_config("test", nested_number.as_bytes(), &env()).unwrap();
        assert!(
            validate_input(
                &config.tools["read"].schema,
                &serde_json::json!({"name": [9007199254740993_i64]})
            )
            .is_ok()
        );
    }

    fn nested_schema(depth: usize) -> Value {
        let mut schema = serde_json::json!({"type": "string"});
        for _ in 1..depth {
            schema = serde_json::json!({
                "type": "object",
                "properties": {"child": schema},
                "required": ["child"],
                "additionalProperties": false
            });
        }
        schema
    }

    #[test]
    fn schema_and_binding_depth_are_bounded_at_adoption() {
        assert!(compile_schema(&nested_schema(MAX_SCHEMA_DEPTH), "/schema", true, 1).is_ok());
        assert!(compile_schema(&nested_schema(MAX_SCHEMA_DEPTH + 1), "/schema", true, 1).is_err());

        let config = parse_config("test", mcp_valid().as_bytes(), &mcp_env()).unwrap();
        let mut binding = RawMcpBinding::Literal(RawLiteralBinding {
            literal: Value::Null,
        });
        for _ in 1..MAX_SCHEMA_DEPTH {
            binding = RawMcpBinding::Object(RawObjectBinding {
                object: BTreeMap::from([(String::from("child"), binding)]),
            });
        }
        assert!(
            compile_mcp_binding(
                &binding,
                &config.tools["query"].schema,
                "/binding",
                &mcp_env(),
                1
            )
            .is_ok()
        );
        binding = RawMcpBinding::Object(RawObjectBinding {
            object: BTreeMap::from([(String::from("child"), binding)]),
        });
        assert!(
            compile_mcp_binding(
                &binding,
                &config.tools["query"].schema,
                "/binding",
                &mcp_env(),
                1
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_json_pointer_escape_is_rejected() {
        let text = valid().replace("{input: /name}", "{input: /name~2}");
        let error = parse_config("test", text.as_bytes(), &env()).unwrap_err();
        assert!(error.path.contains("/invoke/cli/argv"));
    }

    #[test]
    fn mcp_json_output_kind_is_rejected() {
        let text = mcp_valid().replace("output: {kind: structured}", "output: {kind: json}");
        assert!(parse_config("test", text.as_bytes(), &mcp_env()).is_err());
    }

    #[test]
    fn cli_negative_zero_is_normalized_before_argv() {
        let text = config_with_property_schema("{type: number}");
        let config = parse_config("test", text.as_bytes(), &env()).unwrap();
        let negative_zero: Value = serde_json::from_str("-0").unwrap();
        let invocation =
            resolve_cli(&config, "read", &serde_json::json!({"name": negative_zero})).unwrap();
        assert_eq!(
            invocation.argv,
            vec![OsString::from("hi"), OsString::from("0")]
        );
    }

    proptest! {
        #![proptest_config(property_config())]
        #[test]
        fn arbitrary_unknown_input_never_resolves(
            extra in "[a-z]{1,8}".prop_filter("must not be the configured property", |key| key != "name"),
            value in any::<i64>(),
        ) {
            let config = parse_config("test", valid().as_bytes(), &env()).unwrap();
            let input = serde_json::json!({"name": "ok", extra: value});
            prop_assert!(resolve_cli(&config, "read", &input).is_err());
        }

        #[test]
        fn valid_inputs_cannot_change_mcp_target_or_fixed_bindings(owner in "[a-z]{1,16}", limit in 1i64..=100) {
            let config = parse_config("test", mcp_valid().as_bytes(), &mcp_env()).unwrap();
            let invocation = resolve_mcp(
                &config,
                "query",
                &serde_json::json!({"owner": owner, "limit": limit}),
            ).unwrap();
            prop_assert_eq!(invocation.target_id, "upstream");
            prop_assert_eq!(invocation.command, PathBuf::from("/bin/fake-mcp"));
            prop_assert_eq!(invocation.args, vec![String::from("--fixed")]);
            prop_assert_eq!(invocation.cwd, PathBuf::from("/"));
            prop_assert_eq!(invocation.upstream_tool, "private-query");
            prop_assert_eq!(invocation.arguments["query"]["visibility"].as_str(), Some("private"));
            prop_assert_eq!(invocation.arguments["credential"].as_str(), Some("secret-sentinel"));
        }

        #[test]
        fn policy_catalog_and_invocation_are_closed_over_config(
            candidate in "[a-z]{1,16}",
            value in "[a-z]{1,16}",
        ) {
            let config = parse_config("test", valid().as_bytes(), &env()).unwrap();
            let catalog = tools_json(&config);
            prop_assert_eq!(catalog["tools"].as_array().map(Vec::len), Some(1));
            prop_assert_eq!(catalog["tools"][0]["name"].as_str(), Some("read"));

            let input = serde_json::json!({"name": value});
            if candidate == "read" {
                let invocation = resolve_cli(&config, &candidate, &input)?;
                prop_assert_eq!(invocation.target_id, "fake");
                prop_assert_eq!(invocation.executable, PathBuf::from("/bin/echo"));
                prop_assert_eq!(invocation.cwd, PathBuf::from("/"));
                prop_assert_eq!(&invocation.argv[0], &OsString::from("hi"));
            } else {
                prop_assert!(resolve_cli(&config, &candidate, &input).is_err());
            }
        }

        #[test]
        fn map_order_and_input_order_do_not_change_normalized_observations(
            reverse_tools in any::<bool>(),
            owner in "[a-z]{1,16}",
            limit in 1i64..=100,
        ) {
            let first = r#"  alpha:
    input_schema: {type: object, properties: {value: {type: string}}, required: [value], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{input: /value}]}}
    output: {kind: text}
"#;
            let second = r#"  zed:
    input_schema: {type: object, properties: {z: {type: string}, a: {type: integer}}, required: [z, a], additionalProperties: false}
    invoke: {target: fake, cli: {argv: [{literal: fixed}, {input: /z}, {input: /a}]}}
    output: {kind: text}
"#;
            let tools = if reverse_tools {
                format!("{second}{first}")
            } else {
                format!("{first}{second}")
            };
            let source = format!(r#"version: 1
server: {{name: test, transport: {{kind: stdio}}}}
targets:
  fake:
    kind: cli
    executable: /bin/echo
    cwd: /
    limits: {{timeout_ms: 1000, stdout_bytes: 1024, stderr_bytes: 1024}}
tools:
{tools}"#);
            let config = parse_config("test", source.as_bytes(), &env()).unwrap();
            let input_a = serde_json::json!({"z": owner.clone(), "a": limit});
            let input_b = serde_json::json!({"a": limit, "z": owner});
            let left = resolve_cli(&config, "zed", &input_a)?;
            let right = resolve_cli(&config, "zed", &input_b)?;
            prop_assert_eq!(tools_json(&config)["tools"].as_array().map(Vec::len), Some(2));
            prop_assert_eq!(&left, &right);
            prop_assert_eq!(left.argv, vec![OsString::from("fixed"), OsString::from(owner), OsString::from(limit.to_string())]);
        }

        #[test]
        fn fixed_process_context_is_independent_of_valid_input(value in "[a-z]{1,32}") {
            let source = valid().replace(
                "    cwd: /\n",
                "    cwd: /\n    environment:\n      set: {BOUNDARY_FIXED: fixed}\n",
            );
            let config = parse_config("test", source.as_bytes(), &env()).unwrap();
            let invocation = resolve_cli(&config, "read", &serde_json::json!({"name": value}))?;
            prop_assert_eq!(invocation.cwd, PathBuf::from("/"));
            prop_assert_eq!(invocation.environment.inherit, Vec::<String>::new());
            prop_assert_eq!(invocation.environment.set.get("BOUNDARY_FIXED"), Some(&String::from("fixed")));
            prop_assert_eq!(&invocation.argv[0], &OsString::from("hi"));
        }

        #[test]
        fn finite_target_limits_accept_boundary_and_reject_one_over(
            request_bytes in 1usize..=MAX_CONFIG_BYTES,
            json_depth in 1usize..=MAX_JSON_DEPTH,
            timeout_ms in 1u64..=300_000,
            output_bytes in 1usize..=16 * 1024 * 1024,
            stderr_bytes in 1usize..=1024 * 1024,
        ) {
            let source = limits_config(request_bytes, json_depth, timeout_ms, output_bytes, stderr_bytes);
            let config = parse_config("test", source.as_bytes(), &env()).unwrap();
            prop_assert_eq!(config.request_bytes, request_bytes);
            prop_assert_eq!(config.json_depth, json_depth);
            prop_assert_eq!(config.targets["fake"].limits.timeout_ms, timeout_ms);
            prop_assert_eq!(config.targets["fake"].limits.output_bytes, output_bytes);
            prop_assert_eq!(config.targets["fake"].limits.stderr_bytes, stderr_bytes);

            // The configured value may be any point in the finite domain.  A
            // one-over assertion must therefore be relative to the product
            // limit, rather than to the randomly selected valid value.
            prop_assert!(parse_config(
                "test",
                limits_config(MAX_CONFIG_BYTES + 1, json_depth, timeout_ms, output_bytes, stderr_bytes).as_bytes(),
                &env(),
            ).is_err());
            prop_assert!(parse_config(
                "test",
                limits_config(request_bytes, MAX_JSON_DEPTH + 1, timeout_ms, output_bytes, stderr_bytes).as_bytes(),
                &env(),
            ).is_err());
            prop_assert!(parse_config(
                "test",
                limits_config(request_bytes, json_depth, 300_001, output_bytes, stderr_bytes).as_bytes(),
                &env(),
            ).is_err());
            prop_assert!(parse_config(
                "test",
                limits_config(request_bytes, json_depth, timeout_ms, 16 * 1024 * 1024 + 1, stderr_bytes).as_bytes(),
                &env(),
            ).is_err());
            prop_assert!(parse_config(
                "test",
                limits_config(request_bytes, json_depth, timeout_ms, output_bytes, 1024 * 1024 + 1).as_bytes(),
                &env(),
            ).is_err());
        }

        #[test]
        fn published_schema_is_the_runtime_schema(value in "[a-z]{1,32}") {
            let config = parse_config("test", valid().as_bytes(), &env()).unwrap();
            let published = tools_json(&config)["tools"][0]["inputSchema"].clone();
            prop_assert_eq!(&published, &config.tools["read"].schema.public_json);
            let input = serde_json::json!({"name": value});
            prop_assert!(validate_input(&config.tools["read"].schema, &input).is_ok());
            prop_assert!(resolve_cli(&config, "read", &input).is_ok());
        }

        #[test]
        fn invalid_config_is_never_partially_adopted(extra_tool in "[a-z]{1,16}") {
            let source = format!(r#"{}  {extra_tool}:
    input_schema: {{type: object, properties: {{}}, required: [], additionalProperties: false}}
    invoke: {{target: missing-target, cli: {{argv: []}}}}
    output: {{kind: text}}
"#, valid());
            let error = parse_config("test", source.as_bytes(), &env()).unwrap_err();
            prop_assert_eq!(error.code, BrokerErrorCode::InvalidArguments);
            prop_assert!(error.path.contains("invoke/target"));
        }

        #[test]
        fn broker_diagnostics_never_echo_environment_secrets(secret in "[A-Z0-9_]{1,32}") {
            let environment = BTreeMap::from([(String::from("TOKEN"), secret.clone())]);
            let config = parse_config("test", mcp_valid().as_bytes(), &environment).unwrap();
            let error = resolve_mcp(
                &config,
                "query",
                &serde_json::json!({"owner": "team-a", "limit": 20, "credential": "attacker"}),
            )
            .unwrap_err();
            let rendered = error.to_string();
            prop_assert!(!rendered.contains(&secret));
            prop_assert!(!rendered.contains("attacker"));
        }
    }
}
