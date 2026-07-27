use std::time::Duration;

use anyhow::{Context, Result};
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter},
    platform::Manager,
};
use tokio::time::sleep;

const PIT_BOSS_NAME_MARKERS: &[&str] = &["pit boss", "pitboss", "pbv4", "dansons", "mongoose"];

pub async fn scan(duration: Duration, show_all: bool) -> Result<()> {
    let manager = Manager::new()
        .await
        .context("could not initialize Windows Bluetooth")?;
    let adapters = manager
        .adapters()
        .await
        .context("could not enumerate Bluetooth adapters")?;

    if adapters.is_empty() {
        anyhow::bail!("Windows reported no Bluetooth adapters");
    }

    for adapter in adapters {
        let adapter_name = adapter
            .adapter_info()
            .await
            .unwrap_or_else(|_| "unknown adapter".to_owned());
        println!(
            "Scanning on {adapter_name} for {} seconds...",
            duration.as_secs()
        );

        adapter
            .start_scan(ScanFilter::default())
            .await
            .context("could not start BLE scan")?;
        sleep(duration).await;

        let peripherals = adapter
            .peripherals()
            .await
            .context("could not read discovered BLE devices")?;
        adapter.stop_scan().await.ok();

        let mut shown = 0;
        for peripheral in peripherals {
            let Some(properties) = peripheral.properties().await? else {
                continue;
            };
            let name = properties.local_name.as_deref().unwrap_or("(unnamed)");

            if !show_all && !looks_like_pit_boss(name) {
                continue;
            }

            shown += 1;
            println!(
                "{}  name={name:?}  rssi={}  connected={}",
                peripheral.address(),
                properties
                    .rssi
                    .map(|value| format!("{value} dBm"))
                    .unwrap_or_else(|| "unknown".to_owned()),
                peripheral.is_connected().await.unwrap_or(false),
            );

            if !properties.services.is_empty() {
                let services = properties
                    .services
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  advertised services: {services}");
            }
        }

        if shown == 0 {
            if show_all {
                println!("No BLE peripherals were discovered.");
            } else {
                println!(
                    "No obvious Pit Boss device was found. Retry with `scan --all` while the smoker controller is powered on."
                );
            }
        }
    }

    Ok(())
}

fn looks_like_pit_boss(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    PIT_BOSS_NAME_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::looks_like_pit_boss;

    #[test]
    fn recognizes_likely_names_case_insensitively() {
        assert!(looks_like_pit_boss("PIT BOSS 1234"));
        assert!(looks_like_pit_boss("PBV4DX"));
        assert!(looks_like_pit_boss("Dansons Grill"));
        assert!(!looks_like_pit_boss("Wireless Headphones"));
    }
}
