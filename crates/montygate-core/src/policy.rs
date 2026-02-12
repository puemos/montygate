use crate::registry::ToolRoute;
use crate::types::{PolicyAction, PolicyConfig, Result};
use crate::MontygateError;
use dashmap::DashMap;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use regex::Regex;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Rate limit entry for a tool
#[derive(Debug, Clone)]
struct RateLimitEntry {
    limiter: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    quota_desc: String,
}

/// Policy engine for managing tool access and approvals
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    config: PolicyConfig,
    rate_limits: Arc<DashMap<String, RateLimitEntry>>,
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig) -> Self {
        let engine = Self {
            config,
            rate_limits: Arc::new(DashMap::new()),
        };

        // Pre-compile rate limiters for rules that have them
        for rule in &engine.config.rules {
            if let Some(rate_limit_str) = &rule.rate_limit {
                if let Ok(limiter) = parse_rate_limit(rate_limit_str) {
                    engine.rate_limits.insert(
                        rule.match_pattern.clone(),
                        RateLimitEntry {
                            limiter: Arc::new(limiter),
                            quota_desc: rate_limit_str.clone(),
                        },
                    );
                }
            }
        }

        engine
    }

    /// Check if a tool call is allowed
    pub async fn check(
        &self,
        route: &ToolRoute,
        _args: &serde_json::Value,
    ) -> Result<PolicyDecision> {
        let tool_name = format!("{}.{}", route.server_name, route.tool_name);

        debug!("Checking policy for tool '{}'", tool_name);

        // Check rules in order (more specific rules should come first)
        for rule in &self.config.rules {
            if matches_pattern(&rule.match_pattern, &tool_name) {
                match rule.action {
                    PolicyAction::Allow => {
                        // Check rate limiting if configured
                        if let Some(entry) = self.rate_limits.get(&rule.match_pattern) {
                            if let Err(_e) = entry.limiter.check() {
                                warn!(
                                    "Rate limit exceeded for tool '{}': {}",
                                    tool_name, entry.quota_desc
                                );
                                return Ok(PolicyDecision::RateLimitExceeded {
                                    tool: tool_name,
                                    limit: entry.quota_desc.clone(),
                                });
                            }
                        }

                        info!("Tool '{}' allowed by rule '{}'", tool_name, rule.match_pattern);
                        return Ok(PolicyDecision::Allow);
                    }
                    PolicyAction::Deny => {
                        warn!("Tool '{}' denied by rule '{}'", tool_name, rule.match_pattern);
                        return Ok(PolicyDecision::Deny {
                            reason: format!("Tool '{}' is not allowed", tool_name),
                        });
                    }
                    PolicyAction::RequireApproval => {
                        info!(
                            "Tool '{}' requires approval by rule '{}'",
                            tool_name, rule.match_pattern
                        );
                        return Ok(PolicyDecision::RequireApproval {
                            tool: tool_name,
                            args: _args.clone(),
                        });
                    }
                }
            }
        }

        // No matching rule, apply default action
        match self.config.defaults.action {
            PolicyAction::Allow => {
                debug!("Tool '{}' allowed by default policy", tool_name);
                Ok(PolicyDecision::Allow)
            }
            PolicyAction::Deny => {
                warn!("Tool '{}' denied by default policy", tool_name);
                Ok(PolicyDecision::Deny {
                    reason: format!("Tool '{}' is not allowed by default policy", tool_name),
                })
            }
            PolicyAction::RequireApproval => {
                info!("Tool '{}' requires approval by default policy", tool_name);
                Ok(PolicyDecision::RequireApproval {
                    tool: tool_name,
                    args: _args.clone(),
                })
            }
        }
    }

    /// Check if a tool call should be rate limited
    pub fn check_rate_limit(&self, tool_name: &str) -> Result<()> {
        // This is a simplified version - in practice, you'd want per-tool rate limiting
        // For now, we just check if there are any rate limit entries that match
        for entry in self.rate_limits.iter() {
            if matches_pattern(entry.key(), tool_name) {
                if let Err(_e) = entry.limiter.check() {
                    return Err(MontygateError::RateLimitExceeded(format!(
                        "Rate limit exceeded for pattern '{}'",
                        entry.key()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Get the policy configuration
    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// Update the policy configuration
    pub fn update_config(&mut self, config: PolicyConfig) {
        // Clear old rate limits
        self.rate_limits.clear();

        // Re-initialize with new config
        for rule in &config.rules {
            if let Some(rate_limit_str) = &rule.rate_limit {
                if let Ok(limiter) = parse_rate_limit(rate_limit_str) {
                    self.rate_limits.insert(
                        rule.match_pattern.clone(),
                        RateLimitEntry {
                            limiter: Arc::new(limiter),
                            quota_desc: rate_limit_str.clone(),
                        },
                    );
                }
            }
        }

        self.config = config;
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new(PolicyConfig::default())
    }
}

/// Decision from the policy engine
#[derive(Debug, Clone)]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    RequireApproval { tool: String, args: serde_json::Value },
    RateLimitExceeded { tool: String, limit: String },
}

impl PolicyDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, PolicyDecision::Allow)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, PolicyDecision::Deny { .. })
    }

    pub fn requires_approval(&self) -> bool {
        matches!(self, PolicyDecision::RequireApproval { .. })
    }
}

/// Check if a tool name matches a pattern (supports wildcards)
fn matches_pattern(pattern: &str, tool_name: &str) -> bool {
    // Handle exact match
    if pattern == tool_name {
        return true;
    }

    // Handle wildcard patterns
    if pattern.contains('*') {
        let regex_pattern = pattern.replace(".", r"\.").replace("*", ".*");
        if let Ok(regex) = Regex::new(&format!("^{}$", regex_pattern)) {
            return regex.is_match(tool_name);
        }
    }

    // Handle server.* patterns
    if let Some(server) = pattern.strip_suffix(".*") {
        return tool_name.starts_with(&format!("{}.", server));
    }

    false
}

/// Parse a rate limit string like "10/min", "100/hour", "1000/day"
fn parse_rate_limit(limit_str: &str) -> Result<RateLimiter<NotKeyed, InMemoryState, DefaultClock>> {
    let parts: Vec<&str> = limit_str.split('/').collect();
    if parts.len() != 2 {
        return Err(MontygateError::Configuration(format!(
            "Invalid rate limit format: {}",
            limit_str
        )));
    }

    let count: u32 = parts[0]
        .parse()
        .map_err(|_| MontygateError::Configuration(format!("Invalid rate limit count: {}", parts[0])))?;

    let duration = match parts[1] {
        "sec" | "second" | "s" => Duration::from_secs(1),
        "min" | "minute" | "m" => Duration::from_secs(60),
        "hour" | "h" => Duration::from_secs(3600),
        "day" | "d" => Duration::from_secs(86400),
        _ => {
            return Err(MontygateError::Configuration(format!(
                "Invalid rate limit duration: {}",
                parts[1]
            )))
        }
    };

    let quota = Quota::with_period(duration)
        .ok_or_else(|| MontygateError::Configuration("Invalid quota period".to_string()))?
        .allow_burst(NonZeroU32::new(count).unwrap_or(NonZeroU32::new(1).unwrap()));

    Ok(RateLimiter::direct(quota))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PolicyConfig, PolicyDefaults, PolicyRule};

    fn create_test_policy_config() -> PolicyConfig {
        PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![
                PolicyRule {
                    match_pattern: "*.delete_*".to_string(),
                    action: PolicyAction::Deny,
                    rate_limit: None,
                },
                PolicyRule {
                    match_pattern: "github.create_issue".to_string(),
                    action: PolicyAction::Allow,
                    rate_limit: Some("10/min".to_string()),
                },
                PolicyRule {
                    match_pattern: "salesforce.update_record".to_string(),
                    action: PolicyAction::RequireApproval,
                    rate_limit: None,
                },
            ],
        }
    }

    fn make_route(server: &str, tool: &str) -> ToolRoute {
        ToolRoute {
            server_name: server.to_string(),
            tool_name: tool.to_string(),
            definition: crate::types::ToolDefinition {
                name: tool.to_string(),
                description: None,
                input_schema: serde_json::json!({}),
            },
        }
    }

    // === matches_pattern ===

    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern(
            "github.create_issue",
            "github.create_issue"
        ));
        assert!(!matches_pattern(
            "github.create_issue",
            "github.list_issues"
        ));
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern("*.delete_*", "github.delete_repo"));
        assert!(matches_pattern("*.delete_*", "postgres.delete_row"));
        assert!(!matches_pattern("*.delete_*", "github.create_issue"));
    }

    #[test]
    fn test_matches_pattern_server_wildcard() {
        assert!(matches_pattern("github.*", "github.create_issue"));
        assert!(matches_pattern("github.*", "github.delete_repo"));
        assert!(!matches_pattern("github.*", "slack.post_message"));
    }

    #[test]
    fn test_matches_pattern_no_match() {
        assert!(!matches_pattern("exact.match", "other.tool"));
        assert!(!matches_pattern("server.tool", "server.other"));
    }

    // === PolicyEngine ===

    #[test]
    fn test_policy_engine_default() {
        let engine = PolicyEngine::default();
        assert!(engine.config.rules.is_empty());
        assert!(matches!(engine.config.defaults.action, PolicyAction::Allow));
    }

    #[test]
    fn test_policy_engine_config() {
        let config = create_test_policy_config();
        let engine = PolicyEngine::new(config);
        assert_eq!(engine.config().rules.len(), 3);
    }

    #[tokio::test]
    async fn test_allow_policy() {
        let engine = PolicyEngine::new(create_test_policy_config());
        let route = make_route("github", "create_issue");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn test_deny_policy() {
        let engine = PolicyEngine::new(create_test_policy_config());
        let route = make_route("github", "delete_repo");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(decision.is_denied());
    }

    #[tokio::test]
    async fn test_require_approval_policy() {
        let engine = PolicyEngine::new(create_test_policy_config());
        let route = make_route("salesforce", "update_record");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(decision.requires_approval());
    }

    #[tokio::test]
    async fn test_default_policy_allow_fallthrough() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("any", "tool");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn test_default_policy_deny_fallthrough() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Deny,
            },
            rules: vec![],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("any", "tool");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(decision.is_denied());
    }

    #[tokio::test]
    async fn test_default_policy_require_approval_fallthrough() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::RequireApproval,
            },
            rules: vec![],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("any", "tool");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(decision.requires_approval());
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("2/min".to_string()),
            }],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("test", "tool");

        let d1 = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(d1.is_allowed());
        let d2 = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(d2.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiting_exceeded() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.limited".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("test", "limited");

        // First call should succeed
        let d1 = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(d1.is_allowed());

        // Second call should be rate limited
        let d2 = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(matches!(d2, PolicyDecision::RateLimitExceeded { .. }));
    }

    #[test]
    fn test_check_rate_limit_standalone() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        };
        let engine = PolicyEngine::new(config);

        // First call ok
        assert!(engine.check_rate_limit("test.tool").is_ok());

        // Second should fail
        let result = engine.check_rate_limit("test.tool");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_rate_limit_no_matching_pattern() {
        let engine = PolicyEngine::default();
        assert!(engine.check_rate_limit("any.tool").is_ok());
    }

    #[test]
    fn test_update_config() {
        let mut engine = PolicyEngine::default();
        assert!(engine.config().rules.is_empty());

        let new_config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Deny,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.*".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("5/min".to_string()),
            }],
        };
        engine.update_config(new_config);

        assert_eq!(engine.config().rules.len(), 1);
        assert!(matches!(
            engine.config().defaults.action,
            PolicyAction::Deny
        ));
    }

    #[tokio::test]
    async fn test_update_config_clears_old_rate_limits() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        };
        let mut engine = PolicyEngine::new(config);
        let route = make_route("test", "tool");

        // Exhaust the rate limit
        let _ = engine.check(&route, &serde_json::json!({})).await;

        // Now update config with a fresh rate limit
        let new_config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("10/min".to_string()),
            }],
        };
        engine.update_config(new_config);

        // Should be allowed again
        let d = engine.check(&route, &serde_json::json!({})).await.unwrap();
        assert!(d.is_allowed());
    }

    // === parse_rate_limit ===

    #[test]
    fn test_parse_rate_limit_all_durations() {
        assert!(parse_rate_limit("10/sec").is_ok());
        assert!(parse_rate_limit("10/second").is_ok());
        assert!(parse_rate_limit("10/s").is_ok());
        assert!(parse_rate_limit("10/min").is_ok());
        assert!(parse_rate_limit("10/minute").is_ok());
        assert!(parse_rate_limit("10/m").is_ok());
        assert!(parse_rate_limit("10/hour").is_ok());
        assert!(parse_rate_limit("10/h").is_ok());
        assert!(parse_rate_limit("10/day").is_ok());
        assert!(parse_rate_limit("10/d").is_ok());
    }

    #[test]
    fn test_parse_rate_limit_errors() {
        // Missing slash
        assert!(parse_rate_limit("invalid").is_err());
        // Invalid count
        assert!(parse_rate_limit("abc/min").is_err());
        // Invalid duration
        assert!(parse_rate_limit("10/invalid_duration").is_err());
        // Too many slashes
        assert!(parse_rate_limit("10/min/extra").is_err());
    }

    // === PolicyDecision ===

    #[test]
    fn test_policy_decision_allow() {
        let d = PolicyDecision::Allow;
        assert!(d.is_allowed());
        assert!(!d.is_denied());
        assert!(!d.requires_approval());
    }

    #[test]
    fn test_policy_decision_deny() {
        let d = PolicyDecision::Deny {
            reason: "nope".into(),
        };
        assert!(!d.is_allowed());
        assert!(d.is_denied());
        assert!(!d.requires_approval());
    }

    #[test]
    fn test_policy_decision_require_approval() {
        let d = PolicyDecision::RequireApproval {
            tool: "test.tool".into(),
            args: serde_json::json!({"key": "val"}),
        };
        assert!(!d.is_allowed());
        assert!(!d.is_denied());
        assert!(d.requires_approval());
    }

    #[test]
    fn test_policy_decision_rate_limit_exceeded() {
        let d = PolicyDecision::RateLimitExceeded {
            tool: "test.tool".into(),
            limit: "10/min".into(),
        };
        assert!(!d.is_allowed());
        assert!(!d.is_denied());
        assert!(!d.requires_approval());
    }

    #[tokio::test]
    async fn test_require_approval_passes_args() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.approve".to_string(),
                action: PolicyAction::RequireApproval,
                rate_limit: None,
            }],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("test", "approve");
        let args = serde_json::json!({"key": "value"});

        let decision = engine.check(&route, &args).await.unwrap();
        match decision {
            PolicyDecision::RequireApproval { tool, args: a } => {
                assert_eq!(tool, "test.approve");
                assert_eq!(a["key"], "value");
            }
            _ => panic!("Expected RequireApproval"),
        }
    }

    #[tokio::test]
    async fn test_deny_includes_tool_name() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.blocked".to_string(),
                action: PolicyAction::Deny,
                rate_limit: None,
            }],
        };
        let engine = PolicyEngine::new(config);
        let route = make_route("test", "blocked");
        let decision = engine.check(&route, &serde_json::json!({})).await.unwrap();
        match decision {
            PolicyDecision::Deny { reason } => {
                assert!(reason.contains("test.blocked"));
            }
            _ => panic!("Expected Deny"),
        }
    }

    #[test]
    fn test_invalid_rate_limit_rule_ignored() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test.tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("invalid".to_string()),
            }],
        };
        // Should not panic - invalid rate limits are simply skipped
        let _engine = PolicyEngine::new(config);
    }
}