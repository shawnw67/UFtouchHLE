# LAN Multiplayer：房间发现与手动连接

本阶段提供**独立传输控制台**，复用第三阶段的 TCP P2P 模块。它可以发布 / 浏览房间、按房间 ID 加入或手动输入 IP:端口，并验证实例之间的数据通信。**尚未接入 GameKit 或 MHP2G；看到 joined / connected 不表示游戏已经可以多人狩猎。**

源码基线为 `47e9bf7c` 加前阶段未提交改动，日期 2026-10-27。前阶段设计见 [TRANSPORT_DESIGN.md](TRANSPORT_DESIGN.md)。本阶段不修改游戏、存档或 Framework 行为，不使用外部服务器、Relay 或专用服务器程序。

## 快速使用

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release
```

桌面版统一入口为 `touchHLE --multiplayer`，该参数必须放在第一个位置。查看帮助：

```sh
target/release/Podroid --multiplayer --help
```

网络默认关闭，所有模式都必须明确提供 `--allow-network-access`。这个入口不加载 IPA、不启动 SDL 窗口、不修改 `Podroid_options.txt`；普通游戏启动流程保持原样。本阶段选择命令行而不是新建游戏内界面或 Android 启动器页面。

### A. 自动发现

Host（也是一名玩家）运行：

```sh
target/release/Podroid --multiplayer host --name="雪山集会所" --allow-network-access
```

另一台电脑运行：

```sh
target/release/Podroid --multiplayer browse --allow-network-access
```

输出是逐行 JSON。`rooms` 事件包含完整房间列表；每项包含 `id`、`name`、`ip`、`port`、`endpoints`、`players`、`capacity`、`protocol` 和 `joinable`。IP 从收到公告的 UDP 源地址生成，而不是相信远端声称的任意地址。一个房间从多个网络接口被发现时，合并为同一个 ID，保留多个 endpoint。

浏览器控制台支持：

```text
list
join 房间的32位十六进制UUID
leave
quit
```

`list` 输出当前快照；`join UUID` 选择该房间第一个 endpoint，调用同一个 TCP Transport。若有多个网卡地址且首选地址无法连接，可使用下方的手动连接命令指定另一个 endpoint。房间已过期、已满或协议不支持时，返回清楚的命令错误；人数和可加入状态只是公告快照，最终准入仍由 Host 校验。

`joined` 包含实际传输会话 ID；从发现列表选择的房间会核对实际 SessionId，防止房主重启后 IP:端口相同却悄悄进入另一会话。`connected` 表示对应玩家直连完成，四人房间的直连可能分步建立。

### B. 手动 IP / VPN

```sh
target/release/Podroid --multiplayer join --address=192.168.1.10:42018 --allow-network-access
# 已有 VPN 提供可路由的 IPv4 地址时，同一路径也可使用：
target/release/Podroid --multiplayer join --address=10.8.0.2:42018 --allow-network-access
```

手动 join 模式**不创建 UDP 发现实例**，不依赖浏览器、广播或房间公告。也可在 browse 控制台输入 `join 192.168.1.10:42018`，此时复用同一 `connect` 函数，但已经运行的浏览器继续更新列表。

只接受数字 IP 和明确端口，不做 DNS 查找。支持 IPv4；解析器认识 `[IPv6]:port`，但会明确提示 transport v1 暂不支持 IPv6，不尝试错误转换。公开的房间 endpoint 使用 `SocketAddr` 为后续 IPv6 留出表示空间，当前 TCP 和发现报文仍为 IPv4。

VPN 需要各端地址与 TCP 监听端口实际可达。第三阶段采用全直连网状结构：Host 协调名单，不转发其他 Client 的载荷；三四人时各 Client 之间也必须直接可路由。没有 VPN 配置、NAT 打洞或路由自动修复功能，不能承诺任意 VPN 均可运行。

### 同一台电脑双实例

```sh
# 终端 A，Host 使用默认 TCP 42018
target/release/Podroid --multiplayer host --name=Local --allow-network-access
# 终端 B，Client 自动分配自己的 TCP 监听端口
target/release/Podroid --multiplayer join --address=127.0.0.1:42018 --allow-network-access
```

多开 Host 时给它们不同 TCP 端口，例如 `--bind=0.0.0.0:42020`，或 `--bind=127.0.0.1:0` 让操作系统分配端口，查看 `listening.address`。每个 Host 发布不同的实际 SessionId，因此相同名字也不会覆盖别的房间。

为保持纯标准库、避免跨平台 SO_REUSEPORT / 多播权限差异，浏览器 UDP 监听端口独占：**同一电脑同一端口只启动一个 browse 控制台**。Host 用临时 UDP 源端口，不占用浏览器端口；多个 Host 可同时发布。需要同一电脑多个浏览器时，分别指定不同 `--discovery-bind` 端口，并为 Host 重复配置对应的 `--broadcast` 目标；或多开的 Client 使用手动 IP 模式。

### 验证二进制数据（诊断用途）

收到 joined / connected 后，向目标玩家 ID 发送 JSON 命令：

```json
{"command":"send","to":"目标玩家UUID","bytes":[0,255,0,128]}
```

对端应收到 `data` 事件，字节数组完全一致。`command_result.error=null` 只表示命令入队成功，不是已送达确认。控制台不是游戏菜单，以上字节不会传给 MHP2G。

## 默认端口与选项

| 选项 | 默认 / 说明 |
| --- | --- |
| Host `--bind` | `0.0.0.0:42018`，TCP |
| Client `--bind` | `0.0.0.0:0`，TCP 临时端口；三四人直连需要允许入站 |
| `--discovery-bind` | `0.0.0.0:42019`，浏览器 UDP |
| `--announce-bind` | `0.0.0.0:0`，Host UDP 源地址 / 临时端口 |
| `--broadcast` | `255.255.255.255:42019`；可重复，最多 16 个目标 |
| `--interval-ms` | 1000，发布间隔；允许 50–10000 ms |
| `--ttl-ms` | 4000，按浏览器本地接收时间过期；至少 interval 的 2 倍，最多 60000 ms |
| `--name` | `touchHLE Room`，1–96 UTF-8 字节，不允许控制字符 |
| `--player-id` | 未指定时生成 UUID v4；重连要复用身份可明确提供 |

这些端口是本实现的可配置默认值，不保证不会与其他软件冲突。标准输入 EOF（Ctrl-D）和 `quit` 请求正常退出；Ctrl-C 或进程崩溃不保证发送离开公告，此时依赖过期清理。

## 为什么使用 UDP Broadcast

发现层仅发布小型 UDP 公告；实际数据继续走现有 TCP。选择广播是因为 Rust 标准库在 macOS / Windows / Android 等目标都提供 bind、set_broadcast、send_to、recv_from，无须引入 mDNS 服务或多播锁 / 第三方 socket 依赖。**本次实测只覆盖 macOS，不以 API 存在宣称其他平台已经验证。**

Host 不监听公共发现端口，每个 Host 从自己的临时 UDP socket 周期发布，因此多个房间不争用端口。浏览器被动监听公共端口，不发送查询，不要求 Host 响应任意来源的请求。没有额外网络发现服务器。

多网卡时，有限广播 `255.255.255.255` 不保证发到每张网卡。可指定网络的定向广播地址：

```sh
target/release/Podroid --multiplayer host --name=MultiNIC --allow-network-access \
  --broadcast=192.168.1.255:42019 --broadcast=192.168.50.255:42019
```

上面的 `.255` 只是 `/24` 网络示例，实际广播地址由子网掩码决定。宿主系统路由决定出口；必要时用 `--announce-bind=192.168.1.10:0` 指定单一发布源地址，TCP 可同时用 `--bind=192.168.1.10:42018`。绑定单一源 IP 不保证还能向其他接口广播；应配置与该源地址匹配的目标。当前不自动枚举网络接口或修改系统路由。

浏览器绑定 wildcard 地址可接收多个接口到来的公告，按真实 SessionId 合并，最多保留每房间 8 个 endpoint。多个不同房间可同时显示，最多 64 个；超出上限的新房间忽略，以控制内存占用。

## 协议和实现定位

| 文件 | 作用 |
| --- | --- |
| [src/lan/discovery.rs](src/lan/discovery.rs) | `Discovery::advertise` / `browse`、有界队列、后台 UDP worker、公告解析、去重与过期表 |
| [src/lan/cli.rs](src/lan/cli.rs) | `run_cli`、命令解析、Transport 事件轮询、名单人数更新、按房间 / 地址连接 |
| [src/lan/mod.rs](src/lan/mod.rs) | `ipv4_endpoint`，无网络 I/O 的手动地址解析 |
| [src/bin.rs](src/bin.rs) | 仅桌面命令行的独立入口分流，未增加游戏内代码路径 |
| [src/transport/mod.rs](src/transport/mod.rs) | 原有 Transport；共享 `PROTOCOL_VERSION=1` |

发现报文为大端：`UFLD`(4)、发现版本 u16=1、类型 u8（1 公告 / 2 离开）、flags u8=0、发布序号 u64、RoomId(16)、传输协议版本 u16、TCP port u16、人数 u8、容量 u8、compatibility tag(16)、名称字节数 u8、UTF-8 名称。固定部分 55 字节，报文最大 151 字节。

当前 tag 与第三阶段控制台一致：`UF-TRANSPORT-V1!`。浏览器过滤不同 tag 的公告，显示不同 transport 版本但不允许加入。它不是已实现的 MHP2G 版本 / group 匹配条件，也不是原版 GameKit 协议。

房间 ID 是 Transport 实际生成的 SessionId。新 Host 生命周期产生新 ID。以 ID 去重、端点独立计时；较旧序号不回滚信息。离开公告仅移除已知源 IP + TCP 端口的 endpoint，并短期保留有界 tombstone 防止延迟旧公告立即复活记录。无公告则默认约 4 秒删除，单个接口过期不会误删其他仍活跃的 endpoint。

所有 socket 操作在独立 worker。主调用方通过 try_send / try_recv 风格接口取事件和更新人数，不阻塞模拟器网络 I/O；CLI 自己轮询且只传普通 Rust 数据，不持有 Environment 或客体对象。事件队列最多 64 项，更新队列最多 8 项；坏报文丢弃不产生错误洪水。消费者持续跟不上时停止发现并报告背压，不无限积压。

网络公告没有加密或认证。ID、序号、tag 和 SessionId 是误连接 / 去重措施，不是防伪证据；应仅在可信 LAN / VPN 中使用，最终连接以实际 TCP 握手为准。

## 防火墙和故障排查

| 现象 | 检查 / 处理 |
| --- | --- |
| 没有 rooms | 等待至少一个公告周期；检查 UDP 42019 入站、Host 广播出站、两端 tag；确认浏览器未占用错误接口 / 端口 |
| UDP 端口已被占用 | 关闭同一电脑的另一个 browse；或改端口并同步修改 Host 的广播目标；可直接使用手动 join |
| 浏览能看到但连接失败 | 检查公告里的 TCP endpoint、Host 是否还在监听，以及 TCP 入站规则；房间可能刚退出或满员 |
| Connection refused | IP、端口错误或 Host 未监听；查看 listening 输出。部分系统对不可连接端口表现为 Timeout |
| Timeout | 路由 / 防火墙 / Wi-Fi AP 隔离 / 休眠 / Host 退出；不是广播服务能修复的问题 |
| 两人可连，三四人失败 | Client 之间的监听端口也要可达；固定各设备 Client `--bind=0.0.0.0:42018` 便于设置规则 |
| 多网卡只看到一个地址 | 添加各子网定向广播目标，检查出口 / 源地址；用手动 IP 选择另一个 endpoint |
| 访客网络 / 热点没有发现 | AP 可能禁止广播；手动 IP 可绕过发现受限，但不能绕过设备间 TCP 隔离 |
| VPN 不显示房间 | 三层 VPN 常不转发广播；直接使用 VPN IPv4，检查所有玩家之间路由 / TCP 端口 |
| 房主退出后列表短暂保留 | 正常退出的 UDP 离开公告也可能丢失；异常退出依赖 TTL；默认 4 秒，缓存不是在线保证 |
| 不支持 IPv6 | v1 仅 IPv4；使用 IPv4 地址。没有实现双栈回退 |

建议仅在需要的私有网络接口 / 配置范围放行程序：浏览器 UDP 42019；Host TCP 42018；多人时 Client 的 TCP 监听端口（默认临时端口，亦可固定）。不要为排查问题直接关闭整个系统防火墙。本阶段不更改防火墙或设备设置。

## 测试结果与未验证项

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --release --lib
python3 dev-scripts/lan-integration-tests.py
python3 dev-scripts/transport-integration-tests.py
python3 dev-scripts/macos-integration-tests.py
```

新增脚本通过真实独立控制台进程测试多房间、去重、名称 / endpoint / 版本 / 人数字段、按发现结果加入、手动加入、双向相关数据路径、人数增减、坏 UDP 报文、房主正常离开和异常退出后的超时清理。为了使测试仅使用本机地址，Host 配置了 `--broadcast=127.0.0.1:临时端口`；该测试是 UDP 单播注入的协议验证，**不是实际两台设备广播发现测试**。

本阶段新增 5 项 Rust 单元测试，覆盖协议 / UTF-8 / 长度、名单去重与多接口、旧序号、容量、BYE 延迟包抑制、过期和手动地址 / 权限参数。

**无法完成的真实测试，明确未验证：**同一 LAN 的两台物理设备广播互相发现、Windows 执行、Android 实机、真实 VPN、IPv6。Android 当前仍无这个控制台的启动器交互入口；没有构建 / 安装 APK，也没有把独立控制台冒充成手机游戏联机。

日志位于 `target/lan-tests/*.jsonl`、`target/lan-tests/manifest.json`（实际测试二进制 SHA-256）和 `target/network-analysis/lan-*.log`。本阶段没有提交、发布或开始 GameKit 适配。

本次最终检查：

| 检查 | 结果 |
| --- | --- |
| Release 主程序及传输探针构建 | 通过，`lan-build.log` |
| Rust 单元测试 | 111 通过，1 忽略，0 失败，`lan-unit.log` |
| LAN 控制台多进程测试 | 通过，`lan-process.log` |
| 第三阶段四人直连 / 异常恢复回归 | 通过，`lan-transport-regression.log` |
| macOS ARM 客体集成测试 | 117 / 117 通过，`lan-guest-regression.log` |
| 网络追踪对照回归 | 通过，`lan-trace-regression.log` |
| 新增 / 修改的 LAN、传输与桌面入口格式检查 | rustfmt 与空白检查通过 |
| 严格 Clippy | 被工作区原有 / 并行的 crypto、posix_io 调试代码警告阻断，未宣称通过 |
| 排除上述无关警告后的定向 Clippy | 通过，`lan-clippy-scoped.log` |

定向 Clippy 命令保留于此，避免将有例外的检查误报为全项目无警告：

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo clippy --lib --bin touchHLE -- \
  --deny warnings --allow unused-variables --allow clippy::useless_conversion \
  --allow clippy::unnecessary_cast --allow clippy::manual_range_contains
```

工作区并行的 crypto 改动曾在最后检查中产生类型错误；未撤销或代改该文件，随后整项目重新编译成功，最终验证使用正常项目构建，**没有使用隔离版或替代运行时来冒充整项目成功**。原有版本号提示和部分 rust-objcopy/libLLVM 提示仍可能出现；没有更改工具链。与本任务无关的工作区改动均保留。
