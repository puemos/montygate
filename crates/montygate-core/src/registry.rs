use crate::types::{Result, ToolDefinition};
use crate::MontygateError;
use dashmap::DashMap;
use regex::Regex;
use std::sync::Arc;
use tracing::{info, warn};

/// Central registry for managing tool definitions
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: Arc<DashMap<String, ToolDefinition>>,
    name_pattern: Regex,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(DashMap::new()),
            name_pattern: Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_]*$").unwrap(),
        }
    }

    /// Register a single tool
    pub fn register_tool(&self, definition: ToolDefinition) -> Result<()> {
        if !self.name_pattern.is_match(&definition.name) {
            warn!("Skipping tool '{}': invalid name", definition.name);
            return Err(MontygateError::Validation(format!(
                "Invalid tool name: '{}'. Must match [a-zA-Z_][a-zA-Z0-9_]*",
                definition.name
            )));
        }

        info!("Registering tool '{}'", definition.name);
        self.tools.insert(definition.name.clone(), definition);
        Ok(())
    }

    /// Register multiple tools at once
    pub fn register_tools(&self, tools: Vec<ToolDefinition>) -> Result<()> {
        for tool in tools {
            self.register_tool(tool)?;
        }
        Ok(())
    }

    /// Get a tool definition by name
    pub fn get_tool(&self, name: &str) -> Result<ToolDefinition> {
        self.tools
            .get(name)
            .map(|entry| entry.clone())
            .ok_or_else(|| MontygateError::ToolNotFound(name.to_string()))
    }

    /// Check if a tool exists
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get all registered tool names
    pub fn list_tools(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get all tool definitions
    pub fn all_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Search tools by query string (substring match on name and description)
    pub fn search_tools(&self, query: &str, top_k: usize) -> Vec<ToolDefinition> {
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

        if query_terms.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(ToolDefinition, f64)> = self
            .tools
            .iter()
            .filter_map(|entry| {
                let def = entry.value();
                let text = build_search_text(def);
                let text_lower = text.to_lowercase();
                let score = compute_score(&query_terms, &query_lower, &text_lower);
                if score > 0.0 {
                    Some((def.clone(), score))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored.into_iter().map(|(def, _)| def).collect()
    }

    /// Build a tool catalog string for inclusion in execute tool description
    pub fn tool_catalog(&self) -> String {
        let mut catalog = String::new();
        let mut tools: Vec<ToolDefinition> = self.all_tools();
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        for tool in tools {
            catalog.push_str(&format!("- {}(", tool.name));
            if let Some(props) = tool
                .input_schema
                .as_object()
                .and_then(|o| o.get("properties"))
                .and_then(|p| p.as_object())
            {
                let required: Vec<String> = tool
                    .input_schema
                    .as_object()
                    .and_then(|o| o.get("required"))
                    .and_then(|r| r.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let params: Vec<String> = props
                    .iter()
                    .map(|(name, schema)| {
                        let type_str = schema
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("any");
                        if required.contains(name) {
                            format!("{}: {}", name, type_str)
                        } else {
                            format!("{}?: {}", name, type_str)
                        }
                    })
                    .collect();
                catalog.push_str(&params.join(", "));
            }
            catalog.push(')');
            if let Some(desc) = &tool.description {
                catalog.push_str(&format!(" - {}", desc));
            }
            catalog.push('\n');
        }
        catalog
    }

    /// Unregister a tool
    pub fn unregister(&self, name: &str) {
        self.tools.remove(name);
    }

    /// Clear all registrations
    pub fn clear(&self) {
        self.tools.clear();
    }

    /// Get the number of registered tools
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn build_search_text(def: &ToolDefinition) -> String {
    match &def.description {
        Some(desc) => format!("{}: {}", def.name, desc),
        None => def.name.clone(),
    }
}

fn compute_score(query_terms: &[&str], full_query: &str, text: &str) -> f64 {
    if query_terms.is_empty() {
        return 0.0;
    }

    let matching_terms = query_terms
        .iter()
        .filter(|term| text.contains(**term))
        .count();

    if matching_terms == 0 {
        return 0.0;
    }

    let term_score = matching_terms as f64 / query_terms.len() as f64;
    let phrase_bonus = if text.contains(full_query) { 0.2 } else { 0.0 };

    (term_score * 0.8 + phrase_bonus).min(1.0)
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

    // === ToolRegistry ===

    #[test]
    fn test_registry_new() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_registry_default() {
        let registry = ToolRegistry::default();
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_register_tool() {
        let registry = ToolRegistry::new();
        let tool = create_test_tool("create_issue", "Create a GitHub issue");
        registry.register_tool(tool).unwrap();
        assert_eq!(registry.tool_count(), 1);
        assert!(registry.has_tool("create_issue"));
    }

    #[test]
    fn test_register_tools() {
        let registry = ToolRegistry::new();
        let tools = vec![
            create_test_tool("create_issue", "Create a GitHub issue"),
            create_test_tool("list_issues", "List GitHub issues"),
        ];
        registry.register_tools(tools).unwrap();
        assert_eq!(registry.tool_count(), 2);
    }

    #[test]
    fn test_get_tool() {
        let registry = ToolRegistry::new();
        let tool = create_test_tool("create_issue", "Create a GitHub issue");
        registry.register_tool(tool).unwrap();

        let found = registry.get_tool("create_issue").unwrap();
        assert_eq!(found.name, "create_issue");
        assert_eq!(found.description, Some("Create a GitHub issue".to_string()));
    }

    #[test]
    fn test_get_tool_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.get_tool("nonexistent");
        assert!(matches!(result, Err(MontygateError::ToolNotFound(_))));
    }

    #[test]
    fn test_has_tool() {
        let registry = ToolRegistry::new();
        let tool = create_test_tool("echo", "Echo tool");
        registry.register_tool(tool).unwrap();

        assert!(registry.has_tool("echo"));
        assert!(!registry.has_tool("nope"));
    }

    #[test]
    fn test_list_tools() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("tool_a", "A"),
                create_test_tool("tool_b", "B"),
            ])
            .unwrap();

        let tools = registry.list_tools();
        assert_eq!(tools.len(), 2);
        assert!(tools.contains(&"tool_a".to_string()));
        assert!(tools.contains(&"tool_b".to_string()));
    }

    #[test]
    fn test_all_tools() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("tool_a", "A"),
                create_test_tool("tool_b", "B"),
            ])
            .unwrap();

        let tools = registry.all_tools();
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn test_search_tools() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("create_issue", "Create a GitHub issue"),
                create_test_tool("list_issues", "List GitHub issues"),
                create_test_tool("post_message", "Post a Slack message"),
            ])
            .unwrap();

        let results = registry.search_tools("create", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "create_issue");

        let results = registry.search_tools("issue", 5);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_tools_by_description() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("query", "Run a database query"),
                create_test_tool("insert", "Insert a record"),
            ])
            .unwrap();

        let results = registry.search_tools("database", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "query");
    }

    #[test]
    fn test_search_tools_case_insensitive() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("CreateUser", "Creates a NEW user"))
            .unwrap();

        let results = registry.search_tools("createuser", 5);
        assert_eq!(results.len(), 1);

        let results = registry.search_tools("NEW", 5);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_tools_no_match() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("echo", "Echo"))
            .unwrap();

        let results = registry.search_tools("xyznonexistent", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_tools_top_k() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("issue_create", "Create issue"),
                create_test_tool("issue_list", "List issues"),
                create_test_tool("issue_close", "Close issue"),
            ])
            .unwrap();

        let results = registry.search_tools("issue", 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_tools_empty_query() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("echo", "Echo"))
            .unwrap();

        let results = registry.search_tools("", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_tools_no_description() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "special_tool".to_string(),
                description: None,
                input_schema: serde_json::json!({}),
            })
            .unwrap();

        let results = registry.search_tools("special", 5);
        assert_eq!(results.len(), 1);

        let results = registry.search_tools("nonexistent_desc", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_tool_catalog() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("create_issue", "Create a GitHub issue"),
                create_test_tool("list_issues", "List GitHub issues"),
            ])
            .unwrap();

        let catalog = registry.tool_catalog();
        assert!(catalog.contains("create_issue("));
        assert!(catalog.contains("list_issues("));
        assert!(catalog.contains("Create a GitHub issue"));
    }

    #[test]
    fn test_unregister() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("echo", "Echo"))
            .unwrap();
        assert!(registry.has_tool("echo"));

        registry.unregister("echo");
        assert!(!registry.has_tool("echo"));
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_clear() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("a", "A"),
                create_test_tool("b", "B"),
            ])
            .unwrap();
        assert_eq!(registry.tool_count(), 2);

        registry.clear();
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_invalid_tool_name() {
        let registry = ToolRegistry::new();
        let result = registry.register_tool(ToolDefinition {
            name: "invalid-tool-name!".to_string(),
            description: Some("Invalid".to_string()),
            input_schema: serde_json::json!({}),
        });
        assert!(result.is_err());
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_register_overwrites() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("echo", "Old description"))
            .unwrap();
        registry
            .register_tool(create_test_tool("echo", "New description"))
            .unwrap();

        assert_eq!(registry.tool_count(), 1);
        let tool = registry.get_tool("echo").unwrap();
        assert_eq!(tool.description, Some("New description".to_string()));
    }

    // === compute_score ===

    #[test]
    fn test_compute_score_all_terms_match() {
        let terms = vec!["create", "issue"];
        let score = compute_score(
            &terms,
            "create issue",
            "create_issue: create a new issue in a github repository",
        );
        assert!(score > 0.5);
    }

    #[test]
    fn test_compute_score_partial_match() {
        let terms = vec!["create", "database"];
        let score = compute_score(
            &terms,
            "create database",
            "create_issue: create a new issue",
        );
        assert!(score > 0.0);
        assert!(score < 0.8);
    }

    #[test]
    fn test_compute_score_no_match() {
        let terms = vec!["nonexistent"];
        let score = compute_score(&terms, "nonexistent", "create_issue");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_compute_score_empty_terms() {
        let terms: Vec<&str> = vec![];
        let score = compute_score(&terms, "", "some text");
        assert_eq!(score, 0.0);
    }
}
