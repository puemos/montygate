use async_trait::async_trait;
use montygate_core::{
    bridge::McpClientPool, MontyGateError, Result, ToolDefinition, TransportConfig,
};
use rmcp::{model::CallToolRequestParams, RoleClient, ServiceExt};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use std::collections::HashMap;
use tracing::{debug, info, instrument, warn};

/// Manages connections to downstream MCP servers
pub struct ClientPool {
    services: HashMap<String, RunningService<RoleClient, ()>>,
}

impl std::fmt::Debug for ClientPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientPool")
            .field("connected_servers", &self.services.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ClientPool {
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
        }
    }

    /// Connect to a downstream MCP server
    #[instrument(skip(self, config))]
    pub async fn connect(&mut self, name: String, config: TransportConfig) -> Result<()> {
        info!("Connecting to downstream server '{}'", name);

        let service = match config {
            TransportConfig::Stdio { command, args, env } => {
                debug!("Using stdio transport: {} {:?}", command, args);
                let mut cmd = tokio::process::Command::new(&command);
                cmd.args(&args);
                for (k, v) in &env {
                    cmd.env(k, v);
                }

                let transport = TokioChildProcess::new(cmd).map_err(|e| {
                    MontyGateError::Mcp(format!(
                        "Failed to spawn process '{}': {}",
                        command, e
                    ))
                })?;

                ().serve(transport).await.map_err(|e| {
                    MontyGateError::Mcp(format!(
                        "Failed to initialize MCP connection to '{}': {}",
                        name, e
                    ))
                })?
            }
            TransportConfig::Sse { url } | TransportConfig::StreamableHttp { url } => {
                debug!("Using streamable HTTP transport: {}", url);
                let config = StreamableHttpClientTransportConfig::with_uri(&*url);
                let transport = StreamableHttpClientTransport::from_config(config);

                ().serve(transport).await.map_err(|e| {
                    MontyGateError::Mcp(format!(
                        "Failed to initialize MCP connection to '{}': {}",
                        name, e
                    ))
                })?
            }
        };

        info!("Successfully connected to server '{}'", name);
        self.services.insert(name, service);
        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&mut self, name: &str) -> Result<()> {
        info!("Disconnecting from server '{}'", name);

        if let Some(service) = self.services.remove(name) {
            let _ = service.cancel().await;
            debug!("Disconnected from server '{}'", name);
        }

        Ok(())
    }

    /// Check if a server is connected
    pub fn is_connected(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }

    /// List all connected servers
    pub fn list_connections(&self) -> Vec<String> {
        self.services.keys().cloned().collect()
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&mut self) {
        info!("Disconnecting from all servers");

        let names: Vec<String> = self.services.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.disconnect(&name).await {
                warn!("Error disconnecting from '{}': {}", name, e);
            }
        }
    }
}

impl Default for ClientPool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl McpClientPool for ClientPool {
    #[instrument(skip(self, arguments))]
    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        debug!(
            "Calling tool '{}' on server '{}'",
            tool_name, server_name
        );

        let service = self
            .services
            .get(server_name)
            .ok_or_else(|| MontyGateError::ServerNotFound(server_name.to_string()))?;

        let params = CallToolRequestParams {
            meta: None,
            name: tool_name.to_string().into(),
            arguments: arguments.as_object().cloned(),
            task: None,
        };

        let result = service.call_tool(params).await.map_err(|e| {
            MontyGateError::Mcp(format!(
                "Tool call '{}' on server '{}' failed: {}",
                tool_name, server_name, e
            ))
        })?;

        // Convert CallToolResult to serde_json::Value
        serde_json::to_value(&result).map_err(MontyGateError::from)
    }

    async fn list_server_tools(&self, server_name: &str) -> Result<Vec<ToolDefinition>> {
        debug!("Listing tools for server '{}'", server_name);

        let service = self
            .services
            .get(server_name)
            .ok_or_else(|| MontyGateError::ServerNotFound(server_name.to_string()))?;

        let tools = service.list_all_tools().await.map_err(|e| {
            MontyGateError::Mcp(format!(
                "Failed to list tools from server '{}': {}",
                server_name, e
            ))
        })?;

        Ok(tools
            .into_iter()
            .map(|t| ToolDefinition {
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()),
                input_schema: serde_json::Value::Object((*t.input_schema).clone()),
            })
            .collect())
    }

    fn is_server_connected(&self, server_name: &str) -> bool {
        self.is_connected(server_name)
    }

    fn connected_servers(&self) -> Vec<String> {
        self.list_connections()
    }
}

/// Configuration for the client pool
#[derive(Debug, Clone)]
pub struct ClientPoolConfig {
    pub connection_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for ClientPoolConfig {
    fn default() -> Self {
        Self {
            connection_timeout_secs: 30,
            request_timeout_secs: 60,
            max_retries: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use montygate_core::bridge::McpClientPool;

    // === ClientPool basic ===

    #[test]
    fn test_client_pool_new() {
        let pool = ClientPool::new();
        assert!(pool.list_connections().is_empty());
        assert!(!pool.is_connected("any"));
    }

    #[test]
    fn test_client_pool_default() {
        let pool = ClientPool::default();
        assert!(pool.list_connections().is_empty());
    }

    #[test]
    fn test_client_pool_debug() {
        let pool = ClientPool::new();
        let debug = format!("{:?}", pool);
        assert!(debug.contains("ClientPool"));
    }

    // === McpClientPool trait ===

    #[test]
    fn test_mcp_client_pool_is_server_connected() {
        let pool = ClientPool::new();
        assert!(!<ClientPool as McpClientPool>::is_server_connected(
            &pool, "srv"
        ));
    }

    #[test]
    fn test_mcp_client_pool_connected_servers() {
        let pool = ClientPool::new();
        let servers = <ClientPool as McpClientPool>::connected_servers(&pool);
        assert!(servers.is_empty());
    }

    // === ClientPoolConfig ===

    #[test]
    fn test_client_pool_config_default() {
        let config = ClientPoolConfig::default();
        assert_eq!(config.connection_timeout_secs, 30);
        assert_eq!(config.request_timeout_secs, 60);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_client_pool_config_custom() {
        let config = ClientPoolConfig {
            connection_timeout_secs: 10,
            request_timeout_secs: 30,
            max_retries: 5,
        };
        assert_eq!(config.connection_timeout_secs, 10);
        assert_eq!(config.max_retries, 5);
    }
}
