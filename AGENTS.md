# AGENTS.md

## Project

`pitboss-tools` is a local-first Rust monitor and supervisory controller for a
Pit Boss PBV4DX vertical pellet smoker. The production executable runs natively
on Windows so it can use the Windows Bluetooth stack while being developed and
cross-compiled from WSL.

## Safety

- Keep new device interactions read-only until they have been validated against
  the physical smoker and reviewed by the user.
- Never implement remote startup or bypass the smoker's factory safety logic.
- Never switch mains power, the auger, fan, or igniter directly. Future control
  may adjust only the factory temperature setpoint.
- Automatic control must default off and include stale-sensor, disconnect,
  over-temperature, flameout, command-rate, and setpoint-limit protections.
- Do not log credentials, grill passwords, or unique device identifiers.

## Development

- Use Rust and prefer a single native binary with minimal runtime dependencies.
- Keep platform-independent parsing and controller logic testable on Linux.
- Gate Windows Bluetooth implementation behind `cfg(target_os = "windows")`.
- Format and check changes with:

  ```sh
  cargo fmt --check
  cargo test
  cargo clippy --all-targets -- -D warnings
  cargo clippy --target x86_64-pc-windows-gnu -- -D warnings
  ```

- Build the Windows release with:

  ```sh
  cargo build --release --target x86_64-pc-windows-gnu
  ```

## Version control

This is a colocated Jujutsu repository. Use `jj` for mutations and Git only for
read-only compatibility. Always provide `-m` to description-writing commands.
