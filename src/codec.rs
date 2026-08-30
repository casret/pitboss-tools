//! Pit Boss command-password codec.
//!
//! The grill does not receive the password as plain text. It expects a padded,
//! rolling XOR payload using a key derived from the controller uptime.

use std::time::{SystemTime, UNIX_EPOCH};

pub const KEY: [u8; 8] = [0x8f, 0x80, 0x19, 0xcf, 0x77, 0x6c, 0xfe, 0xb7];

/// Derive the firmware's eight-byte key for a device uptime in seconds.
pub fn timed_key(uptime: f64) -> [u8; 8] {
    let mut n = ((uptime - 5.0).max(0.0) / 10.0).floor() as u64;
    let mut key = KEY.to_vec();
    let mut result = [0; 8];

    for slot in &mut result[..7] {
        let index = (n % key.len() as u64) as usize;
        let value = key.remove(index);
        *slot = ((u64::from(value) ^ n) & 0xff) as u8;
        n = (n * u64::from(value) + u64::from(value)) & 0xff;
    }
    result[7] = key[0];
    result
}

/// Encode a password with explicit padding. This is useful for deterministic
/// tests and mirrors the firmware's `codec(data, key, 1)` operation.
pub fn encode_with_padding(data: &[u8], key: [u8; 8], padding: [u8; 16]) -> Vec<u8> {
    let mut input = Vec::with_capacity(17 + data.len());
    input.extend_from_slice(&padding);
    input.push(0xff);
    input.extend_from_slice(data);

    let mut rolling_key = key;
    let mut encoded = Vec::with_capacity(input.len());
    for (index, byte) in input.into_iter().enumerate() {
        let key_index = index % rolling_key.len();
        let encoded_byte = byte ^ rolling_key[key_index];
        encoded.push(encoded_byte);
        let next_key_index = (index + 1) % rolling_key.len();
        rolling_key[next_key_index] =
            (rolling_key[next_key_index] ^ encoded_byte).wrapping_add((index & 0xff) as u8);
    }
    encoded
}

/// Encode a password using fresh non-secret padding.
pub fn encode_password(password: &[u8], uptime: f64) -> Vec<u8> {
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut padding = [0u8; 16];
    for byte in &mut padding {
        // Padding only needs to be non-constant. It is not a secret or an
        // authentication factor; this avoids an additional RNG dependency.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state % 255) as u8;
    }
    encode_with_padding(password, timed_key(uptime), padding)
}

#[cfg(test)]
mod tests {
    use super::{encode_with_padding, timed_key};

    #[test]
    fn timed_key_is_stable_within_a_firmware_bucket() {
        assert_eq!(timed_key(100.0), timed_key(104.9));
        assert_ne!(timed_key(100.0), timed_key(110.0));
    }

    #[test]
    fn codec_round_trips_the_payload() {
        let key = timed_key(123.0);
        let padding = [0x11; 16];
        let encoded = encode_with_padding(b"secret", key, padding);
        assert_eq!(decode(&encoded, key), b"secret");
    }

    fn decode(data: &[u8], key: [u8; 8]) -> Vec<u8> {
        let mut rolling_key = key;
        let mut decoded = Vec::with_capacity(data.len());
        for (index, byte) in data.iter().copied().enumerate() {
            let key_index = index % rolling_key.len();
            let decoded_byte = byte ^ rolling_key[key_index];
            decoded.push(decoded_byte);
            let next_key_index = (index + 1) % rolling_key.len();
            rolling_key[next_key_index] =
                (rolling_key[next_key_index] ^ byte).wrapping_add((index & 0xff) as u8);
        }
        decoded
            .iter()
            .position(|byte| *byte == 0xff)
            .map_or_else(Vec::new, |index| decoded[index + 1..].to_vec())
    }
}
