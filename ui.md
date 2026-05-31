# KMSync 桌面端配置页面实现方案

## 1. 目标

为 Windows 和 macOS 桌面端增加一个配置页面，让用户可以在桌面软件里完成设备状态查看、主电脑设置、设备布局配置和连接状态判断。

本次页面需要覆盖四个核心能力：

1. 展示本机的内网 IP 和公网 IP。
2. 展示连接状态：是否已连接服务器、是否已连接主电脑。
3. 支持把本机设置为主电脑，由本机采集键盘、鼠标事件并同步给其他电脑。
4. 支持最多 4 台目标电脑的位置配置：以主电脑为中心，上、下、左、右各绑定一台电脑；鼠标移动到对应边缘时切换到对应目标电脑。

## 2. 当前代码现状

当前工程已经有几个可以复用的基础：

- `crates/kmsync-daemon` 是桌面端主入口，负责 core-service、心跳、设备列表、profiles、输入捕获和输入发送。
- `crates/kmsync-ui` 已经是库模式，包含 `control_panel` 和 `layout_editor` 两个静态 HTML 生成模块。
- `crates/kmsync-core/src/profile.rs` 已有 `device_layout.targets` 和 `device_layout.edges.left/right/top/bottom`，可以直接承载 4 个方向的设备布局。
- 服务端 `/v1/devices` 已返回设备列表和 presence，presence 包含 `lan_ips`、`public_ip`、`listen_port`、`online`、`last_seen_at`。
- 桌面端心跳已经会上报 `lan_ips` 和 `listen_port`，服务端会根据请求来源记录公网 IP。
- 本地 IPC 当前只有 `Ping` 和 `Status`，状态信息偏少，不能支撑完整配置页面。

因此推荐在现有结构上扩展，而不是重做一套独立 UI。

## 3. 推荐方案

推荐使用“桌面 WebView + 本地 IPC/本地命令桥”的方式实现配置页面。

页面本身继续放在 `kmsync-ui`，由 Rust 生成或内嵌 HTML/CSS/JS；Windows 使用 WebView2，macOS 使用 WKWebView。页面不直接访问服务端，不直接保存配置文件，而是调用桌面端提供的本地桥接 API。桌面端 core-service 统一负责读写配置、访问服务器、保存 token、刷新设备状态和应用输入同步配置。

这样做的好处：

- Windows 和 macOS 可以共享同一套页面和交互逻辑。
- 登录 token、设备私钥、配置文件都留在本机 Rust 进程中，页面只拿脱敏状态。
- 输入事件热路径仍然不经过 UI，不影响鼠标键盘同步性能。
- 现有 `device_layout` 配置模型可以继续使用。
- 后续如果要换成 Tauri 或原生 UI，状态模型和 IPC API 仍然可以复用。

备选方案：

- 使用浏览器打开本地 HTML：实现更快，但浏览器不能直接调用 named pipe / unix socket，需要额外本地 HTTP 服务，体验也不像桌面软件。
- 做纯原生 UI：体验最好，但 Windows/macOS 需要分别投入更多平台代码，不适合当前阶段。

## 4. 页面信息架构

配置页面建议分为 4 个区域。

### 4.1 顶部状态栏

展示当前设备的核心状态：

- 当前设备名。
- 当前设备 ID，默认折叠或只显示短 ID。
- 当前角色：主电脑 / 普通电脑。
- 服务器状态：连接中 / 已连接 / 未连接 / 登录失效 / 正在重试。
- 主电脑连接状态：连接中 / 已连接 / 未连接 / 本机就是主电脑。
- 最近刷新时间。

顶部状态栏要持续轮询或订阅 core-service 状态，建议 2 到 5 秒刷新一次。

用户启动桌面端后，不应该看到黑色命令行窗口。Windows 启动 `kmsync.exe` 时应直接进入配置页面或托盘常驻；macOS 从 Applications 启动时应打开配置页面。core-service 的启动、重连、错误和权限状态都展示在页面顶部状态栏和对应卡片中。

### 4.2 网络信息卡片

展示本机网络信息：

- 内网 IP：展示多个地址，例如 `192.168.1.23`、`10.0.0.8`。
- 公网 IP：展示服务端记录的 `public_ip`。
- 监听端口：展示当前 data-plane 端口，例如 `24800`。
- NAT 类型：当前已有字段为 `unknown`，页面可先展示。
- 心跳时间：展示 `last_seen_at` 或转换后的本地时间。

公网 IP 的来源建议优先使用服务端 presence。流程是 core-service 心跳后，再从 `/v1/devices` 找到当前 device 的 presence，取 `public_ip`。后续可以把 `public_ip` 加到 heartbeat response，减少一次列表请求。

### 4.3 本机角色配置

提供一个明确的开关：

```text
[ ] 将本机作为主电脑
```

打开后：

- core-service 启用输入捕获。
- 本机作为 `source_device_id`。
- 布局编辑区域可编辑。
- 鼠标触碰屏幕边缘时，根据 `device_layout.edges` 切换目标设备。

关闭后：

- 本机只作为被控电脑或普通节点。
- 不主动捕获本机键盘鼠标事件。
- 页面展示“主电脑连接状态”，表示是否有主电脑连接到本机。
- 布局编辑区域默认只读，除非用户再次切回主电脑模式。

### 4.4 设备布局区域

布局区域以主电脑为中心，最多四个方向：

```text
          [ 上方电脑 ]

[ 左边电脑 ] [ 主电脑 ] [ 右边电脑 ]

          [ 下方电脑 ]
```

每个方向是一个下拉选择框或设备卡片插槽：

- 可选择同账号下的其他设备。
- 每个设备只能绑定到一个方向。
- 不允许选择当前设备作为目标设备。
- 最多 4 台目标设备。
- 空方向表示该边缘不触发远程同步。
- 离线设备可以保留配置，但要显示离线状态。

设备卡片展示：

- 设备名称。
- 系统类型：Windows / macOS / Linux。
- 在线状态。
- 内网 IP 和公网 IP。
- 最近在线时间。
- 当前连接候选地址，例如 LAN 地址优先。

布局保存后，同步写入本地配置和服务端 profile。

## 5. 配置模型设计

建议新增或扩展一个桌面端本地配置文件，例如 `kmsync.desktop.json`。现有 `daemon.example.json` 仍可兼容，但最终建议统一为一个桌面端配置文件。

示例：

```json
{
  "server_url": "http://127.0.0.1:24888",
  "email": "dev@example.com",
  "device_name": "Kevin Windows",
  "identity_path": "kmsync-device-identity.json",
  "listen_port": 24800,
  "heartbeat_interval_seconds": 15,
  "role": "master",
  "master_device_id": null,
  "layout": {
    "left": "device-windows-left",
    "right": "device-mac-right",
    "top": null,
    "bottom": null
  },
  "profile_path": "profiles/current.profile.json"
}
```

字段说明：

- `role`: `master` 表示本机是主电脑，`client` 表示本机是被控电脑。
- `master_device_id`: 本机不是主电脑时，记录期望连接的主电脑；本机是主电脑时为 `null`。
- `layout`: 主电脑模式下的四方向绑定。
- `profile_path`: 本地保存的 profile JSON 路径，用于离线启动和快速恢复。

保存布局时，同时生成或更新现有 profile 结构：

```json
{
  "device_layout": {
    "targets": [
      {
        "device_id": "device-windows-left",
        "display_name": "Windows Left"
      },
      {
        "device_id": "device-mac-right",
        "display_name": "Mac Right"
      }
    ],
    "edges": {
      "left": "device-windows-left",
      "right": "device-mac-right"
    }
  }
}
```

这样可以复用当前 `Profile::from_config_json` 和现有边缘路由逻辑。

## 6. 本地 IPC/API 设计

当前本地 IPC 只有 `Ping` 和 `Status`，需要扩展成配置页面可用的 API。页面不要直接访问服务端，所有请求都经过桌面端本地桥。

建议新增请求：

```text
GetDesktopState
RefreshNetwork
SetDeviceRole
SetLayout
ApplyConfig
ConnectTarget
DisconnectTarget
OpenPermissionSettings
```

`GetDesktopState` 返回页面首屏所需全部状态：

```json
{
  "device": {
    "id": "current-device-id",
    "name": "Kevin Windows",
    "os": "windows",
    "app_version": "0.1.0",
    "role": "master"
  },
  "network": {
    "lan_ips": ["192.168.1.23"],
    "public_ip": "203.0.113.10",
    "listen_port": 24800,
    "last_seen_at": 1710000000
  },
  "connections": {
    "server": {
      "state": "connecting",
      "last_error": null
    },
    "master": {
      "state": "self",
      "device_id": null,
      "last_error": null
    }
  },
  "devices": [
    {
      "id": "device-mac-right",
      "name": "Mac Right",
      "os": "macos",
      "online": true,
      "lan_ips": ["192.168.1.24"],
      "public_ip": "203.0.113.10",
      "listen_port": 24800,
      "last_seen_at": 1710000000
    }
  ],
  "layout": {
    "left": null,
    "right": "device-mac-right",
    "top": null,
    "bottom": null
  },
  "permissions": [
    {
      "key": "windows.interactive_desktop",
      "status": "granted",
      "label": "Windows interactive desktop"
    }
  ]
}
```

连接状态枚举建议统一为：

- `connecting`: 正在连接服务器或主电脑。
- `connected`: 已连接。
- `disconnected`: 未连接。
- `auth_expired`: 服务器登录态失效，仅用于服务器连接。
- `retrying`: 上次连接失败，正在等待下一次重试。
- `self`: 本机就是主电脑，仅用于主电脑连接状态。

`SetDeviceRole` 请求：

```json
{
  "role": "master",
  "master_device_id": null
}
```

`SetLayout` 请求：

```json
{
  "left": null,
  "right": "device-mac-right",
  "top": null,
  "bottom": "device-linux-bottom"
}
```

core-service 收到配置变更后必须：

1. 校验设备数量不超过 4。
2. 校验不能绑定当前设备。
3. 校验同一设备不能重复绑定多个方向。
4. 原子写入本地配置。
5. 更新本地 profile 文件。
6. 尝试 `PUT /v1/profiles` 同步到服务端。
7. 热加载输入路由配置，不要求重启软件。

## 7. 状态刷新流程

页面打开后的流程：

1. UI 调用 `GetDesktopState`。
2. core-service 读取本地配置和设备身份。
3. core-service 检查本地 IPC、输入捕获、输入注入权限。
4. core-service 使用当前登录态访问服务器。
5. 如果服务器可访问，刷新 `/v1/devices` 和 `/v1/profiles`。
6. core-service 合并本地配置、服务端 profile、presence，返回统一状态给 UI。
7. UI 每 2 到 5 秒刷新一次状态，或通过本地事件订阅接收变更。

服务器断开时：

- 页面显示“服务器未连接”。
- 本地已保存的 layout 仍然可展示。
- 已建立的 LAN 输入连接不应被 UI 状态影响。
- core-service 按现有心跳重试机制继续重连。

目标电脑掉线后又上线时：

- 目标电脑心跳会上报新的 `lan_ips` 和 `listen_port`。
- 服务端更新 presence。
- 主电脑通过轮询 `/v1/devices` 或 `/v1/events/ws` 得到新的 presence。
- 连接候选地址自动刷新。
- 用户不需要重新配置方向绑定。

## 8. 边缘触发和连接逻辑

主电脑模式下，core-service 负责监听鼠标位置。

触发规则：

- 鼠标到达左边缘，且 `layout.left` 有目标设备时，切换到左边目标。
- 鼠标到达右边缘，且 `layout.right` 有目标设备时，切换到右边目标。
- 鼠标到达上边缘，且 `layout.top` 有目标设备时，切换到上方目标。
- 鼠标到达下边缘，且 `layout.bottom` 有目标设备时，切换到下方目标。
- 对应方向没有绑定设备时，鼠标保持本机。
- 目标设备离线或无候选地址时，鼠标保持本机，并在 UI 中显示该方向不可连接。

连接候选地址优先级：

1. mDNS / 局域网发现地址。
2. 服务端 presence 里的 LAN IP + listen_port。
3. NAT traversal 候选。
4. Relay 地址。

当前代码已经有 `collect_connection_candidates`，页面只需要展示候选摘要，实际选择由 core-service 完成。

## 9. 主电脑连接状态定义

“是否已连接主电脑”在不同角色下含义不同：

- 本机是主电脑：状态显示“本机就是主电脑”。
- 本机不是主电脑：显示是否有主电脑正在连接或最近连接过本机。
- 如果配置了 `master_device_id`，则展示该主电脑名称、在线状态和最近连接时间。
- 如果没有配置主电脑，则显示“未选择主电脑”。

建议 core-service 维护一个 runtime connection table：

```json
{
  "active_target_device_id": "device-mac-right",
  "connected_peers": [
    {
      "device_id": "master-device-id",
      "direction": "incoming",
      "state": "connected",
      "last_seen_at": 1710000000
    }
  ]
}
```

这样 UI 可以准确区分“服务器在线”和“输入同步通道在线”。

## 10. 页面交互细节

建议页面首版包含这些操作：

- “刷新状态”：手动调用 `RefreshNetwork` 和设备列表刷新。
- “作为主电脑”：开关当前设备角色。
- “保存布局”：写入本地配置并同步 profile。
- “测试连接”：对某个方向的目标设备尝试建立连接，但不切换输入。
- “断开当前目标”：释放远端按键状态并回到本机。
- “打开权限设置”：Windows/macOS 下跳转到对应权限说明或系统设置。

布局编辑体验：

- 设备可从列表拖到上下左右插槽。
- 插槽也提供下拉框，方便键盘操作。
- 已选择设备在其他方向自动禁用。
- 离线设备保留在插槽中，但显示灰色和“离线”。
- 保存前做校验，校验失败不写入配置。

## 11. 错误处理

需要明确展示以下错误：

- 服务器不可达。
- 登录态失效，需要重新登录。
- 本机未注册设备身份。
- 本机缺少输入捕获权限。
- 本机缺少输入注入权限。
- 目标电脑离线。
- 目标电脑在线但没有可用连接候选地址。
- 配置保存失败。
- 服务端 profile 同步失败。

错误处理原则：

- 本地配置保存成功但服务端同步失败时，页面提示“本地已保存，等待服务器恢复后同步”。
- 服务端断开不影响已有 LAN 数据通道。
- 切换目标失败时必须释放已按下的远端按键，避免远端卡键。

## 12. 实现阶段

### 阶段一：状态模型和 IPC

- 扩展 `LocalIpcRequest` / `LocalIpcResponse`。
- 增加 `GetDesktopState`。
- core-service 汇总本机 IP、服务端连接状态、设备列表、presence、权限状态。
- UI 页面先只读展示状态。

### 阶段二：角色和布局配置

- 增加 `SetDeviceRole` 和 `SetLayout`。
- 增加本地配置原子写入。
- 将布局转换为现有 `device_layout.targets` 和 `device_layout.edges`。
- 保存后热加载路由配置。

### 阶段三：连接状态和自动刷新

- core-service 维护 active peer / active target 状态。
- 目标掉线后标记插槽不可连接。
- 目标重新心跳后自动刷新候选地址。
- UI 显示“已连接服务器”和“已连接主电脑”两个独立状态。

### 阶段四：桌面打包集成

- Windows 桌面端安装包启动后默认进入托盘或配置页，不弹出黑色命令行窗口。
- macOS 安装包提供 Applications 入口和 LaunchAgent 后台能力。
- macOS 从 Applications 启动时打开配置页面，LaunchAgent/core-service 后台运行，不把日志窗口暴露给用户。
- 页面、core-service 和配置样例一起打包。
- 配置文件路径在 Windows/macOS 使用用户级应用数据目录，避免写入安装目录。

## 13. 测试与验收标准

需要覆盖这些验收点：

- Windows 页面能展示本机 LAN IP。
- macOS 页面能展示本机 LAN IP。
- Windows 双击启动桌面端时不出现黑色命令行窗口。
- macOS 从 Applications 启动时打开配置页面，不暴露后台日志窗口。
- 服务端在线时页面能展示公网 IP。
- 连接服务器或主电脑过程中，页面显示“连接中”。
- 服务端断开时页面显示未连接服务器，不闪退。
- 本机可切换为主电脑。
- 非主电脑模式下不主动捕获键盘鼠标。
- 最多只能配置 4 个方向。
- 同一个目标设备不能被配置到多个方向。
- 当前设备不能配置为自己的目标设备。
- 目标设备掉线后，页面显示离线，但保留方向配置。
- 目标设备重新上线并更新 IP 后，主电脑自动使用新的连接地址。
- 鼠标移动到左、右、上、下边缘时，分别路由到对应设备。
- 目标不可连接时，鼠标保持本机，不丢失本机控制。
- 保存布局后，本地配置和服务端 profile 内容一致。

## 14. 推荐最终形态

最终用户看到的是一个单独的 KMSync 桌面软件：

- 打开后进入配置页面或托盘常驻，不出现黑色命令行窗口。
- 页面顶部清楚显示服务器连接状态和主电脑连接状态，并支持“连接中”状态。
- 网络区域展示内网 IP、公网 IP 和端口。
- 一个开关决定本机是否作为主电脑。
- 中间是四方向设备布局。
- 设备 IP 变化、掉线、重连都由心跳和 presence 自动刷新，不需要用户重新配置。

输入同步仍由 core-service 在后台执行，UI 只负责配置和状态展示，不进入鼠标键盘事件热路径。
