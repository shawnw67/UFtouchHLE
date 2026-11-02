# MHP2G 原版在线集会所验证

日期：2026-10-28。本文记录真实游戏结果与合成测试的区别；不将匹配对象、TCP 连接或收包次数当作多人狩猎成功。

## 当前结论

| 验收项 | 已确认结果 |
| --- | --- |
| 能否进入在线集会所 | 已在同一台 Mac 的两个真实 MHP2G 进程中进入同一集会所；不等同物理双机 / 长期稳定验收 |
| 是否显示其他玩家 | 已双端视觉核对：名单各有两名玩家，黑红铠甲和轻装头盔两个角色可互相看见 |
| 是否建立会话 | 两个真实 MHP2G 实例完成过同一 Transport Session 的加入、GameKit 匹配回调、原游戏安装 GKMatch 及实际数据回调 |
| 当前阻塞点 | 主路径的计数时序修正已由真实双端大厅验证。电话 / Lobi UI 触发 transform.scale KVC 缺失，已补代码，仍需真实电话入口回归；退出与重连未完成验收 |
| 多人狩猎 | 未开始验收，不宣称成功 |

本轮真实测试是**同一台 Mac 的两个独立进程，127.0.0.1 回环**。没有物理局域网双机、Android 或 VPN 的本阶段实测结果。最新房间发现桥接通过了合成 ARM 客体，尚未在真实 MHP2G 两端验收。

前置文档：[NETWORK_ANALYSIS.md](NETWORK_ANALYSIS.md)、[NETWORK_TRACE.md](NETWORK_TRACE.md)、[TRANSPORT_DESIGN.md](TRANSPORT_DESIGN.md)、[LAN_MULTIPLAYER.md](LAN_MULTIPLAYER.md)、[GAMEKIT_IMPLEMENTATION.md](GAMEKIT_IMPLEMENTATION.md)。它们是前阶段记录；下文说明本轮新增和修正。

## 实际测试环境与存档

- 游戏：`jp.co.capcom.mhp2gjp`，1.00.02，ARMv7；未修改 IPA 或游戏指令。
- 两个独立工作目录：`target/mhp2g-lobby/host/` 和 `target/mhp2g-lobby/client/`。
- 存档分别位于各目录的 `Podroid_sandbox/jp.co.capcom.mhp2gjp/Documents/savedata.dat`。原始项目存档不作为测试写入目标。
- 起初副本损坏，已改名 `savedata.pre-lobby-*.dat` 保留。用户提供修复后的文件，两端均已成功读取并进入村庄。下午测试为区分角色，Host 选第一角色，Client 选第二角色。用户协助将角色移到入口后，已继续完成原版匹配并视觉核对双方互见。
- 测试 app 包仅为本地启动包装，位于各工作目录下 `P2G Lobby Host.app` / `P2G Lobby Client.app`；没有把游戏打入分发包。
- `target/mhp2g-lobby/touchHLE-lobby-next` 是真实游戏重测用的固定二进制；其后新增的房间发现代码使用新的 Release 程序通过合成测试，不能把两者混称为同一轮实测。

## 真实调用顺序及发现

以下来自真实 MHP2G，区别于前阶段的静态引用或合成客体：

1. 游戏初始化 `GameCenterSupport` / Reachability，进入在线入口前用 `SCNetworkReachabilityCreateWithAddress(0.0.0.0:0)` 查询默认路由。原 GetFlags 返回 false，游戏提示“没有连接到通信。请确认连接情况。”；尚未发起认证或匹配。
2. 补实际路由检查后，游戏调用 `GKLocalPlayer.localPlayer`、`authenticateHandler`、`setAuthenticateHandler:`；主线程认证回调执行后，游戏读到 `isAuthenticated == YES`、获取 playerID，`GameCenterSupport.authenticated` 变为 true。
3. 原版“无条件”入口创建请求：default=2、min=2、max=4、group=1000000000、attributes=`0x0000ffff`，调用 `findMatchForRequest:withCompletionHandler:`。这些是本次路径的值，不保证所有 HR / 密码入口相同。
4. 游戏自身约 15 秒后取消未完成的首次搜索；按原版流程建立集会所时，后续请求可变为 attributes=`0xffff0000`。两个低掩码请求即使真实 TCP 已连，也不满足 OR 掩码条件，后端不补造成功。
5. 当两端真实连接且掩码互补，完成 Block 返回 GKMatch；游戏读取 expectedPlayerCount、调用 `finishMatchmakingForMatch:`、设置 `myMatch` 和 delegate，再收到 `match:player:didChangeState:`。
6. 真实游戏调用 `sendDataToAllPlayers:withDataMode:error:`，对端实际执行 `GameCenterSupport match:didReceiveData:fromPlayer:`。游戏还调用 `matchAdditional` / `addPlayersToMatch:matchRequest:completionHandler:`，并可能将已有请求的 attributes 从低掩码切换为高掩码。
7. 早期实现把完成 Block 中 expectedPlayerCount 提前清零，游戏明确设置 `matchAutoCancel = 1`，随后重新 findMatch，旧 match 尚在而得到错误 17。这是实际状态机差异，不是通过放宽“成功”条件解决。

### 可复核的日志定位

保留的 `target/mhp2g-lobby/host/before-expected-count.log`：

- PID 78425，seq 10183：首次低掩码请求。
- seq 31859：游戏取消，后端异步错误 2。
- seq 38158：高掩码请求。
- seq 46883 起：真实匹配后创建 GKMatch；seq 46901 读取 expectedPlayerCount，seq 46903 设置 matchAutoCancel，seq 46915 设置 myMatch，seq 46921 设置 delegate。
- seq 46940：原游戏玩家连接回调；seq 47108 再次 findMatch；seq 47122 错误 17。
- 此日志累计 1812 次原游戏数据接收 delegate 调用；这证明消息到达客体，不证明角色同步成功。

对应 `client/before-expected-count.log`：PID 78448，两次加入记录中可与 Host 的 SessionId 对照；有 1812 次发送 API 调用。早先 Host 取消导致真实 `Stopped(HostLost)`、错误 3；不将其归因于未知公网服务。该轮接收不对称，未通过双向游戏同步验收。

第一轮 `first-real-match.log`、补人修复前 `pre-admission.log`、上午末轮 `before-afternoon-run.log` 也分别保留。`run3.log` / `run.log` 是当前运行日志，重启会覆盖，分析时应优先使用已归档文件。

## 本轮代码修正

| 问题 | 文件 / 入口 | 行为与验证边界 |
| --- | --- | --- |
| 默认路由查询恒失败 | [route.rs](src/frameworks/game_kit/route.rs) `local_route` / `probe`；[sc_network_reachability.rs](src/frameworks/system_configuration/sc_network_reachability.rs) `SCNetworkReachabilityGetFlags` | 显式本地 GameKit 模式下，后台线程执行 UDP bind/connect 获取 OS 路由快照，不发送数据包、不探测 Apple 服务、不宣称对端可达；首轮未完成时仍失败，缓存后返回真实快照 flags；关闭本模式保留原路径 |
| 复制回调崩溃：capture flags 0x8 | [dispatch.rs](src/libc/dispatch.rs) `copy_byref` / `release_byref`、`_Block_object_assign` / `_Block_object_dispose` | 按 Clang 32 位 Block ABI 处理共享 forwarding cell、栈 / 多 Block 所有权、keep/destroy helper；MRC BYREF_CALLER 不额外 retain 捕获对象。编译出来的 ARM 客体覆盖离开栈帧、多副本共享、再次 copy、逐个释放和对象生命周期 |
| 路由恢复后广告 SDK nil 连接超时崩溃 | [ns_url_connection.rs](src/frameworks/foundation/ns_url_connection.rs) `initWithRequest:delegate:startImmediately:` / `start` / `cancel` / `pump`；[dispatch.rs](src/libc/dispatch.rs) `drain_main_queue` | 仅本地 GameKit 模式提供连接对象并在既有主队列异步报告 NSURLErrorDomain/-1009，未提供 HTTP、不发送请求。保留 request/delegate 至失败或取消清理，避免 SDK 用 nil 连接当字典 key；默认模式仍沿用原 stub。不是完整 NSURLConnection / 任意 RunLoop 实现 |
| 游戏端角色不一定是传输 Host | [local_network.rs](src/frameworks/game_kit/local_network.rs) `request` / `admission` / `receive` | 已连接 Client 可通过可靠内部控制帧请求协调 Host 开放准入；允许兼容 group/max 下更新 attributes，向现有玩家交换真实元数据。补人仍等待真实新成员，不直接返回成功；关闭一个成员的准入请求不会关闭其他成员仍请求的准入 |
| 完成回调 expectedPlayerCount 提前为 0 | 同文件 `expected` / `player_ids` / `deliver_notices` | 将物理 connected 与已向客体 announced 分开。完成回调仅基于真实连接和元数据；尚待 delegate 通知的实际玩家仍计入 expected。投递连接通知前更新 playerIDs / expected，断开与重置清理状态。两人和三人客体分别验证真实待通知人数；该修正后真实双端已进入同一大厅并互见，未做长期稳定性验收 |
| LAN 目录尚未接入 GameKit | [rooms.rs](src/frameworks/game_kit/rooms.rs) `State::start` / `poll` / `observe` / `admission`；同文件 `select` | 使用原发现 worker 发布实际 SessionId、监听端口、人数及兼容 tag；自动 / 按 UUID 发现后仍建立同一 Transport。核对加入结果 SessionId，拒绝旧公告误连重启房间；Room 元数据不会充当虚拟玩家 |

Block ABI 依据：[Clang Block Implementation Specification](https://clang.llvm.org/docs/Block-ABI-Apple.html)。仅实现上述已遇到的运行时需求，不声称支持整个现代 Blocks / ARC / GC ABI。

新增诊断事件由原 `--network-trace` 开关控制：`gamekit_request` 记录请求快照；`gamekit_transport` 记录真实状态事件或消息长度 / 交付模式，不输出消息载荷；`gamekit_completion_error` 记录 completion 的错误码、原因和是否补人。原 ObjC 分发日志继续记录回调和游戏写入状态。

## 连接与房间的使用方法

最少变量的真实重测先使用手动 IP：

```sh
# 在两个独立工作目录中运行，分别放置存档和资源链接
/absolute/path/to/touchHLE /absolute/path/to/p2g.ipa --allow-network-access \
  --gamekit-host=127.0.0.1:42018 --network-trace
/absolute/path/to/touchHLE /absolute/path/to/p2g.ipa --allow-network-access \
  --gamekit-join=127.0.0.1:42018 --network-trace
```

真实局域网将 Host 绑定改为 `0.0.0.0:42018`，Client 填 Host 的实际 IPv4。不要将两个进程的 Documents 指向同一个可写目录。

房间自动发现（新增桥接，真实游戏尚未验收）：

```sh
touchHLE p2g.ipa --allow-network-access --gamekit-host=0.0.0.0:42018 \
  --gamekit-room="MHP2G LAN" --network-trace
touchHLE p2g.ipa --allow-network-access --gamekit-discover=auto --network-trace
```

- `--gamekit-room=NAME` 仅 Host 发布，默认不发布公告。公告只在真实传输监听及 Session 已就绪、准入仍开放时存在；关闭准入停止发布，原缓存按发现层 TTL 过期。
- Client 的发现仅在游戏发起匹配时启动。auto 收集首个公告后约 1 秒的当前列表；多个兼容可加入房间明确失败，不静默选择一个。房间 UUID、名字与 endpoint 输出到本地日志，可改用 `--gamekit-discover=ROOM_UUID` 精确选择，或手动 IP。
- 兼容 tag 含游戏 Bundle ID/version、group、maxPlayers 和适配版本；attributes 仍经连接后的 GameKit 元数据握手验证，不用公告伪造互补角色。
- 默认 UDP 发现端口 42019。`--gamekit-discovery-bind=IP:PORT` 改浏览器监听；`--gamekit-broadcast=IP:PORT` 可重复指定 Host 目标，支持定向广播或本机回环测试。单机同端口只运行一个浏览器。
- 自动发现、Host、手动 join 角色参数互斥；只配置发现不会自动打开“允许游戏联网”。
- VPN 走手动 IPv4；三层 VPN 一般不转发广播。TCP/UDP、防火墙、多网卡和 AP 隔离边界沿用前阶段文档；本阶段未验证物理设备或 VPN。

原游戏的“搜索 → 未发现 → 建立集会所”流程保留。两端都在搜索分支时可能同为低掩码，请按原版提示创建一方的集会所，再让另一方搜索。不能用强行改掩码、强制成功回调或改游戏状态来通过验收。

## 崩溃与剩余验证

### 电话 / Lobi 入口回归（2026-10-28）

点击电话会进入 Lobi（HexNakamap / NKChatWebViewController）聊天和在线集会所选择界面。此前每一轮都崩在一个缺失的 API 上，以下均已补齐，崩溃日志归档在 `target/mhp2g-lobby/{host,client}/`：

| 崩溃 | 修正 | 归档日志 |
| --- | --- | --- |
| `-[UIViewController navigationItem]` | 新增 UINavigationItem、UIBarButtonItem / UIBarItem、UIToolbar（只存配置；工具栏显示自定义视图） | `client/phone-crash-latest.log` |
| `-[UIWebView scrollView]` | UIWebView 增加 scrollView、delegate、request、isLoading、stopLoading、stringByEvaluatingJavaScriptFromString:（无网页引擎，页面空白） | `{host,client}/webview-scrollview-crash.log` |
| `-[NSURL host]` | NSURL 按 RFC 3986 拆分：scheme、host、path（解码）、query、fragment、pathExtension；非 http absoluteString 不再断言；`file:` 字符串转为文件 URL | `{host,client}/nsurl-host-crash.log` |
| `CFURLCreateStringByAddingPercentEscapes` | 同时实现它与 `CFURLCreateStringByReplacingPercentEscapesUsingEncoding`（CF 的合法字符与 NULL 语义） | `{host,client}/cfurl-escape-crash.log` |
| `CGGradientCreateWithColorComponents` | 新增 CGGradient（线性 / 径向）、CGContext 路径（MoveTo / LineTo / ArcToPoint / Rect / ClosePath）、非零环绕裁剪、填充和抗锯齿描边、描边颜色与线宽 | `host/cggradient-crash.log` |

游戏自己的输入栏确认键 `hxGradationButton` 用圆角路径裁剪加渐变绘制。第一版只按包围盒处理路径，结果显示成绿色方块（`{host,client}/green-block.log`）；改为真实路径后，用户确认显示正常。

Lobi 聊天本身是 nakamap.com 在线网页，模拟器没有网页引擎，界面弹出（`presentModalViewController`）也未实现，所以点击后看不到内容。这一步验收的是不崩溃，与局域网联机无关。

当前仍需真实双实例完成：

1. 已通过：最新计数时序下，两端完成原版匹配并进入同一大厅，游戏不再因完成时计数清零立即重新匹配。
2. 已通过：双方名单与不同角色的视觉核对，日志也记录到双方发送及接收 delegate；不由计数单独推断人物同步。
3. 已通过（2026-10-28）：Client 从集会所退出后用同一实例重新加入，双方再次互相看见（用户视觉确认）。`{host,client}/regress-1.log` 中两端都有游戏安装的 match，Host 发出 5 次、Client 发出 3 次匹配请求；Host 收到 198 条、Client 收到 270 条数据，无 panic。日志里的 `Local matchmaking cancelled` 是游戏退出时自行取消搜索。
4. 新发现模式的真实游戏复测，再做物理 LAN 双机。

没有做任务接取、入图、战斗或狩猎结算的联机验收。

## 自动检查与日志分析

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --release --lib
python3 dev-scripts/gamekit-integration-tests.py
python3 dev-scripts/macos-integration-tests.py
python3 dev-scripts/analyze-mhp2g-lobby-log.py --self-test
python3 dev-scripts/analyze-mhp2g-lobby-log.py HOST_LOG CLIENT_LOG > lobby-summary.json
```

[analyze-mhp2g-lobby-log.py](dev-scripts/analyze-mhp2g-lobby-log.py) 只读取日志，输出认证观察、游戏安装 match、发送 / delegate 次数、请求参数、会话事件、completion 错误、最后一个已观察 API、截断 / 损坏记录和 panic 标记。它明确将“同一大厅并互相看见”保留为待视觉验证，不根据计数生成成功结论。最后一个已观察调用也不一定是最后一个执行的调用，因为过滤与截断均存在。

本次已完成检查：

| 检查 | 结果 / 限制 |
| --- | --- |
| Release 主程序 | 编译通过；有已有版本前缀提示及无关 posix_io unused-variable 警告 |
| Rust 单元测试 | 117 通过、1 忽略、0 失败，含路由、模式互斥、房间选择与 SessionId 校验 |
| ARM GameKit 多进程测试 | 全部通过，含 __block、异步 HTTP 失败 / 取消、认证、两人 TCP 顺序 / UDP 收发、掩码反例、Host / Client 请求真实第三玩家、自动发现后真实会话收发 |
| 原 ARM 框架集成 | 117 / 117 通过 |
| 定向 Clippy | 通过；显式排除 unused-variables 等前阶段列出的既有警告类别，不声称严格全项目无警告 |
| 日志分析脚本 | 自测试通过；对归档真实游戏日志产生了 `target/mhp2g-lobby/before-expected-summary.json` |

检查日志在 `target/mhp2g-lobby/`；ARM 客体逐进程日志与二进制 / 关键源码哈希在 `target/gamekit-tests/`。合成客体没有加载 MHP2G，也没有伪造匹配对端。

本轮未提交、推送、打 tag、构建 Android APK 或发布。既有工作区其他修改和用户修复的存档均保留。
