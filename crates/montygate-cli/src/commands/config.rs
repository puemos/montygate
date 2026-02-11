use crate::config::{get_config_path, load_config, save_config, ensure_config_dir};
use anyhow::{Context, Result, bail};
use montygate_core::types::MontyGateConfig;
use tracing::info;

pub async fn init_config(
    config_path: Option<std::path::PathBuf>,
    force: bool,
    name: String,
) -> Result<()> {
    let path = get_config_path(config_path)?;

    // Check if config already exists
    if path.exists() && !force {
        bail!(
            "Configuration file already exists at {:?}\nUse --force to overwrite",
            path
        );
    }

    // Ensure config directory exists
    ensure_config_dir(&path)?;

    // Create default config
    let config = MontyGateConfig {
        server: montygate_core::types::ServerInfo {
            name: name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        servers: vec![],
        limits: Default::default(),
        policy: Default::default(),
    };

    save_config(&config, &path).await?;
    
    info!("Initialized configuration at {:?}", path);
    println!("Configuration file created at: {:?}", path);
    println!("Use 'montygate server add <name>' to add MCP servers.");
    
    Ok(())
}

pub async fn show_config(
    config_path: Option<std::path::PathBuf>,
    format: String,
) -> Result<()> {
    let path = get_config_path(config_path)?;
    let config = load_config(Some(path))?;

    match format.as_str() {
        "toml" => {
            let toml_str = toml::to_string_pretty(&config)
                .context("Failed to serialize configuration")?;
            println!("{}", toml_str);
        }
        "json" => {
            let json_str = serde_json::to_string_pretty(&config)
                .context("Failed to serialize configuration")?;
            println!("{}", json_str);
        }
        _ => bail!("Unknown format: {}. Use 'toml' or 'json'", format),
    }

    Ok(())
}

pub async fn validate_config(config_path: Option<std::path::PathBuf>) -> Result<()> {
    let path = get_config_path(config_path)?;

    match load_config(Some(path.clone())) {
        Ok(config) => {
            println!("Configuration is valid!");
            println!();
            println!("Server: {} v{}", config.server.name, config.server.version);
            println!("Configured servers: {}", config.servers.len());

            if !config.servers.is_empty() {
                println!("\nServers:");
                for server in &config.servers {
                    println!("  - {}", server.name);
                }
            }

            Ok(())
        }
        Err(e) => {
            bail!("Configuration validation failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // === init_config ===

    #[tokio::test]
    async fn test_init_config_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let result = init_config(Some(path.clone()), false, "test".to_string()).await;
        assert!(result.is_ok());
        assert!(path.exists());

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("test"));
    }

    #[tokio::test]
    async fn test_init_config_no_overwrite() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        // Create first
        init_config(Some(path.clone()), false, "first".to_string())
            .await
            .unwrap();

        // Try to create again without force
        let result = init_config(Some(path), false, "second".to_string()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_init_config_force_overwrite() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        init_config(Some(path.clone()), false, "first".to_string())
            .await
            .unwrap();

        let result = init_config(Some(path.clone()), true, "second".to_string()).await;
        assert!(result.is_ok());

        let config = load_config(Some(path)).unwrap();
        assert_eq!(config.server.name, "second");
    }

    #[tokio::test]
    async fn test_init_config_nested_dir() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a").join("b").join("config.toml");

        let result = init_config(Some(path.clone()), false, "nested".to_string()).await;
        assert!(result.is_ok());
        assert!(path.exists());
    }

    // === show_config ===

    #[tokio::test]
    async fn test_show_config_toml() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        init_config(Some(path.clone()), false, "showme".to_string())
            .await
            .unwrap();

        let result = show_config(Some(path), "toml".to_string()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_show_config_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        init_config(Some(path.clone()), false, "showme".to_string())
            .await
            .unwrap();

        let result = show_config(Some(path), "json".to_string()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_show_config_invalid_format() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        init_config(Some(path.clone()), false, "test".to_string())
            .await
            .unwrap();

        let result = show_config(Some(path), "yaml".to_string()).await;
        assert!(result.is_err());
    }

    // === validate_config ===

    #[tokio::test]
    async fn test_validate_config_valid() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        init_config(Some(path.clone()), false, "valid".to_string())
            .await
            .unwrap();

        let result = validate_config(Some(path)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_config_missing_file() {
        let result =
            validate_config(Some(std::path::PathBuf::from("/tmp/nonexistent_mg.toml"))).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_config_with_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let config = montygate_core::MontyGateConfig {
            server: montygate_core::types::ServerInfo {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
            },
            servers: vec![
                montygate_core::types::ServerConfig {
                    name: "github".to_string(),
                    transport: montygate_core::TransportConfig::Sse {
                        url: "http://localhost:3000".into(),
                    },
                },
                montygate_core::types::ServerConfig {
                    name: "slack".to_string(),
                    transport: montygate_core::TransportConfig::Sse {
                        url: "http://localhost:3001".into(),
                    },
                },
            ],
            limits: Default::default(),
            policy: Default::default(),
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&path, toml_str).unwrap();

        let result = validate_config(Some(path)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_config_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(&path, "not valid toml {{{").unwrap();

        let result = validate_config(Some(path)).await;
        assert!(result.is_err());
    }
}
