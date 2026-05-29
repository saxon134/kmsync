# Compatibility Matrix

This matrix records the current KMSync MVP compatibility target. It separates
"implemented", "degraded", and "planned" states so packaging, support, and test
work do not accidentally imply more platform coverage than the code provides.

## Status Legend

| Status | Meaning |
| --- | --- |
| Implemented | Code path exists and is covered by unit tests or cross-target compile checks. |
| Degraded | Platform is detected and returns explicit capability errors or permission hints. |
| Planned | Product target exists, but the runtime backend is not wired yet. |
| Unsupported | Not a current target. |

## Desktop OS Matrix

| Platform | Version target | Architecture | Input capture | Input injection | Clipboard | Current status | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Windows | Windows 10 | x86_64 | Low Level Keyboard/Mouse Hook | `SendInput` | Native clipboard API, text, URL, basic HTML, image | Implemented | Requires interactive desktop. UAC secure desktop and login screen are out of scope for MVP. |
| Windows | Windows 11 | x86_64 | Low Level Keyboard/Mouse Hook | `SendInput` | Native clipboard API, text, URL, basic HTML, image | Implemented | Same runtime path as Windows 10. Packaging target is Windows x64. |
| macOS | macOS 12 Monterey | x86_64, arm64 | `CGEventTap` | `CGEventPost` | `NSPasteboard` via native API, text, URL, basic HTML | Implemented | Requires Input Monitoring and Accessibility permissions. |
| macOS | macOS 13 Ventura | x86_64, arm64 | `CGEventTap` | `CGEventPost` | `NSPasteboard` via native API, text, URL, basic HTML | Implemented | Same permission requirements as macOS 12. |
| macOS | macOS 14 Sonoma | x86_64, arm64 | `CGEventTap` | `CGEventPost` | `NSPasteboard` via native API, text, URL, basic HTML | Implemented | Same permission requirements as macOS 12. |
| macOS | macOS 15 Sequoia | x86_64, arm64 | `CGEventTap` | `CGEventPost` | `NSPasteboard` via native API, text, URL, basic HTML | Implemented | Must be manually smoke-tested after permission prompt or notarization changes. |
| Linux | X11 sessions | x86_64 | XInput2 raw events | XTest | X selection | Partial | Current runtime detects X11 and reports capture as `x11_xinput2`, injection as `x11_xtest`, and clipboard as `x11_clipboard_not_wired`. |
| Linux | Wayland sessions | x86_64 | Portal or compositor-specific backend | Portal or compositor-specific backend | xdg-desktop-portal clipboard | Degraded | Current runtime reports explicit Wayland capability limits because global capture and injection are blocked by the compositor security model. |

## Linux Desktop Matrix

| Desktop | Display server | Capture status | Injection status | Clipboard status | Notes |
| --- | --- | --- | --- | --- | --- |
| GNOME | X11 | Supported: XInput2 raw events | Supported: XTest | Planned: X selection | Runtime verifies XInput2 when capture starts and XTEST when the first remote event is injected. |
| KDE Plasma | X11 | Supported: XInput2 raw events | Supported: XTest | Planned: X selection | Same XInput2/XTest backend family as GNOME X11. |
| XFCE | X11 | Supported: XInput2 raw events | Supported: XTest | Planned: X selection | Same XInput2/XTest backend family as other X11 desktops. |
| GNOME | Wayland | Degraded | Degraded | Planned portal support | Global capture and synthetic input require portal or Mutter-specific support. |
| KDE Plasma | Wayland | Degraded | Degraded | Planned portal support | Global capture and synthetic input require portal or KWin-specific support. |
| wlroots compositors | Wayland | Degraded | Degraded | Planned portal support | Capability details vary by compositor. |

## Verification Commands

The current compatibility claims are guarded by these checks:

```powershell
cargo fmt -- --check
cargo test
cargo check -p kmsync-server
cargo check -p kmsync-daemon --target x86_64-apple-darwin
cargo check -p kmsync-daemon --target x86_64-apple-darwin --tests
cargo check -p kmsync-daemon --target x86_64-unknown-linux-gnu
cargo check -p kmsync-daemon --target x86_64-unknown-linux-gnu --tests
```

Manual smoke tests are still required before release on each OS version because
input capture, input injection, clipboard ownership, and permission prompts are
system-integrated behaviors that cross-target compilation cannot fully prove.
