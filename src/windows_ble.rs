use std::time::Duration;

use anyhow::{Context, Result};
use btleplug::{
    api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter},
    platform::Manager,
};
use futures::StreamExt;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

const PIT_BOSS_NAME_MARKERS: &[&str] =
    &["pit boss", "pitboss", "pbv4", "pbl", "dansons", "mongoose"];

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
            if show_all {
                println!(
                    "{}  name={name:?}  rssi={}  connected={}",
                    peripheral.address(),
                    properties
                        .rssi
                        .map(|value| format!("{value} dBm"))
                        .unwrap_or_else(|| "unknown".to_owned()),
                    peripheral.is_connected().await.unwrap_or(false),
                );
            } else {
                println!(
                    "Pit Boss candidate  rssi={}  connected={}",
                    properties
                        .rssi
                        .map(|value| format!("{value} dBm"))
                        .unwrap_or_else(|| "unknown".to_owned()),
                    peripheral.is_connected().await.unwrap_or(false),
                );
            }

            if show_all && !properties.services.is_empty() {
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

/// Discovers a Pit Boss candidate, connects only long enough to enumerate GATT,
/// and always disconnects. No characteristic is read, written, or subscribed.
pub async fn inspect(scan_duration: Duration) -> Result<()> {
    let manager = Manager::new()
        .await
        .context("could not initialize Windows Bluetooth")?;
    let adapters = manager
        .adapters()
        .await
        .context("could not enumerate Bluetooth adapters")?;
    let adapter = adapters
        .into_iter()
        .next()
        .context("Windows reported no Bluetooth adapters")?;

    println!(
        "Scanning for a Pit Boss for {} seconds before a read-only GATT inspection...",
        scan_duration.as_secs()
    );
    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("could not start BLE scan")?;
    sleep(scan_duration).await;

    let candidates = adapter
        .peripherals()
        .await
        .context("could not read discovered BLE devices")?;
    adapter.stop_scan().await.ok();

    let mut smoker = None;
    for peripheral in candidates {
        let name = peripheral
            .properties()
            .await?
            .and_then(|properties| properties.local_name);
        if name.is_some_and(|name| looks_like_pit_boss(&name)) {
            smoker = Some(peripheral);
            break;
        }
    }
    let smoker = smoker.context(
        "no Pit Boss BLE advertisement found; leave its controller powered on and retry",
    )?;

    println!("Connecting to the Pit Boss to enumerate GATT services; no commands will be sent...");
    smoker.connect().await.context(
        "could not connect to the Pit Boss; close the Pit Boss app and disable Bluetooth on any phone connected to the smoker, then retry from closer range",
    )?;

    let result = async {
        smoker
            .discover_services()
            .await
            .context("could not discover Pit Boss GATT services")?;

        for service in smoker.services() {
            println!("Service {}", service.uuid);
            for characteristic in service.characteristics {
                println!(
                    "  Characteristic {}: read={}, write={}, write_without_response={}, notify={}, indicate={}",
                    characteristic.uuid,
                    characteristic.properties.contains(CharPropFlags::READ),
                    characteristic.properties.contains(CharPropFlags::WRITE),
                    characteristic
                        .properties
                        .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE),
                    characteristic.properties.contains(CharPropFlags::NOTIFY),
                    characteristic.properties.contains(CharPropFlags::INDICATE),
                );
            }
        }
        Ok(())
    }
    .await;

    smoker.disconnect().await.ok();
    result
}

/// Subscribes only to the controller's existing debug-log notifications. This
/// does not read or write a characteristic and sends no smoker command.
pub async fn monitor(duration: Duration) -> Result<()> {
    let manager = Manager::new()
        .await
        .context("could not initialize Windows Bluetooth")?;
    let adapters = manager
        .adapters()
        .await
        .context("could not enumerate Bluetooth adapters")?;
    let adapter = adapters
        .into_iter()
        .next()
        .context("Windows reported no Bluetooth adapters")?;

    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("could not start BLE scan")?;
    sleep(Duration::from_secs(10)).await;
    let candidates = adapter
        .peripherals()
        .await
        .context("could not read discovered BLE devices")?;
    adapter.stop_scan().await.ok();

    let mut smoker = None;
    for peripheral in candidates {
        let name = peripheral
            .properties()
            .await?
            .and_then(|properties| properties.local_name);
        if name.is_some_and(|name| looks_like_pit_boss(&name)) {
            smoker = Some(peripheral);
            break;
        }
    }
    let smoker = smoker.context("no Pit Boss BLE advertisement found")?;

    println!(
        "Connecting and subscribing to existing temperature reports; no smoker commands will be sent..."
    );
    smoker.connect().await.context(
        "could not connect to the Pit Boss; close the Pit Boss app and disable Bluetooth on any phone connected to the smoker",
    )?;

    let result = async {
        smoker
            .discover_services()
            .await
            .context("could not discover Pit Boss GATT services")?;

        let debug_log_uuid = Uuid::parse_str("306d4f53-5f44-4247-5f6c-6f675f5f5f30")
            .expect("constant debug-log UUID is valid");
        let debug_log = smoker
            .services()
            .iter()
            .flat_map(|service| service.characteristics.iter())
            .find(|characteristic| characteristic.uuid == debug_log_uuid)
            .cloned()
            .context("Pit Boss debug-log characteristic was not found")?;

        let mut notifications = smoker
            .notifications()
            .await
            .context("could not create BLE notification stream")?;
        smoker
            .subscribe(&debug_log)
            .await
            .context("could not subscribe to Pit Boss temperature reports")?;

        println!("Listening for {} seconds...", duration.as_secs());
        let until = tokio::time::Instant::now() + duration;
        let mut reports = 0;
        while let Some(notification) = timeout(
            until.saturating_duration_since(tokio::time::Instant::now()),
            notifications.next(),
        )
        .await
        .ok()
        .flatten()
        {
            if notification.uuid != debug_log_uuid {
                continue;
            }
            let Ok(message) = std::str::from_utf8(&notification.value) else {
                continue;
            };
            if let Some(temps) = parse_temperature_report(message) {
                reports += 1;
                println!(
                    "setpoint={}°F  chamber={}°F  probe-1={}°F  probe-2={}°F",
                    display_temperature(temps.setpoint),
                    display_temperature(temps.chamber),
                    display_temperature(temps.probe_1),
                    display_temperature(temps.probe_2),
                );
            }
        }
        println!("Monitoring ended after {reports} temperature reports.");
        Ok(())
    }
    .await;

    smoker.disconnect().await.ok();
    result
}

#[derive(Debug, PartialEq, Eq)]
struct TemperatureReport {
    setpoint: Option<u16>,
    chamber: Option<u16>,
    probe_1: Option<u16>,
    probe_2: Option<u16>,
}

fn parse_temperature_report(message: &str) -> Option<TemperatureReport> {
    let payload = message
        .split_whitespace()
        .nth(1)
        .filter(|payload| payload.starts_with("FE0C"))?;
    let bytes = (0..payload.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(payload.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    if bytes.len() < 27 {
        return None;
    }

    Some(TemperatureReport {
        probe_1: decode_temperature(&bytes, 5),
        probe_2: decode_temperature(&bytes, 8),
        setpoint: decode_temperature(&bytes, 20),
        chamber: decode_temperature(&bytes, 23),
    })
}

fn decode_temperature(bytes: &[u8], start: usize) -> Option<u16> {
    let temperature = u16::from(*bytes.get(start)?) * 100
        + u16::from(*bytes.get(start + 1)?) * 10
        + u16::from(*bytes.get(start + 2)?);
    (temperature != 960).then_some(temperature)
}

fn display_temperature(temperature: Option<u16>) -> String {
    temperature
        .map(|temperature| temperature.to_string())
        .unwrap_or_else(|| "--".to_owned())
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
        assert!(looks_like_pit_boss("PBL3-9451DC480624"));
        assert!(looks_like_pit_boss("Dansons Grill"));
        assert!(!looks_like_pit_boss("Wireless Headphones"));
    }
}
