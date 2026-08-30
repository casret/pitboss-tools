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
