# pitboss-tools

Local monitoring and guarded supervisory temperature control for a Pit Boss
PBV4DX vertical pellet smoker.

The native Windows Rust binary uses the Windows BLE stack and provides a local
web UI/API. Automatic control defaults off and only adjusts the factory
setpoint; it never operates the auger, fan, igniter, or mains power directly.

The original two-channel ThermoWorks Smoke has separate receive-only
ESP32/nRF24L01+ firmware in
[firmware/thermoworks-smoke](firmware/thermoworks-smoke/README.md).
The Windows dashboard can also read its USB serial stream without pairing or
turning off the handheld receiver. Only one program can open the COM port at a
time: close any serial monitor before starting `serve`.

## Build

Rust 1.89 or newer is required.

From WSL, install the Windows GNU target and linker:

```sh
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64
```

Then build:

```sh
cargo build --release --target x86_64-pc-windows-gnu
```

The executable will be at:

```text
target/x86_64-pc-windows-gnu/release/pitboss-tools.exe
```

## First scan

Power on the smoker controller without starting a cook, then run from
PowerShell:

```powershell
pitboss-tools.exe scan --seconds 20
```

If the filtered scan finds nothing:

```powershell
pitboss-tools.exe scan --seconds 20 --all
```

The scanner does not connect to discovered devices.

## Read-only GATT inspection

After a candidate is visible, this command connects only long enough to list
its BLE services and characteristics, then disconnects. It does not read,
subscribe to, or write any characteristic and does not send smoker commands:

```powershell
pitboss-tools.exe inspect
```

Pit Boss smokers generally accept a single BLE connection. Close the Pit Boss
mobile app and disable Bluetooth on any phone connected to the smoker before
running the inspection.

## Dashboard and guarded control

The `serve` command records temperature frames in SQLite and serves a
single-page dashboard on loopback. By default it selects **Smoke bottom / probe
2** for grate temperature, and displays the Pit Boss probe 2 as meat. The
dashboard's grate-source selector can instead choose Smoke top or either Pit
Boss probe. Change the COM port with `--smoke-port` if Windows assigned
something other than COM3:

```powershell
.\pitboss-tools.exe serve --database pitboss.sqlite3 --target 225 --smoke-port COM3
```

Control defaults off. The optional `--control` flag requests control at
startup; using the dashboard to enable it *after* all selected sensors are
fresh and the already-running smoker is hot is safer. Selecting a different
grate source automatically disables control and clears the integral; it does
not silently fall back to another probe if the selected source disconnects.
The dashboard can enable/disable control and change the grate target. Control
adjusts only the Pit Boss factory temperature
setpoint; it never switches mains power or directly operates the auger, fan, or
igniter.

The command password is read from `conf.toml` (ignored by Git) or the
`PITBOSS_GRILL_PASSWORD` environment variable. A minimal config is:

```toml
grill_id = "PBL3-your-device-id"
grill_password = "your-grill-password"
```

Keep `conf.toml` private. The server does not print the password. Its guarded
controller stops on stale/disconnected selected grate data (Smoke: 30 seconds;
Pit Boss BLE: 60 seconds), BLE disconnect, over-temperature, possible
flameout, and failed commands. It enforces 5°F setpoint increments plus a
120-second command interval, requires recent Pit Boss BLE telemetry even when
using Smoke for grate, and will not arm until the factory chamber reads at
least 100°F. It also stops if the chamber rises more than 100°F above the
grate target or differs from the selected grate probe by over 100°F; these
conservative thresholds may need tuning after a monitored cook. It does not
resume automatically after a sensor fault. Remote startup is not implemented.

The SQLite `samples` table contains timestamped selected grate (with source),
meat, chamber, factory setpoint, control-enabled, and target values; the
`smoke_samples` table stores both Smoke probes without the transmitter ID.
Each guarded-control tick is also
written to `control_events` with the selected grate source, error, integral,
P/I/D terms, clamped adjustment, recommended factory setpoint, action, and
reason. Disabling control cannot retract a factory command already in flight;
source changes are blocked until such a command finishes. The dashboard
shows the same live calculation and history. The optional smoker-side HTTP service can be inspected over BLE with:

```powershell
.\pitboss-tools.exe http-config
```

If the firmware exposes a Mongoose `http.enable` setting, pass
`--enable` to write that setting and reboot the smoker:

```powershell
.\pitboss-tools.exe http-config --enable
```

This command deliberately prints only the enable flag, never a full config.
A missing `http` subtree means the firmware does not include the HTTP service;
BLE cannot add it.

The JSON endpoints are:

- `GET /api/state` (live temperatures plus the latest P/I/D calculation)
- `GET /api/history?limit=100`
- `GET /api/control-events?limit=100`
- `POST /api/control` with `{ "enabled": true, "target_f": 225 }` or
  `{ "grate_source": "smoke_bottom" }` (also `smoke_top`, `pit_boss_probe1`,
  `pit_boss_probe2`; selecting a new source turns control off)
- `POST /api/shutdown` to stop the dashboard and BLE connection (the smoker
  remains on its current factory setting)
