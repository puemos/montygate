mod common;

use common::*;
use montygate_core::{
    bridge::MockClientPool,
    engine::{EngineManager, ToolDispatcher},
    types::{PolicyAction, PolicyConfig, PolicyDefaults, PolicyRule, ResourceLimits, RunProgramInput},
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
