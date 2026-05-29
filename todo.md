# KMSync Todo

本文档基于当前代码和设计文档整理，分为“尚未完成的功能点”和“需要优化的功能点”。项目目标是跨 macOS / Windows / Linux 的低延迟键鼠共享、剪贴板同步、设备发现、跨系统功能键/快捷键习惯映射。

## 当前状态概览

- 已有 Rust workspace：`kmsync-core`、`kmsync-daemon`、`kmsync-server`。
- 已有输入事件模型、基础键位映射、滚轮/鼠标倍率转换、二进制协议。
- 已接入 macOS `CGEventTap` 捕获、`CGEventPost` 注入。
- 已接入 Windows Low Level Hook 捕获、`SendInput` 注入。
- 已有 QUIC Datagram / Stream 数据面、边缘激活、本地事件拦截。
- 已有文本剪贴板读写、发送和平台通知/原生变更计数同步演示。
- 已有开发版后端：开发登录、设备注册、心跳/IP presence、Redis presence TTL、设备列表、profile 同步、PostgreSQL/开发 JSON 持久化。

## P0: 尚未完成的核心功能

### 1. 产品级数据传输通道

- [x] 用 QUIC 替换当前 UDP 演示通道。
- [x] 建立 `input_unreliable` 通道，用 QUIC Datagram 发送鼠标移动事件。
- [x] 建立 `input_reliable` 通道，用可靠有序流发送键盘、鼠标点击、滚轮事件。
- [x] 建立 `clipboard` 通道，用可靠流发送文本、图片、文件元数据和分片内容。
- [x] 建立 `control` 通道，用于心跳、能力协商、配置版本和会话状态。
- [x] 实现协议版本协商和向后兼容。
- [x] 对输入事件增加源设备 ID、目标设备 ID、协议版本和通道类型。
- [x] 对可靠输入事件实现序列号检测、乱序处理和丢包恢复策略。
- [x] 鼠标移动事件支持丢弃旧帧、只保留最新状态。

### 2. 局域网自动发现与连接建立

- [x] 实现 mDNS / Bonjour 局域网发现。
- [x] 客户端自动发现同账户局域网设备。
- [x] 从后端 presence 获取 LAN IP 和端口后自动尝试直连。
- [x] 实现连接候选地址收集和优先级排序。
- [x] 连接优先级：mDNS LAN 直连 -> 云端 LAN IP 直连 -> NAT 穿透 -> Relay。
- [x] 网络变化、设备唤醒、IP 变化时自动重新发现和重连。
- [x] 连接断开后自动恢复输入状态，避免远端卡键。

### 3. 信令服务

- [x] 后端增加 WebSocket 或 gRPC streaming 信令连接。
- [x] 实现 `connect.request`、`connect.accept`、`connect.reject`、`candidate.add`、`session.close`。
- [x] 实现设备上下线实时推送。
- [x] 实现 profile/config 变更推送。
- [x] 信令只交换连接元数据，不承载键鼠事件或剪贴板内容。

### 4. 端到端安全与设备身份

- [x] 每台设备首次登录后生成密钥对。
- [x] 私钥存入 macOS Keychain / Windows Credential Manager 或 DPAPI。
- [x] 后端保存设备公钥并绑定账户。
- [x] 建连时验证对端设备身份。
- [x] 数据面实现端到端加密，Relay 不可读取业务内容。
- [x] 增加重放攻击防护、会话密钥轮换和设备撤销。
- [x] 剪贴板、文件传输和诊断日志遵守数据最小化原则。

### 5. 账户与后端产品化

- [x] 用正式 OAuth / 邮箱登录替换开发登录。
- [x] PostgreSQL 替换 JSON 文件持久化。
- [x] Redis 保存在线状态和 presence TTL。
- [x] 增加 refresh token、session 管理和登出。
- [x] 增加设备解绑、重命名、禁用、重新授权。
- [x] 增加 Relay token 发放和 Relay 调度。
- [x] 增加配置增量同步接口。
- [x] 增加客户端版本检查、自动更新策略和灰度发布。

### 6. 多显示器和多设备布局

- [x] 支持多显示器坐标读取和布局建模。
- [x] 支持多目标设备布局配置。
- [x] 支持每条屏幕边缘绑定不同目标设备。
- [x] 支持跨屏进入远端后保持远端指针位置连续。
- [x] 支持边缘阈值、热角、锁定当前设备、释放快捷键配置。
- [x] 支持图形化设备布局配置。

### 7. 键盘功能键和快捷键体系

- [x] 补齐键位模型：数字键盘、F13-F24、媒体键、亮度、音量、播放控制。
- [x] 明确 macOS `Fn` / Globe 键、Windows 键、Command、Option、Control、Alt、Super 的跨平台语义。
- [x] 支持左右修饰键分别映射和显示。
- [x] 支持功能键行模式：标准 F1-F12 或系统媒体功能。
- [x] 支持快捷键语义映射：复制、粘贴、剪切、撤销、重做、全选、查找、切换应用。
- [x] 支持 Physical mode 和 Text mode。
- [x] 支持键盘布局配置和非英文键盘输入策略。
- [x] 增加卡键保护：断线、进程退出、目标切换时释放所有远端按键。

### 8. 剪贴板同步产品化

- [x] 用平台原生剪贴板 API 替换命令行调用。
- [x] 用平台剪贴板变更通知替换轮询。
- [x] 实现剪贴板内容 hash、版本号和来源标记，避免同步循环。
- [x] 支持纯文本、URL、基础富文本。
- [x] 支持图片剪贴板。
- [x] 支持文件剪贴板和文件传输分片。
- [x] 支持大小限制、同步开关、自动过期和敏感应用黑名单。
- [x] 支持密码管理器来源过滤或提示。

### 9. Linux 支持

- [x] 增加 Linux 平台层。
- [x] X11 捕获支持 XInput2 / evdev。
- [x] X11 注入支持 XTest / uinput。
- [x] Wayland 做能力探测和降级策略。
- [x] 不同桌面环境提供兼容性矩阵。

### 10. 桌面 UI 和本地服务化

- [x] 拆分 Core Service 和 UI App。
- [x] Core Service 常驻后台，输入热路径不经过 UI。
- [x] UI 提供登录、设备列表、布局配置、习惯设置、剪贴板设置、网络诊断、权限引导。
- [x] 本地 IPC 使用 Windows named pipe / Unix domain socket / gRPC over local socket。
- [x] macOS 使用 LaunchAgent，Windows 使用 Service + 用户态 companion。
- [x] 增加权限检测和引导：macOS Accessibility / Input Monitoring，Windows 交互桌面限制。

## P0: 需要优先优化的低延迟问题

### 1. 输入热路径解耦

- [x] 捕获回调中只做最小事件采集，不直接执行网络发送。
- [x] 捕获回调写入 lock-free ring buffer 或高性能有界队列。
- [x] 独立 TX 线程从队列读取事件、映射、编码、发送。
- [x] 独立 RX 线程接收远端事件。
- [x] 独立 Injection 线程注入远端输入。
- [x] 剪贴板、控制面、日志和 UI 线程不得阻塞输入链路。

### 2. 减少锁和分配

- [x] 移除高频 hook 回调里的 `Mutex`。
- [x] 鼠标移动热路径做到 0 次或接近 0 次堆分配。
- [x] 输入事件编码使用栈上固定缓冲区，避免每包 `Vec` 分配。
- [x] Windows 滚轮注入避免每次 `Vec::with_capacity`。
- [x] macOS 捕获事件列表避免运行时重复分配。
- [x] Profile 编译后使用查表结构，热路径只做数组访问和简单乘法。

### 3. 网络发送优化

- [x] UDP/QUIC socket 使用 `connect` 后 `send`，减少每包目标地址处理。
- [x] 针对鼠标移动实现批量合并或 latest-wins 队列。
- [x] 键盘、点击、滚轮走可靠有序通道，不和大剪贴板共享阻塞路径。
- [x] 设置合适 socket buffer。
- [x] 增加发送队列长度指标和丢弃策略。
- [x] 输入包默认不做同步日志输出。

### 4. 时间戳和指标

- [x] 使用单调时钟记录本机链路耗时。
- [x] 增加端到端输入延迟统计。
- [x] 增加捕获到发送、发送到接收、接收到注入的分段耗时。
- [x] 增加队列长度、丢包率、重连次数、CPU、内存指标。
- [x] 日志只记录事件类型、延迟和错误码，不记录按键内容和剪贴板内容。

### 5. 平台注入优化

- [x] macOS 缓存 `CGEventSource`，避免每个事件重新创建。
- [x] macOS 鼠标移动避免每次读取当前位置导致额外开销，维护远端指针状态。
- [x] Windows 使用 scan code 注入补足物理键位准确性。
- [x] Windows 区分左右 Ctrl / Shift / Alt / Meta。
- [x] 标记自身注入事件，捕获端过滤，避免本机注入回环。
- [x] 处理 macOS 和 Windows 系统级快捷键无法捕获或无法注入的边界。

## P1: 稳定性和可用性优化

### 1. 输入状态管理

- [x] 维护远端按键按下集合。
- [x] 断线、切换目标、退出程序时释放所有按下键。
- [x] 远端注入失败时进入保护状态并释放键。
- [x] 处理 Caps Lock、输入法、系统快捷键差异。
- [x] 鼠标按钮按下后断线需要释放按钮。

### 2. 边缘切换体验

- [x] 支持多显示器边缘判断。
- [x] 激活远端控制时隐藏或限制本地指针。
- [x] 从远端返回本机时恢复本机指针位置。
- [x] 支持边缘穿越冷却时间，避免反复抖动切换。
- [x] 增加可配置释放快捷键。

### 3. 错误处理和诊断

- [x] 将字符串错误逐步替换为结构化错误类型。
- [x] 增加权限缺失、连接失败、注入失败的用户可读诊断。
- [x] 增加本机自检命令，覆盖捕获、注入、剪贴板、网络连通性。
- [x] 增加连接诊断报告，但不包含敏感输入内容。

### 4. 配置系统

- [x] 本地配置存储支持原子写入。
- [x] Profile JSON 与 `kmsync-core::Profile` 类型打通。
- [x] 支持从后端 profile 动态加载并编译映射。
- [x] 支持配置版本、冲突解决和回滚。
- [x] 支持应用级例外规则。

### 5. 测试覆盖

- [x] 协议编解码增加更多事件类型和错误输入测试。
- [x] 键位映射覆盖 macOS -> Windows、Windows -> macOS、左右修饰键。
- [x] 滚轮方向、水平滚轮、鼠标倍率测试。
- [x] CaptureRouter 覆盖边缘激活、释放快捷键、阈值边界。
- [x] 后端 API 增加集成测试。
- [x] 增加高频鼠标移动性能测试。
- [x] 增加断线释放按键测试。
- [x] 增加剪贴板循环抑制测试。

## P2: 产品化和交付

- [x] macOS Developer ID 签名和 notarization。
- [x] Windows Authenticode 签名。
- [x] 自动更新与版本灰度。
- [x] 安装包完善：开机自启、权限引导、卸载清理。
- [x] 崩溃报告和匿名性能指标。
- [x] 文档补齐：用户安装、权限设置、网络诊断、常见问题。
- [x] 兼容性矩阵：Windows 10/11、macOS 多版本、Linux X11/Wayland。

## 建议实施顺序

1. 先重构输入热路径：捕获回调入队，TX/RX/Injection 独立线程，固定缓冲区编码。
2. 再补可靠数据面：QUIC Datagram + Stream，鼠标可丢包，键盘/点击/滚轮可靠有序。
3. 补齐键位和功能键：左右修饰键、数字键盘、功能键行、系统媒体键、Fn 策略。
4. 做连接自动化：mDNS、后端 presence 候选地址、自动重连。
5. 做剪贴板产品化：原生 API、变更通知、hash/来源标记、循环抑制。
6. 做安全和身份：设备密钥、端到端加密、撤销和日志最小化。
7. 做 UI、本地服务化和安装包，进入产品可用阶段。

## 当前验证方式

- 当前 Windows 环境已安装 Rust stable GNU toolchain 和 WinLibs。
- 当前可用测试命令：`$env:PATH='C:\Users\Administrator\AppData\Local\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin;C:\Users\Administrator\.cargo\bin;' + $env:PATH; C:\Users\Administrator\.cargo\bin\cargo.exe +stable-x86_64-pc-windows-gnu test`。
- 当前目录不是 git 仓库，无法通过 git 状态确认历史改动来源。
