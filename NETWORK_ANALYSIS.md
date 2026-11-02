# MHP2G 多人联机：源码分析与技术验证

分析日期：2026-10-27。源码基线：`47e9bf7ca817e2237c5718861f516768c138a612`，UFtouchHLE `trunk`。

本报告只覆盖第一阶段：源码阅读、IPA / Mach-O 静态分析、既有接口回环验证和架构建议。未修改游戏逻辑、Rust 核心或 Android 网络实现，未实现 GameKit 联机、发现服务或真实多人匹配。这里的“建议新增”模块和协议均尚不存在。

## 1. 结论与证据等级

**该 P2G 样本的多人代码使用 Game Center / GameKit 实时匹配接口。推荐在 touchHLE 内补齐游戏实际使用的旧版 GameKit 接口，以设备间直连传输替换其服务与传输依赖，保留游戏自己的匹配状态机、协议载荷和狩猎同步逻辑。**

这不是依据游戏年代或 `GameKit.framework` 链接作出的预设：已解析到 `GameCenterSupport` 类、`GKMatchDelegate` 协议、匹配回调、发送接口及具体方法的 Thumb 指令证据。尤其要注意，游戏名为 `sendUDP` / `sendTCP` 的方法实际调用 `GKMatch` 发送接口，不能将其直接当成 BSD socket 调用。

| 等级 | 本报告中的含义 |
| --- | --- |
| 已确认：源码 | 可直接从本次基线的函数体、导出表或状态结构读出 |
| 已确认：静态 | 样本的加载命令、导入符号、ObjC 元数据、选择器引用或具体指令中存在 |
| 已验证：执行 | 本次实际执行的测试及其通过条件；仅限文中列出的夹具 |
| 推断 / 建议 | 依据证据提出的设计或依赖判断，尚未由 P2G 多人运行轨迹验证 |

**边界：**没有执行 P2G 双端联机、抓取游戏网络包，或验证真实 Game Center、Lobi、CAPCOM 服务的可用性。静态发现的方法不等于本次已执行的方法；没有找到某符号，也不构成不存在动态查找、自定义协议或其他资源依赖的证明。

## 2. 分析样本与复现方式

本地样本：`Podroid_apps/p2g.ipa`，主程序 `Payload/MHP2G.app/MHP2G`。

| 字段 | 实际结果 |
| --- | --- |
| Bundle ID | `jp.co.capcom.mhp2gjp` |
| Bundle / short version | `1.00.02` / `1.00.02` |
| MinimumOSVersion | `6.0` |
| Device family | iPhone、iPad（`[1, 2]`） |
| 架构 | 薄 Mach-O，32 位 ARMv7；不是 arm64 |
| 加密标记 | 未发现非零 `cryptid`；可读取代码和元数据 |
| 主程序 SHA-256 | `94a95a4bfaabc0f08b1d521f25c270a1707d9d134788419267a7c75a49f53fdf` |
| ObjC / 导入统计 | 99 个自定义类，532 个未定义外部导入，1289 个选择器引用槽 |
| `UIRequiredDeviceCapabilities` | `auto-focus-camera`、`front-facing-camera`、`gamekit`、`armv7` |

结论只针对以上样本，不外推到其他日版修改包或英文版 `MONSTER_HUNTER_FREEDOM_UNITE`。IPA 未修改，也未加入 Git。

新增只读分析脚本：[dev-scripts/analyze-mhp2g-network.py](dev-scripts/analyze-mhp2g-network.py)。基础扫描只用 Python 标准库，直接在 ZIP 中读取 plist 和主程序，不解包或运行游戏，不访问发现的 URL。输出完整导入、类名、协议名、网络候选方法、选择器地址和字符串。

```sh
mkdir -p target/network-analysis
python3 dev-scripts/analyze-mhp2g-network.py Podroid_apps/p2g.ipa \
  > target/network-analysis/p2g.json

# 可选：用 macOS llvm 工具提供可核对的静态指令证据。
unzip -oqj Podroid_apps/p2g.ipa Payload/MHP2G.app/MHP2G -d target/network-analysis
python3 dev-scripts/analyze-mhp2g-network.py Podroid_apps/p2g.ipa \
  --disassemble target/network-analysis/MHP2G \
  > target/network-analysis/p2g.json
xcrun nm -um target/network-analysis/MHP2G
```

`--disassemble` 验证被反汇编文件与选中 slice 完全一致；只识别有界的 `MOVW/MOVT/ADD PC` 选择器加载序列，不声称完成控制流、接收者数据流或全部调用图恢复。网络关键字过滤可能命中音频 `Session`、手柄连接通知等无关结果，必须结合方法归属判断。当前解析器不支持 arm64 ObjC 元数据或相对方法列表，遇到不支持的格式会报错。

## 3. touchHLE 架构与源码定位

| 层次 | 文件与关键入口 | 与联机的关系 |
| --- | --- | --- |
| 客体 CPU / 内存 | [src/cpu.rs](src/cpu.rs) `Cpu::new`；[src/mem.rs](src/mem.rs) `Mem`、`Ptr` | dynarmic 执行 32 位 ARM；客体地址不能当作宿主指针跨线程或跨网络传递 |
| 应用运行上下文 | [src/environment.rs](src/environment.rs) `Environment`、`new`、`run`、`run_inner`、`schedule_next_thread` | 统一持有 CPU、内存、ObjC、dyld、libc 和 Framework 状态；客体线程由协程调度，不等于独立宿主线程 |
| 客体 ABI | [src/abi.rs](src/abi.rs) `GuestFunction`、`CallFromHost` | 从 Rust 调用客体 Block / 函数必须使用正确的 ARM ABI 和 Thumb 位 |
| ObjC Runtime | [src/objc.rs](src/objc.rs) `ObjC`、`HostObject`；[src/objc/messages.rs](src/objc/messages.rs) `objc_msgSend`、`msg_send` | 宿主 Rust 类与客体 ObjC 对象互通；delegate 回调可沿用消息分发 |
| 类与选择器 | [src/objc/classes.rs](src/objc/classes.rs) `objc_classes!`、`link_class`、`register_bin_classes`；[src/objc/selectors.rs](src/objc/selectors.rs) `register_bin_selectors`、`register_host_selectors` | 注册新 GameKit 类、方法；类被占位链接成功不等于已有可用实现 |
| Framework 注册 | [src/dyld.rs](src/dyld.rs) `HostDylib`、`search_host_dylibs`、`Dyld::do_initial_linking`、`do_late_linking`、`do_lazy_link` | 通过 `class_exports`、`constant_exports`、`function_exports` 绑定系统接口 |
| 全局库清单 | [src/dyld/dylib_list.rs](src/dyld/dylib_list.rs) `DYLIB_LIST` | GameKit 已列入清单，无须再造另一套动态库加载器 |
| Framework 状态 | [src/frameworks.rs](src/frameworks.rs) `State`、`ThreadLocalState` | 已有 `game_kit` 状态，可承载后续会话状态；传输本身可拆成共享宿主层 |
| libc 注册与状态 | [src/libc.rs](src/libc.rs) `DYLIB`、`State` | socket、DNS-SD、接口地址及 dispatch 的入口分别在子模块注册 |

`Dyld::do_initial_linking` 依次注册客体 / 宿主选择器，建立惰性与非惰性链接，注册主程序的 ObjC 类、category 和常量字符串。现有实现明确假设 ObjC 定义来自主程序；本样本主应用内未发现独立嵌入式 Framework / dylib 文件，但加载命令仍引用系统库。

### 主事件循环与回调约束

- [src/frameworks/uikit/ui_application.rs](src/frameworks/uikit/ui_application.rs) 的 `UIApplicationMain` 最后调用 `[NSRunLoop mainRunLoop] run`。
- [src/frameworks/foundation/ns_run_loop.rs](src/frameworks/foundation/ns_run_loop.rs) 的 `run_run_loop` 处理输入、定时器、音频、待执行 selector、主 dispatch 队列及媒体 / OpenAL 回调，再通过 `Environment::sleep` 让出执行；当前轮询上限间隔约 16 ms。
- [src/libc/dispatch.rs](src/libc/dispatch.rs) 的 `dispatch_async` 复制 Block 后入队，`drain_main_queue` 在客体主线程执行。`block_invoke_function`、`_Block_copy`、`_Block_release` 可复用；当前 global queue 没有完整并发语义，不能把它当作真正的后台 I/O 线程池。
- [src/frameworks/uikit.rs](src/frameworks/uikit.rs) 的 `handle_events` 在退后台后进入 `Window::wait_until_active`，冻结客体处理。联机心跳和回调不能假设此时仍正常运行。

**建议：**宿主网络线程或非阻塞传输器仅处理字节、连接和普通 Rust 事件，经有界队列交回模拟线程，再构造 `NSData`、更新玩家对象和执行 Block / delegate。禁止网络线程直接调用 `msg!`、借用 `Environment`、操作客体内存。先按主 RunLoop 投递验证游戏的时序，是否需要更细的调度唤醒由后续延迟测量决定。

## 4. GameKit.framework 的当前状态

[src/frameworks/game_kit.rs](src/frameworks/game_kit.rs) 已导出 GameKit 的标准路径，并注册以下类集合。**“已注册”与“多人联机已实现”是两回事。**

| 类 | 当前实现 | 对 P2G 的影响 |
| --- | --- | --- |
| `GKLocalPlayer` | [gk_local_player.rs](src/frameworks/game_kit/gk_local_player.rs)：`+localPlayer` 返回 `nil`，继承暂为 `NSObject` | 没有玩家身份和认证状态 / Block 生命周期；消息发给 nil 可能安静失败，游戏进不去匹配条件 |
| `GKMatchRequest` | [gk_matchmaker.rs](src/frameworks/game_kit/gk_matchmaker.rs)：保存 min/max/default 玩家数、group、attributes、邀请列表 | 属性容器可复用，需要实际校验和匹配语义 |
| `GKMatchmaker` | 同文件：单例已存在；`findMatchForRequest:withCompletionHandler:` 不回调，`addPlayersToMatch:matchRequest:completionHandler:` 只记录日志；finish / cancel 无行为 | 即使绕过登录，仍无法拿到真实 match 或完成状态机 |
| `GKMatch` / `GKPlayer` | 未发现宿主类实现及注册 | 必须按实际调用补齐；是否需要完整 GKPlayer 父类行为由测试决定 |
| `GKSession` | [gk_session.rs](src/frameworks/game_kit/gk_session.rs)：无发现、固定 peer ID `"1"`、收包 handler 不保存、连接 / 发送不能完成 | 是另一套旧式 API 的占位实现；没有证据表明它是本 P2G 的首选突破口 |
| 排行榜 / 分数 | [gk_leaderboard.rs](src/frameworks/game_kit/gk_leaderboard.rs)、[gk_score.rs](src/frameworks/game_kit/gk_score.rs) 主要返回 nil | 不应把排行榜实现列为双人联机前置条件 |

`GKPlayerAuthenticationDidChangeNotificationName` 常量已经导出，但当前未建立完整的认证通知流程。不能仅把 `isAuthenticated` 改成 true 就宣称完成 Game Center 支持。

## 5. 已有局域网网络代码及可复用程度

### 5.1 BSD socket 路径：有实际实现，也有较多窄路径限制

[src/libc/sys/socket.rs](src/libc/sys/socket.rs) 的 `State` / `SocketHostObject` 管理 guest fd 与 `std::net::{TcpListener,TcpStream,UdpSocket}` 的映射。

| 接口 | 已确认行为 | 限制 / 风险 |
| --- | --- | --- |
| `socket` | 受 `Options::network_access` 控制，支持 IPv4 TCP / UDP | 不支持的 family、类型、协议可能断言；默认联网关闭 |
| `bind`、`listen` | UDP 实际 bind；TCP 在 bind 阶段直接建立 `TcpListener`，listen 仅返回成功 | 多处 unwrap；端口占用等普通失败可变成崩溃；并非完整 POSIX 语义 |
| `connect` | 实际 `TcpStream::connect`，之后才设非阻塞 | 建连本身会阻塞宿主；失败 unwrap，不适合直接拿来作为可靠 VPN 连接器 |
| `select` | 通过 peek 检测读取、通过 accept 预收 TCP 连接，检查写就绪 | 主要支持零超时轮询；无数据且要求阻塞时可能断言；异常集和写就绪判断不完整 |
| `accept` | 支持接收 select 预先取得的 stream | 不经该路径的直接 accept 仍有 unimplemented |
| `send`、`recv`、`sendto`、`recvfrom` | 实际 TCP / UDP 读写 | WouldBlock 多处未实现；flags 只接受 0；TCP 不是天然消息边界 |
| `setsockopt` | 保存 `SO_REUSEADDR` / `SO_BROADCAST`；bind / 延迟创建时可为 UDP 开启广播 | REUSEADDR 未真正应用；广播设置时机、关闭选项、多播等不完整 |
| `getsockname`、`shutdown`、`close_socket` | UDP 端口查询；shutdown 限 SHUT_RDWR 并关闭；销毁 socket 状态 | TCP getsockname 未实现，半关闭不支持 |

[src/libc/posix_io.rs](src/libc/posix_io.rs) 的 `find_or_create_socket`、`is_socket`、`close` 提供 fd 生命周期；`fcntl` 没有完整的 socket 非阻塞控制。`ThreadBlock` 当前无专用网络等待类型；文件顶部对“支持阻塞 socket”的说明不能替代逐条检查实际阻塞路径。

**复用建议：**复用地址转换、fd 管理思路和既有回归用例；GameKit 的宿主传输层可直接使用 Rust 非阻塞 socket / 有界 worker，避免绕道 guest libc ABI，也避免让全量修复 POSIX 网络成为 MVP 的前提。通用 libc 改进可另立任务，不能因为已存在 socket 就宣称 GameKit 已接通。

### 5.2 地址、发现和网络状态

| 模块 | 实际状态 |
| --- | --- |
| [src/libc/arpa/inet.rs](src/libc/arpa/inet.rs) | `inet_addr`、`inet_ntop`、`inet_pton` 的 IPv4 转换已有实现；部分无效输入会 panic |
| [src/libc/netdb.rs](src/libc/netdb.rs) | `getaddrinfo` 仅支持 node 为 NULL、AI_PASSIVE 等窄路径；不是通用 DNS；`gethostbyname` 返回 NULL |
| [src/libc/ifaddrs.rs](src/libc/ifaddrs.rs) | `getifaddrs` 返回 -1；接口枚举未实现 |
| [src/libc/net/if_.rs](src/libc/net/if_.rs) | `if_nameindex` 返回 NULL，`if_nametoindex` 返回 0 |
| [src/libc/dns_sd.rs](src/libc/dns_sd.rs) | 只有 `DNSServiceBrowse`、`DNSServiceRefSockFD` 占位，返回 -65544；没有可用 Bonjour 发现链 |
| [src/frameworks/core_foundation/cf_socket.rs](src/frameworks/core_foundation/cf_socket.rs) | `CFSocketCreate` 返回 NULL |
| [src/frameworks/cf_network/cf_host.rs](src/frameworks/cf_network/cf_host.rs) | CFHost / CFStream 建连接口返回 NULL / 空流 |
| [src/frameworks/foundation/ns_url_connection.rs](src/frameworks/foundation/ns_url_connection.rs) | 同步请求返回 nil / -1009；异步初始化返回 nil，未执行 HTTP |
| [src/frameworks/system_configuration/sc_network_reachability.rs](src/frameworks/system_configuration/sc_network_reachability.rs) | 联网关闭时 GetFlags 失败；开启时对 link-local 地址返回 Reachable / IsDirect，未真正探测路由；SetCallback 和 RunLoop 注册返回 false |

没有找到可直接复用的 `NSNetService` / `CFNetService` 完整实现。上游已描述 Asphalt 4、N.O.V.A. 局域网用途，但其工作路径不能直接证明 P2G 能用同一协议发现；本次执行验证见第 10 节。

## 6. P2G 真实依赖和网络调用清单

### 6.1 Framework 加载依赖

主程序包含 23 个 dylib 加载项。与本任务直接相关的有：

- `GameKit.framework/GameKit`：强链接。
- `SystemConfiguration.framework/SystemConfiguration`、`Foundation.framework/Foundation`、`CoreFoundation.framework/CoreFoundation`、`Security.framework/Security`、`libSystem.B.dylib`。
- 另有 UIKit、OpenGLES、OpenAL、AudioToolbox、AVFoundation、CoreMedia、CoreText、QuartzCore、CoreGraphics、CoreData、AssetsLibrary、Accelerate、MessageUI、libobjc.A、libc++.1；AdSupport、GameController 为 weak framework。

本主程序加载命令中**没有单独的 CFNetwork、MultipeerConnectivity 或自带网络 dylib**。这不排除 Foundation 内部网络、静态链接 SDK 或动态查找。

### 6.2 直接导入和 ObjC 证据

| 类别 | 样本中实际发现的接口 / 定义 | 当前宿主状态 |
| --- | --- | --- |
| GameKit 类导入 | `_OBJC_CLASS_$_GKLocalPlayer`、`_OBJC_CLASS_$_GKMatchRequest`、`_OBJC_CLASS_$_GKMatchmaker` | 有占位类；缺身份、匹配结果和传输 |
| 本地身份 | `localPlayer`、`authenticateHandler`、`setAuthenticateHandler:`、`isAuthenticated`、`playerID` | localPlayer 返回 nil，其余行为未补齐 |
| 请求属性 | `setDefaultNumberOfPlayers:`、`setMinPlayers:`、`setMaxPlayers:`、`setPlayerGroup:`、`setPlayerAttributes:` | 属性已存在，匹配语义未实现 |
| 匹配 | `sharedMatchmaker`、`findMatchForRequest:withCompletionHandler:`、`addPlayersToMatch:matchRequest:completionHandler:`、`finishMatchmakingForMatch:`、`cancel` | 未建立可完成的异步流程 |
| Match 对象 | `expectedPlayerCount`、`playerIDs`、`setDelegate:`、`disconnect` | GKMatch 尚未实现；setDelegate 是通用选择器，必须按接收者判定 |
| Match 发送 | `sendData:toPlayers:withDataMode:error:`、`sendDataToAllPlayers:withDataMode:error:` | GKMatch 尚未实现 |
| 游戏回调实现 | `match:player:didChangeState:`、`match:didReceiveData:fromPlayer:`、`match:didFailWithError:`、`match:shouldReinvitePlayer:` | 在游戏自己的 `GameCenterSupport` 类中；应回调原逻辑，不改写它 |
| Reachability C 导入 | `SCNetworkReachabilityCreateWithAddress` / `CreateWithName` / `GetFlags` / `SetCallback` / `ScheduleWithRunLoop` / `UnscheduleFromRunLoop` | 部分模拟、部分 stub |
| 接口索引 | `_if_nametoindex` | stub，具体调用场景未确定 |
| HTTP 类与选择器 | NSURLConnection、NSURLRequest；`connectionWithRequest:delegate:`、`initWithRequest:delegate:startImmediately:` | HTTP stub；与 SDK / 聊天等支线相关性需运行时验证 |
| 安全及异步 | CCCrypt、CCHmac、CC_SHA1、SecItemAdd / CopyMatching、多种 dispatch 接口 | 有相关实现；仅凭导入不能判断是游戏包加密还是 SDK 登录 / 缓存用途 |

主程序未发现 `_socket`、`_connect`、`_bind`、`_send`、`_sendto`、`_recv`、`_recvfrom`、`_select` 等 BSD socket 直接导入，也没有 DNSService 直接导入。没有发现 `GKSession` / `GKPeerPickerController` 类导入或 `GKSessionDelegate` 协议；发现了 **`GKMatchDelegate`，并确认 GameCenterSupport 声明实现它**。

没有直接导入 `_OBJC_CLASS_$_GKMatch` **不代表未使用 GKMatch**：它由匹配完成 Block 返回，程序可以只通过实例发送消息。该样本还有 `@"GKMatch"` 属性 / Block 类型字符串，以及实际发送接口的指令证据。

### 6.3 关键方法地址及反汇编核对

以下地址是此 SHA 样本的静态 VM 地址；IMP 最低位 1 表示 Thumb。不能在未来实现中盲目当成稳定补丁地址。反汇编工具可能用“最近一个保留的 C++ 符号”给整段代码命名；本报告按 ObjC method list 的 IMP 定界，不采用那些误导性标签。

| 游戏方法 / IMP | 静态确认的行为 |
| --- | --- |
| `GameCenterSupport::authenticatePlayer` `0x379a55` | 访问 localPlayer、authenticateHandler / setAuthenticateHandler、isAuthenticated、playerID；还包含登录 UI 呈现分支 |
| `matchMaking:` `0x379e1d` | 设置 default=2、min=2、max=4；使用 group / attributes；出现 findMatch 完成 Block 和 expectedPlayerCount 检查 |
| `matchAdditional` `0x37a2d5` | 通过 addPlayersToMatch 补充玩家，随后检查 expectedPlayerCount 和完成匹配 |
| `disconnect` `0x37a629` | 获取 myMatch 后发送 disconnect |
| `match:player:didChangeState:` `0x37a719` | 使用玩家 ID 字符串和列表 / 锁更新匹配状态 |
| `match:didReceiveData:fromPlayer:` `0x37a8b9` | 读取 NSData bytes / length，分配到 recvDataUDP、recvDataTCP、recvDataCon 等队列；游戏内有自己的包分类和连接控制消息 |
| `sendUDP:playerID:` `0x37c34d` | 选择器槽 `0x578dc8`；`0x37c3ce` 调用发送，withDataMode 的栈参数为 1 |
| `sendUDPALL:` `0x37c3e5` | 槽 `0x578dcc`；`0x37c432` 将 mode 参数设为 1，随后调用广播发送接口 |
| `sendTCP:playerID:` `0x37c449` | 与 sendUDP 使用同一个发送选择器；`0x37c47a` 设 mode=0，`0x37c4ca` 调用 |
| `TcpSend:senddata:sendlen:` `0x37ca9d` | 同样通过 myMatch 的发送选择器，`0x37cb76` 附近加载；没有变为 BSD send |
| `TcpConnect:` `0x37c769`、`TcpAccept:sender:port:` `0x37c989` | 使用 receiveCon、NSData 和 sendTCP 实现游戏自己的连接流程，不是可直接复用的宿主 socket |
| `Reachability::reachabilityForLocalWiFi` `0xd268d` | 构造 AF_INET sockaddr，地址字节为 169.254.0.0，传入 reachabilityWithAddress；可对应当前宿主 link-local 判断分支 |

按 Apple 头文件 / 文档语义，GKMatch 的 mode 0 为 reliable、1 为 unreliable；本次原生 SDK 枚举数值检查也纳入验证。上述“TCP/UDP”更准确地说是游戏的可靠 / 非可靠消息抽象。**传输层应保留完整游戏载荷，不重新设计狩猎协议。**

## 7. 外部服务和额外协议依赖

### 已确认存在

1. **Game Center 身份与匹配接口。**原版匹配不等于普通 LAN 自动发现。在“不用公网服务 / Relay”的目标下，需要本地玩家身份和 LAN 匹配适配，提供游戏需要的对象与异步事件，而不是请求 Apple 账户密码或实现 Apple 服务协议。
2. **Lobi / Nakamap SDK。**自定义类包含 `LobiViewController`、`HexNakamap`、`Nakamap`、`NKNakamapViewController` 和多个网络 / JSON 类；存在 signup、createGroup、joinGroup、chat 等方法。发现 `nakamap.com`、`nakamapapps.com`、`thanks.nakamap.com`、`thanks-stream.nakamap.com` 及测试域名字符串，Info.plist 也注册了 Nakamap URL scheme。
3. **Chartboost SDK。**发现 CBAPIConnection、CBReachability 等类及 `https://live.chartboost.com`，另有 CAPCOM 支持页面、App Store URL。不能把所有 HTTP 字符串当作狩猎服务器。
4. **游戏内自定义控制与数据协议。**GameCenterSupport 的 recvDataCon / TCP / UDP 队列、TcpConnect / TcpAccept 及 update 匹配状态机表明：SDK 连通后仍需让原逻辑完成握手、玩家排序、同步和超时处理。

### 尚未验证

- Lobi 群组 / 登录是否为某个多人入口的强制门槛，还是可选聊天、邀请支线；不能只凭 SDK 存在便认定必须重建该服务。
- group、playerAttributes 的实际值与含义、房间筛选规则、玩家排序 / 领导者选择、包格式与大小、回调要求的精确顺序和超时。
- 是否还存在资源中的关卡 / 下载任务服务、证书校验或完整性校验；本次未进行全资源协议逆向。
- 未查服务当前在线状态，未向这些域名发请求；不宣称其存活或已关闭。
- 没有证明与真实 iOS 客户端可互通。建议首个支持目标明确为相同游戏版本的 UFtouchHLE ↔ UFtouchHLE。

已确认的身份层阻塞点是 `GKLocalPlayer.localPlayer == nil`；`findMatch` 永不回调也是独立阻塞点。具体多人入口先遇到 Reachability、登录还是其他条件，仍须动态跟踪。仅开启启动器的“允许游戏联网”不会修复这两个问题。

## 8. 最小可行架构（建议，未实现）

```text
未修改的 P2G 游戏逻辑
    │ 原有 GKLocalPlayer / GKMatchmaker / GKMatch 调用
    ▼
GameKit 宿主兼容层：身份、匹配状态、玩家列表、Block / delegate 事件
    │ 普通 Rust 事件及不透明游戏字节载荷
    ▼
直连会话与传输层
    ├─ 手动 IP:端口 ──────────────┐
    ├─ 后续 LAN 发现 ─────────────┤ 同一套会话握手与版本筛选
    └─ 后续 VPN 地址 ─────────────┘
    │ 每对玩家直接传输：可靠通道 + 非可靠数据报
    ▼
对端 UFtouchHLE → 对端 GameKit 兼容层 → 原游戏接收回调
```

### 可复用与必须补齐

| 项目 | 处理建议 |
| --- | --- |
| Runtime、ABI、NSString / NSArray / NSData / NSError、通知中心 | 复用；认真处理 retain / release、nil delegate、字节拷贝和 Block 所有权 |
| GameKit dylib 注册、GKMatchRequest、GKMatchmaker 单例 | 扩展现有模块，不新建平行 Framework 注册机制 |
| GKLocalPlayer | 补本地身份、isAuthenticated、playerID、authenticateHandler 的保存、复制、异步回调及取消；界面明确为本地身份，不能表示真实 Apple 登录 |
| GKMatch | 建议新增 `src/frameworks/game_kit/gk_match.rs`，注册真实已观察的旧版属性、发送、断开和 delegate 行为；该文件当前不存在 |
| Matchmaker 状态机 | LAN 建房 / 加入、2–4 人、group / attributes、超时、取消、补人、expectedPlayerCount 和完成 Block 的语义必须新增 |
| 宿主网络层 | 建议单独新增共享传输模块；对外只暴露字节与事件，不暴露客体对象地址；实际文件名在下一阶段确定 |
| LAN 自动发现 | 当前 DNS-SD stub 无法复用；可先新增受控 UDP 广播发现，也可后续选宿主 mDNS 实现。并非先把全套 DNSService API 补齐 |
| 手动地址 UI / 配置 | 必须新增；宿主侧提供建房 / 加入和 IP:端口，不修改游戏逻辑来塞入菜单或重定向函数 |
| Reachability | 验证 P2G 的 flags / 通知门槛后补正确的本地连通状态；不可全局强制互联网可达 |
| TCP / UDP socket shim | 可作已有功能基线；GameKit 后端不必通过 guest fd 调用这些有窄路径限制的接口 |

### 传输与会话原则

- MVP 先验证两人手动 IP，最终加入自动发现时仍复用同一条会话链。自动发现只是地址来源，不是第二套匹配协议。
- 可靠消息可先用每对玩家一条 TCP 连接承载，增加长度分帧、队列上限、断线与写背压；非可靠消息用 UDP，保留消息边界，设计必要的版本 / 会话标识及分片限制。不能把 unreliable 静默改成 reliable 后就判定性能通过。
- 多人扩展优先最多四人的直接连接拓扑：一个玩家可负责发布房间 / 协调名单，但不转发其他玩家的游戏数据；避免把房主做成 Relay。玩家全互联的可达性和角色变化必须验证。
- 握手需校验协议版本、游戏 Bundle ID、游戏版本 / 兼容标识、会话及玩家身份、group / attributes、连接能力。具体包头、端口号、字节序都是待定设计，不能当作从游戏恢复出的原版协议。
- 载荷保持不透明，交给游戏原回调。跨端不能传递 `id` / `Ptr`、内存地址或裸 Rust 结构体。
- 身份需要跨重连保持一致且不同设备唯一。反汇编里多处出现与玩家 ID 相关的 16 字节复制 / 比较，具体限制待测；不要未经验证就把 36 字符 UUID 当作 guest playerID。
- 后台 worker 事件必须附会话代次，断开或 cancel 后丢弃过期事件；Block 和对象回调在模拟线程执行，并处理重入与 delegate 变更。
- 新网络功能必须服从现有 `network_access` 开关，测试默认不开网时维持单人流程。

### LAN、手动 IP 与 VPN 的边界

局域网广播仅在可转发广播的网络中工作，访客网络、AP 隔离或热点策略可能阻止发现或互通。手动 IP 可以绕开发现失败，但不能绕过路由 / 防火墙限制。

未来 VPN 需要其自身提供设备间可路由地址和所需端口可达性。三层 VPN 通常不跨网转发 LAN 广播，故手动 VPN IP 应作为独立可用入口；不能承诺“接入任意 VPN 即自动发现”。本项目不实现公网匹配、NAT 打洞服务或 Relay，也不承诺无公网可达路径的任意两端能直接连接。

## 9. 风险与下一阶段开发顺序

| 风险 | 下一阶段需要的证据 / 应对 |
| --- | --- |
| API 存在但消息顺序不兼容 | 记录 localPlayer 认证、request 创建、完成 Block、setDelegate、player state、第一包的线程和顺序；以游戏实际轨迹为准 |
| 匹配成功却无法开始任务 | 保留并观察 recvDataCon 与可靠消息握手，不只验证 findMatch 返回对象 |
| 2 人进入后补第 3/4 人死锁 | 单独验证 addPlayers、expectedPlayerCount、finish、cancel 与玩家名单一致性 |
| 网络失败变成宿主 panic | 明确超时、WouldBlock、短读短写、端口冲突、重复连接、错误 NSError 语义；不要直接依赖 socket shim 的 unwrap 路径 |
| 帧率 / 延迟导致掉线 | 阻塞建连和 I/O 不得占住模拟线程；测量 16 ms 轮询、回调队列积压及游戏内超时 |
| ID / 版本 / 数据边界 | 验证 guest ID 长度及稳定性，拒绝不兼容版本、异常长度和未知会话；确保各端载荷一致 |
| Android 网络与生命周期 | Manifest 已有 INTERNET，但未有 Wi-Fi 多播锁和相关权限实现；若选 mDNS，要核对目标系统要求。后台冻结会停投回调，应先定义掉线策略 |
| VPN 路由与多网卡 | 避免硬编码 wlan0 / en0 或仅按物理 Wi-Fi 判定可达，显示监听 / 连接地址，验证 VPN MTU 和 UDP 可达性 |
| Lobi 等 SDK 门槛不明 | 先确认游戏入口的动态调用；不要先搭建未知第三方服务，也不要改写游戏状态机绕过 |
| 破坏已正常的单人模式 | 联机显式启用，单人、存读档、音频和切后台都需回归；首轮测试使用独立存档备份 |

建议阶段顺序（以下均未开始）：

1. **补齐运行时调用证据。**在现有 host Framework 边界做可关闭跟踪，实际点入多人入口，定位第一个阻塞调用；不修改游戏二进制或游戏状态。验证 Lobi 是否为门槛，并确定 localPlayer / match 的最小属性和回调需求。
2. **只验证兼容层对象及异步行为。**用合成 ARM 客体测 Block 复制 / 释放、身份、取消、delegate 生命周期、预期玩家数和错误语义，先不接真实游戏联机。
3. **两实例手动地址握手。**实现可显式关闭的本地 GameKit 适配和点对点传输；通过原版调用流程完成两人加入，确认游戏自带连接控制消息能往返。
4. **两人狩猎闭环。**进入同一集会所、接任务、入图、战斗、结算、返回，核对可靠 / 非可靠数据和断线表现；先 Mac 双实例，再真实 Android 双设备。
5. **LAN 自动发现与多人补齐。**同一协议加入广播 / mDNS 地址发现，测试超时、旧房间清理、第三 / 四人加入和房主退出；保持每对玩家直传数据。
6. **VPN 验证。**沿用手动地址和数据面，验证跨网延迟、丢包、MTU、多网卡、重连；不增加公网服务器或 Relay。

每一步都有独立通过条件。不能用“对象返回非 nil”“收到一包”或“出现玩家名字”替代完整双端狩猎验证。

## 10. 本阶段检查结果与交付范围

执行结果：

- 当前基线 `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release` 成功；构建日志有现有的版本前缀及部分构建脚本 rust-objcopy 警告，不影响该次成功退出。
- 主程序已用自写解析器读取，架构及 GameKit / Reachability 导入另用 `xcrun otool -hv`、`xcrun nm -um` 交叉核对；532 个导入符号与 Apple nm 的集合完全一致，IPA 与直接 Mach-O 输入的 hash / 导入结果相同。
- 解析器薄 ARM / FAT ARMv7 输入检查通过，6 个截断、损坏、加密或不支持格式的输入按预期拒绝；Python 语法、报告本地链接、代码块配对及新增文件空白检查通过。原生 SDK 的 `GKMatch.h` 同时确认 reliable=0、unreliable=1 的枚举顺序。
- `cargo test --release --lib`：98 通过、1 忽略、0 失败；`python3 dev-scripts/macos-integration-tests.py`：117 / 117 通过。
- ARMv7 合成客体通过 **联网开关拒绝路径、UDP 本机回环、TCP 本机回环**。脚本：[dev-scripts/network-analysis-smoke.py](dev-scripts/network-analysis-smoke.py)。测试只绑定 `127.0.0.1` 临时端口，没有调用公网服务、广播、多播或发现远端设备。
- 已确认 tracked 源码无改动，本次仅新增本报告和两个分析 / 验证脚本；不以这些测试宣称 GameKit 或 LAN 已运行。

```sh
PYTHONDONTWRITEBYTECODE=1 python3 dev-scripts/network-analysis-smoke.py
```

该夹具使用现有 macOS ARMv7 构建辅助工具生成独立测试应用，资源与日志在 `target/network-analysis/`。最初遇到现代 SDK 引入的 fd_set 检查和编译器除法辅助符号，已仅在测试夹具中改用 Darwin 32 位 fd_set 的有界位操作；没有为通过测试而增加 runtime 接口。

当前明确**未验证**：双设备互通、广播发现、VPN、真实 P2G 匹配时序、认证兼容回调、游戏控制包握手、多人战斗同步、第三方服务可选性、真实 iOS 客户端互通。本阶段没有开始实现下一阶段的任何核心联机模块。

## 11. 官方语义参考

源码与该样本是主要证据，Apple 文档用于核对接口语义，不能替代样本调用分析：

- [Game Center Programming Guide：Real-Time Matches](https://developer.apple.com/library/archive/documentation/NetworkingInternet/Conceptual/GameKit_Guide/Matchmaking/Matchmaking.html)：匹配、预期玩家、发送及 delegate 职责。
- [Working with Players in Game Center](https://developer.apple.com/library/archive/documentation/NetworkingInternet/Conceptual/GameKit_Guide/Users/Users.html)：原版玩家身份与认证流程；原 API 将 playerID 视为不透明标识。
- [GKMatch.SendDataMode](https://developer.apple.com/documentation/gamekit/gkmatch/senddatamode)：可靠与非可靠模式的交付区别。

文档中的 Game Center 行为是原版 Apple 服务语义；本报告建议的是适用于 UF 客户端的本地兼容实现，不是恢复或冒充 Apple 公网服务。
