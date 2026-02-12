use crate::types::{Result, ToolDefinition};
use crate::MontyGateError;
use dashmap::DashMap;
use regex::Regex;
use std::sync::Arc;
use tracing::{info, warn};

/// Unique identifier for a tool in the registry
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolId {
    pub server: String,
    pub tool: String,
}

impl ToolId {
    pub fn new(server: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            tool: tool.into(),
        }
    }

    pub fn namespaced(&self) -> String {
        format!("{}.{}", self.server, self.tool)
    }
}

/// Route information for dispatching a tool call
#[derive(Debug, Clone)]
pub struct ToolRoute {
    pub server_name: String,
    pub tool_name: String,
    pub definition: ToolDefinition,
}

/// Central registry for managing tools from downstream MCP servers
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    /// Maps namespaced tool names to their routes
    routes: Arc<DashMap<String, ToolRoute>>,
    /// Maps server names to their available tools
    server_tools: Arc<DashMap<String, Vec<String>>>,
    /// Generated type stubs for each server
    stubs: Arc<DashMap<String, String>>,
    /// Pattern for validating tool names
    name_pattern: Regex,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(DashMap::new()),
            server_tools: Arc::new(DashMap::new()),
            stubs: Arc::new(DashMap::new()),
            name_pattern: Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_]*$").unwrap(),
        }
    }

    /// Register tools from a downstream server
    pub fn register_server_tools(
        &self,
        server_name: &str,
        tools: Vec<ToolDefinition>,
    ) -> Result<()> {
        info!(
            "Registering {} tools from server '{}'",
            tools.len(),
            server_name
        );

        let mut tool_names = Vec::with_capacity(tools.len());

        for tool in tools {
            // Validate tool name
            if !self.name_pattern.is_match(&tool.name) {
                warn!(
                    "Skipping tool '{}' from server '{}': invalid name",
                    tool.name, server_name
                );
                continue;
            }

            let namespaced_name = format!("{}.{}", server_name, tool.name);
            tool_names.push(tool.name.clone());

            let route = ToolRoute {
                server_name: server_name.to_string(),
                tool_name: tool.name.clone(),
                definition: tool,
            };

            self.routes.insert(namespaced_name, route);
        }

        self.server_tools
            .insert(server_name.to_string(), tool_names);

        // Regenerate stubs for this server
        self.generate_stubs_for_server(server_name)?;

        Ok(())
    }

    /// Resolve a namespaced tool name to its route
    pub fn resolve(&self, tool_name: &str) -> Result<ToolRoute> {
        self.routes
            .get(tool_name)
            .map(|entry| entry.clone())
            .ok_or_else(|| MontyGateError::ToolNotFound(tool_name.to_string()))
    }

    /// Check if a tool exists in the registry
    pub fn has_tool(&self, tool_name: &str) -> bool {
        self.routes.contains_key(tool_name)
    }

    /// Get all registered tool names
    pub fn list_tools(&self) -> Vec<String> {
        self.routes
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get tools for a specific server
    pub fn list_server_tools(&self, server_name: &str) -> Vec<String> {
        self.server_tools
            .get(server_name)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Get all registered servers
    pub fn list_servers(&self) -> Vec<String> {
        self.server_tools
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Search tools by query string (matches name and description)
    pub fn search_tools(&self, query: &str) -> Vec<(String, ToolDefinition)> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for entry in self.routes.iter() {
            let route = entry.value();
            let matches_name = route.tool_name.to_lowercase().contains(&query_lower);
            let matches_desc = route
                .definition
                .description
                .as_ref()
                .map(|d| d.to_lowercase().contains(&query_lower))
                .unwrap_or(false);

            if matches_name || matches_desc {
                results.push((entry.key().clone(), route.definition.clone()));
            }
        }

        results
    }

    /// Get the type stubs for a server
    pub fn get_stubs(&self, server_name: &str) -> Option<String> {
        self.stubs.get(server_name).map(|entry| entry.clone())
    }

    /// Get all stubs combined
    pub fn get_all_stubs(&self) -> String {
        let mut all_stubs = String::new();
        all_stubs.push_str("# Auto-generated Monty stubs\n\n");
        all_stubs.push_str("from typing import Any\n\n");
        all_stubs.push_str("def tool(name: str, **kwargs: Any) -> Any:\n");
        all_stubs.push_str("    \"\"\"Call a tool by name.\"\"\"\n");
        all_stubs.push_str("    ...\n\n");
        all_stubs.push_str("# Available tools:\n\n");

        for entry in self.stubs.iter() {
            all_stubs.push_str(&format!("# === {} ===\n", entry.key()));
            all_stubs.push_str(entry.value());
            all_stubs.push('\n');
        }

        all_stubs
    }

    /// Generate type stubs for Monty from tool definitions
    fn generate_stubs_for_server(&self, server_name: &str) -> Result<()> {
        let tools = self.list_server_tools(server_name);
        let mut stubs = String::new();

        stubs.push_str(&format!("# Server: {}\n\n", server_name));

        for tool_name in tools {
            let namespaced = format!("{}.{}", server_name, tool_name);
            if let Some(entry) = self.routes.get(&namespaced) {
                let stub = self.generate_tool_stub(&entry.definition);
                stubs.push_str(&stub);
                stubs.push('\n');
            }
        }

        self.stubs.insert(server_name.to_string(), stubs);
        Ok(())
    }

    /// Generate a single tool stub
    fn generate_tool_stub(&self, tool: &ToolDefinition) -> String {
        let mut stub = String::new();

        // Add docstring
        if let Some(desc) = &tool.description {
            stub.push_str(&format!("# {}\n", desc));
        }

        // Parse JSON schema to generate signature
        if let Some(schema) = tool.input_schema.as_object() {
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                let required: Vec<String> = schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                stub.push_str(&format!("# Signature: {}.{}(", tool.name, tool.name));

                for (i, (prop_name, prop_schema)) in props.iter().enumerate() {
                    if i > 0 {
                        stub.push_str(", ");
                    }

                    let is_required = required.contains(prop_name);
                    let type_str = schema_to_python_type(prop_schema);

                    if is_required {
                        stub.push_str(&format!("{}: {}", prop_name, type_str));
                    } else {
                        stub.push_str(&format!("{}: {} = ...", prop_name, type_str));
                    }
                }

                stub.push_str(") -> Any\n");
            }
        }

        stub
    }

    /// Unregister all tools from a server
    pub fn unregister_server(&self, server_name: &str) {
        info!("Unregistering server '{}'", server_name);

        // Remove all tools from this server
        let tools = self.list_server_tools(server_name);
        for tool_name in tools {
            let namespaced = format!("{}.{}", server_name, tool_name);
            self.routes.remove(&namespaced);
        }

        self.server_tools.remove(server_name);
        self.stubs.remove(server_name);
    }

    /// Clear all registrations
    pub fn clear(&self) {
        self.routes.clear();
        self.server_tools.clear();
        self.stubs.clear();
    }

    /// Get the number of registered tools
    pub fn tool_count(&self) -> usize {
        self.routes.len()
    }

    /// Get the number of registered servers
    pub fn server_count(&self) -> usize {
        self.server_tools.len()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert JSON schema type to Python type annotation
fn schema_to_python_type(schema: &serde_json::Value) -> String {
    let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");

    match type_str {
        "string" => "str".to_string(),
        "integer" => "int".to_string(),
        "number" => "float".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            if let Some(items) = schema.get("items") {
                format!("list[{}]", schema_to_python_type(items))
            } else {
                "list[Any]".to_string()
            }
        }
        "object" => {
            if let Some(_props) = schema.get("properties") {
                // Check if it's a dict with additional properties
                if schema.get("additionalProperties").is_some() {
                    "dict[str, Any]".to_string()
                } else {
                    "dict".to_string()
                }
            } else {
                "dict".to_string()
            }
        }
        "null" => "None".to_string(),
        _ => "Any".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: Some(description.to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string"},
                    "title": {"type": "string"}
                },
                "required": ["repo", "title"]
            }),
        }
    }

    // === ToolId ===

    #[test]
    fn test_tool_id_new() {
        let id = ToolId::new("github", "create_issue");
        assert_eq!(id.server, "github");
        assert_eq!(id.tool, "create_issue");
    }

    #[test]
    fn test_tool_id_namespaced() {
        let id = ToolId::new("github", "create_issue");
        assert_eq!(id.namespaced(), "github.create_issue");
    }

    #[test]
    fn test_tool_id_equality() {
        let id1 = ToolId::new("github", "create_issue");
        let id2 = ToolId::new("github", "create_issue");
        let id3 = ToolId::new("slack", "create_issue");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_tool_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ToolId::new("github", "create_issue"));
        set.insert(ToolId::new("github", "create_issue"));
        assert_eq!(set.len(), 1);
    }

    // === ToolRegistry ===

    #[test]
    fn test_tool_registry_new() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.tool_count(), 0);
        assert_eq!(registry.server_count(), 0);
    }

    #[test]
    fn test_tool_registry_default() {
        let registry = ToolRegistry::default();
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_tool_registry_registration() {
        let registry = ToolRegistry::new();

        let tools = vec![
            create_test_tool("create_issue", "Create a GitHub issue"),
            create_test_tool("list_issues", "List GitHub issues"),
        ];

        registry.register_server_tools("github", tools).unwrap();

        assert_eq!(registry.tool_count(), 2);
        assert_eq!(registry.server_count(), 1);

        let all_tools = registry.list_tools();
        assert!(all_tools.contains(&"github.create_issue".to_string()));
        assert!(all_tools.contains(&"github.list_issues".to_string()));
    }

    #[test]
    fn test_tool_resolution() {
        let registry = ToolRegistry::new();
        let tools = vec![create_test_tool("create_issue", "Create a GitHub issue")];

        registry.register_server_tools("github", tools).unwrap();

        let route = registry.resolve("github.create_issue").unwrap();
        assert_eq!(route.server_name, "github");
        assert_eq!(route.tool_name, "create_issue");
    }

    #[test]
    fn test_tool_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.resolve("nonexistent.tool");
        assert!(matches!(result, Err(MontyGateError::ToolNotFound(_))));
    }

    #[test]
    fn test_has_tool() {
        let registry = ToolRegistry::new();
        let tools = vec![create_test_tool("echo", "Echo tool")];
        registry.register_server_tools("test", tools).unwrap();

        assert!(registry.has_tool("test.echo"));
        assert!(!registry.has_tool("test.nope"));
        assert!(!registry.has_tool("other.echo"));
    }

    #[test]
    fn test_list_server_tools() {
        let registry = ToolRegistry::new();
        let tools = vec![
            create_test_tool("tool_a", "A"),
            create_test_tool("tool_b", "B"),
        ];
        registry.register_server_tools("myserver", tools).unwrap();

        let server_tools = registry.list_server_tools("myserver");
        assert_eq!(server_tools.len(), 2);
        assert!(server_tools.contains(&"tool_a".to_string()));
        assert!(server_tools.contains(&"tool_b".to_string()));

        let empty = registry.list_server_tools("nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_list_servers() {
        let registry = ToolRegistry::new();
        let tools = vec![create_test_tool("t", "T")];
        registry
            .register_server_tools("server_a", tools.clone())
            .unwrap();
        registry.register_server_tools("server_b", tools).unwrap();

        let servers = registry.list_servers();
        assert_eq!(servers.len(), 2);
        assert!(servers.contains(&"server_a".to_string()));
        assert!(servers.contains(&"server_b".to_string()));
    }

    #[test]
    fn test_search_tools() {
        let registry = ToolRegistry::new();
        let tools = vec![
            create_test_tool("create_issue", "Create a GitHub issue"),
            create_test_tool("list_issues", "List GitHub issues"),
        ];

        registry.register_server_tools("github", tools).unwrap();

        let results = registry.search_tools("create");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "github.create_issue");

        let results = registry.search_tools("issue");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_tools_by_description() {
        let registry = ToolRegistry::new();
        let tools = vec![
            create_test_tool("query", "Run a database query"),
            create_test_tool("insert", "Insert a record"),
        ];
        registry.register_server_tools("db", tools).unwrap();

        let results = registry.search_tools("database");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.name, "query");
    }

    #[test]
    fn test_search_tools_case_insensitive() {
        let registry = ToolRegistry::new();
        let tools = vec![create_test_tool("CreateUser", "Creates a NEW user")];
        registry.register_server_tools("auth", tools).unwrap();

        let results = registry.search_tools("createuser");
        assert_eq!(results.len(), 1);

        let results = registry.search_tools("NEW");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_tools_no_description() {
        let registry = ToolRegistry::new();
        let tools = vec![ToolDefinition {
            name: "special_tool".to_string(),
            description: None,
            input_schema: serde_json::json!({}),
        }];
        registry.register_server_tools("s", tools).unwrap();

        let results = registry.search_tools("special");
        assert_eq!(results.len(), 1);

        let results = registry.search_tools("nonexistent_desc");
        assert!(results.is_empty());
    }

    #[test]
    fn test_get_stubs() {
        let registry = ToolRegistry::new();
        assert!(registry.get_stubs("nope").is_none());

        let tools = vec![create_test_tool("echo", "Echo back")];
        registry.register_server_tools("test", tools).unwrap();

        let stubs = registry.get_stubs("test");
        assert!(stubs.is_some());
        assert!(stubs.unwrap().contains("Server: test"));
    }

    #[test]
    fn test_get_all_stubs() {
        let registry = ToolRegistry::new();
        let tools1 = vec![create_test_tool("tool_a", "Tool A")];
        let tools2 = vec![create_test_tool("tool_b", "Tool B")];
        registry.register_server_tools("srv1", tools1).unwrap();
        registry.register_server_tools("srv2", tools2).unwrap();

        let all = registry.get_all_stubs();
        assert!(all.contains("Auto-generated Monty stubs"));
        assert!(all.contains("def tool"));
        assert!(all.contains("=== srv1 ==="));
        assert!(all.contains("=== srv2 ==="));
    }

    #[test]
    fn test_get_all_stubs_empty() {
        let registry = ToolRegistry::new();
        let all = registry.get_all_stubs();
        assert!(all.contains("Auto-generated Monty stubs"));
        assert!(all.contains("Available tools"));
    }

    #[test]
    fn test_stub_generation() {
        let registry = ToolRegistry::new();
        let tools = vec![ToolDefinition {
            name: "test_tool".to_string(),
            description: Some("A test tool".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "str_arg": {"type": "string"},
                    "int_arg": {"type": "integer"},
                    "bool_arg": {"type": "boolean"},
                    "list_arg": {"type": "array", "items": {"type": "string"}},
                    "opt_arg": {"type": "string"}
                },
                "required": ["str_arg", "int_arg"]
            }),
        }];

        registry.register_server_tools("test", tools).unwrap();

        let stubs = registry.get_stubs("test").unwrap();
        assert!(stubs.contains("str_arg: str"));
        assert!(stubs.contains("int_arg: int"));
        assert!(stubs.contains("bool_arg: bool"));
        assert!(stubs.contains("list_arg: list[str]"));
    }

    #[test]
    fn test_stub_generation_no_description() {
        let registry = ToolRegistry::new();
        let tools = vec![ToolDefinition {
            name: "no_desc".to_string(),
            description: None,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "arg": {"type": "string"}
                }
            }),
        }];
        registry.register_server_tools("test", tools).unwrap();
        let stubs = registry.get_stubs("test").unwrap();
        // Should not contain a description comment line before signature
        assert!(stubs.contains("Signature:"));
    }

    #[test]
    fn test_stub_generation_no_schema() {
        let registry = ToolRegistry::new();
        let tools = vec![ToolDefinition {
            name: "bare_tool".to_string(),
            description: Some("Bare tool".to_string()),
            input_schema: serde_json::json!({}),
        }];
        registry.register_server_tools("test", tools).unwrap();
        let stubs = registry.get_stubs("test").unwrap();
        assert!(stubs.contains("Bare tool"));
    }

    #[test]
    fn test_server_unregister() {
        let registry = ToolRegistry::new();
        let tools = vec![create_test_tool("tool1", "Tool 1")];

        registry
            .register_server_tools("server1", tools.clone())
            .unwrap();
        registry.register_server_tools("server2", tools).unwrap();

        assert_eq!(registry.tool_count(), 2);

        registry.unregister_server("server1");

        assert_eq!(registry.tool_count(), 1);
        assert!(!registry.has_tool("server1.tool1"));
        assert!(registry.has_tool("server2.tool1"));
    }

    #[test]
    fn test_unregister_nonexistent_server() {
        let registry = ToolRegistry::new();
        registry.unregister_server("ghost");
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_clear() {
        let registry = ToolRegistry::new();
        let tools = vec![create_test_tool("t", "T")];
        registry
            .register_server_tools("s1", tools.clone())
            .unwrap();
        registry.register_server_tools("s2", tools).unwrap();

        assert_eq!(registry.tool_count(), 2);
        assert_eq!(registry.server_count(), 2);

        registry.clear();

        assert_eq!(registry.tool_count(), 0);
        assert_eq!(registry.server_count(), 0);
        assert!(registry.get_stubs("s1").is_none());
    }

    #[test]
    fn test_invalid_tool_name() {
        let registry = ToolRegistry::new();
        let tools = vec![ToolDefinition {
            name: "invalid-tool-name!".to_string(),
            description: Some("Invalid".to_string()),
            input_schema: serde_json::json!({}),
        }];

        registry.register_server_tools("test", tools).unwrap();
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_mixed_valid_invalid_tools() {
        let registry = ToolRegistry::new();
        let tools = vec![
            ToolDefinition {
                name: "valid_tool".to_string(),
                description: None,
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "invalid-name!".to_string(),
                description: None,
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "another_valid".to_string(),
                description: None,
                input_schema: serde_json::json!({}),
            },
        ];
        registry.register_server_tools("test", tools).unwrap();
        assert_eq!(registry.tool_count(), 2);
    }

    #[test]
    fn test_register_multiple_servers() {
        let registry = ToolRegistry::new();
        registry
            .register_server_tools(
                "github",
                vec![
                    create_test_tool("create_issue", "Create issue"),
                    create_test_tool("list_repos", "List repos"),
                ],
            )
            .unwrap();
        registry
            .register_server_tools(
                "slack",
                vec![create_test_tool("post_message", "Post message")],
            )
            .unwrap();

        assert_eq!(registry.tool_count(), 3);
        assert_eq!(registry.server_count(), 2);
        assert!(registry.has_tool("github.create_issue"));
        assert!(registry.has_tool("slack.post_message"));
    }

    // === schema_to_python_type ===

    #[test]
    fn test_schema_to_python_type() {
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "string"})),
            "str"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "integer"})),
            "int"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "number"})),
            "float"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "boolean"})),
            "bool"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "null"})),
            "None"
        );
        assert_eq!(schema_to_python_type(&serde_json::json!({})), "Any");
    }

    #[test]
    fn test_schema_to_python_type_array() {
        assert_eq!(
            schema_to_python_type(
                &serde_json::json!({"type": "array", "items": {"type": "string"}})
            ),
            "list[str]"
        );
        assert_eq!(
            schema_to_python_type(
                &serde_json::json!({"type": "array", "items": {"type": "integer"}})
            ),
            "list[int]"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "array"})),
            "list[Any]"
        );
    }

    #[test]
    fn test_schema_to_python_type_object() {
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "object"})),
            "dict"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({
                "type": "object",
                "properties": {"x": {"type": "string"}}
            })),
            "dict"
        );
        assert_eq!(
            schema_to_python_type(&serde_json::json!({
                "type": "object",
                "properties": {"x": {"type": "string"}},
                "additionalProperties": true
            })),
            "dict[str, Any]"
        );
    }

    #[test]
    fn test_schema_to_python_type_nested_array() {
        assert_eq!(
            schema_to_python_type(&serde_json::json!({
                "type": "array",
                "items": {"type": "array", "items": {"type": "integer"}}
            })),
            "list[list[int]]"
        );
    }

    #[test]
    fn test_schema_to_python_type_unknown() {
        assert_eq!(
            schema_to_python_type(&serde_json::json!({"type": "custom_type"})),
            "Any"
        );
    }
}
