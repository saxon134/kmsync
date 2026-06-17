# User Guide

This guide covers the current KMSync MVP: user installation, platform
permissions, network diagnostics, and common troubleshooting. It intentionally
states current limits where production features are still pending.

## User Installation

### macOS

1. Build or download the macOS package artifact.

   ```bash
   bash packaging/macos/build-pkg.sh
   ```

2. Install the generated package from `dist/macos`.

3. Confirm the daemon is available.

   ```bash
   /usr/local/bin/kmsync-daemon info
   ```

4. Start a separately built control-plane server when testing locally.

   ```bash
   kmsync-server configs/kmsync-server.example.json
   ```

5. Start the resident Core Service for local testing.

   ```bash
   /usr/local/bin/kmsync-daemon core-service configs/daemon.example.json
   ```

6. Check the control command can reach the Core Service.

   ```bash
   /usr/local/bin/kmsync-daemon status
   ```

The current macOS package is unsigned and not notarized. Gatekeeper may require
manual approval for local MVP testing.

Open `KMSync.app` from `/Applications` to start the desktop Core Service with
the app's macOS Accessibility and Input Monitoring permissions. The Core
Service keeps the QUIC data-plane listener, heartbeat loop, and local IPC
control endpoint alive while the desktop app is in use. The package also
installs a permission guide at
`/usr/local/share/kmsync/docs/USER_GUIDE.md` and an uninstall cleanup helper at
`/usr/local/share/kmsync/uninstall-macos.sh`.

The same `kmsync-daemon` executable also exposes lightweight control commands
such as `status`, `ping`, `layout-editor`, and `control-panel`; those commands
talk to the Core Service over local IPC and do not handle keyboard or mouse
events.

### Windows

1. Build or download the Windows x64 installer artifact.

   ```powershell
   packaging\windows\build-nsis.ps1
   ```

2. Install the generated setup executable from `dist\windows`.

3. Run the daemon from an interactive user desktop.

   ```powershell
   kmsync-daemon.exe info
   ```

4. Start a separately built control-plane server when testing locally.

   ```powershell
   kmsync-server.exe configs\kmsync-server.example.json
   ```

5. Start the resident Core Service for local testing.

   ```powershell
   kmsync-daemon.exe core-service configs\daemon.example.json
   ```

6. Check the control command can reach the Core Service.

   ```powershell
   kmsync-daemon.exe status
   ```

The current Windows installer is unsigned. Windows SmartScreen may warn during
local MVP testing.

The installer adds a machine-wide Run entry for the user-mode companion. The
companion launches `kmsync-daemon.exe core-service`, keeping input
capture/injection in the interactive desktop session. The installer also adds
Start Menu shortcuts for daemon diagnostics, status, and the permission guide,
and removes those entries during uninstall.

For the portable Windows zip, unzip the package, then run
`enable-firewall.cmd` once as administrator. It adds the inbound UDP 24800
Windows Firewall rule required for direct LAN input sync. Start KMSync from
`kmsync.exe`; if diagnostics still show the sync receiver offline, run
`start-core-service.cmd` from the same folder to start the user-mode receiver
against `configs\daemon.example.json`.

### Linux

Linux is present as a platform layer with X11 input capture wired through
XInput2 raw events and X11 input injection wired through the XTEST extension.
Wayland portal/compositor backends are still degraded. Use:

```bash
cargo run -p kmsync-daemon -- info
```

The output explains whether the session is X11, Wayland, or unknown. On X11 it
reports input capture and injection as available when the XInput2 and XTest
backends can be selected; Linux clipboard support still reports degraded
capability status.

## Permission Settings

### macOS Permissions

Open System Settings and grant the daemon:

| Permission | Why it is needed |
| --- | --- |
| Accessibility | Required for synthetic keyboard, mouse, and scroll injection. |
| Input Monitoring | Required for global keyboard and mouse capture. |

After changing these permissions, restart `kmsync-daemon`. If input capture
or injection still fails, run:

```bash
kmsync-daemon info
kmsync-daemon self-test mac-to-windows
```

Both commands print `permission_check` lines for Accessibility and Input
Monitoring, including `granted` or `missing` status and the next action.

### Windows Permissions

Run the user-mode companion in the active interactive user session. The Windows
installer starts only that companion, because normal desktop hooks and
`SendInput` must run in the logged-in user's desktop. This MVP still does not
control the secure desktop, UAC consent screen, login screen, or locked
workstation.

If injection fails, run:

```powershell
kmsync-daemon.exe info
kmsync-daemon.exe self-test windows-to-mac
```

Both commands print a `windows.interactive_desktop` permission check. A missing
status means the companion is not running in the active user desktop and should
be started from the signed-in user session.

### Linux Permissions

On X11, capture uses the XInput2 extension and injection uses the XTEST
extension. KMSync verifies those extensions when capture starts or the first
remote event is injected. Wayland needs a trusted portal or compositor-specific
backend for both capture and injection.

## Network Diagnostics

### Local Server

Start the development server:

```bash
kmsync-server /etc/kmsync/kmsync-server.json
```

The server reads a JSON configuration file. All control-plane state, including
users, devices, sessions, profiles, relay/signaling state, and heartbeat
presence, is stored in the local file named by `data_path`:

```json
{
  "bind": "0.0.0.0:24888",
  "data_path": "/var/lib/kmsync/server-state.json"
}
```

Check that each client can reach it through the configured server URL in
`configs/daemon.example.json`.

After deploying a server build, verify that the health response exposes the
relay receiver status capability. Older servers can keep device heartbeats
online but cannot tell the desktop whether the sync receiver is connected:

```bash
curl http://<server-ip>:24888/health
```

The response should include `"capabilities":{"relay_rx_status":true}`.

### Device Presence

Refresh presence and LAN IP information:

```bash
kmsync-daemon heartbeat configs/daemon.example.json
kmsync-daemon devices configs/daemon.example.json
```

The device list should show the target device as online with a reachable LAN
address and port. Newer servers also show `relay_rx_online`; if that value is
missing, update the server before trusting the sync-channel status.

Probe the configured target from the master before testing edge capture:

```bash
kmsync-daemon target-probe configs/daemon.example.json <target_device_id>
kmsync-daemon target-input-test configs/daemon.example.json <target_device_id>
```

`target-probe` sends a control frame. `target-input-test` sends a reliable
zero-delta scroll input through the same desktop transport path as real sync; it
should not move the pointer, but it verifies that reliable input frames can
reach the target receiver. If it fails with `TargetOffline`, the target's Core
Service receiver or the server relay is not connected.

Run the resident Core Service when you want one process to keep the network data
plane, heartbeat, and local IPC control endpoint alive:

```bash
kmsync-daemon core-service configs/daemon.example.json
```

### Local IPC Control Plane

The daemon exposes a local control channel for the UI/Core Service split. It
uses Windows named pipes on Windows and Unix domain sockets on macOS/Linux.
Input events do not travel over this channel.

Print the default endpoint:

```bash
kmsync-daemon ipc-endpoint
```

Run a one-shot local IPC server for smoke testing:

```bash
kmsync-daemon ipc-serve-once
```

From another terminal, verify the channel:

```bash
kmsync-daemon ipc-ping
kmsync-daemon ping
kmsync-daemon status
```

The network protocol also has a separate control lane for heartbeat,
capability negotiation, configuration version, and session state messages. This
keeps control traffic out of the input injection and clipboard queues in the
MVP listener.

### Profile Sync

List profiles:

```bash
kmsync-daemon profiles configs/daemon.example.json
```

Upload a profile when needed:

```bash
kmsync-daemon profile-set configs/daemon.example.json <source_device_id> <target_device_id> configs/mac-to-windows.profile.json
```

### Device Layout Profiles

Profiles may include a `device_layout` block for multi-target layouts. The
daemon validates that each edge binding references one of the declared target
devices when profiles are parsed or compiled.

```json
{
  "device_layout": {
    "targets": [
      { "device_id": "linux-left", "display_name": "Linux Workstation" },
      { "device_id": "windows-right", "display_name": "Windows Tower" },
      { "device_id": "macbook-top", "display_name": "MacBook" },
      { "device_id": "studio-bottom", "display_name": "Studio" }
    ],
    "edges": {
      "left": "linux-left",
      "right": "windows-right",
      "top": "macbook-top",
      "bottom": "studio-bottom"
    }
  }
}
```

Omit an edge to keep that edge local. Duplicate target IDs and edge bindings to
unknown target IDs are rejected by the profile parser.

Generate a local graphical layout editor from a profile:

```bash
kmsync-daemon layout-editor configs/mac-to-windows.profile.json target/kmsync-layout.html
```

Open the generated HTML file to bind target devices to the left, right, top,
and bottom screen edges with drag-and-drop controls. The editor keeps the
updated `device_layout` in the Profile JSON panel so it can be copied back into
the profile before running `profile-set`.

Generate the broader local control panel:

```bash
kmsync-daemon control-panel configs/mac-to-windows.profile.json target/kmsync-control.html
```

The control panel provides login, device list, layout, keyboard habit,
clipboard sync, network diagnostic, and permission guidance views. It calls the
development server JSON API from the browser, so the server enables local UI
CORS preflight support for those requests.

### Direct LAN Connection Diagnostics

Render a candidate report for a target device:

```bash
kmsync-daemon connection-diagnostics configs/daemon.example.json <target_device_id>
```

Use the report to confirm:

- mDNS candidates are discovered.
- Backend presence candidates contain fresh LAN addresses.
- The target device has a valid Ed25519 public key.
- No sensitive clipboard or key content appears in the report.

### Release And Rollout Diagnostics

Check whether the current daemon version is eligible for an update:

```bash
kmsync-daemon update-check configs/daemon.example.json
```

The report includes `auto_update_action`, `force_update`, rollout percentage,
the deterministic device rollout bucket, download URL, installer SHA-256, and
signature URL. The command sends release metadata only; it does not upload
clipboard content or input data.

### Input Link Smoke Test

On the target computer:

```bash
kmsync-daemon listen 0.0.0.0:24800
```

On the source computer:

```bash
kmsync-daemon send-demo <target-ip>:24800 mac-to-windows
```

For live capture:

```bash
kmsync-daemon capture-send <target-ip>:24800 mac-to-windows right 2
```

Edge mode activates remote control only when the pointer reaches the configured
edge. Press `Ctrl+Alt+Esc` to return control locally.
On activation, the daemon sends a normalized pointer position for the opposite
target edge before forwarding the first movement event, so the remote pointer
enters from the matching screen edge instead of jumping from an unrelated
location.
While remote control is active, supported source platforms hide the local
pointer. Releasing remote control restores local pointer visibility and moves it
back to the position where remote control was activated.

To keep selected applications local even while capture is running, pass a final
comma-separated application exception list:

```bash
kmsync-daemon capture-send <target-ip>:24800 mac-to-windows right 2 ctrl+alt+escape 250 Code,Photoshop
```

Matching uses the foreground application identifier or process name reported by
the platform. The daemon queries that source only when exception rules are
configured, so the default input path stays lean.

## Clipboard Usage

Read or write local clipboard text:

```bash
kmsync-daemon clip-get
kmsync-daemon clip-set "hello"
```

Send current clipboard content once:

```bash
kmsync-daemon clip-send <target-ip>:24800
```

Watch for clipboard changes:

```bash
kmsync-daemon clip-watch <target-ip>:24800 1
```

On Windows this waits for the native `WM_CLIPBOARDUPDATE` notification. On macOS
it uses NSPasteboard `changeCount` before reading clipboard content. The interval
argument is kept as a fallback cadence for platforms without a notification path.

`clip-watch` also accepts product-safety controls:

```bash
kmsync-daemon clip-watch <target-ip>:24800 1 1048576 enabled 300 OnePassword,Bitwarden
```

The optional values are interval seconds, maximum clipboard bytes, sync switch,
expiry TTL seconds, and a comma-separated sensitive application blacklist. When
a clipboard item is blocked, the daemon logs only the reason and byte count.
Common password managers such as 1Password, Bitwarden, KeePass/KeePassXC,
LastPass, Dashlane, Keeper, Enpass, and Proton Pass are filtered by default
when the platform reports the active source application. Windows reports the
foreground process image name for this filter.

The current clipboard channel supports plain text, URLs, basic HTML with a
plain-text fallback, RGBA image clipboard payloads, file clipboard metadata,
and fixed-size file transfer chunks.

Send a file as clipboard file metadata plus transfer chunks:

```bash
kmsync-daemon file-send <target-ip>:24800 ./report.pdf
```

`file-send` defaults to 1024-byte chunks. The listener logs file count, total
bytes, content hash, chunk index, chunk offset, and chunk size only. It does not
print file names or file bytes. These chunks are sent on the reliable QUIC
clipboard stream so large clipboard transfers stay away from mouse-move
Datagrams.

## FAQ

### Why does macOS capture or injection fail after installation?

The daemon needs Accessibility and Input Monitoring permissions. Grant both,
then restart the daemon. If the system prompt does not appear, add the daemon
manually in System Settings.

### Why does Windows stop working on the lock screen or UAC screen?

The MVP runs input capture and injection only in the user-mode companion.
Secure desktop, login screen, and locked workstation control still require
additional service hardening and privilege-bound handoff logic.

### Why does Linux report unavailable input capture or injection?

On X11, raw input capture is wired through XInput2 and injection is wired
through XTest. On Wayland, both capture and injection remain degraded until a
portal or compositor-specific backend is added.

### Why do mouse movements feel more responsive than clipboard sync?

Input events and clipboard data use separate runtime paths. Mouse movement is
coalesced and sent on QUIC Datagram, while keyboard/buttons/scroll and
clipboard data use reliable QUIC streams away from the input hot path.

### Why are some system shortcuts kept local?

Some shortcuts are reserved by the operating system or cannot be safely injected
remotely. Known reserved combinations are kept local so they do not unexpectedly
lock or switch the remote machine.

### Why do two computers need matching profiles?

Profiles define source OS, target OS, modifier mapping, keyboard mode, pointer
speed, and scroll behavior. Use `mac-to-windows` when controlling Windows from
macOS and `windows-to-mac` when controlling macOS from Windows.

### Is the current transport production ready?

Partially. The current data path uses QUIC Datagram for mouse movement and
reliable QUIC streams for reliable input, clipboard, and control frames. Business
frames are additionally wrapped in data-plane encryption with replay protection,
key epochs, and revoked-device rejection. Production builds should still pin the
QUIC certificate or session identity to the verified device public key instead
of using the development self-signed verifier.
