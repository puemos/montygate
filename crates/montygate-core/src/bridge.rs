use crate::engine::ToolDispatcher;
use crate::policy::{PolicyDecision, PolicyEngine};
use crate::registry::ToolRegistry;
use crate::types::Result;
use crate::MontygateError;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, instrument, warn};

/// Handler for tool calls requiring manual approval.
///
/// When the policy engine returns `RequireApproval`, the bridge delegates
/// to this handler. If no handler is configured, approval requests are denied.
#[async_trait]
pub trait ApprovalHandler: Send + Sync + std::fmt::Debug {
    /// Request approval for a tool call. Returns `true` to allow, `false` to deny.
    async fn request_approval(&self, tool: &str, args: &serde_json::Value) -> Result<bool>;
}

/// Auto-approves all tool calls. Useful for testing and non-interactive environments.
#[derive(Debug, Clone, Default)]
pub struct AutoApproveHandler;

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(&self, tool: &str, _args: &serde_json::Value) -> Result<bool> {
        info!("Auto-approving tool call: {}", tool);
        Ok(true)
    }
}

/// Bridge between Monty execution and MCP tool calls
///
/// The Bridge is responsible for:
/// 1. Looking up tools in the registry
/// 2. Checking policies before calling
/// 3. Dispatching to the appropriate MCP client
/// 4. Recording execution traces
#[derive(Debug, Clone)]
pub struct Bridge {
    registry: Arc<ToolRegistry>,
    policy: Arc<PolicyEngine>,
    client_pool: Arc<dyn McpClientPool>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
}

impl Bridge {
    pub fn new(
        registry: Arc<ToolRegistry>,
        policy: Arc<PolicyEngine>,
        client_pool: Arc<dyn McpClientPool>,
    ) -> Self {
        Self {
            registry,
            policy,
            client_pool,
            approval_handler: None,
        }
    }

    /// Create a builder for constructing the bridge
    pub fn builder() -> BridgeBuilder {
        BridgeBuilder::new()
    }

    /// Parse a tool call from the external function call format
    /// 
    /// Expected format from Monty:
    /// - function_name: "tool"
    /// - args: ("server.tool_name", {arg1: val1, ...})
    ///
    /// Or with single dispatch:
    /// - function_name: "tool"
    /// - args: {"name": "server.tool_name", ...}
    pub fn parse_tool_call(
        &self,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<(String, serde_json::Value)> {
        match function_name {
            "tool" => {
                // Single dispatch: tool("server.tool_name", arg1=val1, ...)
                if let Some(obj) = arguments.as_object() {
                    if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                        let mut args = obj.clone();
                        args.remove("name");
                        Ok((name.to_string(), serde_json::Value::Object(args)))
                    } else {
                        Err(MontygateError::Bridge(
                            "Tool call missing 'name' field".to_string(),
                        ))
                    }
                } else if let Some(arr) = arguments.as_array() {
                    // Positional args: tool("server.tool_name", {args})
                    if !arr.is_empty() {
                        let name = arr[0]
                            .as_str()
                            .ok_or_else(|| MontygateError::Bridge(
                                "First argument must be tool name string".to_string(),
                            ))?;
                        let args = if arr.len() >= 2 {
                            arr[1].clone()
                        } else {
                            serde_json::json!({})
                        };
                        Ok((name.to_string(), args))
                    } else {
                        Err(MontygateError::Bridge(
                            "Tool call requires at least tool name".to_string(),
                        ))
                    }
                } else {
                    Err(MontygateError::Bridge(
                        "Invalid tool call format".to_string(),
                    ))
                }
            }
            _ => {
                // Direct dispatch: server_tool_name(args)
                // We'll need to parse the function name to extract server and tool
                let parts: Vec<&str> = function_name.splitn(2, '_').collect();
                if parts.len() == 2 {
                    let tool_name = format!("{}.{}", parts[0], parts[1]);
                    Ok((tool_name, arguments))
                } else {
                    Err(MontygateError::Bridge(format!(
                        "Unknown function: {}",
                        function_name
                    )))
                }
            }
        }
    }
}

#[async_trait]
impl ToolDispatcher for Bridge {
    #[instrument(skip(self, args), fields(tool_name = %tool_name))]
    async fn dispatch(&self, tool_name: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        let start = Instant::now();
        debug!("Bridge dispatching tool call: {}", tool_name);

        // 1. Resolve tool in registry
        let route = self.registry.resolve(tool_name)?;
        debug!("Resolved tool '{}' to server '{}'", tool_name, route.server_name);

        // 2. Check policy
        let decision = self.policy.check(&route, &args).await?;
        
        match decision {
            PolicyDecision::Allow => {
                debug!("Policy check passed for tool '{}'", tool_name);
            }
            PolicyDecision::Deny { reason } => {
                warn!("Tool '{}' denied by policy: {}", tool_name, reason);
                return Err(MontygateError::PolicyViolation(reason));
            }
            PolicyDecision::RequireApproval { tool, args: approval_args } => {
                info!("Tool '{}' requires approval", tool);
                match &self.approval_handler {
                    Some(handler) => {
                        match handler.request_approval(&tool, &approval_args).await {
                            Ok(true) => {
                                info!("Tool '{}' approved", tool);
                            }
                            Ok(false) => {
                                warn!("Tool '{}' approval denied", tool);
                                return Err(MontygateError::PolicyViolation(format!(
                                    "Tool '{}' approval was denied",
                                    tool
                                )));
                            }
                            Err(e) => {
                                error!("Approval request for tool '{}' failed: {}", tool, e);
                                return Err(MontygateError::PolicyViolation(format!(
                                    "Approval request failed for tool '{}': {}",
                                    tool, e
                                )));
                            }
                        }
                    }
                    None => {
                        warn!("Tool '{}' requires approval but no handler is configured", tool);
                        return Err(MontygateError::PolicyViolation(format!(
                            "Tool '{}' requires approval but no approval handler is configured",
                            tool
                        )));
                    }
                }
            }
            PolicyDecision::RateLimitExceeded { tool, limit } => {
                warn!("Rate limit exceeded for tool '{}': {}", tool, limit);
                return Err(MontygateError::RateLimitExceeded(format!(
                    "Rate limit exceeded: {}",
                    limit
                )));
            }
        }

        // 3. Dispatch to MCP client
        debug!("Calling tool '{}' on server '{}'", route.tool_name, route.server_name);
        
        match self
            .client_pool
            .call_tool(&route.server_name, &route.tool_name, args.clone())
            .await
        {
            Ok(result) => {
                let duration = start.elapsed().as_millis() as u64;
                info!(
                    "Tool '{}' completed successfully in {}ms",
                    tool_name, duration
                );
                Ok(result)
            }
            Err(e) => {
                let duration = start.elapsed().as_millis() as u64;
                error!(
                    "Tool '{}' failed after {}ms: {}",
                    tool_name, duration, e
                );
                Err(e)
            }
        }
    }
}

/// Builder for constructing the Bridge
#[derive(Debug, Default)]
pub struct BridgeBuilder {
    registry: Option<Arc<ToolRegistry>>,
    policy: Option<Arc<PolicyEngine>>,
    client_pool: Option<Arc<dyn McpClientPool>>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
}

impl BridgeBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn policy(mut self, policy: Arc<PolicyEngine>) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn client_pool(mut self, client_pool: Arc<dyn McpClientPool>) -> Self {
        self.client_pool = Some(client_pool);
        self
    }

    pub fn approval_handler(mut self, handler: Arc<dyn ApprovalHandler>) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn build(self) -> Result<Bridge> {
        Ok(Bridge {
            registry: self.registry.ok_or_else(|| {
                MontygateError::Configuration("Registry is required".to_string())
            })?,
            policy: self.policy.ok_or_else(|| {
                MontygateError::Configuration("Policy engine is required".to_string())
            })?,
            client_pool: self.client_pool.ok_or_else(|| {
                MontygateError::Configuration("Client pool is required".to_string())
            })?,
            approval_handler: self.approval_handler,
        })
    }
}

/// Trait for managing MCP client connections to downstream servers
#[async_trait]
pub trait McpClientPool: Send + Sync + std::fmt::Debug {
    /// Call a tool on a specific downstream server
    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value>;

    /// List tools available from a specific server
    async fn list_server_tools(&self, server_name: &str) -> Result<Vec<crate::types::ToolDefinition>>;

    /// Check if a server is connected
    fn is_server_connected(&self, server_name: &str) -> bool;

    /// Get list of connected servers
    fn connected_servers(&self) -> Vec<String>;
}

/// Mock client pool for testing
#[derive(Debug, Default)]
pub struct MockClientPool {
    responses: std::collections::HashMap<String, serde_json::Value>,
    tools: std::collections::HashMap<String, Vec<crate::types::ToolDefinition>>,
}

impl MockClientPool {
    pub fn new() -> Self {
        Self {
            responses: std::collections::HashMap::new(),
            tools: std::collections::HashMap::new(),
        }
    }

    pub fn with_response(mut self, tool: &str, response: serde_json::Value) -> Self {
        self.responses.insert(tool.to_string(), response);
        self
    }

    pub fn with_tools(mut self, server: &str, tools: Vec<crate::types::ToolDefinition>) -> Self {
        self.tools.insert(server.to_string(), tools);
        self
    }
}

#[async_trait]
impl McpClientPool for MockClientPool {
    async fn call_tool(
        &self,
        _server_name: &str,
        tool_name: &str,
        _arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.responses
            .get(tool_name)
            .cloned()
            .ok_or_else(|| MontygateError::ToolNotFound(tool_name.to_string()))
    }

    async fn list_server_tools(&self, server_name: &str) -> Result<Vec<crate::types::ToolDefinition>> {
        self.tools
            .get(server_name)
            .cloned()
            .ok_or_else(|| MontygateError::ServerNotFound(server_name.to_string()))
    }

    fn is_server_connected(&self, server_name: &str) -> bool {
        self.tools.contains_key(server_name)
    }

    fn connected_servers(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PolicyAction, PolicyConfig, PolicyDefaults, PolicyRule, ToolDefinition};

    fn create_test_registry() -> Arc<ToolRegistry> {
        let registry = Arc::new(ToolRegistry::new());
        let tools = vec![ToolDefinition {
            name: "create_issue".to_string(),
            description: Some("Create an issue".to_string()),
            input_schema: serde_json::json!({}),
        }];
        registry.register_server_tools("github", tools).unwrap();
        registry
    }

    fn create_test_policy() -> Arc<PolicyEngine> {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![],
        };
        Arc::new(PolicyEngine::new(config))
    }

    fn create_test_client_pool() -> Arc<dyn McpClientPool> {
        let pool = MockClientPool::new()
            .with_response("create_issue", serde_json::json!({"id": 123}))
            .with_tools(
                "github",
                vec![ToolDefinition {
                    name: "create_issue".to_string(),
                    description: Some("Create an issue".to_string()),
                    input_schema: serde_json::json!({}),
                }],
            );
        Arc::new(pool)
    }

    // === Bridge construction ===

    #[test]
    fn test_bridge_new() {
        let bridge = Bridge::new(
            create_test_registry(),
            create_test_policy(),
            create_test_client_pool(),
        );
        let debug = format!("{:?}", bridge);
        assert!(debug.contains("Bridge"));
    }

    #[test]
    fn test_bridge_builder() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build();

        assert!(bridge.is_ok());
    }

    #[test]
    fn test_bridge_builder_missing_registry() {
        let result = Bridge::builder()
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MontygateError::Configuration(_)
        ));
    }

    #[test]
    fn test_bridge_builder_missing_policy() {
        let result = Bridge::builder()
            .registry(create_test_registry())
            .client_pool(create_test_client_pool())
            .build();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MontygateError::Configuration(_)
        ));
    }

    #[test]
    fn test_bridge_builder_missing_client_pool() {
        let result = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .build();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MontygateError::Configuration(_)
        ));
    }

    #[test]
    fn test_bridge_builder_all_missing() {
        let result = BridgeBuilder::new().build();
        assert!(result.is_err());
    }

    // === Bridge dispatch ===

    #[tokio::test]
    async fn test_bridge_dispatch_success() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge
            .dispatch("github.create_issue", serde_json::json!({"title": "Test"}))
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({"id": 123}));
    }

    #[tokio::test]
    async fn test_bridge_dispatch_tool_not_found() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge
            .dispatch("github.unknown_tool", serde_json::json!({}))
            .await;

        assert!(matches!(
            result.unwrap_err(),
            MontygateError::ToolNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_bridge_dispatch_policy_deny() {
        let policy = Arc::new(PolicyEngine::new(PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "github.*".to_string(),
                action: PolicyAction::Deny,
                rate_limit: None,
            }],
        }));

        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(policy)
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge
            .dispatch("github.create_issue", serde_json::json!({}))
            .await;

        assert!(matches!(
            result.unwrap_err(),
            MontygateError::PolicyViolation(_)
        ));
    }

    #[tokio::test]
    async fn test_bridge_dispatch_policy_require_approval_no_handler() {
        let policy = Arc::new(PolicyEngine::new(PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "github.create_issue".to_string(),
                action: PolicyAction::RequireApproval,
                rate_limit: None,
            }],
        }));

        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(policy)
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge
            .dispatch("github.create_issue", serde_json::json!({}))
            .await;

        assert!(matches!(
            result.unwrap_err(),
            MontygateError::PolicyViolation(_)
        ));
    }

    #[tokio::test]
    async fn test_bridge_dispatch_policy_require_approval_with_auto_approve() {
        let policy = Arc::new(PolicyEngine::new(PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "github.create_issue".to_string(),
                action: PolicyAction::RequireApproval,
                rate_limit: None,
            }],
        }));

        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(policy)
            .client_pool(create_test_client_pool())
            .approval_handler(Arc::new(AutoApproveHandler))
            .build()
            .unwrap();

        let result = bridge
            .dispatch("github.create_issue", serde_json::json!({}))
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({"id": 123}));
    }

    #[tokio::test]
    async fn test_bridge_dispatch_rate_limit_exceeded() {
        let policy = Arc::new(PolicyEngine::new(PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "github.create_issue".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        }));

        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(policy)
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        // First call succeeds
        let _ = bridge
            .dispatch("github.create_issue", serde_json::json!({}))
            .await;

        // Second call should be rate limited
        let result = bridge
            .dispatch("github.create_issue", serde_json::json!({}))
            .await;

        assert!(matches!(
            result.unwrap_err(),
            MontygateError::RateLimitExceeded(_)
        ));
    }

    #[tokio::test]
    async fn test_bridge_dispatch_client_pool_error() {
        // Client pool without the response for the tool
        let pool = Arc::new(MockClientPool::new());

        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(pool)
            .build()
            .unwrap();

        let result = bridge
            .dispatch("github.create_issue", serde_json::json!({}))
            .await;

        assert!(result.is_err());
    }

    // === parse_tool_call ===

    #[test]
    fn test_parse_tool_call_single_dispatch() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call(
            "tool",
            serde_json::json!({
                "name": "github.create_issue",
                "title": "Test"
            }),
        );

        assert!(result.is_ok());
        let (name, args) = result.unwrap();
        assert_eq!(name, "github.create_issue");
        assert_eq!(args["title"], "Test");
        // "name" should be removed from args
        assert!(args.get("name").is_none());
    }

    #[test]
    fn test_parse_tool_call_positional() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call(
            "tool",
            serde_json::json!(["github.create_issue", {"title": "Test"}]),
        );

        assert!(result.is_ok());
        let (name, args) = result.unwrap();
        assert_eq!(name, "github.create_issue");
        assert_eq!(args["title"], "Test");
    }

    #[test]
    fn test_parse_tool_call_positional_name_only() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call("tool", serde_json::json!(["github.create_issue"]));

        assert!(result.is_ok());
        let (name, args) = result.unwrap();
        assert_eq!(name, "github.create_issue");
        assert_eq!(args, serde_json::json!({}));
    }

    #[test]
    fn test_parse_tool_call_missing_name() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call("tool", serde_json::json!({"title": "Test"}));
        assert!(matches!(result.unwrap_err(), MontygateError::Bridge(_)));
    }

    #[test]
    fn test_parse_tool_call_empty_array() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call("tool", serde_json::json!([]));
        assert!(matches!(result.unwrap_err(), MontygateError::Bridge(_)));
    }

    #[test]
    fn test_parse_tool_call_array_non_string_name() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call("tool", serde_json::json!([123, {}]));
        assert!(matches!(result.unwrap_err(), MontygateError::Bridge(_)));
    }

    #[test]
    fn test_parse_tool_call_invalid_format() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call("tool", serde_json::json!("just a string"));
        assert!(matches!(result.unwrap_err(), MontygateError::Bridge(_)));
    }

    #[test]
    fn test_parse_tool_call_direct_dispatch() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result =
            bridge.parse_tool_call("github_create_issue", serde_json::json!({"title": "Test"}));

        assert!(result.is_ok());
        let (name, args) = result.unwrap();
        assert_eq!(name, "github.create_issue");
        assert_eq!(args["title"], "Test");
    }

    #[test]
    fn test_parse_tool_call_direct_dispatch_no_underscore() {
        let bridge = Bridge::builder()
            .registry(create_test_registry())
            .policy(create_test_policy())
            .client_pool(create_test_client_pool())
            .build()
            .unwrap();

        let result = bridge.parse_tool_call("nounderscorefunction", serde_json::json!({}));
        assert!(matches!(result.unwrap_err(), MontygateError::Bridge(_)));
    }

    // === MockClientPool ===

    #[test]
    fn test_mock_client_pool_new() {
        let pool = MockClientPool::new();
        assert!(pool.connected_servers().is_empty());
    }

    #[tokio::test]
    async fn test_mock_client_pool_call_tool() {
        let pool = MockClientPool::new()
            .with_response("echo", serde_json::json!({"echoed": true}));

        let result = pool
            .call_tool("any_server", "echo", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"echoed": true}));
    }

    #[tokio::test]
    async fn test_mock_client_pool_call_tool_not_found() {
        let pool = MockClientPool::new();
        let result = pool
            .call_tool("server", "missing", serde_json::json!({}))
            .await;
        assert!(matches!(
            result.unwrap_err(),
            MontygateError::ToolNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_mock_client_pool_list_server_tools() {
        let tools = vec![ToolDefinition {
            name: "tool_a".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        }];
        let pool = MockClientPool::new().with_tools("myserver", tools);

        let result = pool.list_server_tools("myserver").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "tool_a");
    }

    #[tokio::test]
    async fn test_mock_client_pool_list_server_tools_not_found() {
        let pool = MockClientPool::new();
        let result = pool.list_server_tools("missing").await;
        assert!(matches!(
            result.unwrap_err(),
            MontygateError::ServerNotFound(_)
        ));
    }

    #[test]
    fn test_mock_client_pool_is_server_connected() {
        let pool = MockClientPool::new().with_tools("connected", vec![]);

        assert!(pool.is_server_connected("connected"));
        assert!(!pool.is_server_connected("not_connected"));
    }

    #[test]
    fn test_mock_client_pool_connected_servers() {
        let pool = MockClientPool::new()
            .with_tools("server_a", vec![])
            .with_tools("server_b", vec![]);

        let servers = pool.connected_servers();
        assert_eq!(servers.len(), 2);
        assert!(servers.contains(&"server_a".to_string()));
        assert!(servers.contains(&"server_b".to_string()));
    }
}