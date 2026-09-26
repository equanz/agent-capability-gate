//! Version-2 catalog projection. The config is the only source of exposed
//! names and fixed values; upstream discovery supplies definitions for those
//! names, and the protected cache supplies frozen restricted snapshots.

use crate::catalog_cache::{CachedTool, CatalogCache};
use mcp_boundary_core::{
    McpExposure, McpExposureMode, ValidatedConfig, derive_restricted_public_tool,
    restricted_compatibility, serde_json, tools_json,
};
use mcp_boundary_runtime::{Cancellation, McpExecutor};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Default)]
struct CatalogState {
    active: BTreeMap<String, Value>,
    cached: BTreeMap<String, BTreeMap<String, CachedTool>>,
    changed: BTreeMap<String, &'static str>,
    unavailable: BTreeSet<String>,
}

pub struct V2Catalog {
    config: Arc<ValidatedConfig>,
    cache: Option<CatalogCache>,
    state: Mutex<CatalogState>,
    refresh_lock: AsyncMutex<()>,
}

impl V2Catalog {
    pub fn new(config: Arc<ValidatedConfig>, cache: Option<CatalogCache>) -> Self {
        let mut state = CatalogState::default();
        if let Some(cache) = &cache {
            for target_id in config.upstream_targets.keys() {
                let Some(cached) = cache.load(target_id) else {
                    continue;
                };
                for exposure in config
                    .v2_exposures
                    .values()
                    .filter(|item| item.target_id == *target_id)
                {
                    let Some(entry) = cached.get(&exposure.upstream_tool) else {
                        continue;
                    };
                    if entry.upstream.get("name").and_then(Value::as_str)
                        == Some(exposure.upstream_tool.as_str())
                        && entry.public.get("name").and_then(Value::as_str)
                            == Some(exposure.public_name.as_str())
                        && entry
                            .public
                            .get("inputSchema")
                            .and_then(Value::as_object)
                            .is_some()
                    {
                        state
                            .active
                            .insert(exposure.public_name.clone(), entry.public.clone());
                    }
                }
                state.cached.insert(target_id.clone(), cached);
            }
        }
        Self {
            config,
            cache,
            state: Mutex::new(state),
            refresh_lock: AsyncMutex::new(()),
        }
    }

    pub fn list_json(&self) -> Value {
        let mut tools = tools_json(&self.config)
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Ok(state) = self.state.lock() {
            tools.extend(state.active.values().cloned());
        }
        tools.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .cmp(&right.get("name").and_then(Value::as_str))
        });
        json!({"tools": tools})
    }

    pub fn management_json(&self) -> Value {
        let mut result = self.list_json();
        let active = self
            .state
            .lock()
            .ok()
            .map(|state| state.active.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let unavailable = self
            .config
            .v2_exposures
            .keys()
            .filter(|name| !active.contains(*name))
            .map(|name| json!({"name": name, "reason": "METADATA_UNAVAILABLE"}))
            .collect::<Vec<_>>();
        result
            .as_object_mut()
            .expect("catalog is object")
            .insert("unavailable".into(), Value::Array(unavailable));
        result
    }

    pub fn public_tool(&self, name: &str) -> Option<Value> {
        self.state.lock().ok()?.active.get(name).cloned()
    }

    pub fn changed_reason(&self, name: &str) -> Option<&'static str> {
        self.state.lock().ok()?.changed.get(name).copied()
    }

    pub async fn refresh_all(&self, mcp: &McpExecutor, cancellation: &Cancellation) -> bool {
        let mut changed = false;
        for target_id in self.config.upstream_targets.keys() {
            if self
                .config
                .v2_exposures
                .values()
                .any(|item| item.target_id == *target_id)
            {
                changed |= self.refresh_target(target_id, mcp, cancellation).await;
            }
        }
        changed
    }

    pub async fn refresh_target(
        &self,
        target_id: &str,
        mcp: &McpExecutor,
        cancellation: &Cancellation,
    ) -> bool {
        let _serial = self.refresh_lock.lock().await;
        let Some(target) = self.config.upstream_targets.get(target_id) else {
            return false;
        };
        let upstream = match mcp.list_tools(target, cancellation).await {
            Ok(tools) => tools,
            Err(_) => {
                if let Ok(mut state) = self.state.lock() {
                    state.unavailable.insert(target_id.to_owned());
                }
                return false;
            }
        };
        let mut discovered = BTreeMap::new();
        for tool in upstream {
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                return false;
            };
            if discovered.insert(name.to_owned(), tool).is_some() {
                return false;
            }
        }
        let (previous, was_unavailable) = self
            .state
            .lock()
            .ok()
            .map(|state| {
                (
                    state.cached.get(target_id).cloned().unwrap_or_default(),
                    state.unavailable.contains(target_id),
                )
            })
            .unwrap_or_default();
        let mut next_cached = previous.clone();
        let mut next_active = BTreeMap::new();
        let mut tombstones = BTreeMap::new();
        let mut needs_new_freeze = BTreeSet::new();
        for exposure in self
            .config
            .v2_exposures
            .values()
            .filter(|item| item.target_id == target_id)
        {
            let Some(observed) = discovered.get(&exposure.upstream_tool) else {
                if previous.contains_key(&exposure.upstream_tool) {
                    tombstones.insert(
                        exposure.public_name.clone(),
                        "upstream tool is no longer listed",
                    );
                }
                continue;
            };
            match projected_tool(exposure, observed, previous.get(&exposure.upstream_tool)) {
                Ok((public, is_new_freeze)) => {
                    if is_new_freeze {
                        needs_new_freeze.insert(exposure.public_name.clone());
                    }
                    next_cached.insert(
                        exposure.upstream_tool.clone(),
                        CachedTool {
                            upstream: observed.clone(),
                            public: public.clone(),
                        },
                    );
                    next_active.insert(exposure.public_name.clone(), public);
                }
                Err(reason) => {
                    if previous.contains_key(&exposure.upstream_tool) {
                        tombstones.insert(exposure.public_name.clone(), reason);
                    }
                }
            }
        }
        if previous != next_cached {
            let stored = self
                .cache
                .as_ref()
                .is_some_and(|cache| cache.store(target_id, &next_cached).is_ok());
            if !stored {
                // A restriction cannot gain a first public schema without a
                // durable, administrator-protected frozen snapshot.
                for name in needs_new_freeze {
                    next_active.remove(&name);
                    if let Some(exposure) = self.config.v2_exposures.get(&name) {
                        next_cached.remove(&exposure.upstream_tool);
                    }
                }
            }
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let had_prior_catalog = state.cached.contains_key(target_id)
            || state.active.keys().any(|name| {
                self.config
                    .v2_exposures
                    .get(name)
                    .is_some_and(|item| item.target_id == target_id)
            });
        let prior_active = state.active.clone();
        state.active.retain(|name, _| {
            self.config
                .v2_exposures
                .get(name)
                .is_none_or(|item| item.target_id != target_id)
        });
        state.changed.retain(|name, _| {
            self.config
                .v2_exposures
                .get(name)
                .is_none_or(|item| item.target_id != target_id)
        });
        state.active.extend(next_active);
        state.changed.extend(tombstones);
        state.cached.insert(target_id.to_owned(), next_cached);
        state.unavailable.remove(target_id);
        was_unavailable || (had_prior_catalog && state.active != prior_active)
    }
}

fn projected_tool(
    exposure: &McpExposure,
    upstream: &Value,
    previous: Option<&CachedTool>,
) -> Result<(Value, bool), &'static str> {
    match &exposure.mode {
        McpExposureMode::Proxy => {
            let mut public = upstream.clone();
            let object = public
                .as_object_mut()
                .ok_or("upstream tool definition is invalid")?;
            object.insert("name".into(), Value::String(exposure.public_name.clone()));
            Ok((public, false))
        }
        McpExposureMode::Restriction(_) => {
            if let Some(previous) = previous {
                if !restricted_compatibility(exposure, &previous.public, upstream) {
                    return Err("restricted arguments are incompatible with upstream definition");
                }
                Ok((previous.public.clone(), false))
            } else {
                derive_restricted_public_tool(exposure, upstream)
                    .map(|public| (public, true))
                    .map_err(|_| "restricted definition cannot be derived")
            }
        }
    }
}
