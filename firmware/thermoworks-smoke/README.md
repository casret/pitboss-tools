# ThermoWorks Smoke → USB serial

Small, receive-only firmware for the **original two-channel ThermoWorks Smoke**
and an ESP32-WROOM-32 development board (PlatformIO `esp32dev`, Arduino). An
nRF24L01+ listens on hardware VSPI; no Wi-Fi, BLE, pairing, MQTT, cloud, or
handheld-receiver shutdown is involved. The handheld receiver can stay on.
This is separate from the Pit Boss BLE dashboard.

The RF settings, CRC search, 5-byte radio ID and 21-byte payload layout follow
[stefslon/esphome-thermoworks-smoke](https://github.com/stefslon/esphome-thermoworks-smoke/tree/9da52e863b184717864a4c9cbc4b9a72e7d79ae6)
(MIT; see `UPSTREAM-LICENSE`). RF24 is the only external firmware library.
Not tested against a physical Smoke yet: a successful build is **not** proof
of receiving live packets.

## Wiring

| nRF24L01+ | ESP32-WROOM-32 |
| --- | --- |
| VCC | **3V3 only, never 5V** |
| GND | GND |
| CE | GPIO 4 |
| CSN | GPIO 5 |
| SCK | GPIO 18 |
| MOSI | GPIO 23 |
| MISO | GPIO 19 |
| IRQ | Not connected |

Use short wires and, if the module resets or reception is flaky, place a
10–47 µF capacitor directly between its 3V3 and GND pins (observe capacitor
polarity). The ESP32's USB-to-serial port connects to the PC; a bare WROOM
module needs an external 3.3 V USB-UART adapter and normal flashing circuitry.

## Build, flash, monitor

Install [PlatformIO Core](https://docs.platformio.org/en/latest/core/installation/index.html),
then from this directory:

```sh
pio run
pio device list
pio run -t upload --upload-port /dev/ttyUSB0
pio device monitor --port /dev/ttyUSB0 --baud 115200
```

On Windows replace `/dev/ttyUSB0` with the board's `COM` port (e.g. `COM5`).
If developing from WSL without a passed-through USB serial device, run the
upload and monitor commands on Windows instead. Some ESP32 boards need the
BOOT button held during the first upload. The monitor should show `boot`,
`radio_ready`, then `search` events. `radio_not_found` suggests the radio
wiring, power, CSN, or SPI bus is wrong; a 250 kbps error suggests a non-+
nRF24L01. It retries initialization every five seconds.

### Radio-ID discovery (default)

Power the Smoke transmitter normally (do **not** use its pairing mode). Leave
`SMOKE_RADIO_ID` unset. The firmware listens for ~22 seconds per combination
of channels **10, 40, 70** and the two address preambles **aa/55** from the
reference project. It validates the Smoke CRC and waits for a second matching
packet to avoid a false ID. Watch for NDJSON lines such as:

```json
{"event":"candidate","radio_id":615308292975,"channel":10}
{"event":"listen","source":"discovered","radio_id":615308292975,"channel":10}
{"probe1_f":44.5,"probe2_f":44.7,"radio_id":615308292975}
```

The ID and readings above are an **upstream example**, not your device. Search
may take several minutes. Once found, it listens on that channel with the
normal five-byte address and hardware CRC. A discovery with no subsequent
normal packets returns to search after 90 seconds. Search starts again after
each reboot unless an ID is configured. The receiver remains entirely passive:
it never transmits data or auto-acknowledges packets.

### Optional: fixed radio ID

Copy the ID from a confirmed `listen` event. Edit `platformio.ini` and uncomment
`-DSMOKE_RADIO_ID=...ULL`, replacing the example with your ID in decimal or
hexadecimal. `radio_id` is a 40-bit number (always exactly representable as a
JavaScript integer). Rebuild/upload. By default it listens on channel 10; if
you observed 40 or 70, uncomment `-DSMOKE_RF_CHANNEL=40` and set that channel.
A configured ID that stops sending generates a `signal_lost` event, but does
not switch to another ID.

## Serial contract

115200 baud, one JSON object per newline. Readings have numeric `probe1_f`,
`probe2_f` and decimal `radio_id`. A disconnected probe is `null`, not a stale
temperature. No clock is set on the ESP32; timestamp readings on the host.
Other objects have an `event` field (`boot`, `radio_ready`, `search`,
`candidate`, `listen`, `signal_lost`, `signal_restored`, `reacquire`, or
`error`). Only firmware-emitted lines use NDJSON; the ESP32 ROM may print a few
non-JSON bootloader lines on reset, so a host reader should skip lines that
aren't JSON objects. `CORE_DEBUG_LEVEL=0` suppresses framework debug output.
The firmware has no Node/Rust dependency; either can consume this line stream.

## Offline protocol check

The decoder has no Arduino dependencies. With a C++17 compiler:

```sh
g++ -std=c++17 -Wall -Wextra -Werror -Isrc tests/test_protocol.cpp -o /tmp/smoke-protocol-test
/tmp/smoke-protocol-test
```

This checks the upstream sample address, CRC, temperatures, Celsius conversion,
disconnected-probe flag, and bad-frame handling without any radio hardware.
