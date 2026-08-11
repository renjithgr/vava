//! The `vava` binary: parse arguments and run the CLI.

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();
    let cli = vava_cli::Cli::parse();
    vava_cli::run(cli).await
}
