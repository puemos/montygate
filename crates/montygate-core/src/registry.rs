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

    /// Validate that the provided kwargs keys match the tool's input_schema properties.
    ///
    /// Returns `Ok(())` if all keys are valid, or an error describing which keys
    /// are unexpected and what the expected parameters are.  This catches wrong
    /// parameter names (e.g. `id=` instead of `customer_id=`) **before** the JS
    /// handler runs, preventing cascading failures from `undefined` values.
    pub fn validate_tool_args(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        let tool = match self.tools.get(tool_name) {
            Some(entry) => entry.clone(),
            None => return Ok(()), // tool-not-found is handled elsewhere
        };

        let arg_keys: Vec<&str> = match args.as_object() {
            Some(map) => map.keys().map(|k| k.as_str()).collect(),
            None => return Ok(()), // non-object args (e.g. empty {}) are fine
        };

        if arg_keys.is_empty() {
            return Ok(());
        }

        let schema_props: Vec<String> = tool
            .input_schema
            .as_object()
            .and_then(|o| o.get("properties"))
            .and_then(|p| p.as_object())
            .map(|props| props.keys().cloned().collect())
            .unwrap_or_default();

        if schema_props.is_empty() {
            return Ok(()); // no schema to validate against
        }

        let unknown_keys: Vec<&str> = arg_keys
            .iter()
            .filter(|k| !schema_props.iter().any(|p| p == *k))
            .copied()
            .collect();

        if unknown_keys.is_empty() {
            return Ok(());
        }

        Err(MontygateError::Validation(format!(
            "{} does not accept parameter(s): {}. Expected: {}",
            tool_name,
            unknown_keys.join(", "),
            schema_props.join(", "),
        )))
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

    /// Return a comma-separated list of registered tool names.
    pub fn tool_names(&self) -> String {
        let mut names: Vec<String> = self.tools.iter().map(|e| e.key().clone()).collect();
        names.sort();
        names.join(", ")
    }

    /// Build compact tool signatures: `name(param1, param2) - Description -> {field1, field2}`
    ///
    /// Includes parameter names (no types) and the tool description.  Output
    /// field names are appended when an output_schema is provided.  This gives
    /// the LLM enough context to write correct scripts without the token cost
    /// of full type annotations or the full catalog.
    pub fn tool_signatures(&self) -> String {
        let mut sigs = String::new();
        let mut tools: Vec<ToolDefinition> = self.all_tools();
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        for tool in tools {
            sigs.push_str(&format!("- {}(", tool.name));

            if let Some(props) = tool
                .input_schema
                .as_object()
                .and_then(|o| o.get("properties"))
                .and_then(|p| p.as_object())
            {
                let param_names: Vec<&String> = props.keys().collect();
                sigs.push_str(
                    &param_names
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }

            sigs.push(')');

            if let Some(desc) = &tool.description {
                sigs.push_str(&format!(" - {}", desc));
            }

            if let Some(output_schema) = &tool.output_schema {
                let formatted = format_signature_output(output_schema);
                if !formatted.is_empty() {
                    sigs.push_str(&format!(" -> {{{}}}", formatted));
                }
            }

            sigs.push('\n');
        }
        sigs
    }

    /// Maximum number of tools to include as compact signatures in the execute
    /// tool description.  Above this threshold the description switches to a
    /// names-only listing and relies on the search tool for discovery.
    ///
    /// The default of 20 is based on RAG-MCP (arXiv 2505.03275) which shows
    /// tool-selection accuracy stays high with small sets but degrades in the
    /// 30-70 range.  Switching at 20 provides headroom before degradation.
    pub const SIGNATURE_THRESHOLD: usize = 20;

    /// Build the canonical description for the "execute" tool exposed to LLMs.
    /// This ensures all SDKs (Node, Python, etc.) present the same instructions.
    ///
    /// When the registry contains ≤ [`Self::SIGNATURE_THRESHOLD`] tools, compact
    /// signatures (parameter + output field names) are inlined so the LLM can
    /// write correct scripts without an extra search round-trip.
    ///
    /// Above the threshold, only tool names are listed and the LLM is directed
    /// to use the search tool — avoiding the prompt-bloat degradation documented
    /// in RAG-MCP and ToolScan (arXiv 2411.13547).
    pub fn execute_tool_description(&self) -> String {
        let tool_count = self.tool_count();
        let tool_listing = if tool_count <= Self::SIGNATURE_THRESHOLD {
            format!(
                "Available tools:\n{}\
                 Use the separate search tool only when a needed signature or return shape is unclear.",
                self.tool_signatures(),
            )
        } else {
            format!(
                "Available tools ({tool_count} total): {}\n\
                 Use the separate search tool to look up tool signatures before writing scripts.",
                self.tool_names(),
            )
        };

        format!(
            "Execute a Python script in a sandboxed environment.\n\
             Use execute when you need to chain multiple tools, use control flow (loops,\n\
             conditionals), or transform data. For simple single-tool operations, prefer\n\
             calling the tool directly.\n\
             \n\
             IMPORTANT RULES:\n\
             - Call tools ONLY via: result = tool('name', key=value)\n\
               Direct function calls like name() will fail with NameError.\n\
             - tool() typically returns a dict. Check the tool's return signature (→ {{...}}) and access fields by key: result = tool(...); value = result['field']\n\
             - The LAST EXPRESSION is the return value. Do NOT use print().\n\
             - No imports allowed. No standard library — json, re, math, datetime, collections, itertools, functools, sys, os, io etc. are NOT available. Use plain dicts, lists, and builtins instead.\n\
             - You CAN define helper functions and dataclasses. No other class types.\n\
             - sorted() and .sort() do NOT support key= or reverse= and cannot sort tuples. Only sort flat lists of numbers or strings. For custom sort order, build the list manually.\n\
             - Chained subscript assignment x[a][b] = val is NOT supported. Instead: inner = x[a]; inner[b] = val; x[a] = inner\n\
             - Set operators (|, &, -, ^) are not supported — use set.update(), set.add(), or loop.\n\
             \n\
             {tool_listing}\n\
             \n\
             Each execute() starts fresh — include ALL tool() calls you need in a single script.\n\
             Maximize tool calls per script to reduce round-trips:\n\
               order = tool('lookup_order', order_id='ORD-123')\n\
               customer = tool('get_customer', customer_id=order['customer_id'])\n\
               eligible = tool('check_refund', order_id='ORD-123', tier=customer['tier'])\n\
               if eligible['approved']:\n\
                   refund = tool('process_refund', order_id='ORD-123', amount=order['total'])\n\
               {{'order': order, 'customer': customer, 'refund': refund if eligible['approved'] else None}}\n\
             \n\
             For independent parallel calls: batch_tools([('name1', {{...}}), ('name2', {{...}})])"
        )
    }

    /// Return the recommended system prompt for LLM conversations.
    ///
    /// This is strategic guidance — HOW to use the sandbox effectively.
    /// Separate from the tool description (WHAT the tool does / syntax rules).
    /// Designed to be prepended or appended to the developer's own system prompt.
    pub fn system_prompt(&self) -> String {
        "You have individual tools AND a sandboxed Python environment (execute).\n\
         \n\
         Choose the right approach for each task:\n\
         - For simple single-tool operations → call the tool directly.\n\
         - For multi-step chains, loops, conditionals, or data transformations → use execute.\n\
         \n\
         When using execute, write ONE comprehensive script that handles the ENTIRE task\n\
         end-to-end. NEVER split work across multiple execute() calls — there is NO shared\n\
         state between them. Each extra call re-sends the entire conversation, multiplying\n\
         token cost. Gather data AND act on it in the same script.\n\
         \n\
         Inside execute scripts:\n\
         - tool() typically returns a dict — check the return signature and access fields by key, e.g. result['field']\n\
         - Chain calls: use the return value of tool('a', ...) as input to tool('b', ...)\n\
         - Use loops for repeated operations, conditionals for branching logic\n\
         - Filter lists by field values — never assume ordering\n\
         - Return a single result dict summarizing everything done\n\
         - For independent parallel calls: batch_tools([('name1', {args}), ('name2', {args})])\n\
         \n\
         Never call execute() to \"gather information first\" — gather and act in one script.\n\
         \n\
         COMMON MISTAKES (avoid these):\n\
         - WRONG: days = tool('days_since', ...); if days > 14\n\
           RIGHT: result = tool('days_since', ...); if result['days'] > 14\n\
           (tool() usually returns a dict — check the return signature (→ {...}) and access fields by key)\n\
         - WRONG: item = items[0]  # assumes first element is the one you want\n\
           RIGHT: item = [i for i in items if i['status'] == 'returned'][0]\n\
           (filter by field values instead of assuming list ordering)\n\
         - WRONG: calling execute() once to gather data, then again to act on it\n\
           RIGHT: gather data AND act on it in a single execute() script\n\
         \n\
         Example — handling a complete workflow in one script:\n\
         \n\
           history = tool('get_order_history', customer_id='CUST-123')\n\
           qualifying = [o for o in history['orders'] if o['total'] > 50 and o['status'] == 'shipped']\n\
           results = []\n\
           for o in qualifying:\n\
               detail = tool('lookup_order', order_id=o['id'])\n\
               refund = tool('process_refund', order_id=o['id'], amount=detail['total'], policy_id='POL-100')\n\
               results.append({'order': o['id'], 'refund_id': refund['refund_id']})\n\
           tool('send_notification', channel='email', recipient='user@example.com',\n\
                subject='Refunds processed', body=f'{len(results)} refunds completed')\n\
           {'processed': results, 'count': len(results)}"
            .to_string()
    }

    /// Build the canonical description for the "search" tool exposed to LLMs.
    pub fn search_tool_description(&self) -> String {
        "Search for available tools by keyword. Use this separate tool only when the execute description does not already give you the exact signature you need.".to_string()
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
/// Format an output schema for compact tool signatures.
///
/// Shows field names with one level of nesting so the LLM knows the exact
/// keys to use.  Examples:
///   `{id, name, email}` — flat object
///   `{items: [{sku, available, warehouse}]}` — array of objects
/// Extract field names from an output schema for compact signature display.
///
/// Recursion is capped at depth 1 to keep signatures short:
/// - depth 0: expand object fields, recurse into nested objects/arrays
/// - depth 1+: just field names, no further nesting
fn format_signature_output(schema: &serde_json::Value) -> String {
    format_sig_fields(schema, 0)
}

fn format_sig_fields(schema: &serde_json::Value, depth: usize) -> String {
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("object") => {
            if let Some(props) = schema
                .as_object()
                .and_then(|o| o.get("properties"))
                .and_then(|p| p.as_object())
            {
                let fields: Vec<String> = props
                    .iter()
                    .map(|(name, field_schema)| {
                        if depth < 1 {
                            let nested = format_sig_fields(field_schema, depth + 1);
                            if nested.is_empty() {
                                name.clone()
                            } else {
                                format!("{}: {}", name, nested)
                            }
                        } else {
                            name.clone()
                        }
                    })
                    .collect();
                if fields.is_empty() {
                    String::new()
                } else {
                    let joined = fields.join(", ");
                    // Nested objects get wrapped in {}; top-level wrapping
                    // is done by the caller in tool_signatures()
                    if depth > 0 {
                        format!("{{{}}}", joined)
                    } else {
                        joined
                    }
                }
            } else {
                String::new()
            }
        }
        Some("array") => {
            if let Some(items) = schema.get("items") {
                let inner = format_sig_fields(items, depth);
                if inner.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", inner)
                }
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

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

    // === execute_tool_input_schema ===

    #[test]
    fn test_execute_tool_input_schema_structure() {
        let registry = ToolRegistry::new();
        let schema = registry.execute_tool_input_schema();

        assert_eq!(schema["type"], "object");

        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("code"));
        assert!(!props.contains_key("inputs"));

        assert_eq!(props["code"]["type"], "string");

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "code");
    }

    #[test]
    fn test_execute_tool_input_schema_has_descriptions() {
        let registry = ToolRegistry::new();
        let schema = registry.execute_tool_input_schema();
        let props = schema["properties"].as_object().unwrap();

        assert!(props["code"]["description"]
            .as_str()
            .unwrap()
            .contains("Python"));
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

    // === tool_names ===

    #[test]
    fn test_tool_names_lists_registered_tools() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("beta_tool", "Beta"),
                create_test_tool("alpha_tool", "Alpha"),
            ])
            .unwrap();
        let names = registry.tool_names();
        assert_eq!(names, "alpha_tool, beta_tool");
    }

    #[test]
    fn test_tool_names_empty_registry() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.tool_names(), "");
    }

    // === tool_signatures ===

    #[test]
    fn test_tool_signatures_include_description() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "lookup_order".to_string(),
                description: Some("Look up order details by ID".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "order_id": { "type": "string", "description": "The order ID" }
                    },
                    "required": ["order_id"]
                }),
                output_schema: None,
            })
            .unwrap();
        let sigs = registry.tool_signatures();
        // Contains param name and description but not types or param descriptions
        assert!(sigs.contains("lookup_order(order_id) - Look up order details by ID"));
        assert!(!sigs.contains("string"));
        assert!(!sigs.contains("The order ID"));
    }

    #[test]
    fn test_tool_signatures_with_output_schema() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "get_customer".to_string(),
                description: Some("Get customer profile".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "customer_id": { "type": "string" }
                    },
                    "required": ["customer_id"]
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "email": { "type": "string" },
                        "tier": { "type": "string" }
                    }
                })),
            })
            .unwrap();
        let sigs = registry.tool_signatures();
        // serde_json sorts keys alphabetically; output uses {field, field} syntax
        assert!(sigs.contains(
            "get_customer(customer_id) - Get customer profile -> {email, name, tier}"
        ));
    }

    #[test]
    fn test_tool_signatures_nested_output_schema() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "check_inventory".to_string(),
                description: Some("Check inventory".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "sku_list": { "type": "array", "items": { "type": "string" } }
                    }
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "sku": { "type": "string" },
                                    "available": { "type": "number" },
                                    "warehouse": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
            })
            .unwrap();
        let sigs = registry.tool_signatures();
        // Nested objects show field names inside array brackets
        assert!(sigs.contains(
            "check_inventory(sku_list) - Check inventory -> {items: [{available, sku, warehouse}]}"
        ));
    }

    #[test]
    fn test_tool_signatures_sorted() {
        let registry = ToolRegistry::new();
        registry
            .register_tools(vec![
                create_test_tool("zebra", "Z"),
                create_test_tool("alpha", "A"),
            ])
            .unwrap();
        let sigs = registry.tool_signatures();
        let alpha_pos = sigs.find("alpha").unwrap();
        let zebra_pos = sigs.find("zebra").unwrap();
        assert!(alpha_pos < zebra_pos);
    }

    // === execute_tool_description — threshold behavior ===

    #[test]
    fn test_execute_description_uses_signatures_below_threshold() {
        let registry = ToolRegistry::new();
        // Register a few tools (below threshold)
        for i in 0..5 {
            registry
                .register_tool(create_test_tool(
                    &format!("tool_{}", i),
                    &format!("Tool {}", i),
                ))
                .unwrap();
        }
        let desc = registry.execute_tool_description();
        // Should contain compact signatures (tool_0(...))
        assert!(desc.contains("tool_0("));
        // Should NOT contain "total)" which signals names-only mode
        assert!(!desc.contains("total)"));
    }

    #[test]
    fn test_execute_description_uses_names_above_threshold() {
        let registry = ToolRegistry::new();
        // Register more than SIGNATURE_THRESHOLD tools
        for i in 0..(ToolRegistry::SIGNATURE_THRESHOLD + 5) {
            registry
                .register_tool(create_test_tool(
                    &format!("tool_{:03}", i),
                    &format!("Tool {}", i),
                ))
                .unwrap();
        }
        let desc = registry.execute_tool_description();
        // Should contain "total)" indicating names-only mode
        assert!(desc.contains("total)"));
        // Should NOT contain full signatures
        assert!(!desc.contains("tool_000("));
        // Should mention search tool
        assert!(desc.contains("Use the separate search tool"));
    }

    // === validate_tool_args ===

    #[test]
    fn test_validate_tool_args_accepts_valid_params() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "get_customer".to_string(),
                description: Some("Get customer".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "customer_id": { "type": "string" }
                    },
                    "required": ["customer_id"]
                }),
                output_schema: None,
            })
            .unwrap();

        let result = registry.validate_tool_args(
            "get_customer",
            &serde_json::json!({"customer_id": "C-123"}),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tool_args_rejects_unknown_params() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "get_customer".to_string(),
                description: Some("Get customer".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "customer_id": { "type": "string" }
                    },
                    "required": ["customer_id"]
                }),
                output_schema: None,
            })
            .unwrap();

        let result = registry.validate_tool_args(
            "get_customer",
            &serde_json::json!({"id": "C-123"}),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not accept parameter(s): id"));
        assert!(err.contains("Expected: customer_id"));
    }

    #[test]
    fn test_validate_tool_args_reports_multiple_unknown() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "send_email".to_string(),
                description: None,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "string" },
                        "body": { "type": "string" }
                    }
                }),
                output_schema: None,
            })
            .unwrap();

        let result = registry.validate_tool_args(
            "send_email",
            &serde_json::json!({"recipient": "a@b.com", "content": "hi"}),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("recipient"));
        assert!(err.contains("content"));
        assert!(err.contains("Expected: "));
    }

    #[test]
    fn test_validate_tool_args_passes_empty_args() {
        let registry = ToolRegistry::new();
        registry
            .register_tool(ToolDefinition {
                name: "ping".to_string(),
                description: None,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" }
                    }
                }),
                output_schema: None,
            })
            .unwrap();

        let result = registry.validate_tool_args("ping", &serde_json::json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tool_args_unknown_tool_passes() {
        let registry = ToolRegistry::new();
        let result = registry.validate_tool_args(
            "nonexistent",
            &serde_json::json!({"anything": "goes"}),
        );
        assert!(result.is_ok()); // unknown tool validation is handled elsewhere
    }
}
