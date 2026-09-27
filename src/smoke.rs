//! Read-only decoding of the ESP32's newline-delimited serial readings.
//! Keep this independent of Windows COM and BLE so failure modes are testable.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Reading {
    pub probe1_f: Option<f64>,
    pub probe2_f: Option<f64>,
    pub radio_id: u64,
}

pub fn parse_line(line: &str) -> Option<Reading> {
    let reading: Reading = serde_json::from_str(line).ok()?;
    if reading.radio_id == 0 || reading.radio_id >= (1 << 40) {
        return None;
    }
    for temperature in [reading.probe1_f, reading.probe2_f].into_iter().flatten() {
        if !temperature.is_finite() || !(-40.0..=500.0).contains(&temperature) {
            return None;
        }
    }
    Some(reading)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepts_plausible_readings() {
        let valid = r#"{"probe1_f":null,"probe2_f":76.0,"radio_id":12345}"#;
        assert_eq!(parse_line(valid).unwrap().probe2_f, Some(76.0));
        assert!(parse_line(r#"{"event":"radio_ready"}"#).is_none());
        assert!(parse_line(r#"{"probe1_f":null,"probe2_f":999.0,"radio_id":12345}"#).is_none());
        assert!(parse_line(r#"{"probe1_f":null,"probe2_f":76.0,"radio_id":0}"#).is_none());
        assert!(parse_line("garbage").is_none());
    }
}
