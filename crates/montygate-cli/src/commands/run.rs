use crate::config::{get_config_path, load_config};
use anyhow::{Context, Result};
use montygate_core::{
    bridge::{Bridge, BridgeBuilder, McpClientPool},
    engine::EngineManager,
    policy::PolicyEngine,
    registry::ToolRegistry,
};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run_server(
    config_path: Option<std::path::PathBuf>,
    transport: String,
    host: String,
    port: u16,
    test_config: bool,
    list_tools: bool,
) -> Result<()> {
    let path = get_config_path(config_path)?;
    let config = load_config(Some(path))?;

    info!("Loaded configuration for server '{}'", config.server.name);

    // Test mode: validate config and exit
    if test_config {
        info!("Configuration test passed");
        return Ok(());
    }

    // Initialize components
    let registry = Arc::new(ToolRegistry::new());
    let policy = Arc::new(PolicyEngine::new(config.policy.clone()));

    // Connect to downstream servers and discover tools
    let mut client_pool = montygate_mcp::ClientPool::new();

    for server_config in &config.servers {
        info!("Connecting to downstream server '{}'", server_config.name);

        if let Err(e) = client_pool
            .connect(server_config.name.clone(), server_config.transport.clone())
            .await
        {
            warn!("Failed to connect to server '{}': {}", server_config.name, e);
            continue;
        }

        // Discover tools from the server
        match McpClientPool::list_server_tools(&client_pool, &server_config.name).await {
            Ok(tools) => {
                info!(
                    "Discovered {} tools from server '{}'",
                    tools.len(),
                    server_config.name
                );
                if let Err(e) = registry.register_server_tools(&server_config.name, tools) {
                    warn!(
                        "Failed to register tools from '{}': {}",
                        server_config.name, e
                    );
                }
            }
            Err(e) => {
                warn!(
                    "Failed to discover tools from '{}': {}",
                    server_config.name, e
                );
            }
        }
    }

    // Create client pool wrapper for the bridge
    let client_pool_arc: Arc<dyn montygate_core::bridge::McpClientPool> = Arc::new(client_pool);

    // Build the bridge
    let bridge = BridgeBuilder::new()
        .registry(registry.clone())
        .policy(policy.clone())
        .client_pool(client_pool_arc.clone())
        .build()
        .context("Failed to build bridge")?;

    // Initialize execution engine
    let engine = EngineManager::with_monty(config.limits.clone());

    // List tools mode
    if list_tools {
        println!("Available tools:");
        for tool in registry.list_tools() {
            println!("  - {}", tool);
        }
        return Ok(());
    }

    // Start MCP server
    info!("Starting MCP server with {} transport", transport);

    match transport.as_str() {
        "stdio" => {
            run_stdio_server(engine, Arc::new(bridge), registry.clone()).await?;
        }
        "sse" | "http" => {
            run_http_server(&host, port, engine, Arc::new(bridge), registry.clone()).await?;
        }
        _ => {
            anyhow::bail!("Unknown transport: {}", transport);
        }
    }

    info!("Montygate shutdown complete");
    Ok(())
}

/// Run the MCP server with stdio transport
async fn run_stdio_server(
    engine: EngineManager,
    bridge: Arc<Bridge>,
    registry: Arc<ToolRegistry>,
) -> Result<()> {
    info!("Starting stdio transport");

    // Create the MCP server with the engine, dispatcher (bridge), and registry
    let server = montygate_mcp::MontygateMcpServer::new(
        engine.engine(),
        bridge,
        registry,
    );

    // Run the server with stdio transport
    server.run_stdio().await?;

    Ok(())
}

/// Run the MCP server with streamable HTTP transport (handles both SSE and HTTP)
async fn run_http_server(
    host: &str,
    port: u16,
    engine: EngineManager,
    bridge: Arc<Bridge>,
    registry: Arc<ToolRegistry>,
) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
        session::local::LocalSessionManager,
    };
    use tokio_util::sync::CancellationToken;

    info!("Starting streamable HTTP transport on {}:{}", host, port);

    let ct = CancellationToken::new();

    let engine = engine.engine();
    let service = StreamableHttpService::new(
        move || {
            Ok(montygate_mcp::MontygateMcpServer::new(
                engine.clone(),
                bridge.clone(),
                registry.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: true,
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
    );

    let router = axum::Router::new().nest_service("/mcp", service);

    let bind_addr = format!("{}:{}", host, port);
    let tcp_listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind to {}", bind_addr))?;

    info!("MCP server listening on http://{}/mcp", bind_addr);

    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Received shutdown signal");
            ct.cancel();
        })
        .await
        .context("HTTP server error")?;

    Ok(())
}
