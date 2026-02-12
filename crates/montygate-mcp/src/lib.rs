//! Montygate MCP Protocol Handling
//!
//! This crate provides MCP protocol handling for Montygate:
//! - Upstream MCP server (what clients connect to)
//! - Downstream MCP client pool
//! - Transport implementations (stdio, SSE, HTTP)

pub mod client_pool;
pub mod mcp_server;
pub mod server;

// Re-export commonly used types
pub use client_pool::{ClientPool, ClientPoolConfig};
pub use mcp_server::MontygateMcpServer;
pub use server::{McpServerBuilder, McpTransport, MontygateServerHandler};