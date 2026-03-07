//! Montygate Core Library
//!
//! Framework-agnostic tool execution engine. Register tools once,
//! execute Python scripts that orchestrate multiple tool calls,
//! and return only the final result to the LLM.

pub mod convert;
pub mod engine;
pub mod observability;
pub mod policy;
pub mod registry;
pub mod retry;
pub mod scheduler;
pub mod types;

// Re-export commonly used types
pub use engine::{EngineManager, ExecutionEngine, MockEngine, MontyEngine, SimpleDispatcher, ToolDispatcher};
pub use observability::{ExecutionTracer, TraceEntry};
pub use policy::{PolicyDecision, PolicyEngine};
pub use registry::ToolRegistry;
pub use retry::{is_retryable_error, retry_with_backoff};
pub use scheduler::Scheduler;
pub use types::{
    ExecutionLimits, ExecutionResult, ExecutionSnapshot, ExecutionState, ExecutionStats,
    ExternalCall, MontygateError, PolicyAction, PolicyConfig, PolicyDefaults, PolicyRule,
    ResourceLimits, Result, RetryConfig, RunProgramInput, ToolCall, ToolCallId, ToolDefinition,
};

/// Version of the montygate-core crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
