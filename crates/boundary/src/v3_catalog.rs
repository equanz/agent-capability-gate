//! Live, policy-derived version-3 MCP catalog.

use crate::upstream_definition_cache::UpstreamDefinitionCache;
use mcp_boundary_core::{
    McpExposureV3, V3ProjectionIssue, ValidatedConfig, derive_v3_public_tool, serde_json,
    tools_json,
};
use mcp_boundary_runtime::{Cancellation, McpExecutor};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, Debug, PartialEq)]
pub enum ListedToolState {
    Ready(Value),
    Blocked {
        tool: Value,
        reason: V3ProjectionIssue,
    },
    Unavailable(Value),
}

impl ListedToolState {
    pub fn public_tool(&self) -> &Value {
        match self {
            Self::Ready(tool) | Self::Unavailable(tool) => tool,
            Self::Blocked { tool, .. } => tool,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CatalogSnapshot {
    pub generation: u64,
    tools: BTreeMap<String, ListedToolState>,
}

impl CatalogSnapshot {
    fn new(generation: u64, tools: BTreeMap<String, ListedToolState>) -> Self {
        Self { generation, tools }
    }

    pub fn tool_state(&self, name: &str) -> Option<&ListedToolState> {
        self.tools.get(name)
    }
}

#[derive(Clone, Debug)]
pub struct RefreshOutcome {
    pub available: bool,
    pub changed: bool,
    pub tool_state: Option<ListedToolState>,
}

#[derive(Clone, Debug)]
pub struct ListRefreshOutcome {
    pub changed: bool,
    pub response: Value,
}

#[derive(Default)]
struct CatalogState {
    snapshot: Arc<CatalogSnapshot>,
    fresh_targets: BTreeSet<String>,
    catalog_was_listed: bool,
}

pub struct V3Catalog {
    config: Arc<ValidatedConfig>,
    caches: BTreeMap<String, UpstreamDefinitionCache>,
    state: Mutex<CatalogState>,
    refresh_lock: AsyncMutex<()>,
    sequence_lock: Arc<AsyncMutex<()>>,
}

impl V3Catalog {
    pub fn new(
        config: Arc<ValidatedConfig>,
        caches: BTreeMap<String, UpstreamDefinitionCache>,
    ) -> Self {
        let mut tools = BTreeMap::new();
        for target_id in config.upstream_targets.keys() {
            let upstream = caches
                .get(target_id)
                .and_then(UpstreamDefinitionCache::load)
                .unwrap_or_default();
            for exposure in config
                .v3_exposures
                .values()
                .filter(|item| item.target_id == *target_id)
            {
                tools.insert(
                    exposure.public_name.clone(),
                    project_or_unavailable(exposure, upstream.get(&exposure.upstream_tool)),
                );
            }
        }
        Self {
            config,
            caches,
            state: Mutex::new(CatalogState {
                snapshot: Arc::new(CatalogSnapshot::new(0, tools)),
                ..CatalogState::default()
            }),
            refresh_lock: AsyncMutex::new(()),
            sequence_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    /// Serialize each catalog refresh with the downstream response or
    /// notification that reports the resulting snapshot.
    pub async fn downstream_sequence(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.sequence_lock).lock_owned().await
    }

    pub fn management_json(&self) -> Value {
        let mut result = tools_json(&self.config);
        let (tools, unavailable) = self
            .state
            .lock()
            .map(|state| {
                let tools = state
                    .snapshot
                    .tools
                    .values()
                    .filter(|item| !matches!(item, ListedToolState::Unavailable(_)))
                    .map(|item| item.public_tool().clone())
                    .collect::<Vec<_>>();
                let unavailable = state
                    .snapshot
                    .tools
                    .iter()
                    .filter(|(_, item)| matches!(item, ListedToolState::Unavailable(_)))
                    .map(|(name, _)| json!({"name":name,"reason":"METADATA_UNAVAILABLE"}))
                    .collect::<Vec<_>>();
                (tools, unavailable)
            })
            .unwrap_or_default();
        let base = result.as_object_mut().expect("tools_json returns object");
        base.insert("tools".into(), Value::Array(tools));
        base.insert("unavailable".into(), Value::Array(unavailable));
        result
    }

    /// Refresh every configured target and return the exact whole-catalog
    /// snapshot adopted by this list request. No target update can interleave
    /// with this refresh or with the response that reports it.
    pub async fn refresh_all(
        &self,
        mcp: &McpExecutor,
        cancellation: &Cancellation,
    ) -> ListRefreshOutcome {
        let _serial = self.refresh_lock.lock().await;
        let (prior, mut fresh_targets, catalog_was_listed) = self.state.lock().map_or_else(
            |_| {
                (
                    Arc::new(CatalogSnapshot::new(0, BTreeMap::new())),
                    BTreeSet::new(),
                    false,
                )
            },
            |state| {
                (
                    Arc::clone(&state.snapshot),
                    state.fresh_targets.clone(),
                    state.catalog_was_listed,
                )
            },
        );
        let mut next_tools = prior.tools.clone();
        let targets = self
            .config
            .v3_exposures
            .values()
            .map(|item| item.target_id.clone())
            .collect::<BTreeSet<_>>();

        for target_id in targets {
            if fresh_targets.contains(&target_id) && mcp.is_connected(&target_id).await {
                continue;
            }
            match self.discover_target(&target_id, mcp, cancellation).await {
                Ok(discovered) => {
                    replace_target_projection(
                        &self.config,
                        &target_id,
                        &discovered,
                        &mut next_tools,
                    );
                    fresh_targets.insert(target_id);
                }
                Err(()) => {
                    fresh_targets.remove(&target_id);
                }
            }
        }

        let (snapshot, changed) = adopt_snapshot(&self.state, next_tools, fresh_targets, true);
        let changed = catalog_was_listed && changed;
        ListRefreshOutcome {
            changed,
            response: self.list_json(&snapshot),
        }
    }

    /// Refresh one target and capture the resulting tool state before another
    /// refresh can adopt a different catalog snapshot. The returned snapshot
    /// remains immutable for the caller's validation and argument rebuilding.
    pub async fn refresh_for_call(
        &self,
        target_id: &str,
        public_name: &str,
        mcp: &McpExecutor,
        cancellation: &Cancellation,
    ) -> RefreshOutcome {
        let _serial = self.refresh_lock.lock().await;
        let (prior, mut fresh_targets, catalog_was_listed) = self.state.lock().map_or_else(
            |_| {
                (
                    Arc::new(CatalogSnapshot::new(0, BTreeMap::new())),
                    BTreeSet::new(),
                    false,
                )
            },
            |state| {
                (
                    Arc::clone(&state.snapshot),
                    state.fresh_targets.clone(),
                    state.catalog_was_listed,
                )
            },
        );
        let mut available = true;
        let mut next_tools = prior.tools.clone();
        if !fresh_targets.contains(target_id) || !mcp.is_connected(target_id).await {
            match self.discover_target(target_id, mcp, cancellation).await {
                Ok(discovered) => {
                    replace_target_projection(
                        &self.config,
                        target_id,
                        &discovered,
                        &mut next_tools,
                    );
                    fresh_targets.insert(target_id.to_owned());
                }
                Err(()) => {
                    fresh_targets.remove(target_id);
                    available = false;
                }
            }
        }
        let (snapshot, changed) = adopt_snapshot(&self.state, next_tools, fresh_targets, false);
        let changed = catalog_was_listed && changed;
        RefreshOutcome {
            available,
            changed,
            tool_state: snapshot.tool_state(public_name).cloned(),
        }
    }

    pub async fn refresh_after_notification(
        &self,
        target_id: &str,
        mcp: &McpExecutor,
        cancellation: &Cancellation,
    ) -> RefreshOutcome {
        let _serial = self.refresh_lock.lock().await;
        let (prior, mut fresh_targets, catalog_was_listed) = self.state.lock().map_or_else(
            |_| {
                (
                    Arc::new(CatalogSnapshot::new(0, BTreeMap::new())),
                    BTreeSet::new(),
                    false,
                )
            },
            |state| {
                (
                    Arc::clone(&state.snapshot),
                    state.fresh_targets.clone(),
                    state.catalog_was_listed,
                )
            },
        );
        let mut next_tools = prior.tools.clone();
        let mut available = true;
        match self.discover_target(target_id, mcp, cancellation).await {
            Ok(discovered) => {
                replace_target_projection(&self.config, target_id, &discovered, &mut next_tools);
                fresh_targets.insert(target_id.to_owned());
            }
            Err(()) => {
                fresh_targets.remove(target_id);
                available = false;
            }
        }
        let (_snapshot, changed) = adopt_snapshot(&self.state, next_tools, fresh_targets, false);
        let changed = catalog_was_listed && changed;
        RefreshOutcome {
            available,
            changed,
            tool_state: None,
        }
    }

    async fn discover_target(
        &self,
        target_id: &str,
        mcp: &McpExecutor,
        cancellation: &Cancellation,
    ) -> Result<BTreeMap<String, Value>, ()> {
        let Some(target) = self.config.upstream_targets.get(target_id) else {
            return Err(());
        };
        let upstream_tools = mcp.list_tools(target, cancellation).await.map_err(|_| ())?;
        let mut discovered = BTreeMap::new();
        let mut duplicate_names = BTreeSet::new();
        for tool in upstream_tools {
            let Some(name) = tool.get("name").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if discovered.insert(name.clone(), tool).is_some() {
                duplicate_names.insert(name);
            }
        }
        for name in &duplicate_names {
            discovered.remove(name);
        }
        if duplicate_names.is_empty()
            && let Some(cache) = self.caches.get(target_id)
        {
            let _ = cache.store(&discovered);
        }
        for duplicate in duplicate_names {
            discovered.insert(
                duplicate.clone(),
                json!({"name":duplicate,"inputSchema":null}),
            );
        }
        Ok(discovered)
    }

    fn list_json(&self, snapshot: &CatalogSnapshot) -> Value {
        let mut tools = tools_json(&self.config)
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        tools.extend(
            snapshot
                .tools
                .values()
                .map(|item| item.public_tool().clone()),
        );
        sort_tools(&mut tools);
        json!({"tools": tools})
    }
}

fn adopt_snapshot(
    state: &Mutex<CatalogState>,
    tools: BTreeMap<String, ListedToolState>,
    fresh_targets: BTreeSet<String>,
    mark_listed: bool,
) -> (Arc<CatalogSnapshot>, bool) {
    let mut state = match state.lock() {
        Ok(state) => state,
        Err(_) => return (Arc::new(CatalogSnapshot::default()), false),
    };
    let changed = tools != state.snapshot.tools;
    state.fresh_targets = fresh_targets;
    if mark_listed {
        state.catalog_was_listed = true;
    }
    if !changed {
        return (Arc::clone(&state.snapshot), false);
    }
    let snapshot = Arc::new(CatalogSnapshot::new(
        state.snapshot.generation.saturating_add(1),
        tools,
    ));
    state.snapshot = Arc::clone(&snapshot);
    (snapshot, changed)
}

fn replace_target_projection(
    config: &ValidatedConfig,
    target_id: &str,
    upstream: &BTreeMap<String, Value>,
    tools: &mut BTreeMap<String, ListedToolState>,
) {
    tools.retain(|name, _| {
        config
            .v3_exposures
            .get(name)
            .is_none_or(|item| item.target_id != target_id)
    });
    for exposure in config
        .v3_exposures
        .values()
        .filter(|item| item.target_id == target_id)
    {
        tools.insert(
            exposure.public_name.clone(),
            match upstream.get(&exposure.upstream_tool) {
                Some(tool) => project(exposure, tool),
                None => blocked_state(exposure, V3ProjectionIssue::ToolNotFound),
            },
        );
    }
}

fn project_or_unavailable(exposure: &McpExposureV3, upstream: Option<&Value>) -> ListedToolState {
    upstream.map_or_else(
        || unavailable_state(exposure),
        |tool| project(exposure, tool),
    )
}

fn project(exposure: &McpExposureV3, upstream: &Value) -> ListedToolState {
    match derive_v3_public_tool(exposure, upstream) {
        Ok(tool) => ListedToolState::Ready(tool),
        Err(reason) => blocked_state(exposure, reason),
    }
}

fn blocked_state(exposure: &McpExposureV3, reason: V3ProjectionIssue) -> ListedToolState {
    let tool = json!({
        "name": exposure.public_name,
        "description": format!("Unavailable: upstream definition is incompatible ({})", reason.as_str()),
        "inputSchema": closed_empty_schema(),
    });
    ListedToolState::Blocked { tool, reason }
}

fn unavailable_state(exposure: &McpExposureV3) -> ListedToolState {
    ListedToolState::Unavailable(json!({
        "name": exposure.public_name,
        "description": "Unavailable: upstream tool metadata has not been retrieved; retry later",
        "inputSchema": closed_empty_schema(),
    }))
}

fn closed_empty_schema() -> Value {
    json!({"type":"object","properties":{},"required":[],"additionalProperties":false})
}

fn sort_tools(tools: &mut [Value]) {
    tools.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    fn marker_tool(name: &str, generation: &str) -> ListedToolState {
        ListedToolState::Ready(json!({
            "name": name,
            "description": generation,
            "inputSchema": {"type":"object","additionalProperties":false}
        }))
    }

    #[test]
    fn concurrent_readers_observe_whole_catalog_snapshots_and_pinned_tool_state() {
        let initial = Arc::new(CatalogSnapshot::new(
            1,
            BTreeMap::from([
                ("tool_a".into(), marker_tool("tool_a", "old")),
                ("tool_b".into(), marker_tool("tool_b", "old")),
            ]),
        ));
        let state = Arc::new(Mutex::new(CatalogState {
            snapshot: Arc::clone(&initial),
            ..CatalogState::default()
        }));
        let pinned_call_state = initial.tool_state("tool_a").unwrap().clone();
        let finished = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(2));
        let reader_state = Arc::clone(&state);
        let reader_finished = Arc::clone(&finished);
        let reader_barrier = Arc::clone(&barrier);
        let reader = thread::spawn(move || {
            let captured = Arc::clone(&reader_state.lock().unwrap().snapshot);
            assert_eq!(
                captured.tool_state("tool_a").unwrap().public_tool()["description"],
                "old"
            );
            reader_barrier.wait();
            let mut reads = 0;
            while !reader_finished.load(Ordering::Acquire) || reads < 10_000 {
                let snapshot = Arc::clone(&reader_state.lock().unwrap().snapshot);
                let a = snapshot.tool_state("tool_a").unwrap().public_tool()["description"]
                    .as_str()
                    .unwrap();
                let b = snapshot.tool_state("tool_b").unwrap().public_tool()["description"]
                    .as_str()
                    .unwrap();
                assert_eq!(a, b, "reader observed a mixed catalog generation");
                reads += 1;
                thread::yield_now();
            }
            assert_eq!(
                captured.tool_state("tool_a").unwrap().public_tool()["description"],
                "old",
                "a call that captured a generation keeps its tool state"
            );
        });
        let writer_state = Arc::clone(&state);
        let writer_finished = Arc::clone(&finished);
        let writer_barrier = Arc::clone(&barrier);
        let writer = thread::spawn(move || {
            writer_barrier.wait();
            for generation in 0..1_000 {
                let marker = if generation % 2 == 0 { "new" } else { "old" };
                let replacement = BTreeMap::from([
                    ("tool_a".into(), marker_tool("tool_a", marker)),
                    ("tool_b".into(), marker_tool("tool_b", marker)),
                ]);
                adopt_snapshot(&writer_state, replacement, BTreeSet::new(), false);
                thread::yield_now();
            }
            let replacement = BTreeMap::from([
                ("tool_a".into(), marker_tool("tool_a", "new")),
                ("tool_b".into(), marker_tool("tool_b", "new")),
            ]);
            adopt_snapshot(&writer_state, replacement, BTreeSet::new(), false);
            writer_finished.store(true, Ordering::Release);
        });
        reader.join().unwrap();
        writer.join().unwrap();
        assert_eq!(
            pinned_call_state.public_tool()["description"],
            "old",
            "an in-flight call must retain the schema snapshot it captured"
        );
    }
}
