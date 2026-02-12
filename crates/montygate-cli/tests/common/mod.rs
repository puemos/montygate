use montygate_core::{
    bridge::{AutoApproveHandler, Bridge, BridgeBuilder, MockClientPool},
    policy::PolicyEngine,
    registry::ToolRegistry,
    types::{
        MontygateConfig, PolicyAction, PolicyConfig, PolicyDefaults,
        ResourceLimits, ServerConfig, ServerInfo, ToolDefinition, TransportConfig,
    },
};
use std::collections::HashMap;
use std::sync::Arc;

/// Create a test registry pre-populated with tools from two mock servers
pub fn create_test_registry() -> Arc<ToolRegistry> {
    let registry = Arc::new(ToolRegistry::new());

    registry
        .register_server_tools(
            "echo",
            vec![ToolDefinition {
                name: "echo".to_string(),
                description: Some("Echo back the input".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {"type": "string"}
                    },
                    "required": ["message"]
                }),
            }],
        )
        .unwrap();

    registry
        .register_server_tools(
            "math",
            vec![ToolDefinition {
                name: "add".to_string(),
                description: Some("Add two numbers".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number"},
                        "b": {"type": "number"}
                    },
                    "required": ["a", "b"]
                }),
            }],
        )
        .unwrap();

    registry
}

/// Create a mock client pool with echo and math tools
pub fn create_test_client_pool() -> Arc<MockClientPool> {
    Arc::new(
        MockClientPool::new()
            .with_response("echo", serde_json::json!({"echoed": "hello"}))
            .with_response("add", serde_json::json!({"result": 42}))
            .with_tools(
                "echo",
                vec![ToolDefinition {
                    name: "echo".to_string(),
                    description: Some("Echo back the input".to_string()),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            )
            .with_tools(
                "math",
                vec![ToolDefinition {
                    name: "add".to_string(),
                    description: Some("Add two numbers".to_string()),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            ),
    )
}

/// Build a fully wired bridge for testing
pub fn create_test_bridge(
    registry: Arc<ToolRegistry>,
    policy: PolicyConfig,
    pool: Arc<MockClientPool>,
) -> Arc<Bridge> {
    Arc::new(
        BridgeBuilder::new()
            .registry(registry)
            .policy(Arc::new(PolicyEngine::new(policy)))
            .client_pool(pool)
            .build()
            .unwrap(),
    )
}

/// Build a bridge with auto-approve handler
pub fn create_test_bridge_with_approval(
    registry: Arc<ToolRegistry>,
    policy: PolicyConfig,
    pool: Arc<MockClientPool>,
) -> Arc<Bridge> {
    Arc::new(
        BridgeBuilder::new()
            .registry(registry)
            .policy(Arc::new(PolicyEngine::new(policy)))
            .client_pool(pool)
            .approval_handler(Arc::new(AutoApproveHandler))
            .build()
            .unwrap(),
    )
}

/// Default allow-all policy
pub fn allow_all_policy() -> PolicyConfig {
    PolicyConfig {
        defaults: PolicyDefaults {
            action: PolicyAction::Allow,
        },
        rules: vec![],
    }
}

/// Write a minimal valid config file and return its path
pub fn write_test_config(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let config = MontygateConfig {
        server: ServerInfo {
            name: "test-montygate".to_string(),
            version: "0.1.0".to_string(),
        },
        servers: vec![],
        limits: ResourceLimits::default(),
        policy: PolicyConfig::default(),
        retry: Default::default(),
    };

    let path = dir.path().join("config.toml");
    let toml_str = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&path, toml_str).unwrap();
    path
}

/// Write a config with servers for downstream connection testing
pub fn write_config_with_servers(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let config = MontygateConfig {
        server: ServerInfo {
            name: "test-montygate".to_string(),
            version: "0.1.0".to_string(),
        },
        servers: vec![ServerConfig {
            name: "echo".to_string(),
            transport: TransportConfig::Stdio {
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                env: HashMap::new(),
            },
        }],
        limits: ResourceLimits::default(),
        policy: PolicyConfig::default(),
        retry: Default::default(),
    };

    let path = dir.path().join("config.toml");
    let toml_str = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&path, toml_str).unwrap();
    path
}
