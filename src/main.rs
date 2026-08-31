use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod codec;
#[cfg(target_os = "windows")]
mod config;
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

    /// Inspect or enable the smoker's optional local HTTP RPC service over BLE.
    HttpConfig {
        /// Enable the HTTP service if the firmware exposes an `http` config.
        /// This writes flash and asks the smoker to reboot.
        #[arg(long)]
        enable: bool,
    },

    /// Set the factory controller's target temperature in Fahrenheit.
    SetTemperature {
        /// Factory-controller target, from 130°F through 420°F.
        temperature: u16,
    },

    /// Run the local dashboard and optional guarded grate-temperature controller.
    Serve {
        /// Local address for the dashboard and JSON API.
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: SocketAddr,

        /// SQLite database path for temperature history.
        #[arg(long, default_value = "pitboss.sqlite3")]
        database: PathBuf,

        /// Desired grate/ambient temperature in Fahrenheit.
        #[arg(long, default_value_t = 225)]
        target: u16,

        /// Start the guarded setpoint controller enabled.
        #[arg(long)]
        control: bool,
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
        Command::HttpConfig { enable } => http_config(enable).await,
        Command::SetTemperature { temperature } => set_temperature(temperature).await,
        Command::Serve {
            bind,
            database,
            target,
            control,
        } => serve(bind, database, target, control).await,
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

#[cfg(target_os = "windows")]
async fn http_config(enable: bool) -> Result<()> {
    windows_ble::http_config(enable).await
}

#[cfg(target_os = "windows")]
async fn set_temperature(temperature: u16) -> Result<()> {
    windows_ble::set_temperature(temperature).await
}

#[cfg(target_os = "windows")]
async fn serve(bind: SocketAddr, database: PathBuf, target: u16, control: bool) -> Result<()> {
    windows_ble::serve(windows_ble::ServeOptions {
        bind,
        database,
        target_f: target,
        control_enabled: control,
    })
    .await
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

#[cfg(not(target_os = "windows"))]
async fn http_config(_enable: bool) -> Result<()> {
    anyhow::bail!(
        "BLE control currently requires the Windows build; run pitboss-tools.exe on Windows"
    )
}

#[cfg(not(target_os = "windows"))]
async fn set_temperature(_temperature: u16) -> Result<()> {
    anyhow::bail!(
        "BLE control currently requires the Windows build; run pitboss-tools.exe on Windows"
    )
}

#[cfg(not(target_os = "windows"))]
async fn serve(_bind: SocketAddr, _database: PathBuf, _target: u16, _control: bool) -> Result<()> {
    anyhow::bail!(
        "the dashboard currently requires the Windows build; run pitboss-tools.exe on Windows"
    )
}
