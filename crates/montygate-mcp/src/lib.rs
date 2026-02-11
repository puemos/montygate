//! MontyGate MCP Protocol Handling
//!
//! This crate provides MCP protocol handling for MontyGate:
//! - Upstream MCP server (what clients connect to)
//! - Downstream MCP client pool
//! - Transport implementations (stdio, SSE, HTTP)

pub mod client_pool;
pub mod mcp_server;
pub mod server;

// Re-export commonly used types
pub use client_pool::ClientPool;
pub use mcp_server::MontyGateMcpServer;
pub use server::{McpServerBuilder, McpTransport, MontyGateServerHandler, RmcpClientPool};