use crate::MontygateError;
use crate::types::{Result, ToolDefinition};
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
            name_pattern: Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_-]*$").unwrap(),
        }
    }

    /// Register a single tool
    pub fn register_tool(&self, definition: ToolDefinition) -> Result<()> {
        if !self.name_pattern.is_match(&definition.name) {
            warn!("Skipping tool '{}': invalid name", definition.name);
            return Err(MontygateError::Validation(format!(
                "Invalid tool name: '{}'. Must match [a-zA-Z_][a-zA-Z0-9_-]*",
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
        self.tools.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Get all tool definitions
    pub fn all_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Search tools by query string using tool names, descriptions, and schemas.
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

    /// Suggest likely tool names for misspelled lookups.
    pub fn suggest_tools(&self, query: &str, top_k: usize) -> Vec<String> {
        let normalized_query = normalize_identifier(query);
        if normalized_query.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(String, f64)> = self
            .tools
            .iter()
            .filter_map(|entry| {
                let name = entry.key().clone();
                let score = compute_suggestion_score(&normalized_query, &name);
                (score >= 0.55).then_some((name, score))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(top_k);
        scored.into_iter().map(|(name, _)| name).collect()
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
                    .map(|(name, schema)| format_parameter(name, schema, required.contains(name)))
                    .collect();
                catalog.push_str(&params.join(", "));
            }
            catalog.push(')');
            if let Some(desc) = &tool.description {
                catalog.push_str(&format!(" - {}", desc));
            }
            if let Some(output_schema) = &tool.output_schema {
                catalog.push_str(&format!(" -> {}", format_output_schema(output_schema)));
            }
            catalog.push('\n');
        }
        catalog
    }

    /// Build the canonical description for the "execute" tool exposed to LLMs.
    /// This ensures all SDKs (Node, Python, etc.) present the same instructions.
    pub fn execute_tool_description(&self) -> String {
        let catalog = self.tool_catalog();
        format!(
            "Execute a Python script with access to these tools:\n\
             {catalog}\n\
             IMPORTANT: Each execute() runs in a FRESH sandbox — variables do NOT persist between calls.\n\
             Do ALL related work (lookups, transformations, actions) in a SINGLE script.\n\
             \n\
             Call tools with: tool('name', key=value)\n\
             The LAST EXPRESSION is the return value. Do NOT use print() — it returns None.\n\
             \n\
             Parallel dispatch: use batch_tools() only when calls are INDEPENDENT (no data flow between them).\n\
             results = batch_tools([('tool_a', {{'x': 1}}), ('tool_b', {{'y': 2}})])\n\
             Do NOT use batch_tools() when tool B needs output from tool A — use sequential tool() calls instead.\n\
             \n\
             Runtime restrictions:\n\
             - No standard library (no json, re, math, datetime, collections, itertools, etc.) — use builtins and plain dicts/lists\n\
             - No class definitions\n\
             - sorted() and .sort() do not support key= or reverse= — sort flat lists only\n\
             - No chained subscript assignment (x[a][b] = val) — use: inner = x[a]; inner[b] = val; x[a] = inner\n\
             \n\
             Example — all work in ONE script (variables are lost between calls):\n\
             order = tool('lookup_order', order_id='123')\n\
             ticket = tool('create_ticket', subject='Issue ' + order['id'])\n\
             {{'order': order, 'ticket': ticket}}"
        )
    }

    /// Build the canonical description for the "search" tool exposed to LLMs.
    pub fn search_tool_description(&self) -> String {
        "Search for available tools by keyword".to_string()
    }

    /// Return the canonical system prompt that guides LLMs toward efficient
    /// single-script usage.  All SDKs (Node, Python, …) should use this so the
    /// instructions stay in sync.
    pub fn system_prompt(&self) -> String {
        "You have access to an `execute` tool that runs a Python script in a sandboxed environment.\n\
         CRITICAL: Each execute() call runs in a FRESH sandbox. Variables do NOT persist between calls.\n\
         Always do ALL related work — lookups, transformations, and actions — in a SINGLE script.\n\
         Use `batch_tools()` only for INDEPENDENT calls that do not rely on each other's outputs.\n\
         The LAST EXPRESSION in the script is the return value. Do NOT use print().\n\
         When a tool returns a dict, check the tool catalog for exact field names (e.g. `ticket_id` not `id`).\n\
         GOOD: order = tool('lookup_order', order_id='123'); ticket = tool('create_ticket', subject='Late ' + order['id']); {'order': order, 'ticket': ticket}\n\
         BAD: first call execute() with order = tool(...), then later call execute() with ticket = tool(... order['id'] ...).\n\
         The BAD pattern fails with NameError because the second execute() cannot see variables from the first."
            .to_string()
    }

    /// Return the canonical JSON Schema for the `execute` tool's input parameters.
    /// All SDK adapters should use this instead of hard-coding the schema.
    pub fn execute_tool_input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python script to execute"
                },
                "inputs": {
                    "type": "object",
                    "description": "Variables to inject into the script"
                }
            },
            "required": ["code"]
        })
    }

    /// Return the canonical JSON Schema for the `search` tool's input parameters.
    pub fn search_tool_input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "top_k": {
                    "type": "number",
                    "description": "Maximum number of results"
                }
            },
            "required": ["query"]
        })
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

fn format_parameter(name: &str, schema: &serde_json::Value, required: bool) -> String {
    let mut rendered = if required {
        format!("{}: {}", name, format_schema_type(schema))
    } else {
        format!("{}?: {}", name, format_schema_type(schema))
    };

    if let Some(annotation) = format_schema_annotation(schema) {
        rendered.push_str(&format!(" ({annotation})"));
    }

    rendered
}

/// Format a JSON Schema output_schema as a compact `{key: type, ...}` string for the catalog.
fn format_output_schema(schema: &serde_json::Value) -> String {
    format_schema_type(schema)
}

fn format_schema_type(schema: &serde_json::Value) -> String {
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("array") => {
            if let Some(items) = schema.get("items") {
                format!("array<{}>", format_schema_type(items))
            } else {
                "array".to_string()
            }
        }
        Some("object") => {
            if let Some(props) = schema
                .as_object()
                .and_then(|o| o.get("properties"))
                .and_then(|p| p.as_object())
            {
                let fields: Vec<String> = props
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, format_schema_type(v)))
                    .collect();
                format!("{{{}}}", fields.join(", "))
            } else {
                "object".to_string()
            }
        }
        Some(other) => other.to_string(),
        None => "any".to_string(),
    }
}

fn format_schema_annotation(schema: &serde_json::Value) -> Option<String> {
    if let Some(values) = schema.get("enum").and_then(|v| v.as_array()) {
        let rendered: Vec<String> = values
            .iter()
            .filter_map(|v| v.as_str().map(|s| format!("\"{}\"", s)))
            .collect();
        if !rendered.is_empty() {
            return Some(rendered.join("|"));
        }
    }

    schema
        .get("description")
        .and_then(|d| d.as_str())
        .map(str::to_string)
}

fn build_search_text(def: &ToolDefinition) -> String {
    let mut parts = vec![def.name.clone(), def.name.replace('_', " ")];

    if let Some(desc) = &def.description {
        parts.push(desc.clone());
    }

    collect_schema_terms(&def.input_schema, &mut parts);
    if let Some(output_schema) = &def.output_schema {
        collect_schema_terms(output_schema, &mut parts);
    }

    parts.join(" ")
}

fn collect_schema_terms(schema: &serde_json::Value, parts: &mut Vec<String>) {
    if let Some(obj) = schema.as_object() {
        if let Some(description) = obj.get("description").and_then(|d| d.as_str()) {
            parts.push(description.to_string());
        }

        if let Some(values) = obj.get("enum").and_then(|v| v.as_array()) {
            for value in values {
                if let Some(s) = value.as_str() {
                    parts.push(s.to_string());
                }
            }
        }

        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
            for (name, child) in props {
                parts.push(name.clone());
                parts.push(name.replace('_', " "));
                collect_schema_terms(child, parts);
            }
        }

        if let Some(items) = obj.get("items") {
            collect_schema_terms(items, parts);
        }

        if let Some(additional) = obj.get("additionalProperties") {
            collect_schema_terms(additional, parts);
        }
    }
}

fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn compute_suggestion_score(normalized_query: &str, candidate: &str) -> f64 {
    let normalized_candidate = normalize_identifier(candidate);
    if normalized_candidate.is_empty() {
        return 0.0;
    }

    if normalized_candidate == normalized_query {
        return 1.0;
    }

    let max_len = normalized_query.len().max(normalized_candidate.len()) as f64;
    let distance = levenshtein(normalized_query, &normalized_candidate) as f64;
    let similarity = (1.0 - distance / max_len).max(0.0);
    let contains_bonus = if normalized_candidate.contains(normalized_query)
        || normalized_query.contains(&normalized_candidate)
    {
        0.15
    } else {
        0.0
    };
    let prefix_bonus = if normalized_candidate.starts_with(normalized_query)
        || normalized_query.starts_with(&normalized_candidate)
    {
        0.1
    } else {
        0.0
    };

    (similarity + contains_bonus + prefix_bonus).min(1.0)
}

fn levenshtein(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut costs: Vec<usize> = (0..=b_chars.len()).collect();

    for (i, a_char) in a.chars().enumerate() {
        let mut prev = costs[0];
        costs[0] = i + 1;

        for (j, b_char) in b_chars.iter().enumerate() {
            let temp = costs[j + 1];
            let substitution = if a_char == *b_char { prev } else { prev + 1 };
            let insertion = costs[j + 1] + 1;
            let deletion = costs[j] + 1;
            costs[j + 1] = substitution.min(insertion).min(deletion);
            prev = temp;
        }
    }

    *costs.last().unwrap_or(&0)
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
            output_schema: None,
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
    fn test_search_tools_by_schema_fields_and_descriptions() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "create_ticket".to_string(),
                description: Some("Create a customer support ticket".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "priority": {
                            "type": "string",
                            "enum": ["low", "medium", "high"]
                        },
                        "subject": {
                            "type": "string",
                            "description": "Ticket subject line"
                        }
                    }
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ticket_id": {"type": "string"},
                        "status": {"type": "string"}
                    }
                })),
            })
            .unwrap();

        assert_eq!(
            registry.search_tools("ticket_id", 5)[0].name,
            "create_ticket"
        );
        assert_eq!(
            registry.search_tools("subject line", 5)[0].name,
            "create_ticket"
        );
        assert_eq!(registry.search_tools("high", 5)[0].name, "create_ticket");
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
                output_schema: None,
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
    fn test_tool_catalog_renders_param_descriptions_and_enum_values() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "create_ticket".to_string(),
                description: Some("Create a ticket".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "priority": {
                            "type": "string",
                            "enum": ["low", "medium", "high"]
                        },
                        "subject": {
                            "type": "string",
                            "description": "Ticket subject line"
                        }
                    },
                    "required": ["priority", "subject"]
                }),
                output_schema: None,
            })
            .unwrap();

        let catalog = registry.tool_catalog();
        assert!(catalog.contains("priority: string (\"low\"|\"medium\"|\"high\")"));
        assert!(catalog.contains("subject: string (Ticket subject line)"));
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
            .register_tools(vec![create_test_tool("a", "A"), create_test_tool("b", "B")])
            .unwrap();
        assert_eq!(registry.tool_count(), 2);

        registry.clear();
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_invalid_tool_name() {
        let registry = ToolRegistry::new();
        let result = registry.register_tool(ToolDefinition {
            name: "invalid tool name!".to_string(),
            description: Some("Invalid".to_string()),
            input_schema: serde_json::json!({}),
            output_schema: None,
        });
        assert!(result.is_err());
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_hyphenated_tool_name() {
        let registry = ToolRegistry::new();
        let result = registry.register_tool(ToolDefinition {
            name: "get-weather".to_string(),
            description: Some("Get weather".to_string()),
            input_schema: serde_json::json!({}),
            output_schema: None,
        });
        assert!(result.is_ok());
        assert!(registry.has_tool("get-weather"));
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

    #[test]
    fn test_suggest_tools_for_typo() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("create_ticket", "Create a support ticket"),
                create_test_tool("create_task", "Create a task"),
            ])
            .unwrap();

        let suggestions = registry.suggest_tools("create_tiket", 3);
        assert_eq!(suggestions[0], "create_ticket");
        assert!(suggestions.contains(&"create_task".to_string()));
    }

    // === format_output_schema ===

    #[test]
    fn test_format_output_schema_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ticket_id": {"type": "string"},
                "status": {"type": "string"}
            }
        });
        let formatted = format_output_schema(&schema);
        assert!(formatted.starts_with('{'));
        assert!(formatted.ends_with('}'));
        assert!(formatted.contains("ticket_id: string"));
        assert!(formatted.contains("status: string"));
    }

    #[test]
    fn test_format_output_schema_simple_type() {
        let schema = serde_json::json!({"type": "string"});
        assert_eq!(format_output_schema(&schema), "string");
    }

    #[test]
    fn test_format_output_schema_no_type() {
        let schema = serde_json::json!({});
        assert_eq!(format_output_schema(&schema), "any");
    }

    #[test]
    fn test_catalog_with_output_schema() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "create_ticket".to_string(),
                description: Some("Create a ticket".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"subject": {"type": "string"}},
                    "required": ["subject"]
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "ticket_id": {"type": "string"},
                        "status": {"type": "string"}
                    }
                })),
            })
            .unwrap();

        let catalog = registry.tool_catalog();
        assert!(catalog.contains("create_ticket("));
        assert!(catalog.contains("-> {"));
        assert!(catalog.contains("ticket_id: string"));
    }

    #[test]
    fn test_catalog_without_output_schema() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("echo", "Echo tool"))
            .unwrap();

        let catalog = registry.tool_catalog();
        assert!(catalog.contains("echo("));
        assert!(!catalog.contains("->"));
    }

    #[test]
    fn test_execute_description_mentions_batch_independence() {
        let registry = ToolRegistry::new();
        let description = registry.execute_tool_description();
        assert!(description.contains("INDEPENDENT"));
        assert!(description.contains("Do NOT use batch_tools()"));
    }

    // === system_prompt ===

    #[test]
    fn test_system_prompt_contains_key_instructions() {
        let registry = ToolRegistry::new();
        let prompt = registry.system_prompt();
        assert!(prompt.contains("FRESH sandbox"));
        assert!(prompt.contains("SINGLE script"));
        assert!(prompt.contains("batch_tools()"));
        assert!(prompt.contains("LAST EXPRESSION"));
        assert!(prompt.contains("Do NOT use print()"));
        assert!(prompt.contains("NameError"));
    }

    #[test]
    fn test_system_prompt_includes_good_and_bad_examples() {
        let registry = ToolRegistry::new();
        let prompt = registry.system_prompt();
        assert!(prompt.contains("GOOD:"));
        assert!(prompt.contains("BAD:"));
    }

    // === execute_tool_input_schema ===

    #[test]
    fn test_execute_tool_input_schema_structure() {
        let registry = ToolRegistry::new();
        let schema = registry.execute_tool_input_schema();

        assert_eq!(schema["type"], "object");

        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("code"));
        assert!(props.contains_key("inputs"));

        assert_eq!(props["code"]["type"], "string");
        assert_eq!(props["inputs"]["type"], "object");

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "code");
    }

    #[test]
    fn test_execute_tool_input_schema_has_descriptions() {
        let registry = ToolRegistry::new();
        let schema = registry.execute_tool_input_schema();
        let props = schema["properties"].as_object().unwrap();

        assert!(props["code"]["description"].as_str().unwrap().contains("Python"));
        assert!(props["inputs"]["description"].as_str().unwrap().contains("Variables"));
    }

    // === search_tool_input_schema ===

    #[test]
    fn test_search_tool_input_schema_structure() {
        let registry = ToolRegistry::new();
        let schema = registry.search_tool_input_schema();

        assert_eq!(schema["type"], "object");

        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("query"));
        assert!(props.contains_key("top_k"));

        assert_eq!(props["query"]["type"], "string");
        assert_eq!(props["top_k"]["type"], "number");

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "query");
    }

    #[test]
    fn test_search_tool_input_schema_top_k_not_required() {
        let registry = ToolRegistry::new();
        let schema = registry.search_tool_input_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(!required.contains(&"top_k"));
    }

    // === execute_tool_description embeds catalog ===

    #[test]
    fn test_execute_description_embeds_tool_catalog() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(create_test_tool("my_tool", "Does something"))
            .unwrap();
        let desc = registry.execute_tool_description();
        assert!(desc.contains("my_tool("));
        assert!(desc.contains("Does something"));
    }

    #[test]
    fn test_execute_description_mentions_runtime_restrictions() {
        let registry = ToolRegistry::new();
        let desc = registry.execute_tool_description();
        assert!(desc.contains("No standard library"));
        assert!(desc.contains("No class definitions"));
        assert!(desc.contains("sorted()"));
    }
}
