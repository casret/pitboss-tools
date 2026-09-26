#include <Arduino.h>
#include <RF24.h>
#include <SPI.h>
#include <inttypes.h>
#include <stdio.h>

#include "smoke_protocol.h"

#ifndef SMOKE_RADIO_ID
#define SMOKE_RADIO_ID 0ULL
#endif
#ifndef SMOKE_RF_CHANNEL
#define SMOKE_RF_CHANNEL 10
#endif

namespace {
constexpr uint8_t kCePin = 4;
constexpr uint8_t kCsnPin = 5;
constexpr uint8_t kSckPin = 18;
constexpr uint8_t kMisoPin = 19;
constexpr uint8_t kMosiPin = 23;
constexpr uint8_t kPipe = 0;
constexpr uint8_t kChannels[] = {10, 40, 70};
constexpr uint64_t kPreambles[] = {0xAAULL, 0x55ULL};
constexpr uint64_t kConfiguredId = static_cast<uint64_t>(SMOKE_RADIO_ID);
constexpr uint8_t kConfiguredChannel = SMOKE_RF_CHANNEL;
constexpr uint32_t kSearchDwellMs = 22000;  // Over one 15-second Smoke cycle.
constexpr uint32_t kCandidateMaxAgeMs = 300000;
constexpr uint32_t kLostMs = 60000;
constexpr uint32_t kReacquireMs = 90000;
constexpr uint32_t kRetryMs = 5000;

// The RF24 is used exclusively in RX mode. Auto-ACK is explicitly disabled;
// no write/openWritingPipe/startConstCarrier calls are made.
RF24 radio(kCePin, kCsnPin, 4000000);
enum class Mode { Search, Listen };
Mode mode = Mode::Search;
bool ready = false;
bool signal_lost = false;
uint8_t search_slot = 0;
uint8_t current_channel = 10;
uint64_t active_id = 0;
uint64_t candidate_id = 0;
uint8_t candidate_count = 0;
uint32_t candidate_at_ms = 0;
uint32_t slot_started_ms = 0;
uint32_t last_packet_ms = 0;
uint32_t last_init_attempt_ms = 0;

void format_temperature(int32_t tenths, bool connected, char *out, size_t size) {
    if (!connected) {
        snprintf(out, size, "null");
        return;
    }
    const bool negative = tenths < 0;
    const int32_t magnitude = negative ? -tenths : tenths;
    snprintf(out, size, "%s%ld.%ld", negative ? "-" : "",
             static_cast<long>(magnitude / 10),
             static_cast<long>(magnitude % 10));
}

void emit_reading(const smoke::Reading &reading) {
    char p1[20], p2[20];
    format_temperature(reading.probe1_tenths_f, reading.probe1_connected,
                       p1, sizeof(p1));
    format_temperature(reading.probe2_tenths_f, reading.probe2_connected,
                       p2, sizeof(p2));
    Serial.printf("{\"probe1_f\":%s,\"probe2_f\":%s,\"radio_id\":%" PRIu64 "}\n",
                  p1, p2, active_id);
}

void search_next_slot() {
    const uint8_t channel = kChannels[search_slot % 3];
    const uint8_t preamble_index = search_slot / 3;
    radio.stopListening();
    radio.setAutoAck(false);
    radio.disableCRC();
    radio.setAddressWidth(2);
    radio.setPayloadSize(smoke::kSearchFrameSize);
    radio.setChannel(channel);
    radio.openReadingPipe(kPipe, kPreambles[preamble_index]);
    radio.flush_rx();
    radio.startListening();
    mode = Mode::Search;
    current_channel = channel;
    slot_started_ms = millis();
    search_slot = (search_slot + 1) % 6;
    Serial.printf("{\"event\":\"search\",\"channel\":%u,\"preamble\":\"%s\"}\n",
                  channel, preamble_index == 0 ? "aa" : "55");
}

void listen_on(uint64_t id, uint8_t channel, const char *source) {
    radio.stopListening();
    radio.setAutoAck(false);
    radio.setAddressWidth(smoke::kAddressSize);
    radio.setPayloadSize(smoke::kPayloadSize);
    radio.setCRCLength(RF24_CRC_16);
    radio.setChannel(channel);
    radio.openReadingPipe(kPipe, id);
    radio.flush_rx();
    radio.startListening();
    mode = Mode::Listen;
    current_channel = channel;
    active_id = id;
    last_packet_ms = millis();
    signal_lost = false;
    Serial.printf("{\"event\":\"listen\",\"source\":\"%s\",\"radio_id\":%" PRIu64 ",\"channel\":%u}\n",
                  source, id, channel);
}

void initialize_radio() {
    last_init_attempt_ms = millis();
    if (!radio.begin(&SPI) || !radio.isChipConnected()) {
        Serial.println(F("{\"event\":\"error\",\"code\":\"radio_not_found\",\"retry_seconds\":5}"));
        ready = false;
        return;
    }
    radio.setAutoAck(false);
    radio.setRetries(0, 0);
    // PA level is immaterial in RX mode, but MIN avoids unnecessary draw.
    radio.setPALevel(RF24_PA_MIN);
    if (!radio.setDataRate(RF24_250KBPS)) {
        Serial.println(F("{\"event\":\"error\",\"code\":\"requires_nrf24l01_plus_250kbps\"}"));
        ready = false;
        return;
    }
    ready = true;
    Serial.println(F("{\"event\":\"radio_ready\",\"rate_kbps\":250,\"auto_ack\":false}"));
    if (kConfiguredId != 0) {
        listen_on(kConfiguredId, kConfiguredChannel, "configured");
    } else {
        search_slot = 0;
        candidate_id = 0;
        candidate_count = 0;
        search_next_slot();
    }
}

void receive_search() {
    // The radio has a three-packet FIFO. Drain it without waiting for a log
    // timer; only CRC-valid full 5-address + 21-payload + 2-CRC frames count.
    for (uint8_t i = 0; i < 3 && radio.available(); ++i) {
        uint8_t frame[smoke::kSearchFrameSize];
        radio.read(frame, sizeof(frame));
        uint64_t id = 0;
        smoke::Reading reading{};
        if (!smoke::decode_search_frame(frame, sizeof(frame), id, reading)) {
            continue;
        }
        const uint32_t now = millis();
        if (id != candidate_id || now - candidate_at_ms > kCandidateMaxAgeMs) {
            candidate_id = id;
            candidate_at_ms = now;
            candidate_count = 1;
            Serial.printf("{\"event\":\"candidate\",\"radio_id\":%" PRIu64 ",\"channel\":%u}\n",
                          id, current_channel);
            continue;
        }
        if (++candidate_count >= 2) {
            // Do not confuse a single coincidental CRC hit with the Smoke.
            // The original reverse engineering prints the same 5-byte ID.
            listen_on(id, current_channel, "discovered");
            last_packet_ms = now;
            emit_reading(reading);
            return;
        }
    }
}

void receive_known() {
    smoke::Reading latest{};
    bool got_reading = false;
    for (uint8_t i = 0; i < 3 && radio.available(); ++i) {
        uint8_t payload[smoke::kPayloadSize];
        radio.read(payload, sizeof(payload));
        if (smoke::decode_payload(payload, sizeof(payload), latest)) {
            got_reading = true;
        }
    }
    if (!got_reading) return;
    last_packet_ms = millis();
    if (signal_lost) {
        Serial.printf("{\"event\":\"signal_restored\",\"radio_id\":%" PRIu64 "}\n",
                      active_id);
        signal_lost = false;
    }
    emit_reading(latest);
}
} // namespace

void setup() {
    Serial.begin(115200);
    delay(150);
    Serial.printf("{\"event\":\"boot\",\"mode\":\"%s\",\"pins\":{\"ce\":4,\"csn\":5,\"sck\":18,\"miso\":19,\"mosi\":23}}\n",
                  kConfiguredId ? "configured" : "search");
    if (kConfiguredId > 0xFFFFFFFFFFULL ||
        (kConfiguredChannel != 10 && kConfiguredChannel != 40 && kConfiguredChannel != 70)) {
        Serial.println(F("{\"event\":\"error\",\"code\":\"invalid_radio_id_or_channel\"}"));
        return;
    }
    SPI.begin(kSckPin, kMisoPin, kMosiPin, kCsnPin);
    initialize_radio();
}

void loop() {
    const uint32_t now = millis();
    if (!ready) {
        if (now - last_init_attempt_ms >= kRetryMs &&
            kConfiguredId <= 0xFFFFFFFFFFULL &&
            (kConfiguredChannel == 10 || kConfiguredChannel == 40 || kConfiguredChannel == 70)) {
            initialize_radio();
        }
    } else if (mode == Mode::Search) {
        receive_search();
        if (mode == Mode::Search && millis() - slot_started_ms >= kSearchDwellMs) {
            search_next_slot();
        }
    } else {
        receive_known();
        const uint32_t elapsed = millis() - last_packet_ms;
        if (elapsed >= kLostMs && !signal_lost) {
            Serial.printf("{\"event\":\"signal_lost\",\"radio_id\":%" PRIu64 ",\"seconds\":%lu}\n",
                          active_id, static_cast<unsigned long>(elapsed / 1000));
            signal_lost = true;
        }
        // A discovered ID is provisional until a normal CRC-checked packet
        // arrives. If it was a false positive, resume passive search.
        if (kConfiguredId == 0 && elapsed >= kReacquireMs) {
            Serial.println(F("{\"event\":\"reacquire\",\"reason\":\"no_valid_packets\"}"));
            candidate_id = 0;
            candidate_count = 0;
            search_slot = 0;
            search_next_slot();
        }
    }
    delay(2);
}
