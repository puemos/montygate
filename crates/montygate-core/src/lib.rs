//! Montygate Core Library
//!
//! This crate provides the core functionality for Montygate:
//! - Tool registry for managing downstream MCP servers
//! - Policy engine for access control
//! - Execution engine for running Monty programs
//! - Bridge for connecting Monty to MCP tool calls

pub mod bridge;
pub mod convert;
pub mod engine;
pub mod policy;
pub mod registry;
pub mod types;

// Re-export commonly used types
pub use bridge::{ApprovalHandler, AutoApproveHandler, Bridge, BridgeBuilder, McpClientPool};
pub use engine::{EngineManager, ExecutionEngine, MockEngine, MontyEngine, SimpleDispatcher, ToolDispatcher};
pub use policy::{PolicyDecision, PolicyEngine};
pub use registry::{ToolId, ToolRegistry, ToolRoute};
pub use types::{
    ExecutionResult, ExecutionSnapshot, ExecutionState, ExecutionStats, ExternalCall,
    MontygateConfig, MontygateError, PolicyAction, PolicyConfig, PolicyRule, ResourceLimits,
    Result, RunProgramInput, ServerConfig, ToolCall, ToolCallId, ToolDefinition, TransportConfig,
};

/// Version of the montygate-core crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");