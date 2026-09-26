#include <cassert>
#include <cstdint>

#include "smoke_protocol.h"

int main() {
    // Stefan Slonevskiy's documented 5-byte address + 21-byte Smoke packet.
    // The last two bytes are the independently calculated CCITT CRC.
    const uint8_t frame[smoke::kSearchFrameSize] = {
        0x8f, 0x43, 0x3b, 0x73, 0x6f,
        0xbd, 0x01, 0x2c, 0x06, 0x40, 0x01,
        0xbf, 0x01, 0x54, 0x0b, 0x40, 0x01,
        0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x96, 0x00,
        0x9b, 0x10
    };
    assert(smoke::crc16(frame, 26) == 0x9b10);
    smoke::Reading reading{};
    uint64_t id = 0;
    assert(smoke::decode_search_frame(frame, sizeof(frame), id, reading));
    assert(id == 0x8F433B736FULL);
    assert(reading.probe1_connected && reading.probe2_connected);
    assert(reading.probe1_tenths_f == 445); // 44.5°F
    assert(reading.probe2_tenths_f == 447); // 44.7°F

    uint8_t bad_crc[sizeof(frame)];
    for (size_t i = 0; i < sizeof(frame); ++i) bad_crc[i] = frame[i];
    bad_crc[6] ^= 1;
    assert(!smoke::decode_search_frame(bad_crc, sizeof(bad_crc), id, reading));
    assert(!smoke::decode_search_frame(frame, sizeof(frame) - 1, id, reading));

    uint8_t payload[smoke::kPayloadSize]{};
    // Celsius: 100.0°C -> 212.0°F and -5.0°C -> 23.0°F.
    payload[0] = 0xe8; payload[1] = 0x03;
    payload[6] = 0xce; payload[7] = 0xff;
    payload[13] = 0; payload[15] = 0; payload[16] = 0;
    assert(smoke::decode_payload(payload, sizeof(payload), reading));
    assert(reading.probe1_tenths_f == 2120);
    assert(reading.probe2_tenths_f == 230);
    assert(smoke::as_tenths_f(-1, false) == 318); // -0.1°C -> 31.8°F
    payload[15] = 1; // No second probe: firmware emits JSON null.
    assert(smoke::decode_payload(payload, sizeof(payload), reading));
    assert(!reading.probe2_connected);
    payload[16] = 3;
    assert(!smoke::decode_payload(payload, sizeof(payload), reading));
    return 0;
}
