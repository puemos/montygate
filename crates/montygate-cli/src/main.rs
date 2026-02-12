use anyhow::Result;
use clap::Parser;
use tracing::info;

mod commands;
mod config;

use commands::{Cli, Commands, ConfigCommand, ServerCommand};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(&args.log_level)
        .with_writer(std::io::stderr)
        .init();

    info!("Starting Montygate v{}", env!("CARGO_PKG_VERSION"));

    match args.command {
        Commands::Run {
            transport,
            host,
            port,
            test_config,
            list_tools,
        } => {
            commands::run::run_server(
                args.config,
                transport,
                host,
                port,
                test_config,
                list_tools,
            )
            .await?;
        }
        Commands::Server(server_cmd) => {
            let config_path = args.config;
            match server_cmd {
            ServerCommand::Add {
                name,
                transport,
                command,
                args: server_args,
                env,
                url,
            } => {
                commands::server::add_server(config_path, name, transport, command, server_args, env, url)
                    .await?;
            }
            ServerCommand::Remove { name, force } => {
                commands::server::remove_server(config_path, name, force).await?;
            }
            ServerCommand::List { verbose } => {
                commands::server::list_servers(config_path, verbose).await?;
            }
            ServerCommand::Edit {
                name,
                new_name,
                transport,
                command,
                args: server_args,
                env,
                url,
            } => {
                commands::server::edit_server(
                    config_path, name, new_name, transport, command, server_args, env, url,
                )
                .await?;
            }
            ServerCommand::Test { name } => {
                commands::server::test_server(config_path, name).await?;
            }
        }},
        Commands::Config(config_cmd) => {
            let config_path = args.config;
            match config_cmd {
            ConfigCommand::Init { force, name } => {
                commands::config::init_config(config_path, force, name).await?;
            }
            ConfigCommand::Show { format } => {
                commands::config::show_config(config_path, format).await?;
            }
            ConfigCommand::Validate => {
                commands::config::validate_config(config_path).await?;
            }
        }},
    }

    Ok(())
}
