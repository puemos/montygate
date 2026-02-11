use async_trait::async_trait;
use montygate_core::{
    bridge::McpClientPool, MontyGateError, Result, ToolDefinition, TransportConfig,
};
use std::collections::HashMap;
use tracing::{debug, info, instrument, warn};

/// Manages connections to downstream MCP servers
#[derive(Debug)]
pub struct ClientPool {
    connections: HashMap<String, ServerConnection>,
}

impl ClientPool {
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }

    /// Connect to a downstream MCP server
    #[instrument(skip(self, config))]
    pub async fn connect(&mut self, name: String, config: TransportConfig) -> Result<()> {
        info!("Connecting to downstream server '{}'", name);

        let connection = match config {
            TransportConfig::Stdio { command, args, env } => {
                debug!("Using stdio transport: {} {:?}", command, args);
                // Implementation would:
                // 1. Spawn the process
                // 2. Initialize MCP connection via rmcp
                // 3. Store the connection handle
                ServerConnection::Stdio { command, args, env }
            }
            TransportConfig::Sse { url } => {
                debug!("Using SSE transport: {}", url);
                ServerConnection::Sse { url }
            }
            TransportConfig::StreamableHttp { url } => {
                debug!("Using streamable HTTP transport: {}", url);
                ServerConnection::StreamableHttp { url }
            }
        };

        self.connections.insert(name, connection);
        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&mut self, name: &str) -> Result<()> {
        info!("Disconnecting from server '{}'", name);
        
        if let Some(_conn) = self.connections.remove(name) {
            // Implementation would gracefully close the connection
            debug!("Disconnected from server '{}'", name);
        }
        
        Ok(())
    }

    /// Get a connection by name
    pub fn get_connection(&self, name: &str) -> Option<&ServerConnection> {
        self.connections.get(name)
    }

    /// List all connected servers
    pub fn list_connections(&self) -> Vec<String> {
        self.connections.keys().cloned().collect()
    }

    /// Check if a server is connected
    pub fn is_connected(&self, name: &str) -> bool {
        self.connections.contains_key(name)
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&mut self) {
        info!("Disconnecting from all servers");
        
        for name in self.list_connections() {
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
    #[instrument(skip(self, _arguments))]
    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        _arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        debug!(
            "Calling tool '{}' on server '{}'",
            tool_name, server_name
        );

        if !self.is_connected(server_name) {
            return Err(MontyGateError::ServerNotFound(server_name.to_string()));
        }

        // Implementation would:
        // 1. Get the connection for the server
        // 2. Send MCP tools/call request
        // 3. Parse and return the result

        todo!("Implement actual tool calling via rmcp")
    }

    async fn list_server_tools(&self, server_name: &str) -> Result<Vec<ToolDefinition>> {
        debug!("Listing tools for server '{}'", server_name);

        if !self.is_connected(server_name) {
            return Err(MontyGateError::ServerNotFound(server_name.to_string()));
        }

        // Implementation would:
        // 1. Get the connection for the server
        // 2. Send MCP tools/list request
        // 3. Parse and return the tools

        todo!("Implement actual tool listing via rmcp")
    }

    fn is_server_connected(&self, server_name: &str) -> bool {
        self.is_connected(server_name)
    }

    fn connected_servers(&self) -> Vec<String> {
        self.list_connections()
    }
}

/// Represents a connection to a downstream MCP server
#[derive(Debug, Clone)]
pub enum ServerConnection {
    Stdio {
        command: String,
        args: Vec<String>,
        env: std::collections::HashMap<String, String>,
    },
    Sse {
        url: String,
    },
    StreamableHttp {
        url: String,
    },
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

    // === Connect all transports ===

    #[tokio::test]
    async fn test_client_pool_connect_stdio() {
        let mut pool = ClientPool::new();

        let result = pool
            .connect(
                "test".to_string(),
                TransportConfig::Stdio {
                    command: "echo".to_string(),
                    args: vec!["hello".to_string()],
                    env: HashMap::new(),
                },
            )
            .await;

        assert!(result.is_ok());
        assert!(pool.is_connected("test"));
    }

    #[tokio::test]
    async fn test_client_pool_connect_sse() {
        let mut pool = ClientPool::new();

        let result = pool
            .connect(
                "sse_server".to_string(),
                TransportConfig::Sse {
                    url: "http://localhost:3000/sse".to_string(),
                },
            )
            .await;

        assert!(result.is_ok());
        assert!(pool.is_connected("sse_server"));
    }

    #[tokio::test]
    async fn test_client_pool_connect_http() {
        let mut pool = ClientPool::new();

        let result = pool
            .connect(
                "http_server".to_string(),
                TransportConfig::StreamableHttp {
                    url: "http://localhost:8080/mcp".to_string(),
                },
            )
            .await;

        assert!(result.is_ok());
        assert!(pool.is_connected("http_server"));
    }

    // === get_connection ===

    #[tokio::test]
    async fn test_get_connection() {
        let mut pool = ClientPool::new();
        pool.connect(
            "test".to_string(),
            TransportConfig::Sse {
                url: "http://localhost:3000".to_string(),
            },
        )
        .await
        .unwrap();

        let conn = pool.get_connection("test");
        assert!(conn.is_some());
        assert!(matches!(conn.unwrap(), ServerConnection::Sse { .. }));

        let missing = pool.get_connection("missing");
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_get_connection_stdio() {
        let mut pool = ClientPool::new();
        pool.connect(
            "stdio_srv".to_string(),
            TransportConfig::Stdio {
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                env: HashMap::new(),
            },
        )
        .await
        .unwrap();

        let conn = pool.get_connection("stdio_srv");
        assert!(matches!(conn.unwrap(), ServerConnection::Stdio { .. }));
    }

    // === list_connections ===

    #[tokio::test]
    async fn test_list_connections() {
        let mut pool = ClientPool::new();
        pool.connect(
            "a".to_string(),
            TransportConfig::Sse {
                url: "http://a".to_string(),
            },
        )
        .await
        .unwrap();
        pool.connect(
            "b".to_string(),
            TransportConfig::Sse {
                url: "http://b".to_string(),
            },
        )
        .await
        .unwrap();

        let list = pool.list_connections();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"a".to_string()));
        assert!(list.contains(&"b".to_string()));
    }

    // === Disconnect ===

    #[tokio::test]
    async fn test_client_pool_disconnect() {
        let mut pool = ClientPool::new();

        pool.connect(
            "test".to_string(),
            TransportConfig::Sse {
                url: "http://localhost:3000".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(pool.is_connected("test"));

        pool.disconnect("test").await.unwrap();
        assert!(!pool.is_connected("test"));
    }

    #[tokio::test]
    async fn test_disconnect_nonexistent() {
        let mut pool = ClientPool::new();
        // Should not error
        let result = pool.disconnect("ghost").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_client_pool_disconnect_all() {
        let mut pool = ClientPool::new();

        pool.connect(
            "server1".to_string(),
            TransportConfig::Sse {
                url: "http://localhost:3001".to_string(),
            },
        )
        .await
        .unwrap();

        pool.connect(
            "server2".to_string(),
            TransportConfig::Sse {
                url: "http://localhost:3002".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(pool.list_connections().len(), 2);

        pool.disconnect_all().await;
        assert!(pool.list_connections().is_empty());
    }

    // === McpClientPool trait ===

    #[test]
    fn test_mcp_client_pool_is_server_connected() {
        let mut pool = ClientPool::new();
        // Using tokio::runtime::Runtime to setup state
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            pool.connect(
                "srv".to_string(),
                TransportConfig::Sse {
                    url: "http://x".to_string(),
                },
            )
            .await
            .unwrap();
        });

        assert!(<ClientPool as McpClientPool>::is_server_connected(
            &pool, "srv"
        ));
        assert!(!<ClientPool as McpClientPool>::is_server_connected(
            &pool, "nope"
        ));
    }

    #[test]
    fn test_mcp_client_pool_connected_servers() {
        let mut pool = ClientPool::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            pool.connect(
                "a".to_string(),
                TransportConfig::Sse {
                    url: "http://a".to_string(),
                },
            )
            .await
            .unwrap();
            pool.connect(
                "b".to_string(),
                TransportConfig::Sse {
                    url: "http://b".to_string(),
                },
            )
            .await
            .unwrap();
        });

        let servers = <ClientPool as McpClientPool>::connected_servers(&pool);
        assert_eq!(servers.len(), 2);
    }

    // === ServerConnection ===

    #[test]
    fn test_server_connection_debug() {
        let conn = ServerConnection::Stdio {
            command: "node".into(),
            args: vec!["s.js".into()],
            env: HashMap::new(),
        };
        let debug = format!("{:?}", conn);
        assert!(debug.contains("Stdio"));
        assert!(debug.contains("node"));
    }

    #[test]
    fn test_server_connection_clone() {
        let conn = ServerConnection::Sse {
            url: "http://x".into(),
        };
        let cloned = conn.clone();
        assert!(matches!(cloned, ServerConnection::Sse { url } if url == "http://x"));
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