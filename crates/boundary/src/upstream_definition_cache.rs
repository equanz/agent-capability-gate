//! Version-3 cache of upstream definitions only.
//!
//! Exposure policy, projected schemas, fixed values, calls, and results are
//! deliberately absent. Every load is re-projected through the current config.

use crate::catalog_cache::sha256_hex;
use mcp_boundary_core::{MAX_TOOLS, UpstreamMcpTarget, serde_json};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_CACHE_BYTES: u64 = 1024 * 1024;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct UpstreamDefinitionCache {
    directory: PathBuf,
    target_id: String,
    identity: String,
    persistent: bool,
}

impl UpstreamDefinitionCache {
    pub fn new(target: &UpstreamMcpTarget) -> io::Result<Self> {
        // Deterministic hashes of environment values or launch arguments can
        // expose low-entropy credentials. Only targets with neither use a
        // persistent cache; the runtime still reuses definitions in-process.
        let persistent = target.args.is_empty()
            && target.environment.inherit.is_empty()
            && target.environment.inherited_values.is_empty()
            && target.environment.set.is_empty();
        let (directory, identity) = if persistent {
            let parent = match std::env::var_os("XDG_CACHE_HOME") {
                Some(path) if Path::new(&path).is_absolute() => PathBuf::from(path),
                _ => {
                    let home = std::env::var_os("HOME").ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "HOME is not set")
                    })?;
                    PathBuf::from(home).join(".cache")
                }
            };
            let identity_json = json!({
                "id": target.id,
                "command": target.command.to_string_lossy(),
                "args": target.args,
                "cwd": target.cwd.to_string_lossy(),
                "limits": {
                    "timeout_ms": target.limits.timeout_ms,
                    "stdout_bytes": target.limits.stdout_bytes,
                    "output_bytes": target.limits.output_bytes,
                    "stderr_bytes": target.limits.stderr_bytes,
                },
            });
            let encoded = serde_json::to_vec(&identity_json).map_err(io::Error::other)?;
            let identity = sha256_hex(&encoded);
            (
                parent
                    .join("mcp-boundary")
                    .join("catalog-v3")
                    .join(&identity),
                identity,
            )
        } else {
            (PathBuf::new(), String::new())
        };
        Ok(Self {
            directory,
            target_id: target.id.clone(),
            identity,
            persistent,
        })
    }

    pub fn load(&self) -> Option<BTreeMap<String, Value>> {
        if !self.persistent {
            return None;
        }
        let file = File::open(self.path()?).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_CACHE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return None;
        }
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        let object = value.as_object()?;
        if object.get("format_version")?.as_u64()? != 3
            || object.get("target")?.as_str()? != self.target_id
            || object.get("identity")?.as_str()? != self.identity
        {
            return None;
        }
        let tools = object.get("upstream_tools")?.as_object()?;
        if tools.len() > MAX_TOOLS {
            return None;
        }
        tools
            .iter()
            .map(|(name, tool)| {
                let tool = tool.as_object()?;
                if tool.get("name")?.as_str()? != name {
                    return None;
                }
                Some((name.clone(), Value::Object(tool.clone())))
            })
            .collect()
    }

    pub fn store(&self, tools: &BTreeMap<String, Value>) -> io::Result<()> {
        if !self.persistent {
            return Ok(());
        }
        if tools.len() > MAX_TOOLS
            || tools.iter().any(|(name, tool)| {
                tool.as_object()
                    .and_then(|object| object.get("name"))
                    .and_then(Value::as_str)
                    != Some(name.as_str())
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid upstream tool definitions",
            ));
        }
        fs::create_dir_all(&self.directory)?;
        let data = json!({
            "format_version": 3,
            "target": self.target_id,
            "identity": self.identity,
            "upstream_tools": tools,
        });
        let encoded = serde_json::to_vec(&data).map_err(io::Error::other)?;
        if encoded.len() as u64 > MAX_CACHE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream definition cache exceeds limit",
            ));
        }
        let destination = self.path().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid target identifier")
        })?;
        let temporary = self.directory.join(format!(
            ".{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(temporary, destination)
    }

    fn path(&self) -> Option<PathBuf> {
        if self.target_id.is_empty()
            || self.target_id.len() > 128
            || !self
                .target_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            || self.target_id == "."
            || self.target_id == ".."
        {
            return None;
        }
        Some(self.directory.join(format!("{}.json", self.target_id)))
    }
}
