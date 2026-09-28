# ThermoWorks Smoke → USB serial

Small, receive-only firmware for the **original two-channel ThermoWorks Smoke**
and an ESP32-WROOM-32 development board (PlatformIO `esp32dev`, Arduino). An
nRF24L01+ listens on hardware VSPI; no Wi-Fi, BLE, pairing, MQTT, cloud, or
handheld-receiver shutdown is involved. The handheld receiver can stay on.
The firmware is separate from the Pit Boss BLE code; the Windows dashboard can
read its USB serial stream.

The RF settings, CRC search, 5-byte radio ID and 21-byte payload layout follow
[stefslon/esphome-thermoworks-smoke](https://github.com/stefslon/esphome-thermoworks-smoke/tree/9da52e863b184717864a4c9cbc4b9a72e7d79ae6)
(MIT; see `UPSTREAM-LICENSE`). RF24 is the only external firmware library.
Validated with a physical original Smoke: with only the lower probe plugged in,
the receiver reported probe 1 as `null` and probe 2 as 76.0°F, matching the
handheld. Other transmitters and board variants have not been validated.

## Wiring

| nRF24L01+ | ESP32-WROOM-32 label | Current harness color |
| --- | --- | --- |
| VCC | **3V3 only, never VIN/5V** | Red |
| GND | GND | Tan/brown |
| CE | D4 / GPIO 4 | Gray |
| CSN | D5 / GPIO 5 | Green/teal |
| SCK | D18 / GPIO 18 | Purple |
| MOSI | D23 / GPIO 23 | Yellow |
| MISO | D19 / GPIO 19 | Blue |
| IRQ | Not connected | None |

**Radio orientation:** with the component side facing you, antenna at the top
and 2×4 pins at the bottom, the row **toward the antenna** reads left to right
`VCC (red) · CSN (green) · MOSI (yellow) · IRQ (empty)`. The row **toward the
board edge** reads `GND (tan) · CE (gray) · SCK (purple) · MISO (blue)`.
The square pad identifies GND. The colors above identify the wires seen at
the ESP32 end in the wiring photos; they are **not a universal color standard**.
Before powering a rebuilt harness, verify each radio-end connection against
its pin and check for a VCC–GND short rather than trusting color alone.

**ESP32 empty gaps:** look at the board's printed/component side with its USB
connector on the **left**. On the long header that starts with `3V3, GND` at
the USB end, read **left to right** toward the ESP32 antenna. Every position
below is one adjacent pin; **EMPTY means no wire**:

| Position | ESP32 label | Wire |
| ---: | --- | --- |
| 1 | 3V3 | Red |
| 2 | GND | Tan/brown |
| 3 | D15 | **EMPTY** |
| 4 | D2 | **EMPTY** |
| 5 | D4 | Gray (CE) |
| 6 | RX2 | **EMPTY** |
| 7 | TX2 | **EMPTY** |
| 8 | D5 | Green/teal (CSN) |
| 9 | D18 | Purple (SCK) |
| 10 | D19 | Blue (MISO) |
| 11 | D21 | **EMPTY** |
| 12 | RX0 | **EMPTY** |
| 13 | TX0 | **EMPTY** |
| 14 | D22 | **EMPTY** |
| 15 | D23 | Yellow (MOSI) |

The radio's only empty position is **IRQ**: antenna-side row, fourth pin
from the left in the orientation above. Do not insert a wire there.

Unplug USB before changing wiring. Inspect the radio's VCC and GND solder
pads for a bridge before powering it, and never reuse a module that overheated
after 5 V was applied. Use short wires and, if the module resets or reception
is flaky, place a 10–47 µF capacitor directly between its 3V3 and GND pins
(observe capacitor polarity). The ESP32's USB-to-serial port connects to the PC;
a bare WROOM module needs an external 3.3 V USB-UART adapter and normal
flashing circuitry.

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
`search_status`, `crc_frame`, `candidate`, `listen`, `signal_lost`,
`signal_restored`, `reacquire`, or `error`). Each `search_status` summarizes the
last 22-second slot's raw packet count and CRC matches (CRC positions 3–26),
which helps distinguish RF silence from decoding failures. Up to three
`crc_frame` events per slot include CRC-validated raw hex to diagnose payload
variants; this contains the transmitter's radio ID. Only firmware-emitted
lines use NDJSON; the ESP32 ROM may print a few non-JSON bootloader lines on
reset, so a host reader should skip lines that
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
