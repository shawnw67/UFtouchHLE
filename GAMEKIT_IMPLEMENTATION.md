# MHP2G 历史 GameKit 的本地网络适配

日期：2026-10-27。本阶段在前面未提交的诊断、传输和 LAN 工作之上实现了**显式启用的本地 GameKit 子集**。没有修改 IPA 或游戏指令，没有使用 Apple Game Center 服务器，也没有将返回非 nil 当作联机成功。

**当前结论：**合成 ARMv7 客体已能通过历史 GameKit 选择器，在独立模拟器进程间建立真实连接、完成匹配、收发 TCP / UDP 数据和补入第三名玩家。**这不是 MHP2G 在线集会所或多人狩猎已通过的结论。**实际游戏入口的运行轨迹、第三方 SDK 门槛和双设备效果仍未验证。

## 1. 接口范围的证据

已阅读 [NETWORK_ANALYSIS.md](NETWORK_ANALYSIS.md)、[NETWORK_TRACE.md](NETWORK_TRACE.md)、[TRANSPORT_DESIGN.md](TRANSPORT_DESIGN.md)、[LAN_MULTIPLAYER.md](LAN_MULTIPLAYER.md)。

第一阶段分析的样本为 `jp.co.capcom.mhp2gjp` 1.00.02，ARMv7 主程序 SHA-256：`94a95a4bfaabc0f08b1d521f25c270a1707d9d134788419267a7c75a49f53fdf`。类导入、GameCenterSupport 的方法 / GKMatchDelegate 声明、selector 引用及有界 Thumb 指令证据确认了 LocalPlayer、Matchmaker 和 Match 路径。没有依据实现 GKSession、MultipeerConnectivity 或现代 GKPlayer 收发接口。

第二阶段已有可配置追踪与合成客体验证，**尚未取得真实 P2G 进入在线集会所后的动态日志**。因此以下 API 选择依据是已确认的静态证据，不把静态引用声称为实际游戏已经执行。

本阶段补查静态证据：

| 地址 / 内容 | 已确认结果 |
| --- | --- |
| `GameCenterSupport::matchMaking:`，IMP `0x379e1d` | 请求 default=2、min=2、max=4，使用 group 和 attributes |
| selector 槽 `0x578cf8` | `setPlayerAttributes:`，由原分析 JSON 的 selector_references 核对 |
| `0x379f7c..0x379fba` 的两条分支 | 非零参数分支把 attributes 设为 `0xffff0000`，另一分支设为 `0x0000ffff` |
| `matchAdditional` 中 `0x37a364..0x37a378` | 同样向 attributes setter 写入 `0xffff0000` |
| 原阶段 sendUDP / sendTCP 调用点 | mode 1 / mode 0 分别是不可靠 / 可靠模式，而非直接 BSD socket 导入 |

本机 SDK `GKMatch.h`、`GKMatchmaker.h`、`GKError.h` 用于核对旧选择器、Block 参数和错误数值。SDK 明确 attributes 的语义为所有玩家掩码按位 OR 覆盖 `0xffffffff`，0 视为不限制；**不是属性必须相等**。本实现据此交换并校验互补角色属性。静态分支与游戏具体 UI 操作的对应关系仍待实测，未改游戏来强制选中某一分支。

## 2. 显式启用与默认兼容行为

同一台电脑两实例示例：

```sh
# Host：游戏自己的匹配 API 发起时才创建监听器
target/release/Podroid Podroid_apps/p2g.ipa --allow-network-access \
  --gamekit-host=127.0.0.1:42018 --network-trace
# Client：使用原有游戏流程发起匹配
target/release/Podroid Podroid_apps/p2g.ipa --allow-network-access \
  --gamekit-join=127.0.0.1:42018 --network-trace
```

真实设备将 `127.0.0.1` 换为 Host 的可达 IPv4；Host 通常绑定 `0.0.0.0:42018`，Client 可用 `--gamekit-bind=0.0.0.0:0` 或固定自己的监听端口。TCP 和 UDP 使用相同数字端口，三四人直连要求所有成员监听端口相互可达。VPN 沿用数字 IPv4，但真实 VPN 尚未验证。

必须同时有 `--allow-network-access` 和一个 GameKit 角色；只有其中一个不会打开本适配。未启用时 `GKLocalPlayer.localPlayer` 仍返回 nil，Matchmaker 原有不调度 completion 的占位行为保留；GKSession 等其他模块未改。新增 GKMatch 的发送在无活动匹配时返回真实错误，不会打开网络。

可用选项也能写到应用的 `Podroid_options.txt` 条目中；不要同时配置 Host 和 join 两种角色，冲突会报错。该后端没有修改 Android 启动器界面。

| 选项 | 含义 |
| --- | --- |
| `--gamekit-host=IP:PORT` | 本实例兼任协调 Host，端口 0 可用于测试自动分配 |
| `--gamekit-join=IP:PORT` | 连接指定 Host，端口必须非零 |
| `--gamekit-bind=IP:PORT` | 本地监听绑定，Client 默认为 wildcard 临时端口 |
| `--gamekit-player-id=UUID` | 显式稳定身份；不指定时每个 Environment 生成独立 UUID |
| `--gamekit-timeout-ms=N` | 匹配 / 补人超时，默认 30000，范围 100–300000 |

身份在一个模拟器实例中稳定，跨重启需显式保存 UUID 到用户选项文件；模块不会冒用 Apple 账号。guest `playerID` 是 UUID 的 SHA-256 派生的 16 位 ASCII 十六进制别名，以适配静态代码中观察到的短 ID 操作；原始网络 ID 仍为 128 位。名单安装时检查别名冲突并失败，不把两个玩家静默合并。此别名长度对真实游戏的完整兼容性尚未验证。

## 3. 已实现 API 与测试对应

选择器签名沿用 **NSString playerID 时代**，不是 iOS 8 的 GKPlayer 数组发送接口。Block 的隐含第一个参数始终是 Block 本身。

| API | 源码与行为 | 自动验证 |
| --- | --- | --- |
| `+[GKLocalPlayer localPlayer]` | [gk_local_player.rs](src/frameworks/game_kit/gk_local_player.rs)，启用后创建并持有唯一对象，继承最小 GKPlayer 基类 | 同实例两次返回相同对象，默认关闭回归仍为 nil |
| `-playerID` | 同文件，返回稳定的本地不透明 NSString | 长度 / 重复读取及跨实例收发 ID 一致性 |
| `-isAuthenticated` | 同文件，本地身份就绪状态；不是 Apple 在线登录 | 回调前 false，回调内 true |
| `-authenticateHandler` / `-setAuthenticateHandler:` | 同文件及 [local_network.rs](src/frameworks/game_kit/local_network.rs)，复制 Block，替换时释放旧值，异步 `(UIViewController*, NSError*)`，本地成功为 `(nil,nil)` | 从客体后台线程设置，主线程执行；旧 handler 不调用，捕获 Token 的复制 / 释放及置 nil |
| `GKPlayerAuthenticationDidChangeNotificationName` | 本地认证后经已有 NSNotificationCenter 投递；常量本来已存在 | 投递代码已接入，尚未单独验证游戏 observer 流程 |
| `GKMatchRequest` min/max/default/group/attributes getter、setter | [gk_matchmaker.rs](src/frameworks/game_kit/gk_matchmaker.rs) 复用原属性，发起时快照；2–4 人范围验证 | 所有属性读写、非法人数、请求对象提前释放 |
| `playersToInvite` getter / setter | 复用原 copy / release 行为；非空邀请明确不支持，异步错误 13 | nil、空数组、非空数组拒绝 |
| `+[GKMatchmaker sharedMatchmaker]` | 同文件，复用原单例 | 单例身份 |
| `-findMatchForRequest:withCompletionHandler:` | 同文件及 local_network；只有所需直连、元数据及角色掩码条件满足后返回 GKMatch；Block 为 `(GKMatch*, NSError*)` | 无认证、非法请求、取消、无对端超时、group 不兼容、真实成功 / 不互补属性失败 |
| `-addPlayersToMatch:matchRequest:completionHandler:` | 当前限定配置的协调 Host；同一会话重新开放准入，等待至少一名新玩家且达到请求目标；Block **只有一个 NSError 参数** | 真实第三实例加入后才完成，expectedPlayerCount 由 1 变 0 |
| `-finishMatchmakingForMatch:` | 当前 Host 关闭新成员准入，已有直连继续；Client 不控制协调 Host | 关闭后通过 addPlayers 重新开放，并完成实际补人 |
| `-cancel` | 只取消匹配 / 补人请求；异步返回错误 2，取消首次匹配关闭 Transport，取消补人保留既有 match | completion 不同步触发且只完成一次；之后可再次发起 |
| `GKMatch` 构造 / dealloc | [gk_match.rs](src/frameworks/game_kit/gk_match.rs)，由真实匹配结果创建，后端持有活动引用，disconnect 释放后端引用 | 匹配返回、应用 retain / release、断开后旧对象安全访问 |
| `-playerIDs` | 同文件，NSArray<NSString*>，只列远端已交换元数据的成员 | 两人 / 三人名单；断开后为空 |
| `-expectedPlayerCount` | 同文件，当前请求目标减去就绪人数，完成后为 0 | 首次匹配及补人检查 |
| `-delegate` / `-setDelegate:` | 同文件，非持有引用；投递期间暂 retain，不持有跨回调 borrow | getter / setter、置 nil、主线程 delegate |
| `-sendData:toPlayers:withDataMode:error:` | 同文件及 local_network，mode 0 TCP、1 UDP；验证目标、大小和连接，入队成功才返回 YES | 有效指定玩家、非法目标 / mode、UDP 过大、nil data、NULL NSError 出参 |
| `-sendDataToAllPlayers:withDataMode:error:` | 同文件，向当前全部远端直连入队；不会经 Host 转发 | 可靠广播、顺序 echo、断开后失败 |
| `-disconnect` | 同文件，失效旧代次、关闭网络、取消相关 pending、清空缓存；不改变身份认证 | 对端断开事件、旧 match 名单为空、后续发送错误 3 |
| `match:didReceiveData:fromPlayer:` | local_network 主线程构造 NSData / NSString，恢复原始载荷后投递 | 实际双向 TCP / UDP echo，NUL / 255 字节，10 条可靠消息严格有序 |
| `match:player:didChangeState:` | 同文件，真实成员元数据完成后 state=1，断开后 state=2 | 两人 / 三人连接与断开，回调线程检查 |
| `match:didFailWithError:` | 同文件，终止 / 收发错误通过 NSError 报告 | Host 丢失后客户端错误 3，NSError / 回调线程检查 |
| `_GKErrorDomain` | [gk_local_player.rs](src/frameworks/game_kit/gk_local_player.rs) 常量导出 | ARM 客体直接导入并比较错误域 |
| `-[NSError domain]` | [ns_error.rs](src/frameworks/foundation/ns_error.rs) 补已有域字段的 getter，不替换 NSError 实现 | ARM 错误路径直接调用 |

错误值按本机 SDK 的历史枚举使用：Cancelled=2、CommunicationsFailure=3、NotAuthenticated=6、MatchRequestInvalid=13、InvalidParameter=17。超时使用 3 加本地描述，不编造新的 GKError 编号。成功清空 NSError 出参，失败写入 autoreleased NSError；允许 NULL 出参。广播若部分目标入队后另一个目标失败，不承诺原子回滚，不能据 BOOL 推断全部远端已处理。

## 4. 真实网络映射

```text
GameKit 旧选择器 → 属性快照 / 本地状态机
    → Transport::start(Config, Role::Host / Client)
    → 真实 TCP 直连 + GameKit 元数据握手
    → 人数、名单就绪、attributes OR 校验
    → 主线程 completion(GKMatch,nil)

GKMatch send mode 0 → Transport ReliableOrdered → TCP
GKMatch send mode 1 → 显式启用的 Transport Unreliable → UDP
Transport Event → 主线程构造 NSData / NSString / NSError → 原游戏 delegate
```

协调 / 匹配入口由 `--gamekit-host` / `--gamekit-join` 指定。本阶段**没有将 LAN 控制台的浏览器 UI 塞进 GameKit completion**，也没有自动选择陌生房间。普通 LAN 控制台的 compatibility tag 不同，不能把控制台探针当成 GameKit 玩家混入匹配。自动发现模块保留，GameKit 内房间选择 / 自动发现交互尚未连接。

transport compatibility tag 根据 Bundle ID、Bundle version、group、maxPlayers 和本地后端版本派生；attributes 不参与相等比较，而是在每条真实连接上交换。当前只支持各端相同 maxPlayers 的兼容配置，不完整模拟 Apple 自动匹配服务的请求合并规则。

适配层在 Transport 的消息内使用 `GKU1` + kind 的 5 字节 envelope：kind=1 为 group / attributes / max 元数据（14 字节总长），kind=2 为应用载荷。发送方先排入可靠元数据，再允许完成匹配；收包时剥离 envelope，传给游戏的 NSData 与原输入载荷逐字节相同。UDP 若早于可靠元数据到达可以丢弃，符合不可靠语义，不通过伪造一个 ready 玩家补齐条件。

## 5. 新增不可靠通道

[src/transport/mod.rs](src/transport/mod.rs) 增加 `Config.unreliable`（默认 false）、`Config.max_players`（默认 4）、`UnreliableData` 事件和 Host 准入控制。原 TCP 接口与线帧 v1 保持；没有启用 UDP 的旧调用方仍得到 UnsupportedDelivery。

[src/transport/worker.rs](src/transport/worker.rs) 只在显式配置时绑定与 TCP 监听端口相同数字的 UDP socket。datagram 包头是 `UFUD`、独立 UDP 版本 1、保留位、SessionId、源 / 目标 PlayerId、长度和原始 Transport 载荷；校验会话、目标、已连接成员、源 IP:端口和长度后才发事件。没有 retransmit、排序、UDP 分片重组或静默 TCP fallback；数据可以丢失、乱序，不能承诺送达。

目前 Transport 不可靠载荷上限为 1100 字节；扣除适配层 5 字节后，GameKit mode 1 上限 **1095 字节**，可靠 mode 0 上限 **65531 字节**。过大报错 17。这不是声称还原了原版 GameKit 的完整最大消息能力；原游戏是否需要更大不可靠消息仍须实测。模块不主动分片，但网络自身的 MTU / IP 分片行为未验证。

GameKit only worker 配置 UDP，其他游戏和默认传输模式不额外打开 UDP 端口。网络数据只持有 Rust ID、地址和 Vec，不能访问 Environment 或 ObjC 对象。

## 6. 调度、生命周期和失效处理

- [src/libc/dispatch.rs](src/libc/dispatch.rs) 的既有主队列 drain 调用 `game_kit::pump`；正常主 RunLoop 与合成客体都走这个入口。只在 guest thread 0 投递，重入时 pumping / dispatch draining 标志阻止重复排入。
- authenticateHandler、pending completion 使用已有 `_Block_copy` / `_Block_release`；替换、取消、执行后释放。执行期间没有持有 GameKit State / HostObject 的借用。
- pending 请求保存普通属性快照，不依赖调用者继续持有 GKMatchRequest。闭包在真正调用前已有所有权；测试中捕获的 Token 在栈退出后仍存活，最终释放。
- 后端持有活动 GKMatch 的一份引用，completion / delegate 执行期间另行暂 retain；disconnect 清除后端引用，应用持有的旧对象不会连接到新会话。
- delegate 采用旧版非持有语义，应用应在销毁前置 nil；投递前跳过已从 ObjC 对象表删除的对象。但当前 Runtime 没有通用 ARC zeroing-weak / 地址复用代次机制，不能声称完整 weak 语义已实现。
- 断开 / 取消推进会话 generation，清空旧字节 / notice，已排队成功结果若失效则转为取消错误。错误 completion 仍调用一次，不会因代次变化消失。
- 主线程尚未设置 delegate 时最多缓冲 64 条 notice / 1 MiB 数据；溢出关闭网络并报错，不无限积压或在后台执行游戏代码。Transport 本身继续保留前阶段的队列上限和背压处理。
- 对活动 match 按 Transport 事件顺序更新并投递 delegate，再处理后面的断开事件，避免在先前的数据回调前就应用未来的断开状态；回调中 disconnect 会通过 generation 终止旧 notice。

## 7. 尚未实现与已知边界

- Apple 在线身份、好友、邀请、排行榜等线上服务；本地认证不是 Apple 登录。
- 非空 playersToInvite、GKInvite、自动邀请 / shouldReinvitePlayer 流程、UI 认证页；不凭空触发游戏 reinvite 回调。
- Client 主动要求协调 Host 补人、Host 迁移、断线消息重放、自动恢复游戏状态；当前 addPlayers 明确限定 Host。
- 现代 GKPlayer 属性 / 数组收发 API、完整 GKPlayer 父类、GKSession / PeerPicker 联机后端；仅提供最小 GKPlayer 基类关系。
- 任意 min/max 不一致请求合并、玩家角色约束的全部服务端搜索策略、房间浏览与 GameKit 的交互桥接。
- UDP 大包重组、加密 / 身份防伪、IPv6。现有 LAN 应使用可信网络；ID 与 SessionId 不构成密码学认证。
- UDP 可达性没有独立握手；匹配完成证明真实 TCP / 元数据条件成立，不能证明跨设备 UDP 一定可达。本机 UDP 实际收发已单独验证。
- 真实 MHP2G 在线入口、单人长期回归、双设备狩猎、Android 生命周期、VPN 延迟和其他游戏的全面兼容验证。

## 8. 复现与验证

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release --bin touchHLE --example transport_peer
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --release --lib
python3 dev-scripts/gamekit-integration-tests.py
```

[gamekit-integration-tests.py](dev-scripts/gamekit-integration-tests.py) 编译独立 ARMv7 Mach-O，通过真实 objc_msgSend 和既有 Framework 导出调用 API，并启动独立 touchHLE 进程；仅使用 127.0.0.1 临时 TCP / UDP 端口。用 semaphore wait 触发现有主队列 drain，不修改生产 UI 以绕过 headless 限制。

测试包含：后台线程设置认证、主线程回调、捕获对象复制 / 释放、属性读写、未认证 / 非法 / 邀请 / 取消 / 超时错误、group 拒绝、互补掩码成功和不互补真实连接不得成功、两实例可靠顺序 echo / 不可靠 echo、无效发送及 NSError、第三实例真正补入后才完成 addPlayers、Host 丢失和 match 清理。没有模拟一个成功的远端或伪造匹配结果。

原始输出在 `target/gamekit-tests/*.log`，检查日志在 `target/network-analysis/gamekit-*.log`。静态证据来自原 `p2g.json` 和 `gamecenter-disassembly.txt`；IPA 没有修改或提交。完成本阶段后停止，不发布，不将以上合成客体结果表述成真实 MHP2G 联机已完成。

本次执行结果：

| 检查 | 结果 |
| --- | --- |
| Release 主程序与 transport_peer 探针 | 构建通过，`gamekit-build.log` |
| Rust 单元测试 | 113 通过、1 忽略、0 失败，`gamekit-unit.log` |
| ARM GameKit 集成 | 全部场景通过，含空消息、Client 补人拒绝、互补掩码正反例，`gamekit-arm.log` |
| 原四人 TCP / 异常恢复 | 通过，`gamekit-transport-regression.log` |
| LAN 发现 / 手动连接 | 通过，`gamekit-lan-regression.log` |
| 关闭适配时的追踪对照 | 通过，`gamekit-trace-regression.log` |
| 原 ARM 框架集成测试 | 117 / 117，`gamekit-guest-regression.log` |
| 本阶段修改文件格式 / 新文档链接 / Python 语法 / 空白 | 通过 |
| 定向 Clippy（显式排除无关警告） | 通过，`gamekit-clippy-scoped.log`；不声称严格全项目 lint 通过 |

定向 Clippy 排除了工作区既有 / 并行调试代码的 `unused-variables`、`useless_conversion`、`unnecessary_cast`、`manual_range_contains`、`needless_borrows_for_generic_args` 和 `unnecessary_to_owned`，这些排除项不作为全项目无警告的证据。并行的 cpu / generic_char 调试改动曾短暂阻断构建；未代改或撤销其内容，最终整项目重新编译成功。已有版本前缀和 rust-objcopy/libLLVM 构建提示仍可能出现。

`target/gamekit-tests/manifest.json` 保存测试二进制和关键源码 / 脚本 SHA-256，便于核对测试产物。未进行真实 P2G 在线集会所、物理双设备、Android、Windows 或 VPN 测试；未提交、推送、打 tag 或发布。
