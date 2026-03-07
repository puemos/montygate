use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A single trace entry for a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub retries: u32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Records execution trace entries for tool calls
#[derive(Debug, Clone)]
pub struct ExecutionTracer {
    entries: Arc<Mutex<Vec<TraceEntry>>>,
}

impl ExecutionTracer {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Record a successful tool call
    pub fn record_success(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        output: serde_json::Value,
        duration_ms: u64,
        retries: u32,
    ) {
        self.entries.lock().push(TraceEntry {
            tool_name: tool_name.to_string(),
            input,
            output: Some(output),
            error: None,
            duration_ms,
            retries,
            timestamp: chrono::Utc::now(),
        });
    }

    /// Record a failed tool call
    pub fn record_error(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        error: String,
        duration_ms: u64,
        retries: u32,
    ) {
        self.entries.lock().push(TraceEntry {
            tool_name: tool_name.to_string(),
            input,
            output: None,
            error: Some(error),
            duration_ms,
            retries,
            timestamp: chrono::Utc::now(),
        });
    }

    /// Record a raw trace entry
    pub fn record(&self, entry: TraceEntry) {
        self.entries.lock().push(entry);
    }

    /// Get all trace entries
    pub fn entries(&self) -> Vec<TraceEntry> {
        self.entries.lock().clone()
    }

    /// Get number of recorded entries
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Check if tracer has no entries
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Clear all trace entries
    pub fn clear(&self) {
        self.entries.lock().clear();
    }
}

impl Default for ExecutionTracer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracer_new() {
        let tracer = ExecutionTracer::new();
        assert!(tracer.is_empty());
        assert_eq!(tracer.len(), 0);
    }

    #[test]
    fn test_tracer_default() {
        let tracer = ExecutionTracer::default();
        assert!(tracer.is_empty());
    }

    #[test]
    fn test_record_success() {
        let tracer = ExecutionTracer::new();
        tracer.record_success(
            "lookup_order",
            serde_json::json!({"order_id": "123"}),
            serde_json::json!({"id": "123", "status": "shipped"}),
            42,
            0,
        );

        assert_eq!(tracer.len(), 1);
        let entries = tracer.entries();
        assert_eq!(entries[0].tool_name, "lookup_order");
        assert_eq!(entries[0].duration_ms, 42);
        assert_eq!(entries[0].retries, 0);
        assert!(entries[0].output.is_some());
        assert!(entries[0].error.is_none());
    }

    #[test]
    fn test_record_error() {
        let tracer = ExecutionTracer::new();
        tracer.record_error(
            "create_ticket",
            serde_json::json!({"subject": "test"}),
            "connection refused".to_string(),
            5000,
            2,
        );

        assert_eq!(tracer.len(), 1);
        let entries = tracer.entries();
        assert_eq!(entries[0].tool_name, "create_ticket");
        assert_eq!(entries[0].duration_ms, 5000);
        assert_eq!(entries[0].retries, 2);
        assert!(entries[0].output.is_none());
        assert_eq!(entries[0].error, Some("connection refused".to_string()));
    }

    #[test]
    fn test_record_raw_entry() {
        let tracer = ExecutionTracer::new();
        let entry = TraceEntry {
            tool_name: "test_tool".to_string(),
            input: serde_json::json!({}),
            output: Some(serde_json::json!("ok")),
            error: None,
            duration_ms: 10,
            retries: 0,
            timestamp: chrono::Utc::now(),
        };
        tracer.record(entry);
        assert_eq!(tracer.len(), 1);
    }

    #[test]
    fn test_multiple_entries() {
        let tracer = ExecutionTracer::new();
        tracer.record_success("tool_a", serde_json::json!({}), serde_json::json!(1), 10, 0);
        tracer.record_success("tool_b", serde_json::json!({}), serde_json::json!(2), 20, 0);
        tracer.record_error("tool_c", serde_json::json!({}), "fail".into(), 30, 1);

        assert_eq!(tracer.len(), 3);
        assert!(!tracer.is_empty());

        let entries = tracer.entries();
        assert_eq!(entries[0].tool_name, "tool_a");
        assert_eq!(entries[1].tool_name, "tool_b");
        assert_eq!(entries[2].tool_name, "tool_c");
    }

    #[test]
    fn test_clear() {
        let tracer = ExecutionTracer::new();
        tracer.record_success("tool", serde_json::json!({}), serde_json::json!(1), 10, 0);
        assert_eq!(tracer.len(), 1);

        tracer.clear();
        assert!(tracer.is_empty());
        assert_eq!(tracer.len(), 0);
    }

    #[test]
    fn test_clone_independence() {
        let tracer = ExecutionTracer::new();
        let tracer2 = tracer.clone();

        tracer.record_success("tool", serde_json::json!({}), serde_json::json!(1), 10, 0);

        // Clone shares the Arc, so both see the entry
        assert_eq!(tracer2.len(), 1);
    }

    #[test]
    fn test_trace_entry_serialization() {
        let entry = TraceEntry {
            tool_name: "test_tool".to_string(),
            input: serde_json::json!({"key": "value"}),
            output: Some(serde_json::json!({"result": 42})),
            error: None,
            duration_ms: 100,
            retries: 1,
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: TraceEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tool_name, "test_tool");
        assert_eq!(deserialized.duration_ms, 100);
        assert_eq!(deserialized.retries, 1);
    }
}
