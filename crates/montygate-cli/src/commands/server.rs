use crate::config::{get_config_path, load_config, save_config};
use anyhow::{Result, bail};
use montygate_core::types::{ServerConfig, TransportConfig};
use std::collections::HashMap;
use tracing::info;

pub async fn add_server(
    config_path: Option<std::path::PathBuf>,
    name: String,
    transport_type: String,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    url: Option<String>,
) -> Result<()> {
    let path = get_config_path(config_path)?;
    let mut config = load_config(Some(path.clone())).unwrap_or_default();

    // Check if server already exists
    if config.servers.iter().any(|s| s.name == name) {
        bail!("Server '{}' already exists. Use 'server edit' to modify it.", name);
    }

    // Build transport config based on type
    let transport = match transport_type.as_str() {
        "stdio" => {
            let cmd = command.ok_or_else(|| {
                anyhow::anyhow!("--command is required for stdio transport")
            })?;
            
            // Parse environment variables
            let env_map = parse_env_vars(&env)?;
            
            TransportConfig::Stdio {
                command: cmd,
                args: args.clone(),
                env: env_map,
            }
        }
        "sse" => {
            let url = url.ok_or_else(|| {
                anyhow::anyhow!("--url is required for sse transport")
            })?;
            TransportConfig::Sse { url }
        }
        "http" | "streamable_http" => {
            let url = url.ok_or_else(|| {
                anyhow::anyhow!("--url is required for http transport")
            })?;
            TransportConfig::StreamableHttp { url }
        }
        _ => bail!("Unknown transport type: {}. Use 'stdio', 'sse', or 'http'", transport_type),
    };

    let server_config = ServerConfig { name: name.clone(), transport };
    config.servers.push(server_config);

    save_config(&config, &path).await?;
    info!("Added server '{}' to configuration", name);
    
    Ok(())
}

pub async fn remove_server(
    config_path: Option<std::path::PathBuf>,
    name: String,
    force: bool,
) -> Result<()> {
    let path = get_config_path(config_path)?;
    let mut config = load_config(Some(path.clone()))?;

    let initial_len = config.servers.len();
    config.servers.retain(|s| s.name != name);

    if config.servers.len() == initial_len {
        bail!("Server '{}' not found", name);
    }

    if !force {
        // In a real implementation, we might want to prompt for confirmation
        // For now, we just proceed
        info!("Removing server '{}'", name);
    }

    save_config(&config, &path).await?;
    info!("Removed server '{}' from configuration", name);
    
    Ok(())
}

pub async fn list_servers(
    config_path: Option<std::path::PathBuf>,
    verbose: bool,
) -> Result<()> {
    let path = get_config_path(config_path)?;
    let config = load_config(Some(path))?;

    if config.servers.is_empty() {
        println!("No servers configured.");
        println!("Use 'montygate server add <name>' to add a server.");
        return Ok(());
    }

    println!("Configured MCP servers:");
    println!();

    for server in &config.servers {
        println!("  {}:", server.name);
        
        match &server.transport {
            TransportConfig::Stdio { command, args, env } => {
                println!("    Type: stdio");
                println!("    Command: {}", command);
                if !args.is_empty() {
                    println!("    Args: {}", args.join(" "));
                }
                if !env.is_empty() && verbose {
                    println!("    Environment:");
                    for key in env.keys() {
                        println!("      {}: <hidden>", key);
                    }
                }
            }
            TransportConfig::Sse { url } => {
                println!("    Type: sse");
                println!("    URL: {}", url);
            }
            TransportConfig::StreamableHttp { url } => {
                println!("    Type: http");
                println!("    URL: {}", url);
            }
        }
        println!();
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn edit_server(
    config_path: Option<std::path::PathBuf>,
    name: String,
    new_name: Option<String>,
    transport_type: Option<String>,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    url: Option<String>,
) -> Result<()> {
    let path = get_config_path(config_path)?;
    let mut config = load_config(Some(path.clone()))?;

    let server_idx = config
        .servers
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("Server '{}' not found", name))?;

    // Update name if provided
    if let Some(new_name) = new_name {
        if config.servers.iter().any(|s| s.name == new_name && s.name != name) {
            bail!("Server '{}' already exists", new_name);
        }
        config.servers[server_idx].name = new_name.clone();
        info!("Renamed server '{}' to '{}'", name, new_name);
    }

    // Update transport if provided
    if let Some(transport_type) = transport_type {
        let transport = match transport_type.as_str() {
            "stdio" => {
                let cmd = command.ok_or_else(|| {
                    anyhow::anyhow!("--command is required for stdio transport")
                })?;
                
                let env_map = parse_env_vars(&env)?;
                
                TransportConfig::Stdio {
                    command: cmd,
                    args: if args.is_empty() {
                        config.servers[server_idx].transport.args().cloned().unwrap_or_default()
                    } else {
                        args
                    },
                    env: env_map,
                }
            }
            "sse" => {
                let url = url.ok_or_else(|| {
                    anyhow::anyhow!("--url is required for sse transport")
                })?;
                TransportConfig::Sse { url }
            }
            "http" | "streamable_http" => {
                let url = url.ok_or_else(|| {
                    anyhow::anyhow!("--url is required for http transport")
                })?;
                TransportConfig::StreamableHttp { url }
            }
            _ => bail!("Unknown transport type: {}", transport_type),
        };

        config.servers[server_idx].transport = transport;
        info!("Updated transport for server '{}'", config.servers[server_idx].name);
    }

    save_config(&config, &path).await?;
    info!("Updated server configuration");
    
    Ok(())
}

pub async fn test_server(
    _config_path: Option<std::path::PathBuf>,
    _name: String,
) -> Result<()> {
    // This would test connectivity to the server
    // For now, just a placeholder
    println!("Server connectivity test not yet implemented");
    Ok(())
}

fn parse_env_vars(env: &[String]) -> Result<HashMap<String, String>> {
    let mut env_map = HashMap::new();

    for env_var in env {
        let parts: Vec<&str> = env_var.splitn(2, '=').collect();
        if parts.len() != 2 {
            bail!("Invalid environment variable format: {}. Expected KEY=VALUE", env_var);
        }
        env_map.insert(parts[0].to_string(), parts[1].to_string());
    }

    Ok(env_map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use montygate_core::types::MontyGateConfig;
    use tempfile::TempDir;

    fn setup_config(tmp: &TempDir) -> std::path::PathBuf {
        let path = tmp.path().join("config.toml");
        let config = MontyGateConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&path, toml_str).unwrap();
        path
    }

    // === parse_env_vars ===

    #[test]
    fn test_parse_env_vars_valid() {
        let vars = vec!["KEY=value".to_string(), "TOKEN=abc123".to_string()];
        let result = parse_env_vars(&vars).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("KEY").unwrap(), "value");
        assert_eq!(result.get("TOKEN").unwrap(), "abc123");
    }

    #[test]
    fn test_parse_env_vars_value_with_equals() {
        let vars = vec!["URL=http://host?key=val".to_string()];
        let result = parse_env_vars(&vars).unwrap();
        assert_eq!(result.get("URL").unwrap(), "http://host?key=val");
    }

    #[test]
    fn test_parse_env_vars_empty() {
        let result = parse_env_vars(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_env_vars_invalid() {
        let vars = vec!["NO_EQUALS_SIGN".to_string()];
        let result = parse_env_vars(&vars);
        assert!(result.is_err());
    }

    // === add_server ===

    #[tokio::test]
    async fn test_add_server_stdio() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = add_server(
            Some(path.clone()),
            "github".to_string(),
            "stdio".to_string(),
            Some("npx".to_string()),
            vec!["-y".to_string(), "server-github".to_string()],
            vec!["GITHUB_TOKEN=test123".to_string()],
            None,
        )
        .await;

        assert!(result.is_ok());

        let config = crate::config::load_config(Some(path)).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "github");
        assert_eq!(
            config.servers[0].transport.command(),
            Some(&"npx".to_string())
        );
    }

    #[tokio::test]
    async fn test_add_server_sse() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = add_server(
            Some(path.clone()),
            "remote".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://localhost:3000/sse".to_string()),
        )
        .await;

        assert!(result.is_ok());

        let config = crate::config::load_config(Some(path)).unwrap();
        assert_eq!(config.servers[0].transport.url(), Some(&"http://localhost:3000/sse".to_string()));
    }

    #[tokio::test]
    async fn test_add_server_http() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = add_server(
            Some(path.clone()),
            "http_srv".to_string(),
            "http".to_string(),
            None,
            vec![],
            vec![],
            Some("http://localhost:8080".to_string()),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_add_server_duplicate() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "dup".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = add_server(
            Some(path),
            "dup".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://y".to_string()),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_server_stdio_missing_command() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = add_server(
            Some(path),
            "bad".to_string(),
            "stdio".to_string(),
            None, // Missing command
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_server_sse_missing_url() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = add_server(
            Some(path),
            "bad".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            None, // Missing URL
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_server_unknown_transport() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = add_server(
            Some(path),
            "bad".to_string(),
            "grpc".to_string(),
            None,
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_err());
    }

    // === remove_server ===

    #[tokio::test]
    async fn test_remove_server() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "to_remove".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = remove_server(Some(path.clone()), "to_remove".to_string(), false).await;
        assert!(result.is_ok());

        let config = crate::config::load_config(Some(path)).unwrap();
        assert!(config.servers.is_empty());
    }

    #[tokio::test]
    async fn test_remove_server_not_found() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = remove_server(Some(path), "ghost".to_string(), false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_server_force() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = remove_server(Some(path), "srv".to_string(), true).await;
        assert!(result.is_ok());
    }

    // === test_server ===

    #[tokio::test]
    async fn test_test_server_placeholder() {
        let result = test_server(None, "any".to_string()).await;
        assert!(result.is_ok());
    }

    // === edit_server ===

    #[tokio::test]
    async fn test_edit_server_rename() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "old_name".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path.clone()),
            "old_name".to_string(),
            Some("new_name".to_string()),
            None,
            None,
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_ok());

        let config = crate::config::load_config(Some(path)).unwrap();
        assert_eq!(config.servers[0].name, "new_name");
    }

    #[tokio::test]
    async fn test_edit_server_not_found() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = edit_server(
            Some(path),
            "ghost".to_string(),
            Some("new".to_string()),
            None,
            None,
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_err());
    }

    // === list_servers ===

    #[tokio::test]
    async fn test_list_servers_empty() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        let result = list_servers(Some(path), false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_list_servers_with_servers() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "github".to_string(),
            "stdio".to_string(),
            Some("npx".to_string()),
            vec!["-y".to_string(), "server-github".to_string()],
            vec!["GITHUB_TOKEN=test".to_string()],
            None,
        )
        .await
        .unwrap();

        add_server(
            Some(path.clone()),
            "remote".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://localhost:3000".to_string()),
        )
        .await
        .unwrap();

        add_server(
            Some(path.clone()),
            "api".to_string(),
            "http".to_string(),
            None,
            vec![],
            vec![],
            Some("http://localhost:8080".to_string()),
        )
        .await
        .unwrap();

        // Non-verbose
        let result = list_servers(Some(path.clone()), false).await;
        assert!(result.is_ok());

        // Verbose
        let result = list_servers(Some(path), true).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_edit_server_change_transport_to_http() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path.clone()),
            "srv".to_string(),
            None,
            Some("http".to_string()),
            None,
            vec![],
            vec![],
            Some("http://new-url".to_string()),
        )
        .await;

        assert!(result.is_ok());

        let config = crate::config::load_config(Some(path)).unwrap();
        assert_eq!(
            config.servers[0].transport.url(),
            Some(&"http://new-url".to_string())
        );
        assert_eq!(config.servers[0].transport.transport_type(), "http");
    }

    #[tokio::test]
    async fn test_edit_server_change_transport_to_stdio() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path.clone()),
            "srv".to_string(),
            None,
            Some("stdio".to_string()),
            Some("node".to_string()),
            vec!["server.js".to_string()],
            vec!["TOKEN=abc".to_string()],
            None,
        )
        .await;

        assert!(result.is_ok());

        let config = crate::config::load_config(Some(path)).unwrap();
        assert_eq!(
            config.servers[0].transport.command(),
            Some(&"node".to_string())
        );
    }

    #[tokio::test]
    async fn test_edit_server_change_transport_to_sse() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "stdio".to_string(),
            Some("echo".to_string()),
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path.clone()),
            "srv".to_string(),
            None,
            Some("sse".to_string()),
            None,
            vec![],
            vec![],
            Some("http://new-sse".to_string()),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_edit_server_rename_conflict() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "a".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://a".to_string()),
        )
        .await
        .unwrap();

        add_server(
            Some(path.clone()),
            "b".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://b".to_string()),
        )
        .await
        .unwrap();

        // Try to rename "a" to "b" (conflict)
        let result = edit_server(
            Some(path),
            "a".to_string(),
            Some("b".to_string()),
            None,
            None,
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_edit_server_unknown_transport() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path),
            "srv".to_string(),
            None,
            Some("grpc".to_string()),
            None,
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_edit_server_stdio_missing_command() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path),
            "srv".to_string(),
            None,
            Some("stdio".to_string()),
            None, // Missing command
            vec![],
            vec![],
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_edit_server_sse_missing_url() {
        let tmp = TempDir::new().unwrap();
        let path = setup_config(&tmp);

        add_server(
            Some(path.clone()),
            "srv".to_string(),
            "sse".to_string(),
            None,
            vec![],
            vec![],
            Some("http://x".to_string()),
        )
        .await
        .unwrap();

        let result = edit_server(
            Some(path),
            "srv".to_string(),
            None,
            Some("sse".to_string()),
            None,
            vec![],
            vec![],
            None, // Missing URL
        )
        .await;

        assert!(result.is_err());
    }
}
