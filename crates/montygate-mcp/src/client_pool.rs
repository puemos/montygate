use async_trait::async_trait;
use montygate_core::{
    bridge::McpClientPool, MontygateError, Result, RetryConfig, ToolDefinition, TransportConfig,
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
    config: ClientPoolConfig,
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
            config: ClientPoolConfig::default(),
        }
    }

    pub fn with_config(config: ClientPoolConfig) -> Self {
        Self {
            services: HashMap::new(),
            config,
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
                    MontygateError::Mcp(format!(
                        "Failed to spawn process '{}': {}",
                        command, e
                    ))
                })?;

                ().serve(transport).await.map_err(|e| {
                    MontygateError::Mcp(format!(
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
                    MontygateError::Mcp(format!(
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
            .ok_or_else(|| MontygateError::ServerNotFound(server_name.to_string()))?;

        let max_attempts = self.config.max_retries + 1;
        let base_delay = std::time::Duration::from_millis(self.config.retry_base_delay_ms);

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay = base_delay * 2u32.saturating_pow(attempt - 1);
                debug!(
                    "Retrying tool '{}' on server '{}' (attempt {}/{}), delay {:?}",
                    tool_name, server_name, attempt + 1, max_attempts, delay
                );
                tokio::time::sleep(delay).await;
            }

            let params = CallToolRequestParams {
                meta: None,
                name: tool_name.to_string().into(),
                arguments: arguments.as_object().cloned(),
                task: None,
            };

            match service.call_tool(params).await {
                Ok(result) => {
                    return serde_json::to_value(&result).map_err(MontygateError::from);
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    if is_retryable_error(&err_msg) && attempt + 1 < max_attempts {
                        warn!(
                            "Retryable error calling tool '{}' on server '{}': {}",
                            tool_name, server_name, err_msg
                        );
                        continue;
                    }
                    return Err(MontygateError::Mcp(format!(
                        "Tool call '{}' on server '{}' failed: {}",
                        tool_name, server_name, e
                    )));
                }
            }
        }

        // Should not be reached, but just in case
        Err(MontygateError::Mcp(format!(
            "Tool call '{}' on server '{}' failed after {} attempts",
            tool_name, server_name, max_attempts
        )))
    }

    async fn list_server_tools(&self, server_name: &str) -> Result<Vec<ToolDefinition>> {
        debug!("Listing tools for server '{}'", server_name);

        let service = self
            .services
            .get(server_name)
            .ok_or_else(|| MontygateError::ServerNotFound(server_name.to_string()))?;

        let tools = service.list_all_tools().await.map_err(|e| {
            MontygateError::Mcp(format!(
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

/// Check if an error message indicates a retryable transient failure.
pub fn is_retryable_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    lower.contains("connection reset")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("broken pipe")
        || lower.contains("connection refused")
        || lower.contains("stream closed")
        || lower.contains("connection closed")
        || lower.contains("eof")
        || lower.contains("temporarily unavailable")
}

/// Configuration for the client pool
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClientPoolConfig {
    pub connection_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub max_retries: u32,
    pub retry_base_delay_ms: u64,
}

impl Default for ClientPoolConfig {
    fn default() -> Self {
        Self {
            connection_timeout_secs: 30,
            request_timeout_secs: 60,
            max_retries: 3,
            retry_base_delay_ms: 100,
        }
    }
}

impl From<RetryConfig> for ClientPoolConfig {
    fn from(config: RetryConfig) -> Self {
        Self {
            connection_timeout_secs: config.connection_timeout_secs,
            request_timeout_secs: config.request_timeout_secs,
            max_retries: config.max_retries,
            retry_base_delay_ms: config.retry_base_delay_ms,
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
        assert_eq!(config.retry_base_delay_ms, 100);
    }

    #[test]
    fn test_client_pool_config_custom() {
        let config = ClientPoolConfig {
            connection_timeout_secs: 10,
            request_timeout_secs: 30,
            max_retries: 5,
            retry_base_delay_ms: 200,
        };
        assert_eq!(config.connection_timeout_secs, 10);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_base_delay_ms, 200);
    }

    #[test]
    fn test_client_pool_config_serialization() {
        let config = ClientPoolConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ClientPoolConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.connection_timeout_secs, 30);
        assert_eq!(deserialized.request_timeout_secs, 60);
        assert_eq!(deserialized.max_retries, 3);
        assert_eq!(deserialized.retry_base_delay_ms, 100);
    }

    #[test]
    fn test_client_pool_config_from_retry_config() {
        let retry = montygate_core::RetryConfig {
            max_retries: 5,
            retry_base_delay_ms: 200,
            connection_timeout_secs: 15,
            request_timeout_secs: 45,
        };
        let config: ClientPoolConfig = retry.into();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_base_delay_ms, 200);
        assert_eq!(config.connection_timeout_secs, 15);
        assert_eq!(config.request_timeout_secs, 45);
    }

    #[test]
    fn test_client_pool_with_config() {
        let config = ClientPoolConfig {
            connection_timeout_secs: 10,
            request_timeout_secs: 30,
            max_retries: 5,
            retry_base_delay_ms: 200,
        };
        let pool = ClientPool::with_config(config);
        assert!(pool.list_connections().is_empty());
        assert_eq!(pool.config.max_retries, 5);
    }

    // === is_retryable_error ===

    #[test]
    fn test_is_retryable_error_connection_reset() {
        assert!(is_retryable_error("connection reset by peer"));
        assert!(is_retryable_error("Connection Reset"));
    }

    #[test]
    fn test_is_retryable_error_timeout() {
        assert!(is_retryable_error("request timeout"));
        assert!(is_retryable_error("connection timed out"));
        assert!(is_retryable_error("operation Timeout"));
    }

    #[test]
    fn test_is_retryable_error_broken_pipe() {
        assert!(is_retryable_error("broken pipe"));
        assert!(is_retryable_error("Broken Pipe error"));
    }

    #[test]
    fn test_is_retryable_error_connection_refused() {
        assert!(is_retryable_error("connection refused"));
    }

    #[test]
    fn test_is_retryable_error_stream_closed() {
        assert!(is_retryable_error("stream closed unexpectedly"));
        assert!(is_retryable_error("connection closed"));
    }

    #[test]
    fn test_is_retryable_error_eof() {
        assert!(is_retryable_error("unexpected eof"));
    }

    #[test]
    fn test_is_retryable_error_temporarily_unavailable() {
        assert!(is_retryable_error("resource temporarily unavailable"));
    }

    #[test]
    fn test_is_retryable_error_non_retryable() {
        assert!(!is_retryable_error("invalid argument"));
        assert!(!is_retryable_error("permission denied"));
        assert!(!is_retryable_error("not found"));
        assert!(!is_retryable_error("bad request"));
    }
}
