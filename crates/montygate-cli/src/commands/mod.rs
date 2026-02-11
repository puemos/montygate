pub mod config;
pub mod run;
pub mod server;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "montygate")]
#[command(about = "MCP server that aggregates downstream MCP servers")]
#[command(version)]
pub struct Cli {
    /// Path to configuration file (default: ~/.montygate/config.toml)
    #[arg(short, long, value_name = "FILE", global = true)]
    pub config: Option<PathBuf>,

    /// Log level
    #[arg(short, long, default_value = "info", global = true)]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the MCP server
    Run {
        /// Transport type (stdio, sse, http)
        #[arg(short, long, default_value = "stdio")]
        transport: String,

        /// Host for SSE/HTTP transport
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port for SSE/HTTP transport
        #[arg(long, default_value = "8080")]
        port: u16,

        /// Test configuration and exit
        #[arg(long)]
        test_config: bool,

        /// List available tools and exit
        #[arg(long)]
        list_tools: bool,
    },

    /// Manage MCP servers
    #[command(subcommand)]
    Server(ServerCommand),

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Add a new MCP server
    Add {
        /// Server name
        name: String,

        /// Transport type (stdio, sse, http)
        #[arg(short, long)]
        transport: String,

        /// For stdio transport: command to execute
        #[arg(long)]
        command: Option<String>,

        /// For stdio transport: command arguments (comma-separated)
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        args: Vec<String>,

        /// For stdio transport: environment variables (KEY=VALUE, comma-separated)
        #[arg(long, value_delimiter = ',')]
        env: Vec<String>,

        /// For SSE/HTTP transport: URL
        #[arg(long)]
        url: Option<String>,
    },

    /// Remove an MCP server
    Remove {
        /// Server name
        name: String,

        /// Force removal without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// List configured MCP servers
    List {
        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,
    },

    /// Edit an MCP server configuration
    Edit {
        /// Server name
        name: String,

        /// New server name
        #[arg(long)]
        new_name: Option<String>,

        /// Transport type (stdio, sse, http)
        #[arg(short, long)]
        transport: Option<String>,

        /// For stdio transport: command to execute
        #[arg(long)]
        command: Option<String>,

        /// For stdio transport: command arguments (comma-separated)
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        args: Vec<String>,

        /// For stdio transport: environment variables (KEY=VALUE, comma-separated)
        #[arg(long, value_delimiter = ',')]
        env: Vec<String>,

        /// For SSE/HTTP transport: URL
        #[arg(long)]
        url: Option<String>,
    },

    /// Test connectivity to an MCP server
    Test {
        /// Server name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Initialize a new configuration file
    Init {
        /// Force overwrite if config already exists
        #[arg(short, long)]
        force: bool,

        /// Server name
        #[arg(short, long, default_value = "montygate")]
        name: String,
    },

    /// Show current configuration
    Show {
        /// Output format (toml, json)
        #[arg(short, long, default_value = "toml")]
        format: String,
    },

    /// Validate configuration file
    Validate,
}
