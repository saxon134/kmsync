# KMSync MVP Release

## Included Binaries

- `kmsync-daemon`
  - Desktop client daemon for macOS and Windows.
  - Captures local input, sends mapped input to another device, receives remote input, injects events locally, syncs text/HTML/image clipboard content, and exposes local IPC for UI/Core Service control.
  - Also exposes the lightweight control commands: `status`, `ping`, `layout-editor`, and `control-panel`.
- `kmsync-server`
  - Development control-plane server.
  - Provides email-code login, device registration, heartbeat/IP presence, device listing, profile sync, and release policy checks.
  - Reads runtime settings from a JSON configuration file.
  - Persists users, devices, sessions, profiles, relay/signaling state, and heartbeat presence to the local JSON file configured by `data_path`.

## Supported Platforms

- macOS arm64 and x86_64.
- Windows x86_64 via the Windows packaging workflow.
- Detailed OS and Linux degradation coverage is tracked in
  [`COMPATIBILITY.md`](./COMPATIBILITY.md).

Desktop installers contain the desktop client only. Deploy `kmsync-server`
separately on the control-plane host.

## Main Commands

For end-user installation, permission setup, network diagnostics, and FAQ, see
[`USER_GUIDE.md`](./USER_GUIDE.md).

Start a local control-plane server:

```bash
kmsync-server /etc/kmsync/kmsync-server.json
```

Example server configuration:

```json
{
  "bind": "0.0.0.0:24888",
  "data_path": "/var/lib/kmsync/server-state.json"
}
```

Register this device and publish heartbeat/IP:

```bash
kmsync-daemon heartbeat configs/daemon.example.json
```

Run the resident Core Service. This keeps the QUIC data-plane listener,
heartbeat loop, and local IPC control endpoint alive without routing input
through a UI process:

```bash
kmsync-daemon core-service configs/daemon.example.json
```

Listen for remote input and clipboard content:

```bash
kmsync-daemon listen 0.0.0.0:24800
```

Capture all local input and forward it:

```bash
kmsync-daemon capture-send <target-ip>:24800 mac-to-windows all
```

Activate remote control only when the pointer reaches an edge:

```bash
kmsync-daemon capture-send <target-ip>:24800 mac-to-windows right 2
```

When edge mode is active, local events are suppressed and remote events are sent. Press `Ctrl+Alt+Esc` to release local control.

Watch clipboard text, HTML, and images, then send changes:

```bash
kmsync-daemon clip-watch <target-ip>:24800 1
```

`clip-watch` uses Windows clipboard update notifications and macOS pasteboard
change counters; the numeric interval remains a fallback cadence.

Send a file as clipboard file metadata and fixed-size transfer chunks:

```bash
kmsync-daemon file-send <target-ip>:24800 ./report.pdf
```

Check the release policy and deterministic rollout decision for this device:

```bash
kmsync-daemon update-check configs/daemon.example.json
```

Print and smoke-test the local IPC control endpoint:

```bash
kmsync-daemon ipc-endpoint
kmsync-daemon ipc-serve-once
kmsync-daemon ipc-ping
kmsync-daemon ping
kmsync-daemon status
```

Generate a local graphical device layout editor:

```bash
kmsync-daemon layout-editor configs/mac-to-windows.profile.json target/kmsync-layout.html
```

Generate the local control panel with login, devices, layout, habits,
clipboard, network diagnostics, and permission guidance:

```bash
kmsync-daemon control-panel configs/mac-to-windows.profile.json target/kmsync-control.html
```

The data protocol also defines a separate control lane for heartbeat,
capability negotiation, configuration version, and session state messages. The
MVP listener routes control frames away from input injection and clipboard
queues; QUIC reliable stream transport is wired for control, reliable input,
and clipboard frames.

## Permission Notes

macOS:

- Enable Input Monitoring for input capture.
- Enable Accessibility for input injection.
- The `.pkg` is currently unsigned and not notarized.

Windows:

- The installer registers `KMSyncCoreService` and a user-mode companion Run entry.
- Run the companion on the interactive desktop for hooks and `SendInput`.
- Elevated/UAC secure desktop control is not included in this MVP.
- The generated installer is currently unsigned.

## MVP Limitations

- Transport uses QUIC for the daemon data path. Mouse movement is sent as QUIC Datagram; reliable input, clipboard, and control frames use reliable streams.
- Discovery uses mDNS LAN browsing plus control-plane presence; P2P NAT traversal is not implemented yet.
- The server stores its control-plane state in a local JSON file configured by `data_path`; no MySQL or Redis service is required for the MVP backend.
- Authentication is email-code login for MVP testing; production OAuth is not included yet.
- Automatic update installation is not performed yet; the server and daemon expose update metadata, integrity hash, signature URL, force-update status, and deterministic rollout eligibility.
- Clipboard sync supports text, URL, basic HTML, RGBA image payloads, file metadata, and fixed-size file transfer chunks over the QUIC clipboard stream.
- Edge mode supports one target edge at a time.
