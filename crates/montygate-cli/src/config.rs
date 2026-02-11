use anyhow::{Context, Result};
use montygate_core::types::MontyGateConfig;
use std::path::{Path, PathBuf};
use tracing::info;

/// Get the default configuration directory
pub fn default_config_dir() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join(".montygate"))
        .unwrap_or_else(|| PathBuf::from(".montygate"))
}

/// Get the default configuration file path
pub fn default_config_path() -> PathBuf {
    default_config_dir().join("config.toml")
}

/// Get the configuration file path, using provided path or default
pub fn get_config_path(path: Option<PathBuf>) -> Result<PathBuf> {
    Ok(path.unwrap_or_else(default_config_path))
}

/// Ensure the configuration directory exists
pub fn ensure_config_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
    }
    Ok(())
}

/// Load configuration from file
pub fn load_config(path: Option<PathBuf>) -> Result<MontyGateConfig> {
    let path = get_config_path(path)?;
    
    if !path.exists() {
        anyhow::bail!("Configuration file not found at {:?}", path);
    }

    info!("Loading configuration from {:?}", path);
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config file: {:?}", path))?;
    
    let config: MontyGateConfig = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {:?}", path))?;
    
    Ok(config)
}

/// Save configuration to file
pub async fn save_config(config: &MontyGateConfig, path: &Path) -> Result<()> {
    ensure_config_dir(path)?;
    
    let toml_str = toml::to_string_pretty(config)
        .context("Failed to serialize configuration")?;
    
    tokio::fs::write(path, toml_str)
        .await
        .with_context(|| format!("Failed to write config file: {:?}", path))?;
    
    Ok(())
}

/// Check if configuration file exists
#[allow(dead_code)]
pub fn config_exists(path: Option<PathBuf>) -> bool {
    get_config_path(path).map(|p| p.exists()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use montygate_core::types::MontyGateConfig;
    use tempfile::TempDir;

    #[test]
    fn test_default_config_dir() {
        let dir = default_config_dir();
        let dir_str = dir.to_string_lossy();
        assert!(dir_str.contains(".montygate"));
    }

    #[test]
    fn test_default_config_path() {
        let path = default_config_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("config.toml"));
    }

    #[test]
    fn test_get_config_path_with_custom() {
        let custom = PathBuf::from("/tmp/custom.toml");
        let result = get_config_path(Some(custom.clone())).unwrap();
        assert_eq!(result, custom);
    }

    #[test]
    fn test_get_config_path_default() {
        let result = get_config_path(None).unwrap();
        assert!(result.to_string_lossy().contains("config.toml"));
    }

    #[test]
    fn test_ensure_config_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("config.toml");
        ensure_config_dir(&nested).unwrap();
        assert!(tmp.path().join("a").join("b").exists());
    }

    #[test]
    fn test_load_config_missing_file() {
        let result = load_config(Some(PathBuf::from("/tmp/nonexistent_montygate_test.toml")));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_valid() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let config = MontyGateConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&path, toml_str).unwrap();

        let loaded = load_config(Some(path)).unwrap();
        assert_eq!(loaded.server.name, "montygate");
    }

    #[test]
    fn test_load_config_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(&path, "this is not valid toml {{{").unwrap();

        let result = load_config(Some(path));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_save_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("saved.toml");

        let config = MontyGateConfig::default();
        save_config(&config, &path).await.unwrap();

        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("montygate"));
    }

    #[tokio::test]
    async fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("roundtrip.toml");

        let config = MontyGateConfig {
            server: montygate_core::types::ServerInfo {
                name: "test_server".to_string(),
                version: "2.0.0".to_string(),
            },
            servers: vec![montygate_core::types::ServerConfig {
                name: "github".to_string(),
                transport: montygate_core::TransportConfig::Sse {
                    url: "http://localhost:3000".to_string(),
                },
            }],
            limits: montygate_core::ResourceLimits::default(),
            policy: montygate_core::PolicyConfig::default(),
        };

        save_config(&config, &path).await.unwrap();
        let loaded = load_config(Some(path)).unwrap();

        assert_eq!(loaded.server.name, "test_server");
        assert_eq!(loaded.server.version, "2.0.0");
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "github");
    }

    #[test]
    fn test_config_exists_true() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("exists.toml");
        std::fs::write(&path, "").unwrap();
        assert!(config_exists(Some(path)));
    }

    #[test]
    fn test_config_exists_false() {
        assert!(!config_exists(Some(PathBuf::from(
            "/tmp/nonexistent_montygate.toml"
        ))));
    }
}
