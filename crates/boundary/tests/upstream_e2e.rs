use mcp_boundary_core::{EnvironmentSpec, Limits, OutputKind, ResolvedMcpInvocation};
use mcp_boundary_runtime::McpExecutor;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn invocation(mode: &str, output_kind: OutputKind) -> ResolvedMcpInvocation {
    ResolvedMcpInvocation {
        target_id: format!("fake-{mode}"),
        command: PathBuf::from(env!("CARGO_BIN_EXE_fake-mcp")),
        args: vec![mode.into()],
        cwd: PathBuf::from("/"),
        environment: EnvironmentSpec {
            inherit: vec![],
            inherited_values: BTreeMap::new(),
            set: BTreeMap::new(),
        },
        upstream_tool: "echo_arguments".into(),
        arguments: json!({"value":"fixed-and-bound"}),
        limits: Limits {
            timeout_ms: 2000,
            stdout_bytes: 64 * 1024,
            output_bytes: 64 * 1024,
            stderr_bytes: 64 * 1024,
        },
        output_kind,
    }
}

#[tokio::test]
async fn modern_and_legacy_discovery_use_the_same_call_path() {
    let executor = McpExecutor::new();
    for mode in ["modern", "legacy"] {
        let result = executor
            .execute(invocation(mode, OutputKind::Text))
            .await
            .expect("fake MCP call");
        assert!(!result.is_error);
        assert_eq!(
            result.text.as_deref(),
            Some(r#"{"value":"fixed-and-bound"}"#)
        );
    }
}

#[tokio::test]
async fn target_process_is_reused_for_a_second_call() {
    let executor = McpExecutor::new();
    let first = executor
        .execute(invocation("modern", OutputKind::Text))
        .await
        .expect("first call");
    let second = executor
        .execute(invocation("modern", OutputKind::Text))
        .await
        .expect("second call");
    assert_eq!(first.text, second.text);
}

#[tokio::test]
async fn structured_result_is_bounded_and_meta_is_not_forwarded() {
    let executor = McpExecutor::new();
    let result = executor
        .execute(invocation("structured", OutputKind::Structured))
        .await
        .expect("structured call");
    assert!(!result.is_error);
    assert_eq!(
        result.structured,
        Some(json!({"received":{"value":"fixed-and-bound"}}))
    );
}

#[tokio::test]
async fn unsupported_content_is_target_output_failure() {
    let executor = McpExecutor::new();
    let error = executor
        .execute(invocation("bad-content", OutputKind::Text))
        .await
        .expect_err("unsupported content must fail");
    assert_eq!(
        error.code,
        mcp_boundary_core::BrokerErrorCode::InvalidTargetOutput
    );
}

#[tokio::test]
async fn upstream_wire_message_accepts_exact_limit_and_rejects_one_over_before_decode() {
    let executor = McpExecutor::new();
    let exact = executor
        .execute(invocation("wire-padding-1048576", OutputKind::Text))
        .await
        .expect("a one MiB upstream frame is within the transport limit");
    assert!(!exact.is_error);
    assert_eq!(
        exact.text.as_deref(),
        Some(r#"{"value":"fixed-and-bound"}"#)
    );

    let over = McpExecutor::new()
        .execute(invocation("wire-padding-1048577", OutputKind::Text))
        .await
        .expect_err("a one-byte-over upstream frame must be rejected");
    assert_eq!(
        over.code,
        mcp_boundary_core::BrokerErrorCode::OutputLimitExceeded
    );
}
