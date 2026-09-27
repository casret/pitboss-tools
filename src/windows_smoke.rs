//! Windows USB serial reader for the passive ThermoWorks Smoke ESP32 receiver.
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use tokio::sync::mpsc;

use crate::smoke;

pub enum Event {
    Connected,
    Disconnected,
    Reading(smoke::Reading),
}

pub fn spawn(
    port_name: String,
    tx: mpsc::UnboundedSender<Event>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("smoke-serial-reader".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // An absent/busy USB port is retried without ending the dashboard.
                if let Ok(mut port) = serialport::new(&port_name, 115_200)
                    .timeout(Duration::from_secs(1))
                    .open()
                {
                    if tx.send(Event::Connected).is_err() {
                        break;
                    }
                    let mut line = Vec::with_capacity(128);
                    let mut oversize = false;
                    let mut buffer = [0u8; 128];
                    while !stop.load(Ordering::Relaxed) {
                        match port.read(&mut buffer) {
                            Ok(count) => {
                                for &byte in &buffer[..count] {
                                    if byte == b'\n' {
                                        if !oversize
                                            && let Ok(text) = std::str::from_utf8(&line)
                                            && let Some(reading) = smoke::parse_line(text)
                                            && tx.send(Event::Reading(reading)).is_err()
                                        {
                                            return;
                                        }
                                        line.clear();
                                        oversize = false;
                                    } else if line.len() < 512 && !oversize {
                                        line.push(byte);
                                    } else {
                                        oversize = true;
                                    }
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                            Err(_) => break,
                        }
                    }
                }
                if tx.send(Event::Disconnected).is_err() {
                    break;
                }
                for _ in 0..30 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        })
}
