use crate::MontygateError;
use crate::types::{PolicyAction, PolicyConfig, Result};
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

    /// Check if a tool call is allowed by name
    pub async fn check(&self, tool_name: &str, args: &serde_json::Value) -> Result<PolicyDecision> {
        debug!("Checking policy for tool '{}'", tool_name);

        // Check rules in order (more specific rules should come first)
        for rule in &self.config.rules {
            if matches_pattern(&rule.match_pattern, tool_name) {
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
                                    tool: tool_name.to_string(),
                                    limit: entry.quota_desc.clone(),
                                });
                            }
                        }

                        info!(
                            "Tool '{}' allowed by rule '{}'",
                            tool_name, rule.match_pattern
                        );
                        return Ok(PolicyDecision::Allow);
                    }
                    PolicyAction::Deny => {
                        warn!(
                            "Tool '{}' denied by rule '{}'",
                            tool_name, rule.match_pattern
                        );
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
                            tool: tool_name.to_string(),
                            args: args.clone(),
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
                    tool: tool_name.to_string(),
                    args: args.clone(),
                })
            }
        }
    }

    /// Check if a tool call should be rate limited
    pub fn check_rate_limit(&self, tool_name: &str) -> Result<()> {
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
        self.rate_limits.clear();

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
    Deny {
        reason: String,
    },
    RequireApproval {
        tool: String,
        args: serde_json::Value,
    },
    RateLimitExceeded {
        tool: String,
        limit: String,
    },
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
    if pattern == tool_name {
        return true;
    }

    if pattern.contains('*') {
        let regex_pattern = pattern.replace(".", r"\.").replace("*", ".*");
        if let Ok(regex) = Regex::new(&format!("^{}$", regex_pattern)) {
            return regex.is_match(tool_name);
        }
    }

    if let Some(prefix) = pattern.strip_suffix(".*") {
        return tool_name.starts_with(&format!("{}.", prefix));
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

    let count: u32 = parts[0].parse().map_err(|_| {
        MontygateError::Configuration(format!("Invalid rate limit count: {}", parts[0]))
    })?;

    let duration = match parts[1] {
        "sec" | "second" | "s" => Duration::from_secs(1),
        "min" | "minute" | "m" => Duration::from_secs(60),
        "hour" | "h" => Duration::from_secs(3600),
        "day" | "d" => Duration::from_secs(86400),
        _ => {
            return Err(MontygateError::Configuration(format!(
                "Invalid rate limit duration: {}",
                parts[1]
            )));
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
                    match_pattern: "create_issue".to_string(),
                    action: PolicyAction::Allow,
                    rate_limit: Some("10/min".to_string()),
                },
                PolicyRule {
                    match_pattern: "update_record".to_string(),
                    action: PolicyAction::RequireApproval,
                    rate_limit: None,
                },
            ],
        }
    }

    // === matches_pattern ===

    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern("create_issue", "create_issue"));
        assert!(!matches_pattern("create_issue", "list_issues"));
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern("*delete*", "delete_repo"));
        assert!(matches_pattern("*delete*", "bulk_delete_rows"));
        assert!(!matches_pattern("*delete*", "create_issue"));
    }

    #[test]
    fn test_matches_pattern_no_match() {
        assert!(!matches_pattern("exact_match", "other_tool"));
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
        let decision = engine
            .check("create_issue", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn test_deny_policy() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "delete_repo".to_string(),
                action: PolicyAction::Deny,
                rate_limit: None,
            }],
        };
        let engine = PolicyEngine::new(config);
        let decision = engine
            .check("delete_repo", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(decision.is_denied());
    }

    #[tokio::test]
    async fn test_require_approval_policy() {
        let engine = PolicyEngine::new(create_test_policy_config());
        let decision = engine
            .check("update_record", &serde_json::json!({}))
            .await
            .unwrap();
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
        let decision = engine
            .check("any_tool", &serde_json::json!({}))
            .await
            .unwrap();
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
        let decision = engine
            .check("any_tool", &serde_json::json!({}))
            .await
            .unwrap();
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
        let decision = engine
            .check("any_tool", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(decision.requires_approval());
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test_tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("2/min".to_string()),
            }],
        };
        let engine = PolicyEngine::new(config);

        let d1 = engine
            .check("test_tool", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(d1.is_allowed());
        let d2 = engine
            .check("test_tool", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(d2.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiting_exceeded() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test_limited".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        };
        let engine = PolicyEngine::new(config);

        let d1 = engine
            .check("test_limited", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(d1.is_allowed());

        let d2 = engine
            .check("test_limited", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(d2, PolicyDecision::RateLimitExceeded { .. }));
    }

    #[test]
    fn test_check_rate_limit_standalone() {
        let config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test_tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        };
        let engine = PolicyEngine::new(config);

        assert!(engine.check_rate_limit("test_tool").is_ok());
        let result = engine.check_rate_limit("test_tool");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_rate_limit_no_matching_pattern() {
        let engine = PolicyEngine::default();
        assert!(engine.check_rate_limit("any_tool").is_ok());
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
                match_pattern: "test_*".to_string(),
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
                match_pattern: "test_tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("1/min".to_string()),
            }],
        };
        let mut engine = PolicyEngine::new(config);

        let _ = engine.check("test_tool", &serde_json::json!({})).await;

        let new_config = PolicyConfig {
            defaults: PolicyDefaults {
                action: PolicyAction::Allow,
            },
            rules: vec![PolicyRule {
                match_pattern: "test_tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("10/min".to_string()),
            }],
        };
        engine.update_config(new_config);

        let d = engine
            .check("test_tool", &serde_json::json!({}))
            .await
            .unwrap();
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
        assert!(parse_rate_limit("invalid").is_err());
        assert!(parse_rate_limit("abc/min").is_err());
        assert!(parse_rate_limit("10/invalid_duration").is_err());
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
            tool: "test_tool".into(),
            args: serde_json::json!({"key": "val"}),
        };
        assert!(!d.is_allowed());
        assert!(!d.is_denied());
        assert!(d.requires_approval());
    }

    #[test]
    fn test_policy_decision_rate_limit_exceeded() {
        let d = PolicyDecision::RateLimitExceeded {
            tool: "test_tool".into(),
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
                match_pattern: "approve_tool".to_string(),
                action: PolicyAction::RequireApproval,
                rate_limit: None,
            }],
        };
        let engine = PolicyEngine::new(config);
        let args = serde_json::json!({"key": "value"});

        let decision = engine.check("approve_tool", &args).await.unwrap();
        match decision {
            PolicyDecision::RequireApproval { tool, args: a } => {
                assert_eq!(tool, "approve_tool");
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
                match_pattern: "blocked_tool".to_string(),
                action: PolicyAction::Deny,
                rate_limit: None,
            }],
        };
        let engine = PolicyEngine::new(config);
        let decision = engine
            .check("blocked_tool", &serde_json::json!({}))
            .await
            .unwrap();
        match decision {
            PolicyDecision::Deny { reason } => {
                assert!(reason.contains("blocked_tool"));
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
                match_pattern: "test_tool".to_string(),
                action: PolicyAction::Allow,
                rate_limit: Some("invalid".to_string()),
            }],
        };
        let _engine = PolicyEngine::new(config);
    }
}
