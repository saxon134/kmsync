# 打包说明

本文档说明 KMSync MVP 的 macOS 和 Windows 安装包构建方式。

## 交付内容

当前项目包含三个可执行程序：

- `kmsync-daemon`：桌面端 daemon，负责输入捕获、远端输入注入、剪贴板同步、心跳上报，并提供 `status`、`ping`、`layout-editor`、`control-panel` 控制命令。
- `kmsync-server`：控制面服务端，作为独立服务端二进制构建和部署，不放进桌面安装包。

macOS 安装包会同时安装：

```text
/usr/local/bin/kmsync-daemon
/usr/local/share/kmsync/configs/daemon.example.json
/usr/local/share/kmsync/configs/mac-to-windows.profile.json
/usr/local/share/kmsync/configs/windows-to-mac.profile.json
```

## macOS 打包

要求：

- macOS
- Rust toolchain
- `pkgbuild`
- `lipo`

命令：

```bash
bash packaging/macos/build-pkg.sh
```

Developer ID signing and notarization are enabled when credentials are present:

```bash
export CODESIGN_IDENTITY="Developer ID Application: Example, Inc. (TEAMID)"
export PKG_SIGN_IDENTITY="Developer ID Installer: Example, Inc. (TEAMID)"
export APPLE_ID="release@example.com"
export APPLE_TEAM_ID="TEAMID"
export APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx"
bash packaging/macos/build-pkg.sh
```

The script signs staged binaries with hardened runtime, signs the package,
submits it with `xcrun notarytool`, and staples the ticket. If these variables
are not set, local MVP builds remain unsigned and notarization is skipped with
an explicit message.

The generated LaunchAgent starts `kmsync-daemon core-service
/usr/local/share/kmsync/configs/daemon.example.json` at login. That resident
process owns the data-plane listener, heartbeat loop, and local IPC control
endpoint; input forwarding stays inside the daemon hot path rather than a UI
process.

输出：

```text
dist/macos/kmsync-daemon-0.1.0-macos.pkg
```

说明：

- 如果本机安装了 `aarch64-apple-darwin` 和 `x86_64-apple-darwin` 两个 Rust target，脚本会生成 universal binary。
- 当前 `.pkg` 未签名、未 notarize。正式分发需要 Apple Developer ID。

安装后检查：

```bash
/usr/local/bin/kmsync-daemon info
kmsync-server configs/kmsync-server.example.json
```

## Windows 打包

Windows 安装包需要在 Windows 环境生成，不能在普通 macOS 环境直接产出。

要求：

- Windows 2022 / Windows 11
- Visual Studio Build Tools，包含 C++ build tools
- Rust toolchain
- `x86_64-pc-windows-msvc` target
- NSIS，提供 `makensis.exe`

PowerShell 命令：

```powershell
rustup target add x86_64-pc-windows-msvc
packaging\windows\build-nsis.ps1
```

Authenticode signing is enabled by certificate thumbprint or PFX:

```powershell
packaging\windows\build-nsis.ps1 `
  -AuthenticodeCertificateThumbprint "0123456789ABCDEF0123456789ABCDEF01234567" `
  -TimestampUrl "http://timestamp.digicert.com"
```

or:

```powershell
packaging\windows\build-nsis.ps1 `
  -PfxPath C:\certs\kmsync-code-signing.pfx `
  -PfxPassword $env:KMSYNC_PFX_PASSWORD
```

The script signs `kmsync-daemon.exe` and the generated NSIS installer when signing is
configured. If signing parameters are absent, it emits a clear skip message and
leaves artifacts unsigned.

The NSIS installer writes a machine-wide Run entry that starts the user-mode
companion via `kmsync-daemon.exe core-service` after login. It also stops and
deletes the legacy `KMSyncCoreService` if an older package left it installed,
so the system service cannot take the data-plane port away from the interactive
desktop companion. Input capture/injection stays in the companion for the MVP;
secure desktop and login-screen control still need additional hardening.

输出：

```text
dist\windows\kmsync-daemon-0.1.0-windows-x64-setup.exe
```

说明：

- 本仓库提供 NSIS 脚本：`packaging/windows/kmsync-daemon.nsi`
- Windows 安装包会包含：
  - `kmsync-daemon.exe`
  - 示例配置文件
- 当前 Windows 安装包未签名。正式分发需要 Authenticode 代码签名证书。

## GitHub Actions 打包

已提供双平台 workflow：

```text
.github/workflows/package.yml
```

触发方式：

- 手动 `workflow_dispatch`
- 推送 `v*` tag

产物：

- macOS artifact：`kmsync-daemon-macos-pkg`
- Windows artifact：`kmsync-daemon-windows-setup`

## 本机检查结果

本机已完成：

- `cargo fmt --check`
- `cargo test --quiet`
- `cargo check -p kmsync-daemon -p kmsync-server --target x86_64-pc-windows-msvc`
- macOS `.pkg` 打包

本机无法直接完成：

- Windows `.exe` 安装包

原因：

- 当前机器是 macOS。
- `x86_64-pc-windows-msvc` release 链接需要 Windows `link.exe`。
- 本机没有 Visual Studio Build Tools。
- 本机没有 NSIS `makensis`。

因此 Windows 安装包请在 Windows 机器或 GitHub Actions Windows runner 上生成。
