use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use btleplug::{
    api::{
        Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
        ValueNotification, WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures::{Stream, StreamExt};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpListener,
    time::{Instant, interval, sleep, timeout},
};
use uuid::Uuid;

use crate::{codec, config};

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

#[allow(clippy::too_many_arguments)]
async fn rpc_call(
    smoker: &Peripheral,
    rpc_data: &Characteristic,
    rpc_tx_control: &Characteristic,
    rpc_rx_control_uuid: Uuid,
    notifications: &mut Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    id: u32,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let payload = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    let length = u32::try_from(payload.len()).context("RPC payload too long")?;
    smoker
        .write(
            rpc_tx_control,
            &length.to_be_bytes(),
            WriteType::WithResponse,
        )
        .await
        .context("could not write Pit Boss RPC request length")?;
    for chunk in payload.as_bytes().chunks(20) {
        smoker
            .write(rpc_data, chunk, WriteType::WithResponse)
            .await
            .context("could not write Pit Boss RPC request data")?;
    }

    let response_length = timeout(Duration::from_secs(10), async {
        loop {
            let notification = notifications
                .next()
                .await
                .context("Pit Boss ended the RPC notification stream")?;
            if notification.uuid != rpc_rx_control_uuid {
                continue;
            }
            let bytes: [u8; 4] = notification
                .value
                .as_slice()
                .try_into()
                .context("Pit Boss returned an invalid RPC response length")?;
            break Ok::<u32, anyhow::Error>(u32::from_be_bytes(bytes));
        }
    })
    .await
    .context("timed out waiting for Pit Boss RPC response")??;
    if response_length > 4_096 {
        anyhow::bail!("Pit Boss returned an oversized RPC response");
    }

    let mut response = Vec::with_capacity(response_length as usize);
    while response.len() < response_length as usize {
        let chunk = smoker
            .read(rpc_data)
            .await
            .context("could not read Pit Boss RPC response")?;
        if chunk.is_empty() {
            anyhow::bail!("Pit Boss returned a truncated RPC response");
        }
        response.extend(chunk);
    }
    response.truncate(response_length as usize);
    let response: serde_json::Value =
        serde_json::from_slice(&response).context("Pit Boss returned invalid RPC JSON")?;
    if let Some(error) = response.get("error") {
        anyhow::bail!("Pit Boss rejected RPC request: {error}");
    }
    Ok(response
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

async fn send_authenticated_temperature(
    smoker: &Peripheral,
    rpc_data: &Characteristic,
    rpc_tx_control: &Characteristic,
    rpc_rx_control_uuid: Uuid,
    notifications: &mut Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    temperature: u16,
) -> Result<()> {
    let uptime = rpc_call(
        smoker,
        rpc_data,
        rpc_tx_control,
        rpc_rx_control_uuid,
        notifications,
        1,
        "PB.GetTime",
        serde_json::json!({}),
    )
    .await?
    .get("time")
    .and_then(serde_json::Value::as_f64)
    .context("Pit Boss returned no usable uptime for password authentication")?;
    let password = config::grill_password()?;
    let command = format!(
        "FE0501{:02X}{:02X}{:02X}FF",
        temperature / 100,
        (temperature % 100) / 10,
        temperature % 10
    );
    let encoded_password = codec::encode_password(password.as_bytes(), uptime);
    rpc_call(
        smoker,
        rpc_data,
        rpc_tx_control,
        rpc_rx_control_uuid,
        notifications,
        2,
        "PB.SendMCUCommand",
        serde_json::json!({
            "command": command,
            "psw": hex_encode(&encoded_password),
        }),
    )
    .await?;
    Ok(())
}

/// Sets only the factory controller's temperature setpoint. It never directly
/// operates the auger, fan, igniter, or mains power.
pub async fn set_temperature(temperature: u16) -> Result<()> {
    if !(130..=420).contains(&temperature) || temperature % 5 != 0 {
        anyhow::bail!("PBV4DX temperature must be from 130°F through 420°F in 5°F increments");
    }

    let manager = Manager::new()
        .await
        .context("could not initialize Windows Bluetooth")?;
    let adapter = manager
        .adapters()
        .await
        .context("could not enumerate Bluetooth adapters")?
        .into_iter()
        .next()
        .context("Windows reported no Bluetooth adapters")?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("could not start BLE scan")?;
    sleep(Duration::from_secs(10)).await;
    let candidates = adapter.peripherals().await?;
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
    smoker.connect().await.context(
        "could not connect to the Pit Boss; close the Pit Boss app and disable Bluetooth on any phone connected to the smoker",
    )?;

    let result = async {
        smoker.discover_services().await?;
        let rpc_data_uuid = Uuid::parse_str("5f6d4f53-5f52-5043-5f64-6174615f5f5f")
            .expect("constant RPC data UUID is valid");
        let rpc_tx_control_uuid = Uuid::parse_str("5f6d4f53-5f52-5043-5f74-785f63746c5f")
            .expect("constant RPC TX control UUID is valid");
        let rpc_rx_control_uuid = Uuid::parse_str("5f6d4f53-5f52-5043-5f72-785f63746c5f")
            .expect("constant RPC RX control UUID is valid");
        let services = smoker.services();
        let rpc_data = services
            .iter()
            .flat_map(|service| service.characteristics.iter())
            .find(|characteristic| characteristic.uuid == rpc_data_uuid)
            .cloned()
            .context("Pit Boss RPC data characteristic was not found")?;
        let rpc_tx_control = services
            .iter()
            .flat_map(|service| service.characteristics.iter())
            .find(|characteristic| characteristic.uuid == rpc_tx_control_uuid)
            .cloned()
            .context("Pit Boss RPC TX control characteristic was not found")?;
        let rpc_rx_control = services
            .iter()
            .flat_map(|service| service.characteristics.iter())
            .find(|characteristic| characteristic.uuid == rpc_rx_control_uuid)
            .cloned()
            .context("Pit Boss RPC RX control characteristic was not found")?;

        let mut notifications = smoker
            .notifications()
            .await
            .context("could not create BLE notification stream")?;
        smoker
            .subscribe(&rpc_rx_control)
            .await
            .context("could not subscribe to Pit Boss RPC responses")?;

        send_authenticated_temperature(
            &smoker,
            &rpc_data,
            &rpc_tx_control,
            rpc_rx_control_uuid,
            &mut notifications,
            temperature,
        )
        .await?;
        println!("Pit Boss accepted factory setpoint request: {temperature}°F");
        Ok(())
    }
    .await;
    smoker.disconnect().await.ok();
    result
}

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub bind: SocketAddr,
    pub database: PathBuf,
    pub target_f: u16,
    pub control_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
struct PidTelemetry {
    control_error_f: Option<f64>,
    integral: f64,
    proportional_term_f: f64,
    integral_term_f: f64,
    derivative_term_f: f64,
    adjustment_f: f64,
    recommended_factory_f: Option<u16>,
    last_control_at: Option<i64>,
    last_control_action: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct DashboardState {
    timestamp: Option<i64>,
    ambient_f: Option<u16>,
    meat_f: Option<u16>,
    chamber_f: Option<u16>,
    factory_setpoint_f: Option<u16>,
    control_enabled: bool,
    target_f: u16,
    last_command_at: Option<i64>,
    fault: Option<String>,
    #[serde(flatten)]
    pid: PidTelemetry,
}

#[derive(Clone, Debug)]
struct ControlSettings {
    enabled: bool,
    target_f: u16,
    integral: f64,
    started_at: Instant,
    last_command_at: Option<Instant>,
    last_command_epoch: Option<i64>,
    last_sample_at: Option<Instant>,
    fault: Option<String>,
    pid: PidTelemetry,
}

#[derive(Clone)]
struct WebState {
    latest: Arc<Mutex<DashboardState>>,
    settings: Arc<Mutex<ControlSettings>>,
    database: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug, Serialize)]
struct StoredSample {
    timestamp: i64,
    ambient_f: Option<u16>,
    meat_f: Option<u16>,
    chamber_f: Option<u16>,
    factory_setpoint_f: Option<u16>,
    control_enabled: bool,
    target_f: u16,
}

#[derive(Clone, Debug, Serialize)]
struct StoredControlEvent {
    timestamp: i64,
    target_f: u16,
    ambient_f: Option<u16>,
    factory_setpoint_f: Option<u16>,
    control_error_f: Option<f64>,
    integral: f64,
    proportional_term_f: f64,
    integral_term_f: f64,
    derivative_term_f: f64,
    adjustment_f: f64,
    recommended_factory_f: Option<u16>,
    action: String,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ControlRequest {
    enabled: Option<bool>,
    target_f: Option<u16>,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn init_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create database directory {}", parent.display()))?;
    }
    let connection = Connection::open(path)
        .with_context(|| format!("could not open SQLite database {}", path.display()))?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS samples (
             id INTEGER PRIMARY KEY,
             timestamp INTEGER NOT NULL,
             ambient_f INTEGER,
             meat_f INTEGER,
             chamber_f INTEGER,
             factory_setpoint_f INTEGER,
             control_enabled INTEGER NOT NULL,
             target_f INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS samples_timestamp ON samples(timestamp);
         CREATE TABLE IF NOT EXISTS control_events (
             id INTEGER PRIMARY KEY,
             timestamp INTEGER NOT NULL,
             target_f INTEGER NOT NULL,
             ambient_f INTEGER,
             factory_setpoint_f INTEGER,
             control_error_f REAL,
             integral REAL NOT NULL,
             proportional_term_f REAL NOT NULL,
             integral_term_f REAL NOT NULL,
             derivative_term_f REAL NOT NULL,
             adjustment_f REAL NOT NULL,
             recommended_factory_f INTEGER,
             action TEXT NOT NULL,
             reason TEXT
         );
         CREATE INDEX IF NOT EXISTS control_events_timestamp ON control_events(timestamp);",
    )?;
    Ok(connection)
}

fn project_control(latest: &mut DashboardState, settings: &ControlSettings) {
    latest.control_enabled = settings.enabled;
    latest.target_f = settings.target_f;
    latest.last_command_at = settings.last_command_epoch;
    latest.fault = settings.fault.clone();
    latest.pid = settings.pid.clone();
}

fn record_sample(state: &WebState, report: &TemperatureReport) -> Result<()> {
    let timestamp = epoch_now();
    let settings_snapshot = {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| anyhow::anyhow!("control settings lock poisoned"))?;
        settings.last_sample_at = Some(Instant::now());
        settings.clone()
    };
    let sample = StoredSample {
        timestamp,
        ambient_f: report.probe_1,
        meat_f: report.probe_2,
        chamber_f: report.chamber,
        factory_setpoint_f: report.setpoint,
        control_enabled: settings_snapshot.enabled,
        target_f: settings_snapshot.target_f,
    };
    {
        let mut latest = state
            .latest
            .lock()
            .map_err(|_| anyhow::anyhow!("dashboard state lock poisoned"))?;
        latest.timestamp = Some(sample.timestamp);
        latest.ambient_f = sample.ambient_f;
        latest.meat_f = sample.meat_f;
        latest.chamber_f = sample.chamber_f;
        latest.factory_setpoint_f = sample.factory_setpoint_f;
        project_control(&mut latest, &settings_snapshot);
    }
    state
        .database
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock poisoned"))?
        .execute(
            "INSERT INTO samples
             (timestamp, ambient_f, meat_f, chamber_f, factory_setpoint_f,
              control_enabled, target_f)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sample.timestamp,
                sample.ambient_f.map(i64::from),
                sample.meat_f.map(i64::from),
                sample.chamber_f.map(i64::from),
                sample.factory_setpoint_f.map(i64::from),
                sample.control_enabled as i64,
                i64::from(sample.target_f),
            ],
        )?;
    Ok(())
}

fn record_control_event(state: &WebState, event: &StoredControlEvent) -> Result<()> {
    state
        .database
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock poisoned"))?
        .execute(
            "INSERT INTO control_events
             (timestamp, target_f, ambient_f, factory_setpoint_f,
              control_error_f, integral, proportional_term_f, integral_term_f,
              derivative_term_f, adjustment_f, recommended_factory_f, action, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                event.timestamp,
                i64::from(event.target_f),
                event.ambient_f.map(i64::from),
                event.factory_setpoint_f.map(i64::from),
                event.control_error_f,
                event.integral,
                event.proportional_term_f,
                event.integral_term_f,
                event.derivative_term_f,
                event.adjustment_f,
                event.recommended_factory_f.map(i64::from),
                event.action,
                event.reason,
            ],
        )?;
    Ok(())
}

fn publish_pid_status(state: &WebState, telemetry: PidTelemetry) {
    if let Ok(mut settings) = state.settings.lock() {
        settings.integral = telemetry.integral;
        settings.pid = telemetry.clone();
        if let Ok(mut latest) = state.latest.lock() {
            latest.pid = telemetry;
        }
    }
}

fn stop_control(state: &WebState, reason: impl Into<String>) {
    let reason = reason.into();
    let settings_snapshot = match state.settings.lock() {
        Ok(mut settings) => {
            settings.enabled = false;
            settings.fault = Some(reason);
            settings.pid.last_control_at = Some(epoch_now());
            settings.pid.last_control_action = settings
                .fault
                .as_ref()
                .map(|fault| format!("stopped: {fault}"));
            settings.clone()
        }
        Err(_) => return,
    };
    if let Ok(mut latest) = state.latest.lock() {
        project_control(&mut latest, &settings_snapshot);
    }
    eprintln!(
        "Guarded control stopped: {}",
        settings_snapshot
            .fault
            .as_deref()
            .unwrap_or("unknown fault")
    );
}

async fn dashboard() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

async fn api_state(State(state): State<WebState>) -> Json<DashboardState> {
    let snapshot = state
        .latest
        .lock()
        .map(|latest| latest.clone())
        .unwrap_or_else(|_| DashboardState {
            timestamp: None,
            ambient_f: None,
            meat_f: None,
            chamber_f: None,
            factory_setpoint_f: None,
            control_enabled: false,
            target_f: 225,
            last_command_at: None,
            fault: Some("dashboard state unavailable".to_owned()),
            pid: PidTelemetry::default(),
        });
    Json(snapshot)
}

async fn api_history(
    State(state): State<WebState>,
    Query(query): Query<HistoryQuery>,
) -> std::result::Result<Json<Vec<StoredSample>>, (StatusCode, Json<ApiError>)> {
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let database = state
        .database
        .lock()
        .map_err(|_| api_internal_error("database lock poisoned"))?;
    let mut statement = database
        .prepare(
            "SELECT timestamp, ambient_f, meat_f, chamber_f, factory_setpoint_f,
                    control_enabled, target_f
             FROM samples ORDER BY timestamp DESC, id DESC LIMIT ?1",
        )
        .map_err(|error| api_internal_error(error.to_string()))?;
    let rows = statement
        .query_map(params![limit as i64], |row| {
            Ok(StoredSample {
                timestamp: row.get(0)?,
                ambient_f: row.get::<_, Option<u16>>(1)?,
                meat_f: row.get::<_, Option<u16>>(2)?,
                chamber_f: row.get::<_, Option<u16>>(3)?,
                factory_setpoint_f: row.get::<_, Option<u16>>(4)?,
                control_enabled: row.get::<_, i64>(5)? != 0,
                target_f: row.get(6)?,
            })
        })
        .map_err(|error| api_internal_error(error.to_string()))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|error| api_internal_error(error.to_string()))?);
    }
    Ok(Json(result))
}

fn api_internal_error(message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

async fn api_control_events(
    State(state): State<WebState>,
    Query(query): Query<HistoryQuery>,
) -> std::result::Result<Json<Vec<StoredControlEvent>>, (StatusCode, Json<ApiError>)> {
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let database = state
        .database
        .lock()
        .map_err(|_| api_internal_error("database lock poisoned"))?;
    let mut statement = database
        .prepare(
            "SELECT timestamp, target_f, ambient_f, factory_setpoint_f,
                    control_error_f, integral, proportional_term_f, integral_term_f,
                    derivative_term_f, adjustment_f, recommended_factory_f, action, reason
             FROM control_events ORDER BY timestamp DESC, id DESC LIMIT ?1",
        )
        .map_err(|error| api_internal_error(error.to_string()))?;
    let rows = statement
        .query_map(params![limit as i64], |row| {
            Ok(StoredControlEvent {
                timestamp: row.get(0)?,
                target_f: row.get(1)?,
                ambient_f: row.get::<_, Option<u16>>(2)?,
                factory_setpoint_f: row.get::<_, Option<u16>>(3)?,
                control_error_f: row.get(4)?,
                integral: row.get(5)?,
                proportional_term_f: row.get(6)?,
                integral_term_f: row.get(7)?,
                derivative_term_f: row.get(8)?,
                adjustment_f: row.get(9)?,
                recommended_factory_f: row.get::<_, Option<u16>>(10)?,
                action: row.get(11)?,
                reason: row.get(12)?,
            })
        })
        .map_err(|error| api_internal_error(error.to_string()))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|error| api_internal_error(error.to_string()))?);
    }
    Ok(Json(result))
}

async fn api_control(
    State(state): State<WebState>,
    Json(request): Json<ControlRequest>,
) -> std::result::Result<Json<DashboardState>, (StatusCode, Json<ApiError>)> {
    let mut settings = state
        .settings
        .lock()
        .map_err(|_| api_internal_error("control settings lock poisoned"))?;
    if let Some(target) = request.target_f {
        if !(130..=350).contains(&target) || target % 5 != 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: "grate target must be 130..350°F in 5°F increments".to_owned(),
                }),
            ));
        }
        settings.target_f = target;
    }
    if let Some(enabled) = request.enabled {
        settings.enabled = enabled;
        settings.fault = None;
        settings.integral = 0.0;
        settings.pid.integral = 0.0;
        settings.pid.last_control_at = Some(epoch_now());
        settings.pid.last_control_action = Some(if enabled {
            "enabled from dashboard".to_owned()
        } else {
            "disabled from dashboard".to_owned()
        });
        if enabled {
            settings.started_at = Instant::now();
        }
    }
    let snapshot = settings.clone();
    drop(settings);
    let mut latest = state
        .latest
        .lock()
        .map_err(|_| api_internal_error("dashboard state lock poisoned"))?;
    project_control(&mut latest, &snapshot);
    Ok(Json(latest.clone()))
}

async fn discover_smoker(adapter: &Adapter, scan_duration: Duration) -> Result<Peripheral> {
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
    for peripheral in candidates {
        let name = peripheral
            .properties()
            .await?
            .and_then(|properties| properties.local_name);
        if name.is_some_and(|name| looks_like_pit_boss(&name)) {
            return Ok(peripheral);
        }
    }
    anyhow::bail!("no Pit Boss BLE advertisement found; leave its controller powered on and retry")
}

async fn run_controller(state: WebState) -> Result<()> {
    let manager = Manager::new()
        .await
        .context("could not initialize Windows Bluetooth")?;
    let adapter = manager
        .adapters()
        .await
        .context("could not enumerate Bluetooth adapters")?
        .into_iter()
        .next()
        .context("Windows reported no Bluetooth adapters")?;
    println!("Scanning for the PBV4DX controller...");
    let smoker = discover_smoker(&adapter, Duration::from_secs(10)).await?;
    smoker.connect().await.context(
        "could not connect to the Pit Boss; close the Pit Boss app and disable Bluetooth on any phone connected to the smoker",
    )?;

    let result = async {
        smoker
            .discover_services()
            .await
            .context("could not discover Pit Boss GATT services")?;
        let debug_uuid = Uuid::parse_str("306d4f53-5f44-4247-5f6c-6f675f5f5f30")
            .expect("constant debug UUID is valid");
        let rpc_data_uuid = Uuid::parse_str("5f6d4f53-5f52-5043-5f64-6174615f5f5f")
            .expect("constant RPC data UUID is valid");
        let rpc_tx_uuid = Uuid::parse_str("5f6d4f53-5f52-5043-5f74-785f63746c5f")
            .expect("constant RPC TX UUID is valid");
        let rpc_rx_uuid = Uuid::parse_str("5f6d4f53-5f52-5043-5f72-785f63746c5f")
            .expect("constant RPC RX UUID is valid");
        let services = smoker.services();
        let find = |uuid| {
            services
                .iter()
                .flat_map(|service| service.characteristics.iter())
                .find(|characteristic| characteristic.uuid == uuid)
                .cloned()
        };
        let debug = find(debug_uuid).context("Pit Boss debug-log characteristic was not found")?;
        let rpc_data = find(rpc_data_uuid).context("Pit Boss RPC data characteristic was not found")?;
        let rpc_tx = find(rpc_tx_uuid).context("Pit Boss RPC TX characteristic was not found")?;
        let rpc_rx = find(rpc_rx_uuid).context("Pit Boss RPC RX characteristic was not found")?;
        let mut notifications = smoker
            .notifications()
            .await
            .context("could not create BLE notification stream")?;
        smoker
            .subscribe(&debug)
            .await
            .context("could not subscribe to Pit Boss temperature reports")?;
        smoker
            .subscribe(&rpc_rx)
            .await
            .context("could not subscribe to Pit Boss RPC responses")?;

        let mut control_tick = interval(Duration::from_secs(30));
        println!("Connected. Dashboard and SQLite export are live; control is guarded and {}.", if state.settings.lock().map(|settings| settings.enabled).unwrap_or(false) { "enabled" } else { "off" });
        loop {
            tokio::select! {
                notification = notifications.next() => {
                    let Some(notification) = notification else {
                        anyhow::bail!("Pit Boss BLE notification stream ended");
                    };
                    if notification.uuid != debug_uuid {
                        continue;
                    }
                    let Ok(message) = std::str::from_utf8(&notification.value) else {
                        continue;
                    };
                    if let Some(report) = parse_temperature_report(message) {
                        if let Err(error) = record_sample(&state, &report) {
                            eprintln!("Could not save temperature sample: {error:#}");
                        }
                        println!("ambient={}°F meat={}°F chamber={}°F factory={}°F", display_temperature(report.probe_1), display_temperature(report.probe_2), display_temperature(report.chamber), display_temperature(report.setpoint));
                    }
                }
                _ = control_tick.tick() => {
                    guarded_control_tick(&state, &smoker, &rpc_data, &rpc_tx, rpc_rx_uuid, &mut notifications).await;
                }
                _ = tokio::signal::ctrl_c() => {
                    println!("Stopping dashboard and controller...");
                    break;
                }
            }
        }
        Ok(())
    }
    .await;
    smoker.disconnect().await.ok();
    result
}

const KP: f64 = 0.70;
const KI: f64 = 0.05;
const CONTROL_INTERVAL_MINUTES: f64 = 0.5;

async fn guarded_control_tick(
    state: &WebState,
    smoker: &Peripheral,
    rpc_data: &Characteristic,
    rpc_tx: &Characteristic,
    rpc_rx_uuid: Uuid,
    notifications: &mut Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
) {
    let settings = match state.settings.lock() {
        Ok(settings) => settings.clone(),
        Err(_) => return,
    };
    if !settings.enabled {
        return;
    }
    let latest = match state.latest.lock() {
        Ok(latest) => latest.clone(),
        Err(_) => return,
    };
    let Some(last_sample) = settings.last_sample_at else {
        return;
    };
    if last_sample.elapsed() > Duration::from_secs(60) {
        stop_control(state, "temperature sensor data is stale");
        return;
    }
    let Some(ambient) = latest.ambient_f else {
        stop_control(state, "grate ambient probe is disconnected");
        return;
    };
    if ambient >= 400
        || latest
            .chamber_f
            .is_some_and(|temperature| temperature >= 420)
    {
        stop_control(state, "over-temperature safety limit reached");
        return;
    }
    if settings.started_at.elapsed() >= Duration::from_secs(300)
        && latest
            .chamber_f
            .is_some_and(|temperature| temperature < 100)
    {
        stop_control(state, "possible flameout detected");
        return;
    }
    let Some(factory) = latest.factory_setpoint_f else {
        let mut telemetry = settings.pid;
        telemetry.last_control_at = Some(epoch_now());
        telemetry.last_control_action = Some("waiting for factory setpoint".to_owned());
        publish_pid_status(state, telemetry);
        return;
    };

    let error = f64::from(settings.target_f) - f64::from(ambient);
    let integral = (settings.integral + error * CONTROL_INTERVAL_MINUTES).clamp(-100.0, 100.0);
    let proportional_term = KP * error;
    let integral_term = KI * integral;
    // Derivative is deliberately disabled for this first controller: the
    // smoker's discrete, slow response makes probe noise more harmful than
    // useful. The zero term is still exported so the complete calculation is
    // visible in the dashboard and database.
    let derivative_term = 0.0;
    let adjustment = (proportional_term + integral_term + derivative_term).clamp(-10.0, 10.0);
    let desired =
        (((f64::from(factory) + adjustment) / 5.0).round() as i32 * 5).clamp(130, 420) as u16;
    let command_rate_limited = settings
        .last_command_at
        .is_some_and(|last| last.elapsed() < Duration::from_secs(120));
    let within_deadband = error.abs() < 2.0;
    let mut telemetry = PidTelemetry {
        control_error_f: Some(error),
        integral,
        proportional_term_f: proportional_term,
        integral_term_f: integral_term,
        derivative_term_f: derivative_term,
        adjustment_f: adjustment,
        recommended_factory_f: Some(desired),
        last_control_at: Some(epoch_now()),
        last_control_action: None,
    };
    let mut event = StoredControlEvent {
        timestamp: epoch_now(),
        target_f: settings.target_f,
        ambient_f: Some(ambient),
        factory_setpoint_f: Some(factory),
        control_error_f: Some(error),
        integral,
        proportional_term_f: proportional_term,
        integral_term_f: integral_term,
        derivative_term_f: derivative_term,
        adjustment_f: adjustment,
        recommended_factory_f: Some(desired),
        action: String::new(),
        reason: None,
    };

    if within_deadband {
        telemetry.last_control_action = Some("hold: within deadband".to_owned());
        event.action = "hold".to_owned();
        event.reason = Some("within ±2°F deadband".to_owned());
    } else if desired == factory {
        telemetry.last_control_action = Some("hold: setpoint unchanged".to_owned());
        event.action = "hold".to_owned();
        event.reason = Some("calculated setpoint rounds to current value".to_owned());
    } else if command_rate_limited {
        telemetry.last_control_action = Some("hold: command rate limited".to_owned());
        event.action = "rate_limited".to_owned();
        event.reason = Some("minimum 120-second command interval".to_owned());
    } else {
        telemetry.last_control_action = Some("command pending".to_owned());
        match send_authenticated_temperature(
            smoker,
            rpc_data,
            rpc_tx,
            rpc_rx_uuid,
            notifications,
            desired,
        )
        .await
        {
            Ok(()) => {
                let now = Instant::now();
                let epoch = epoch_now();
                if let Ok(mut current) = state.settings.lock() {
                    current.last_command_at = Some(now);
                    current.last_command_epoch = Some(epoch);
                    current.fault = None;
                    current.integral = integral;
                    telemetry.last_control_at = Some(epoch);
                    telemetry.last_control_action =
                        Some(format!("command accepted: factory {desired}°F"));
                    current.pid = telemetry.clone();
                    let snapshot = current.clone();
                    if let Ok(mut dashboard) = state.latest.lock() {
                        project_control(&mut dashboard, &snapshot);
                    }
                }
                event.action = "setpoint_command".to_owned();
                event.reason = Some(format!("requested factory setpoint {desired}°F"));
                println!(
                    "Guarded controller requested factory setpoint {desired}°F (grate {ambient}°F, target {}°F)",
                    settings.target_f
                );
            }
            Err(error) => {
                let reason = format!("setpoint command failed: {error}");
                stop_control(state, reason.clone());
                telemetry.last_control_action = Some(format!("stopped: {reason}"));
                event.action = "stopped".to_owned();
                event.reason = Some(reason);
            }
        }
    }

    if let Ok(mut current) = state.settings.lock() {
        current.integral = integral;
    }
    publish_pid_status(state, telemetry);
    if let Err(error) = record_control_event(state, &event) {
        eprintln!("Could not save control calculation: {error:#}");
    }
}

/// Run the local dashboard, SQLite exporter, and guarded controller.
pub async fn serve(options: ServeOptions) -> Result<()> {
    if !(130..=350).contains(&options.target_f) || options.target_f % 5 != 0 {
        anyhow::bail!("grate target must be 130..350°F in 5°F increments");
    }
    let database = init_database(&options.database)?;
    let settings = ControlSettings {
        enabled: options.control_enabled,
        target_f: options.target_f,
        integral: 0.0,
        started_at: Instant::now(),
        last_command_at: None,
        last_command_epoch: None,
        last_sample_at: None,
        fault: None,
        pid: PidTelemetry::default(),
    };
    let latest = DashboardState {
        timestamp: None,
        ambient_f: None,
        meat_f: None,
        chamber_f: None,
        factory_setpoint_f: None,
        control_enabled: settings.enabled,
        target_f: settings.target_f,
        last_command_at: None,
        fault: None,
        pid: PidTelemetry::default(),
    };
    let state = WebState {
        latest: Arc::new(Mutex::new(latest)),
        settings: Arc::new(Mutex::new(settings)),
        database: Arc::new(Mutex::new(database)),
    };
    let app = Router::new()
        .route("/", get(dashboard))
        .route("/api/state", get(api_state))
        .route("/api/history", get(api_history))
        .route("/api/control-events", get(api_control_events))
        .route("/api/control", post(api_control))
        .with_state(state.clone());
    let listener = TcpListener::bind(options.bind)
        .await
        .with_context(|| format!("could not bind dashboard to {}", options.bind))?;
    println!("Dashboard: http://{}", options.bind);
    println!("SQLite history: {}", options.database.display());
    let web_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .context("dashboard server stopped")
    });
    let result = run_controller(state).await;
    web_task.abort();
    let _ = web_task.await;
    result
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
