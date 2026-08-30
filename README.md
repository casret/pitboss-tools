# pitboss-tools

Local monitoring and, eventually, supervisory temperature control for a Pit Boss
PBV4DX vertical pellet smoker.

The application is a native Windows Rust binary so it can use the Windows BLE
stack without taking the Bluetooth dongle away from Windows. It will expose a
local web UI/API after the BLE protocol has been validated.

## Safety status

The current implementation is **read-only**. It only scans BLE advertisements;
it does not connect to or send commands to the smoker.

Automatic control will not be enabled until temperature channels and commands
have been verified against the physical controller. The future controller will
adjust only the factory temperature setpoint; it will not directly operate the
auger, fan, igniter, or mains power.

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

The `serve` command records every temperature frame in SQLite and serves a
single-page dashboard on loopback. It treats probe 1 as grate ambient and probe
2 as meat, matching this PBV4DX setup:

```powershell
.\pitboss-tools.exe serve --database pitboss.sqlite3 --target 225 --control
```

The `--control` flag is required to start automatic control. Without it, the
server remains monitor-only. The dashboard can enable/disable control and
change the grate target. Control adjusts only the Pit Boss factory temperature
setpoint; it never switches mains power or directly operates the auger, fan, or
igniter.

The command password is read from `conf.toml` (ignored by Git) or the
`PITBOSS_GRILL_PASSWORD` environment variable. A minimal config is:

```toml
grill_id = "PBL3-your-device-id"
grill_password = "your-grill-password"
```

Keep `conf.toml` private. The server does not print the password. Its guarded
controller stops on stale/disconnected probes, over-temperature, possible
flameout, failed commands, and enforces 5°F setpoint increments plus a
120-second command interval. It starts only after the smoker is already on;
remote startup is not implemented.

The SQLite `samples` table contains timestamped grate, meat, chamber, factory
setpoint, control-enabled, and target values. The JSON endpoints are:

- `GET /api/state`
- `GET /api/history?limit=100`
- `POST /api/control` with `{ "enabled": true, "target_f": 225 }`
