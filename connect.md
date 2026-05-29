# KMSync 电脑连接逻辑方案

## 结论

用户描述的方向是正确的：所有桌面端都主动连接服务器并上报候选地址，主电脑负责主动连接从电脑，优先局域网直连，失败后走服务端转发。

但原描述里有几个需要修正的点：

1. 服务器不应只下发 IP，而应下发“设备身份 + 候选连接信息 + 候选版本”。连接至少需要 `device_id`、公钥、IP、端口、候选类型、过期时间、presence 版本，后续还可能需要 NAT/Relay 信息。
2. 主电脑不应在每次收到从电脑心跳时都立刻断开重连。只有候选地址集合变化、当前连接异常、或服务端明确通知版本变化时才重连，并且要加去抖和退避，避免网络抖动导致频繁断线。
3. 唯一身份不能依赖服务端每次注册生成的新 UUID。第一次启动应在本机生成稳定 `device_id` 和密钥对，后续一直复用。
4. 主从关系不能只存在本机配置。服务器需要保存同一账号下的主电脑、从电脑布局、设备别名/名称版本，否则换机器或重装 UI 后无法恢复。
5. 服务端转发不是“下发 IP”的一部分，而是连接策略的最后兜底通道。直连失败后，主电脑需要向服务端申请 Relay 会话/token，再把输入、剪贴板等数据经服务端转发给目标从电脑。

## 当前代码现状

当前代码已经具备一部分基础能力：

- 客户端配置从 JSON 读取，`ClientConfig` 包含 `server_url`、`email`、`device_name`、`listen_port`、`heartbeat_interval_seconds`、`identity_path`。
- 客户端有 `DeviceIdentity::load_or_generate`，会生成并持久化 Ed25519 密钥对。
- 客户端心跳会调用 `discover_lan_ips()`，向服务端上报 `lan_ips`、`listen_port`、`nat_type`。
- 服务端 `Presence` 会保存 `lan_ips`、外网观测 IP、监听端口、NAT 类型、最后在线时间。
- 客户端已有候选连接类型：`MdnsLan`、`BackendLan`、`NatTraversal`、`Relay`，优先级分别是 mDNS 局域网、服务端上报局域网、NAT 穿透、Relay。
- 客户端已有 `DirectLanReconnectState`，能根据本机 LAN IP、远端候选地址、当前连接状态判断是否重连。
- 服务端已有 `/v1/events/ws` WebSocket 事件流，可以推送 presence/profile/signal 事件。
- 服务端已有本地文件持久化能力，符合“不走数据库、Redis”的部署方向。
- 服务端已有设备改名 API：`PATCH /v1/devices/{device_id}`，可更新 `name` 和 `disabled`。

当前还没有形成完整闭环的点：

- 服务端 `register_device` 当前每次调用都会 `Uuid::new_v4()`，同一台电脑重启后可能注册成新设备；这不满足“设备唯一 ID 作为后续身份依据”。
- 客户端虽然持久化了公钥/私钥，但请求里没有稳定的本机 `device_id`，服务端也没有基于 `device_id` 或 `public_key` 的幂等注册。
- 主从角色目前主要在桌面端配置中，服务端设备模型里没有明确的 `role`、`master_device_id`、布局拓扑版本。
- `run_capture_connect` 当前是针对一个目标设备的连接循环，不是“主电脑自动连接所有从电脑”的连接管理器。
- `refresh_target_direct_lan_connection` 当前只传入 mDNS 和服务端 LAN 候选，NAT/Relay 候选为空。
- `try_direct_lan_connection` 当前只尝试 `MdnsLan` 和 `BackendLan`，不会自动落到 Relay。
- 服务端 presence 事件只有 `device_id` 和 `online`，没有候选版本、名称版本等，客户端收到后需要重新拉取设备列表才能知道变化内容。
- 设备改名后服务端会保存，但当前没有独立的 `DeviceChanged` 事件同步给对应从电脑。

## 推荐整体架构

### 基本原则

- 所有电脑都是同一个账号下的设备，设备身份以稳定 `device_id` 为准，公钥用于校验该设备身份。
- 所有电脑启动后都主动连接服务器，保持登录、心跳、事件订阅和本机数据监听。
- 只有主电脑主动发起到从电脑的数据连接；从电脑只负责监听、上报候选地址、接收来自主电脑或 Relay 的连接。
- 服务端只负责保存状态、下发拓扑和候选地址、转发无法直连的数据，不直接参与正常局域网输入数据流。
- 局域网直连成功时，输入和剪贴板数据不经过服务端。
- Relay 只作为直连失败、网络不通、或从电脑不在同一局域网时的兜底。

### 角色定义

- 主电脑：当前负责捕获鼠标、键盘、剪贴板事件，并主动连接从电脑的设备。
- 从电脑：接收主电脑事件并执行输入注入、剪贴板同步的设备。
- 服务端：保存设备、presence、拓扑、名称、连接事件和 Relay 会话。

同一账号建议同一时刻只有一个主电脑。若允许多个主电脑，服务端需要按“工作区/布局组”隔离，否则会出现多个主电脑同时控制同一从电脑的问题。

## 目标流程

### 1. 首次启动与唯一身份

首次启动桌面端时：

1. 本机检查本地身份文件。
2. 如果不存在，生成：
   - `device_id`：UUID v4 或 ULID，永久保存。
   - Ed25519 密钥对：公钥随注册请求上报，私钥保存在系统密钥链或本地安全存储。
   - `device_secret_version` 或 `identity_version`：用于后续密钥轮换。
3. 如果身份文件存在，直接复用已有 `device_id` 和密钥对。
4. 客户端向服务端执行幂等注册：
   - 如果 `(user_id, device_id)` 不存在，创建新设备。
   - 如果存在且公钥一致，更新系统信息和 app 版本。
   - 如果存在但公钥不一致，禁止直接覆盖，要求重新授权或用户确认。

建议注册请求调整为：

```json
{
  "device_id": "stable-client-generated-id",
  "name": "Alice-Windows",
  "os_type": "windows",
  "os_version": "11",
  "app_version": "0.1.0",
  "public_key": "ed25519:...",
  "role": "master|client"
}
```

### 2. 启动后连接服务器和上报 IP

每台电脑启动后都要：

1. 读取配置文件中的服务器地址、端口、账号、角色、监听端口等。
2. 登录服务端。
3. 幂等注册设备。
4. 启动本机数据监听端口。
5. 枚举本机候选地址并心跳上报。
6. 建立 `/v1/events/ws` 事件订阅。

心跳上报不建议只传字符串 IP，建议传候选地址对象：

```json
{
  "device_id": "device-a",
  "listen_port": 24800,
  "candidate_version": 42,
  "lan_candidates": [
    {
      "ip": "192.168.1.20",
      "port": 24800,
      "family": "ipv4",
      "source": "interface",
      "interface_hash": "wifi-1",
      "priority": 300
    }
  ],
  "nat_type": "unknown",
  "relay_capable": true
}
```

服务端根据请求来源补充：

- `public_ip`：服务端观察到的远端 IP。
- `last_seen_at`：最后心跳时间。
- `expires_at`：presence 过期时间。
- `presence_version`：候选地址或在线状态变化时递增。

### 3. 主电脑启动后的全量连接

主电脑启动后：

1. 登录并注册自己。
2. 拉取服务端拓扑：当前主电脑、布局、从电脑列表。
3. 拉取设备 presence 和公钥。
4. 对布局中所有从电脑分别创建连接状态机。
5. 每台从电脑按候选优先级尝试连接：
   - mDNS 发现到的同账号局域网地址。
   - 服务端 presence 上报的局域网地址。
   - NAT 穿透候选地址。
   - 服务端 Relay。
6. 连接成功后记录 transport 类型和候选版本。
7. UI 显示每台从电脑的状态：`连接中`、`已连接`、`已断开`、`重试中`。

主电脑应该维护的是“每台从电脑一个连接状态机”，而不是一个全局连接。

### 4. 从电脑上线或 IP 变化

从电脑启动或网络变化后：

1. 从电脑重新枚举本机候选地址。
2. 从电脑心跳上报新的候选地址。
3. 服务端对比旧 presence：
   - 候选集合无变化：只更新 `last_seen_at`，不推送重连事件。
   - 候选集合变化：递增 `presence_version`，推送 `DevicePresenceChanged`。
4. 主电脑收到事件后拉取该从电脑最新候选信息。
5. 主电脑比较本地连接使用的候选版本：
   - 如果当前连接健康，且新候选没有更优路径，可以保持现有连接。
   - 如果当前连接异常，或新候选明显更优，或服务端标记必须刷新，则断开该从电脑连接并重连。
6. 直连仍优先，失败后回退 Relay。

原描述里的“主电脑收到后，断开跟这台从电脑的连接，重新尝试连接”建议调整为：

> 主电脑收到候选版本变化后，先判断是否需要重连；需要时只重连对应从电脑，并加 300ms 到 2s 去抖窗口，避免多次心跳造成连续断开。

### 5. 直连优先，Relay 回退

候选连接排序建议：

1. `MdnsLan`：局域网 mDNS 发现，同账号、同设备公钥校验通过。
2. `BackendLan`：服务端上报的 LAN IP + 监听端口。
3. `NatTraversal`：公网观测地址或信令协商出的穿透候选。
4. `Relay`：服务端转发。

连接策略：

1. 对每个候选地址设置短超时，例如 500ms 到 1500ms。
2. 同类候选可以并发尝试少量地址，但要限制并发数。
3. 任一直连成功后停止后续尝试。
4. 所有直连失败后申请 Relay 会话。
5. Relay 建立后仍保留后台直连重试，直连恢复后可从 Relay 切回直连。
6. 切换 transport 时必须保证事件序列号连续，避免鼠标/键盘状态卡住。

无论走直连还是 Relay，数据通道握手都必须校验：

- 对端 `device_id` 是否是服务端返回的目标设备。
- 对端公钥是否与服务端保存的公钥一致。
- 当前账号和拓扑是否允许主电脑控制该从电脑。

### 6. 设备命名和同步

主电脑修改从电脑名称时：

1. 主电脑调用服务端设备更新接口。
2. 服务端保存新名称，递增 `device_version` 或 `name_version`。
3. 服务端推送 `DeviceChanged` 事件给同账号所有在线设备。
4. 主电脑更新设备列表 UI。
5. 对应从电脑收到事件后拉取自己的设备信息，更新显示名称。

命名规则建议：

- `name` 是服务端保存的显示名称，作为多端一致的名称。
- 本地配置中的 `device_name` 只作为首次注册默认名称或离线 fallback。
- 如果以后需要“不同主电脑给同一从电脑不同备注”，再增加 `DeviceAlias`，不要直接覆盖设备自己的全局名称。

## 服务端数据模型建议

服务端仍使用本地文件持久化，不引入 MySQL、Redis。

### Device

```json
{
  "user_id": "user-id",
  "device_id": "stable-device-id",
  "name": "Alice-Windows",
  "role": "master|client",
  "os_type": "windows",
  "os_version": "11",
  "app_version": "0.1.0",
  "public_key": "ed25519:...",
  "disabled": false,
  "created_at": 1780000000,
  "updated_at": 1780000000,
  "device_version": 3,
  "name_version": 2
}
```

索引建议：

- `(user_id, device_id)` 唯一。
- `(user_id, public_key)` 可用于迁移旧数据或排查重复注册。

### Presence

```json
{
  "user_id": "user-id",
  "device_id": "device-id",
  "online": true,
  "lan_candidates": [],
  "public_ip": "203.0.113.10",
  "listen_port": 24800,
  "nat_type": "unknown",
  "relay_capable": true,
  "last_seen_at": 1780000000,
  "expires_at": 1780000030,
  "presence_version": 42
}
```

presence 只表示临时在线状态，可以由心跳刷新；服务端重启后如果本地文件里存在旧 presence，应按 `expires_at` 判定是否过期，不能直接认为在线。

### Topology

```json
{
  "user_id": "user-id",
  "master_device_id": "master-device-id",
  "layout": {
    "left": "slave-a",
    "right": "slave-b",
    "top": null,
    "bottom": null
  },
  "topology_version": 8,
  "updated_at": 1780000000
}
```

拓扑用于决定主电脑控制哪些从电脑。主电脑启动后以服务端拓扑为准，本地配置作为缓存和离线显示。

### RelaySession

```json
{
  "session_id": "relay-session-id",
  "user_id": "user-id",
  "master_device_id": "master-device-id",
  "slave_device_id": "slave-device-id",
  "relay_token": "opaque-token",
  "relay_url": "wss://server.example.com/v1/relay/session",
  "expires_at": 1780000300
}
```

Relay 会话应短期有效，并绑定主从设备 ID，不能被其他设备复用。

## 服务端事件建议

现有 WebSocket 可以继续使用，但事件类型建议扩展：

```json
{
  "type": "device_presence_changed",
  "device_id": "slave-a",
  "online": true,
  "presence_version": 42
}
```

```json
{
  "type": "device_changed",
  "device_id": "slave-a",
  "device_version": 3,
  "name_version": 2
}
```

```json
{
  "type": "topology_changed",
  "topology_version": 8
}
```

事件只做轻量通知，客户端收到后再调用 API 拉取最新完整状态。这样可以降低事件兼容成本，也避免把过多内网地址直接推到事件流中。

## 客户端状态机建议

### 全局客户端状态

- `Starting`：读取配置、加载身份。
- `Authenticating`：登录服务端。
- `Registering`：幂等注册设备。
- `Online`：心跳和事件订阅正常。
- `AuthExpired`：登录过期，需要重新登录。
- `Offline`：服务端不可达，本机只保留离线状态。

### 单个从电脑连接状态

- `Disconnected`：无连接。
- `Connecting`：正在尝试直连或 Relay。
- `ConnectedDirect`：局域网或 NAT 直连成功。
- `ConnectedRelay`：通过服务端转发。
- `Retrying`：连接失败，按退避重试。
- `Stale`：候选版本变化，等待去抖后刷新。

状态转换建议：

```text
Disconnected -> Connecting -> ConnectedDirect
Disconnected -> Connecting -> ConnectedRelay
ConnectedDirect -> Stale -> Connecting
ConnectedRelay -> Stale -> Connecting
ConnectedDirect -> Retrying -> Connecting
ConnectedRelay -> Retrying -> Connecting
Retrying -> Disconnected
```

UI 中的“连接中”应对应 `Connecting` 和短暂 `Stale` 阶段；“重试中”对应持续失败后的退避阶段。

## 安全和边界条件

- LAN IP 不能完全信任，只能作为候选地址；最终必须通过数据通道握手校验设备 ID 和公钥。
- 过滤无效地址：`0.0.0.0`、loopback、unspecified、明显不可达地址；IPv6 要处理 zone/interface。
- 一台设备多网卡时，应保留多个候选，但需要排序和去重。
- 心跳丢失不应立刻删除设备，只把 presence 标记为离线。
- 服务端重启后，从本地文件恢复的数据需要重新按 TTL 判断在线状态。
- 主电脑切换时，旧主电脑应停止捕获和连接；服务端要推送 `topology_changed`。
- 从电脑被禁用或解绑时，主电脑必须立即断开该设备。
- Relay token 必须短期有效，并绑定用户、主设备、从设备。
- 数据事件要带序列号，重连后从电脑能丢弃重复事件，并清理卡住的按键状态。

## 推荐接口调整

### 幂等注册

- `PUT /v1/devices/{device_id}` 或保留 `POST /v1/devices/register` 但请求体必须包含 `device_id`。
- 服务端按 `(user_id, device_id)` upsert。
- 返回设备完整信息和版本号。

### 心跳

- `POST /v1/devices/{device_id}/heartbeat`
- 返回 `presence_version`、服务端时间、是否需要刷新拓扑。

### 设备列表

- `GET /v1/devices`
- 返回设备、presence、版本号、公钥。

### 拓扑

- `GET /v1/topology`
- `PUT /v1/topology`
- 保存主电脑和布局。

### 改名

- `PATCH /v1/devices/{device_id}`
- 成功后发布 `device_changed`。

### Relay

- `POST /v1/relay/token`
- 请求包含 `source_device_id`、`target_device_id`。
- 返回短期 token 和 relay URL。

## 实施顺序建议

1. 先补稳定身份：本地身份文件增加 `device_id`，服务端注册改为幂等 upsert。
2. 服务端设备模型增加 `role`、`device_version`、`name_version`，改名后推送 `device_changed`。
3. 服务端增加拓扑模型，保存主电脑和从电脑布局。
4. 心跳 presence 增加候选版本和 TTL，只有候选变化时递增版本。
5. 客户端增加主电脑连接管理器，一次维护所有布局内从电脑连接。
6. 将 Relay 候选接入连接流程，直连失败后自动申请并使用 Relay。
7. 客户端事件订阅接入 presence/device/topology 变化，触发增量刷新。
8. UI 显示 `连接中`、直连/转发状态、最后错误和最后在线时间。

## 验收标准

- 同一台电脑重启后，服务端设备列表仍显示同一个 `device_id`，不会生成重复设备。
- 从电脑切换 Wi-Fi、有线网络或 IP 变化后，主电脑能收到变化并只重连该从电脑。
- 同一局域网内优先使用 LAN 直连；断开局域网后能回退到 Relay。
- 服务端转发不可用时，UI 能显示连接失败原因，不影响其他从电脑。
- 主电脑修改从电脑名称后，服务端保存名称，主电脑和对应从电脑 UI 都能同步显示新名称。
- 服务端重启后，本地文件能恢复设备、名称、拓扑；过期 presence 不会被误判为在线。
- 从电脑掉线再上线后，主电脑无需用户重新配置，能根据稳定 `device_id` 自动关联并恢复连接。
