use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[cfg(target_os = "windows")]
mod windows_ble;

#[derive(Debug, Parser)]
#[command(
    name = "pitboss-tools",
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

    /// Connect to the discovered Pit Boss briefly and list its GATT services.
    /// This sends no smoker commands.
    Inspect {
        /// Number of seconds to scan before selecting the smoker.
        #[arg(short, long, default_value_t = 10)]
        seconds: u64,
    },

    /// Display pushed temperature reports without sending smoker commands.
    Monitor {
        /// Number of seconds to listen after connecting.
        #[arg(short, long, default_value_t = 300)]
        seconds: u64,
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
        Command::Inspect { seconds } => inspect(Duration::from_secs(seconds)).await,
        Command::Monitor { seconds } => monitor(Duration::from_secs(seconds)).await,
    }
}

#[cfg(target_os = "windows")]
async fn scan(duration: Duration, all: bool) -> Result<()> {
    windows_ble::scan(duration, all).await
}

#[cfg(target_os = "windows")]
async fn inspect(duration: Duration) -> Result<()> {
    windows_ble::inspect(duration).await
}

#[cfg(target_os = "windows")]
async fn monitor(duration: Duration) -> Result<()> {
    windows_ble::monitor(duration).await
}

#[cfg(not(target_os = "windows"))]
async fn scan(_duration: Duration, _all: bool) -> Result<()> {
    anyhow::bail!(
        "BLE scanning currently requires the Windows build; run pitboss-tools.exe on Windows"
    )
}

#[cfg(not(target_os = "windows"))]
async fn inspect(_duration: Duration) -> Result<()> {
    anyhow::bail!(
        "BLE inspection currently requires the Windows build; run pitboss-tools.exe on Windows"
    )
}

#[cfg(not(target_os = "windows"))]
async fn monitor(_duration: Duration) -> Result<()> {
    anyhow::bail!(
        "BLE monitoring currently requires the Windows build; run pitboss-tools.exe on Windows"
    )
}
