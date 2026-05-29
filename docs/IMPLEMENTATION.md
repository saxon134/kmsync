# MVP Implementation

This document summarizes the current Rust workspace implementation.

## Crates

- `kmsync-core`
  - Cross-platform input event model.
  - Keyboard/mouse mapping profiles.
  - Clipboard payload helpers.
  - Data-plane protocol and tests.
- `kmsync-daemon`
  - Desktop daemon for macOS, Windows, and Linux.
  - Platform adapters for input capture/injection.
  - QUIC data-plane listener and sender commands.
  - Heartbeat, device list, profile sync, local IPC, and clipboard commands.
- `kmsync-ui`
  - Shared renderer/library for local layout editor and control-panel HTML.
  - Its control commands are exposed through `kmsync-daemon` for the desktop package.
- `kmsync-server`
  - Control-plane API for email-code login, device registration, heartbeat/IP presence, device list, profile sync, relay token issuance, signaling, and release policy checks.
  - Reads runtime options only from a JSON config file.
  - Stores users, devices, sessions, refresh tokens, profiles, relay/signaling state, and heartbeat presence in one local JSON state file.

## Server

Run the server with a config file path:

```bash
cargo run -p kmsync-server -- configs/kmsync-server.example.json
```

The config file contains only:

```json
{
  "bind": "0.0.0.0:24888",
  "data_path": "/var/lib/kmsync/server-state.json"
}
```

`data_path` is required. No database, cache service, or environment variable is used by the server runtime.

When a client heartbeats with an existing `device_id`, the server updates that device's presence record in place: LAN IP list, public IP, listen port, NAT type, and `last_seen_at` are refreshed while the device remains attached to the same user.

## Useful Commands

```bash
cargo test
cargo test -p kmsync-server
cargo run -p kmsync-daemon -- info
cargo run -p kmsync-daemon -- heartbeat configs/daemon.example.json
cargo run -p kmsync-daemon -- devices configs/daemon.example.json
cargo run -p kmsync-daemon -- profiles configs/daemon.example.json
cargo run -p kmsync-daemon -- status
```

For Linux service deployment, see `packaging/linux/README.md`.
