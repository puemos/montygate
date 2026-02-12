mod common;

use common::*;
use montygate_core::{
    bridge::MockClientPool,
    engine::{EngineManager, ToolDispatcher},
    types::{
        MontygateConfig, PolicyAction, PolicyConfig, PolicyDefaults, PolicyRule,
        ResourceLimits, RunProgramInput, ServerInfo, ToolDefinition,
    },
};
use montygate_mcp::MontygateMcpServer;
use std::collections::HashMap;
use std::sync::Arc;

// === Full pipeline tests ===

#[tokio::test]
async fn test_full_pipeline_mock_engine() {
    // Config → registry → bridge → mock engine → dispatch → result
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_mock(ResourceLimits::default());

    let input = RunProgramInput {
        code: "# TOOL: echo.echo {\"message\": \"hello\"}".to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert_eq!(result.trace.len(), 1);
    assert_eq!(result.trace[0].server, "echo");
    assert_eq!(result.trace[0].tool, "echo");
    assert!(result.trace[0].result.is_some());
    assert!(result.trace[0].error.is_none());
}

#[tokio::test]
async fn test_full_pipeline_monty_engine() {
    // Same pipeline but with the real Monty engine
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_monty(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"result = tool("echo.echo", message="hello")"#.to_string(),
        inputs: HashMap::new(),
        type_check: false,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert_eq!(result.trace.len(), 1);
    assert_eq!(result.trace[0].server, "echo");
    assert_eq!(result.trace[0].tool, "echo");
    assert!(result.trace[0].result.is_some());
}

// === Multi-server tests ===

#[tokio::test]
async fn test_multi_server_dispatch() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_mock(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"
# TOOL: echo.echo {"message": "hello"}
# TOOL: math.add {"a": 1, "b": 2}
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert_eq!(result.trace.len(), 2);
    assert_eq!(result.trace[0].server, "echo");
    assert_eq!(result.trace[1].server, "math");
    assert_eq!(result.stats.external_calls, 2);
}

#[tokio::test]
async fn test_multi_server_monty_engine() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_monty(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"
r1 = tool("echo.echo", message="hi")
r2 = tool("math.add", a=1, b=2)
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: false,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert_eq!(result.trace.len(), 2);
    assert_eq!(result.trace[0].server, "echo");
    assert_eq!(result.trace[1].server, "math");
}

// === Policy deny E2E ===

#[tokio::test]
async fn test_policy_deny_e2e() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let policy = PolicyConfig {
        defaults: PolicyDefaults {
            action: PolicyAction::Allow,
        },
        rules: vec![PolicyRule {
            match_pattern: "echo.*".to_string(),
            action: PolicyAction::Deny,
            rate_limit: None,
        }],
    };
    let bridge = create_test_bridge(registry.clone(), policy, pool);

    let engine = EngineManager::with_mock(ResourceLimits::default());

    let input = RunProgramInput {
        code: "# TOOL: echo.echo {\"message\": \"blocked\"}".to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    // Tool call should have failed due to policy
    assert_eq!(result.trace.len(), 1);
    assert!(result.trace[0].error.is_some());
    assert!(result.trace[0]
        .error
        .as_ref()
        .unwrap()
        .contains("Policy violation"));
}

#[tokio::test]
async fn test_policy_deny_monty_engine() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let policy = PolicyConfig {
        defaults: PolicyDefaults {
            action: PolicyAction::Allow,
        },
        rules: vec![PolicyRule {
            match_pattern: "echo.*".to_string(),
            action: PolicyAction::Deny,
            rate_limit: None,
        }],
    };
    let bridge = create_test_bridge(registry.clone(), policy, pool);

    let engine = EngineManager::with_monty(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"result = tool("echo.echo", message="blocked")"#.to_string(),
        inputs: HashMap::new(),
        type_check: false,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    // Monty engine reports errors in stderr/output rather than failing the whole execution
    assert_eq!(result.trace.len(), 1);
    assert!(result.trace[0].error.is_some());
}

// === Rate limiting E2E ===

#[tokio::test]
async fn test_rate_limit_e2e() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let policy = PolicyConfig {
        defaults: PolicyDefaults {
            action: PolicyAction::Allow,
        },
        rules: vec![PolicyRule {
            match_pattern: "echo.echo".to_string(),
            action: PolicyAction::Allow,
            rate_limit: Some("1/min".to_string()),
        }],
    };
    let bridge = create_test_bridge(registry.clone(), policy, pool);

    let engine = EngineManager::with_mock(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"
# TOOL: echo.echo {"message": "first"}
# TOOL: echo.echo {"message": "second"}
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert_eq!(result.trace.len(), 2);
    // First call succeeds
    assert!(result.trace[0].error.is_none());
    // Second call should be rate limited
    assert!(result.trace[1].error.is_some());
    assert!(result.trace[1]
        .error
        .as_ref()
        .unwrap()
        .contains("Rate limit"));
}

// === Error propagation ===

#[tokio::test]
async fn test_error_propagation_tool_not_found() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    // Call a tool that exists in registry but pool has no response for
    let result = bridge
        .dispatch("echo.echo", serde_json::json!({"message": "hello"}))
        .await;
    assert!(result.is_ok()); // echo.echo has a response in the mock

    // Call a tool that doesn't exist in registry
    let result = bridge
        .dispatch("nonexistent.tool", serde_json::json!({}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_error_propagation_downstream_error() {
    let registry = create_test_registry();
    // Pool with no responses - all calls will fail
    let pool = Arc::new(MockClientPool::new());
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_mock(ResourceLimits::default());

    let input = RunProgramInput {
        code: "# TOOL: echo.echo {\"message\": \"fail\"}".to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert_eq!(result.trace.len(), 1);
    assert!(result.trace[0].error.is_some());
}

// === Approval flow E2E ===

#[tokio::test]
async fn test_approval_flow_no_handler() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let policy = PolicyConfig {
        defaults: PolicyDefaults {
            action: PolicyAction::Allow,
        },
        rules: vec![PolicyRule {
            match_pattern: "echo.echo".to_string(),
            action: PolicyAction::RequireApproval,
            rate_limit: None,
        }],
    };
    // No approval handler -> should deny
    let bridge = create_test_bridge(registry.clone(), policy, pool);

    let result = bridge
        .dispatch("echo.echo", serde_json::json!({"message": "approve me"}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_approval_flow_with_auto_approve() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let policy = PolicyConfig {
        defaults: PolicyDefaults {
            action: PolicyAction::Allow,
        },
        rules: vec![PolicyRule {
            match_pattern: "echo.echo".to_string(),
            action: PolicyAction::RequireApproval,
            rate_limit: None,
        }],
    };
    let bridge = create_test_bridge_with_approval(registry.clone(), policy, pool);

    let result = bridge
        .dispatch("echo.echo", serde_json::json!({"message": "approve me"}))
        .await;
    assert!(result.is_ok());
}

// === MCP server tests ===

#[test]
fn test_mcp_server_creation_with_tools() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);
    let engine = EngineManager::with_mock(ResourceLimits::default());

    let server = MontygateMcpServer::new(engine.engine(), bridge, registry);
    let debug = format!("{:?}", server);
    assert!(debug.contains("MontygateMcpServer"));
}

// === call_tool escape hatch ===

#[tokio::test]
async fn test_call_tool_direct_dispatch() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    // Directly dispatch through the bridge (same path call_tool uses)
    let result = bridge
        .dispatch("echo.echo", serde_json::json!({"message": "direct"}))
        .await
        .unwrap();
    assert_eq!(result, serde_json::json!({"echoed": "hello"}));
}

// === Config validation tests ===

#[test]
fn test_config_serialization_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_test_config(&dir);

    let content = std::fs::read_to_string(&path).unwrap();
    let config: montygate_core::MontygateConfig = toml::from_str(&content).unwrap();
    assert_eq!(config.server.name, "test-montygate");
    assert!(config.servers.is_empty());
}

#[test]
fn test_config_with_servers_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config_with_servers(&dir);

    let content = std::fs::read_to_string(&path).unwrap();
    let config: montygate_core::MontygateConfig = toml::from_str(&content).unwrap();
    assert_eq!(config.servers.len(), 1);
    assert_eq!(config.servers[0].name, "echo");
}

// === Full flow: all three enhancements ===

/// Full flow: input injection → serial tool() → batch_tools() → output
/// Exercises Enhancement 1 (inputs), Enhancement 2 (batch_tools), and the full
/// Monty → channel → async dispatch → Bridge → MockClientPool pipeline.
#[tokio::test]
async fn test_full_flow_inputs_serial_batch() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_monty(ResourceLimits::default());

    let mut inputs = HashMap::new();
    inputs.insert("greeting".to_string(), serde_json::json!("hello from inputs"));
    inputs.insert("count".to_string(), serde_json::json!(3));

    let input = RunProgramInput {
        code: r#"
# 1. Use injected input variables
print(f"greeting={greeting}")
print(f"count={count}")

# 2. Serial tool call using an input variable
r1 = tool("echo.echo", message=greeting)
print(f"serial_result={r1}")

# 3. Batch parallel tool calls — dispatch count times
calls = []
for i in range(count):
    calls.append(("math.add", {"a": i, "b": 10}))
batch_results = batch_tools(calls)
print(f"batch_len={len(batch_results)}")

# 4. Return combined result (bare expression so Monty captures it)
{"serial": r1, "batch_count": len(batch_results), "input_count": count}
"#
        .to_string(),
        inputs,
        type_check: false,
    };

    let result = engine.execute(input, bridge).await.unwrap();

    // Verify stdout captured input injection
    assert!(result.stdout.contains("greeting=hello from inputs"));
    assert!(result.stdout.contains("count=3"));

    // Verify serial + batch tool calls all recorded in trace
    // 1 serial echo.echo + 3 batch math.add = 4 total
    assert_eq!(result.trace.len(), 4, "Expected 4 trace entries, got {}: {:?}",
        result.trace.len(),
        result.trace.iter().map(|t| format!("{}.{}", t.server, t.tool)).collect::<Vec<_>>()
    );
    assert_eq!(result.trace[0].server, "echo");
    assert_eq!(result.trace[0].tool, "echo");
    assert!(result.trace[0].result.is_some());
    for i in 1..4 {
        assert_eq!(result.trace[i].server, "math");
        assert_eq!(result.trace[i].tool, "add");
        assert!(result.trace[i].result.is_some());
    }

    // Verify batch produced 3 results
    assert!(result.stdout.contains("batch_len=3"));

    // Final result captured from last expression
    assert_eq!(result.output["input_count"], 3);
    assert_eq!(result.output["batch_count"], 3);
}

/// Full flow: batch_tools error handling — one tool succeeds, another fails
#[tokio::test]
async fn test_full_flow_batch_partial_failure() {
    let registry = create_test_registry();
    // Pool only has "echo" response, not "add" — so math.add calls will fail
    let pool = Arc::new(
        MockClientPool::new()
            .with_response("echo", serde_json::json!({"echoed": "ok"}))
            .with_tools("echo", vec![ToolDefinition {
                name: "echo".to_string(),
                description: Some("Echo".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }])
            .with_tools("math", vec![ToolDefinition {
                name: "add".to_string(),
                description: Some("Add".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
    );
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_monty(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"
results = batch_tools([
    ("echo.echo", {"message": "hi"}),
    ("math.add", {"a": 1, "b": 2}),
])
# First result should succeed, second should be error dict
print(f"r0_type={type(results[0])}")
print(f"r1_type={type(results[1])}")
has_error = "error" in results[1] if type(results[1]) == dict else False
print(f"has_error={has_error}")
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: false,
    };

    let result = engine.execute(input, bridge).await.unwrap();

    // Both calls should be in trace
    assert_eq!(result.trace.len(), 2);
    // First succeeds
    assert!(result.trace[0].result.is_some());
    assert!(result.trace[0].error.is_none());
    // Second fails (no mock response)
    assert!(result.trace[1].error.is_some());
}

/// Full flow: input injection with empty inputs (backward compatibility)
#[tokio::test]
async fn test_full_flow_empty_inputs_backward_compat() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let engine = EngineManager::with_monty(ResourceLimits::default());

    let input = RunProgramInput {
        code: r#"
x = 42
r = tool("echo.echo", message="test")
print(f"done={x}")
{"value": x}
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: false,
    };

    let result = engine.execute(input, bridge).await.unwrap();
    assert!(result.stdout.contains("done=42"));
    assert_eq!(result.trace.len(), 1);
    assert_eq!(result.output["value"], 42);
}

/// Full flow: retry config wired through MontygateConfig TOML roundtrip
#[test]
fn test_full_flow_retry_config_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let config = MontygateConfig {
        server: ServerInfo {
            name: "retry-test".to_string(),
            version: "0.1.0".to_string(),
        },
        servers: vec![],
        limits: ResourceLimits::default(),
        policy: PolicyConfig::default(),
        retry: montygate_core::RetryConfig {
            max_retries: 5,
            retry_base_delay_ms: 200,
            connection_timeout_secs: 15,
            request_timeout_secs: 45,
        },
    };

    let path = dir.path().join("config.toml");
    let toml_str = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&path, &toml_str).unwrap();

    // Re-parse and verify retry config survived the roundtrip
    let loaded: MontygateConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(loaded.retry.max_retries, 5);
    assert_eq!(loaded.retry.retry_base_delay_ms, 200);
    assert_eq!(loaded.retry.connection_timeout_secs, 15);
    assert_eq!(loaded.retry.request_timeout_secs, 45);

    // Verify it converts to ClientPoolConfig correctly
    let pool_config: montygate_mcp::ClientPoolConfig = loaded.retry.into();
    assert_eq!(pool_config.max_retries, 5);
    assert_eq!(pool_config.retry_base_delay_ms, 200);
}

/// Full flow via MCP server handler: exercises the same path as run_program MCP tool.
/// Uses MontygateServerHandler which is the same handler that MCP clients hit.
#[tokio::test]
async fn test_full_flow_via_mcp_server_handler() {
    use montygate_mcp::server::MontygateServerHandler;

    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);
    let engine = EngineManager::with_monty(ResourceLimits::default());

    // Build the handler (same path used by MCP server)
    let handler = MontygateServerHandler::new(engine.engine(), bridge);

    // Simulate what an MCP client sends as run_program input
    let input = RunProgramInput {
        code: r#"
x = a + b
result = tool("echo.echo", message="sum is " + str(x))
print(f"computed={x}")
"#
        .to_string(),
        inputs: {
            let mut m = HashMap::new();
            m.insert("a".to_string(), serde_json::json!(10));
            m.insert("b".to_string(), serde_json::json!(20));
            m
        },
        type_check: false,
    };

    let result = handler.handle_run_program(input).await.unwrap();
    assert!(result.stdout.contains("computed=30"));
    assert_eq!(result.trace.len(), 1);
    assert_eq!(result.trace[0].server, "echo");
}

/// Full flow: the "big demo" — inputs + serial tool + batch_tools + output,
/// going through the MontygateServerHandler (same path as MCP clients).
#[tokio::test]
async fn test_full_flow_big_demo() {
    use montygate_mcp::server::MontygateServerHandler;

    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);
    let engine = EngineManager::with_monty(ResourceLimits::default());
    let handler = MontygateServerHandler::new(engine.engine(), bridge);

    let input = RunProgramInput {
        code: r#"
# Step 1: use injected variables
label = prefix + "_" + suffix

# Step 2: serial tool call
echo_result = tool("echo.echo", message=label)

# Step 3: batch parallel tool calls
batch_results = batch_tools([
    ("echo.echo", {"message": "batch-a"}),
    ("math.add", {"a": 1, "b": 2}),
    ("echo.echo", {"message": "batch-b"}),
])

# Step 4: aggregate and return as last expression
print("ALL_DONE")
{"label": label, "echo": echo_result, "batch_count": len(batch_results)}
"#
        .to_string(),
        inputs: {
            let mut m = HashMap::new();
            m.insert("prefix".to_string(), serde_json::json!("hello"));
            m.insert("suffix".to_string(), serde_json::json!("world"));
            m
        },
        type_check: false,
    };

    let result = handler.handle_run_program(input).await.unwrap();

    // Verify output
    assert!(result.stdout.contains("ALL_DONE"));
    assert_eq!(result.output["label"], "hello_world");
    assert_eq!(result.output["batch_count"], 3);

    // 1 serial echo + 3 batch (2 echo + 1 math) = 4 trace entries
    assert_eq!(result.trace.len(), 4);

    // All should have succeeded
    for tc in &result.trace {
        assert!(tc.result.is_some(), "Tool call {}.{} should have succeeded", tc.server, tc.tool);
        assert!(tc.error.is_none(), "Tool call {}.{} should have no error", tc.server, tc.tool);
    }
}

// === Resource limits E2E ===

#[tokio::test]
async fn test_resource_limit_code_length() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let limits = ResourceLimits {
        max_code_length: 10,
        ..Default::default()
    };
    let engine = EngineManager::with_monty(limits);

    let input = RunProgramInput {
        code: "x = 1 + 2 + 3 + 4 + 5".to_string(),
        inputs: HashMap::new(),
        type_check: false,
    };

    let result = engine.execute(input, bridge).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_resource_limit_external_calls() {
    let registry = create_test_registry();
    let pool = create_test_client_pool();
    let bridge = create_test_bridge(registry.clone(), allow_all_policy(), pool);

    let limits = ResourceLimits {
        max_external_calls: 1,
        ..Default::default()
    };
    let engine = EngineManager::with_mock(limits);

    let input = RunProgramInput {
        code: r#"
# TOOL: echo.echo {"message": "first"}
# TOOL: echo.echo {"message": "second"}
"#
        .to_string(),
        inputs: HashMap::new(),
        type_check: true,
    };

    let result = engine.execute(input, bridge).await;
    assert!(result.is_err());
}
