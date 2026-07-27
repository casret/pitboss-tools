use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[cfg(target_os = "windows")]
mod windows_ble;

#[derive(Debug, Parser)]
#[command(
    name = "pitboss-pid",
    version,
    about = "Local Pit Boss smoker monitor and controller"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scan for nearby BLE devices without connecting to them.
    Scan {
        /// Number of seconds to scan.
        #[arg(short, long, default_value_t = 15)]
        seconds: u64,

        /// Show every BLE device, not just likely Pit Boss devices.
        #[arg(long)]
        all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pitboss_pid=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Scan { seconds, all } => scan(Duration::from_secs(seconds), all).await,
    }
}

#[cfg(target_os = "windows")]
async fn scan(duration: Duration, all: bool) -> Result<()> {
    windows_ble::scan(duration, all).await
}

#[cfg(not(target_os = "windows"))]
async fn scan(_duration: Duration, _all: bool) -> Result<()> {
    anyhow::bail!(
        "BLE scanning currently requires the Windows build; run pitboss-pid.exe on Windows"
    )
}
