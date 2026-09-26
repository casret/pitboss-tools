#pragma once

#include <stddef.h>
#include <stdint.h>

// Protocol adapted from Stefan Slonevskiy's MIT-licensed
// esphome-thermoworks-smoke, commit 9da52e863b184717864a4c9cbc4b9a72e7d79ae6:
// https://github.com/stefslon/esphome-thermoworks-smoke
// Copyright (c) 2024-2025 Stefan Slonevskiy. See ../UPSTREAM-LICENSE.
namespace smoke {

constexpr size_t kAddressSize = 5;
constexpr size_t kPayloadSize = 21;
constexpr size_t kSearchFrameSize = kAddressSize + kPayloadSize + 2; // CRC16

struct Reading {
    int32_t probe1_tenths_f;
    int32_t probe2_tenths_f;
    bool probe1_connected;
    bool probe2_connected;
};

// Smoke's fields are 16-bit, signed, little-endian tenths of °F or °C.
inline int16_t read_i16_le(const uint8_t *bytes) {
    const int32_t value = static_cast<int32_t>(bytes[0]) |
                          (static_cast<int32_t>(bytes[1]) << 8);
    return static_cast<int16_t>(value >= 0x8000 ? value - 0x10000 : value);
}

// Return tenths of °F; round Celsius conversion to the nearest tenth.
inline int32_t as_tenths_f(int16_t value, bool fahrenheit) {
    if (fahrenheit) return value;
    const int32_t scaled = static_cast<int32_t>(value) * 9;
    const int32_t rounded = scaled < 0 ? -((-scaled + 2) / 5) : (scaled + 2) / 5;
    return 320 + rounded;
}

// The Smoke payload's first 12 bytes are six int16 temperatures/alarms;
// probe-presence flags are bytes 13 and 15, unit flag is byte 16.
inline bool decode_payload(const uint8_t *payload, size_t size, Reading &out) {
    if (payload == nullptr || size != kPayloadSize || payload[13] > 1 ||
        payload[15] > 1 || payload[16] > 1) {
        return false;
    }
    const bool fahrenheit = payload[16] == 1;
    out.probe1_connected = payload[13] == 0;
    out.probe2_connected = payload[15] == 0;
    out.probe1_tenths_f = as_tenths_f(read_i16_le(payload), fahrenheit);
    out.probe2_tenths_f = as_tenths_f(read_i16_le(payload + 6), fahrenheit);
    return true;
}

// nRF24 promiscuous mode puts the five address bytes ahead of the 21-byte
// payload, followed by a big-endian CRC-16/CCITT-FALSE (init 0xFFFF).
inline uint16_t crc16(const uint8_t *bytes, size_t size) {
    uint16_t crc = 0xffff;
    for (size_t index = 0; index < size; ++index) {
        crc ^= static_cast<uint16_t>(bytes[index]) << 8;
        for (uint8_t bit = 0; bit < 8; ++bit) {
            crc = static_cast<uint16_t>((crc & 0x8000U) != 0
                                             ? (crc << 1) ^ 0x1021U
                                             : crc << 1);
        }
    }
    return crc;
}

inline bool decode_search_frame(const uint8_t *frame, size_t size,
                                uint64_t &radio_id, Reading &reading) {
    if (frame == nullptr || size != kSearchFrameSize) return false;
    const uint16_t received_crc =
        (static_cast<uint16_t>(frame[26]) << 8) | frame[27];
    if (crc16(frame, 26) != received_crc ||
        !decode_payload(frame + kAddressSize, kPayloadSize, reading)) {
        return false;
    }

    // Same printed ID and RF24 openReadingPipe(uint64_t) representation as the
    // reference implementation: 8F 43 3B 73 6F -> 0x8F433B736F.
    uint64_t found = 0;
    for (size_t index = 0; index < kAddressSize; ++index) {
        found = (found << 8) | frame[index];
    }
    if (found == 0) return false;
    radio_id = found;
    return true;
}

} // namespace smoke
