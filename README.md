# KMSync

KMSync 是一个跨电脑共享鼠标、键盘和剪贴板的 Rust 工具。桌面端负责输入捕获、输入注入、剪贴板同步和本地控制面；服务端只负责账号登录、设备注册、设备列表、心跳 presence、profile 同步和信令等控制面能力。

## 项目结构

- `crates/kmsync-core`: 跨平台输入事件、协议、profile、剪贴板和加密等核心逻辑。
- `crates/kmsync-daemon`: 桌面常驻进程，负责本机输入/剪贴板能力和网络数据面。
- `crates/kmsync-ui`: 控制页面和布局编辑器渲染模块；桌面端入口已并入 `kmsync-daemon`。
- `crates/kmsync-server`: 后端控制面服务。
- `configs`: 服务端、daemon 和 profile 示例配置。
- `packaging`: macOS、Windows、Linux 打包和部署样例。
- `docs`: 用户指南、发布说明、架构和打包说明。

## 服务端存储

`kmsync-server` 不依赖 MySQL 或 Redis。运行配置只读 JSON 配置文件，不读取环境变量；所有用户、设备、会话、profile、relay/signaling 状态和 heartbeat presence 都写入 `data_path` 指向的本地 JSON 文件。

示例配置：

```json
{
  "bind": "0.0.0.0:24888",
  "data_path": "/var/lib/kmsync/server-state.json"
}
```

启动服务端：

```bash
cargo run -p kmsync-server -- configs/kmsync-server.example.json
```

设备掉线后重新上线时，客户端继续使用原来的 `device_id` 发送 heartbeat，服务端会刷新该设备的 LAN IP、公网 IP、监听端口、NAT 类型和最后在线时间，并继续把它关联在原用户的设备列表中。

## 常用开发命令

```bash
cargo test
cargo run -p kmsync-daemon -- info
cargo run -p kmsync-daemon -- self-test mac-to-windows
cargo run -p kmsync-daemon -- self-test windows-to-mac
cargo run -p kmsync-daemon -- heartbeat configs/daemon.example.json
cargo run -p kmsync-daemon -- devices configs/daemon.example.json
cargo run -p kmsync-daemon -- status
```

Linux 服务端部署见 `packaging/linux/README.md`；用户安装和诊断说明见 `docs/USER_GUIDE.md`；当前发布包说明见 `docs/RELEASE.md`。
