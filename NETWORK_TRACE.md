# MHP2G 网络调用追踪（第二阶段）

本阶段仅增加诊断能力，不实现 Game Center 认证、匹配、发现、直连或传输协议，不修改游戏逻辑。前一阶段的静态证据见 [NETWORK_ANALYSIS.md](NETWORK_ANALYSIS.md)。分析基线为 `47e9bf7c`；本文描述其上的未提交诊断改动。

## 启用与复现

追踪默认关闭，与“允许游戏联网”是独立开关。仅启用追踪不会授予联网权限，也不会让现有失败接口成功。

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release
mkdir -p target/network-trace
target/release/Podroid Podroid_apps/p2g.ipa \
  --network-trace --network-trace-limit=50000 \
  > target/network-trace/p2g-offline.log 2>&1
```

需要对比允许联网时的实际路径，在另一次运行追加 `--allow-network-access`，并另存日志。当前 Reachability 的 link-local 快捷分支只返回固定 flags，不能证明真实 Wi-Fi、互联网或游戏服务连通。

Android 可在应用数据目录的 `Podroid_options.txt` 中，为现有同名条目追加以下参数（保留原有机型等选项，避免重复条目）：

```text
jp.co.capcom.mhp2gjp: --network-trace --network-trace-limit=50000
```

使用启动器已有的日志导出功能取得 `Podroid_log.txt`。日志也走现有控制台 / Android 日志输出路径，不新增日志文件或上传。重复启动前保存上一份日志，避免覆盖。诊断结束删除这两个选项，或使用 `--network-trace=off`；`--network-trace=on` 等价于无值开关。limit 必须为正整数，默认 50000。

复现步骤：

1. 记录 touchHLE 构建版本、IPA 版本、机型、联网开关和设备系统；从冷启动开始记录。
2. 进入存档村庄，使用游戏原有入口进入在线集会所。记下点击时间、提示文字，以及是否进入等待界面。
3. 等待约 20 秒，正常退出并保存日志。不要通过补丁跳过认证或强行推进游戏状态。
4. 分别保留联网关闭与开启的日志；遇到崩溃也保留完整日志和寄存器 / 栈输出。
5. 若出现 `trace_truncated`，增大上限后从冷启动重录。上限以事件数计，不是文件字节数。

## 日志格式

每个事件占一行，前缀是 `[NetworkTrace] `，后面为 JSON。例如：

```text
[NetworkTrace] {"seq":1,"ms":42,"pid":1234,"thread":0,"parent":null,"event":"api_invoked","details":{"api":"[GKLocalPlayer localPlayer]","args_r0_r3":[805300000,805300004,0,0],"context":{"class":"GKLocalPlayer","receiver":805300000,"selector":"localPlayer","nil_receiver":false}}}
```

上例仅说明结构，不是 P2G 在线集会所的实测记录。

| 字段 | 含义 |
| --- | --- |
| `seq` | 当前模拟器进程的追踪递增序号，从 1 开始；重启后重置 |
| `ms` | 相对模拟器 startup_time 的宿主经过毫秒数；不是网络延迟 |
| `pid` / `thread` | 宿主进程 ID / 客体线程编号（从 0 开始） |
| `parent` | 同一客体线程中尚未返回的最近一个被追踪调用的序号；不是完整调用栈 |
| `event` | 事件种类，见下表 |
| `details.api` | C 导出符号，或运行时分发类名与 ObjC selector；不区分类方法与实例方法的显示符号 |
| `args_r0_r3` | 入口处原始 ARM r0–r3，无符号十进制；ObjC 通常为 receiver、SEL、前两个参数，C 为 ABI 参数寄存器 |
| `return_r0_r1` | 出口原始 r0–r1。只有结合真实签名才可解释；void 方法的数值不是返回状态，r1 也未必有效 |
| `details.call` | 对应入口事件的 `seq`，可跨交错日志配对 |
| `context.receiver` / `block` / `callback` | 客体地址标识，可用于关联同一进程对象；不是宿主指针或网络地址 |
| `errno_if_initialized` | 已初始化时读取已有线程 errno，不创建存储、不改写；非 errno 接口或成功之后可能是旧值 |

不会为了打印参数调用 `description`、读取 NSData 内容或额外解引用参数指针。寄存器中的地址不解码成字符串；栈参数、结构体、浮点值与出参不能靠这四个数完整还原。dispatch Block 在 ABI 参数写入前记录，因此 `args_r0_r3` 为 null，真实 Block 地址单列于 context。仅记录其已知的无额外参数调用，不伪造认证 / 匹配结果。

| 事件 | 解释 |
| --- | --- |
| `api_invoked` / `api_returned` | 已选中的实际入口 / 正常返回；未返回时可能发生原有 panic 或进程退出 |
| `object_created` | alloc / allocWithZone: / new 返回了非零对象地址；嵌套构造会为同一对象出现多条，不能直接累计实例数 |
| `state_written` | 当前真实实现已执行的 GKMatchRequest 标量 setter 或 GKSession setAvailable:；不表示匹配成功 |
| `observed_failure` | 当前 `GKLocalPlayer.localPlayer` 返回 nil 的明确证据 |
| `network_error` | 按已知 C ABI 判定的失败，或现有同步 HTTP stub 写入的 NSError（域、code、地址）；不把所有零寄存器值当错误 |
| `callback_not_scheduled` | 当前已有 GameKit 匹配 / Reachability stub 没有调度传入回调；不是回调已经执行 |
| `callback_scheduled` | 原有 dispatch_async / dispatch_after 已实际入队，且发生在追踪的网络调用上下文内 |
| `callback_invoked` / `callback_returned` | 被筛选的实际 ObjC delegate 调用，或已追踪 dispatch Block 的执行 / 返回；不证明调用者一定是系统 Framework |
| `reachability_address` | 原有 CreateWithAddress 实现已读取的 IPv4 地址与端口 |
| `reachability_result` | 联网禁用分支或 link-local 快捷成功分支的实际 flags 写入情况；其他路径查看 API 返回 |
| `dispatch_error` | 追踪的 ObjC 调用遇到缺失类 / 方法，随后仍执行原来的错误路径 |
| `trace_truncated` | 唯一的终止标记，包含 limit；其后不再输出追踪事件，原有 API 仍照常执行 |

Block 同一地址多次入队时，`scheduled_candidates` 列出可能的调度事件，`correlation=ambiguous_block_reuse` 明确表示不能唯一对应；不会声称已经确定某一次调度。`single_pending` 表示诊断表中只有一个候选。客体线程让出期间，其他线程的日志可能交错，应按 call、thread、parent 关联，不能用相邻行配对。

## 覆盖范围与源码定位

| 位置 | 入口 / 作用 |
| --- | --- |
| [src/network_trace.rs](src/network_trace.rs) | `objc_begin`、`c_begin`、`end`、`record`；过滤、配对、JSON、上限及只读错误分类 |
| [src/options.rs](src/options.rs)、[OPTIONS_HELP.txt](OPTIONS_HELP.txt) | 默认关闭、开关与事件上限 |
| [src/objc/messages.rs](src/objc/messages.rs) | `objc_msgSend_inner` 在原有方法分发前后观察；包括 nil receiver、宿主与客体方法、缺失接口错误 |
| [src/dyld.rs](src/dyld.rs)、[src/environment.rs](src/environment.rs) | `get_svc_handler` 保留导出名，CPU SVC 分发处记录 C API；不替换 Framework 导出和链接过程 |
| [src/libc/dispatch.rs](src/libc/dispatch.rs) | 原有 async / after 入队及 Block 执行处关联诊断信息，不增加 retain、复制、回调或调度 |
| [src/libc/errno.rs](src/libc/errno.rs) | `peek_errno` 只读取已存在的 errno |
| [gk_matchmaker.rs](src/frameworks/game_kit/gk_matchmaker.rs) | 明确记录现有匹配 stub 不调度 completion |
| [sc_network_reachability.rs](src/frameworks/system_configuration/sc_network_reachability.rs) | 既有地址、flags 分支和不支持 callback 的证据 |
| [ns_url_connection.rs](src/frameworks/foundation/ns_url_connection.rs) | 现有同步请求失败时已构造的 NSError，不新增请求 |

ObjC 筛选包括所有 `GK*` 类，以及前阶段静态发现的 `GameCenterSupport`、`ReceiveData`、`Reachability`、`CBReachability`、Lobi / Nakamap 候选类、Chartboost 连接类、Foundation URL 请求 / 连接和 NetService 类。精确名单见 `network_class`。`match:`、`session:`、`connection:` 等候选 delegate selector 也会记录，可能包含无关同名方法，需要核对 class。给 nil 发消息无法恢复原接收者类型，只追踪明确列出的网络 selector，标注 `nil_or_unknown`。

C 筛选包括 SCNetworkReachability、DNSService、CFSocket / CFHost / CFStream 系列、BSD socket 收发 / 连接 / select / poll、DNS 和接口枚举的候选符号；是否已有可执行实现仍由原来的导出表决定。名单不代表游戏实际调用，也不新增缺失接口。通用 read/write/close、任意系统调用、直接调用 IMP、未经过现有消息分发的自定义函数、未知自定义网络类不保证覆盖。

回调覆盖是有限的：ObjC delegate 实际消息分发，以及网络调用上下文中由既有 dispatch_async / dispatch_after 调度的 Block。任意客体直接调用 Block、其他异步库、通知和 dispatch source 的完整生命周期尚未统一追踪。没有日志不能独立证明没有网络依赖。

## 如何分析在线集会所日志

先提取追踪，保留完整原始日志用于核对 panic 和游戏提示：

```sh
rg '\[NetworkTrace\]' target/network-trace/p2g-offline.log
```

可用标准库输出便于阅读的顺序清单：

```sh
python3 - target/network-trace/p2g-offline.log <<'PY'
import json, sys
for line in open(sys.argv[1], errors='replace'):
    if '[NetworkTrace] ' not in line:
        continue
    r = json.loads(line.split('[NetworkTrace] ', 1)[1])
    print(r['seq'], r['ms'], r['thread'], r['parent'], r['event'], r['details'])
PY
```

依次核对以下证据：

1. 入口时间附近是否实际调用 Reachability；返回 false 还是 link-local 固定 flags？不要把 GetFlags 成功当成服务器认证成功。
2. 是否调用 `[GKLocalPlayer localPlayer]` 并得到 `local_player_nil`？后续是否给 nil 发送认证相关 selector？nil 消息不代表真实认证请求已经发出。
3. 是否实际构造 GKMatchRequest、写入人数 / group，调用 findMatch 或 addPlayers？参数地址与 call 可关联，不能由静态 selector 推定已匹配。
4. 是否出现 callback_not_scheduled？若有，当前原有 stub 已是明确阻断点；不能通过诊断代码补发成功回调继续游戏。
5. 是否存在其他 HTTP、DNS-SD 或 socket 调用？区分 SDK 背景请求与进入集会所时的相关调用，以实际时序和父调用作证据。
6. 若遇到缺失类 / 方法，结合原有 panic 的 selector 与栈定位下一阶段所需接口。若截断、崩溃或过滤未覆盖，不把缺失记录当成否定证据。

下一阶段应依据真实 P2G 采集日志决定需要实现的最小接口，并先定义失败 / 取消 / 生命周期语义；本次没有实现玩家身份、GKMatch、发现协议、数据传输或 VPN 支持。

## 验证与行为边界

自动测试入口：

```sh
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --release --lib
python3 dev-scripts/network-trace-smoke.py
python3 dev-scripts/macos-integration-tests.py
```

[network-trace-smoke.py](dev-scripts/network-trace-smoke.py) 在 macOS 编译合成 ARMv7 客体，使用真实现有导出，分别比较追踪开关在联网关闭 / 开启时的返回、errno、属性值、delegate / Block 次数与执行时机。覆盖 nil localPlayer、无完成回调的匹配、Reachability、HTTP 失败、实际 dispatch 执行、重复 Block 关联、序号、上限、显式关闭，以及缺失 selector 仍按原行为失败。测试不使用游戏素材、不访问远端服务，也不为 GameKit 模拟成功。

测试产物与逐次日志位于 `target/network-trace-smoke/`。Rust 单元测试覆盖筛选、错误返回约定和参数解析。此前 socket 回环脚本 `dev-scripts/network-analysis-smoke.py` 可另行验证已有 IPv4 通道；它不代表 GameKit 联机成功。

这些测试验证的是诊断边界与合成客体，不是实际 P2G 在线集会所轨迹或双端联机验证。本次未获得用户实际进入在线集会所后的新日志。日志开启会增加宿主格式化 / I/O 时间，可能影响性能和时序，不能用于无开销帧率比较；关闭时不输出追踪。原有返回值、错误、客体内存写入、联网权限、Block 生命周期和游戏状态机均未由追踪逻辑替换。

2026-10-27 本地检查结果：

| 检查 | 结果 |
| --- | --- |
| `cargo build --release` | 通过 |
| `cargo test --release --lib` | 101 通过，1 忽略，0 失败 |
| `network-trace-smoke.py` | 联网开关两种模式的追踪对照、回调 / errno / 状态、截断及缺失方法检查通过 |
| `macos-integration-tests.py` | 117 / 117 通过 |
| `network-analysis-smoke.py` | 原有联网权限门、UDP / TCP 本机回环通过；沙箱内绑定被拒绝，获准在沙箱外执行后通过 |
| `dev-scripts/lint.sh` | Clippy 与文档检查通过；保留原有 Cargo 版本与 git describe 不匹配的构建提示 |
| `dev-scripts/format.sh --check` | 通过，使用本机 clang-format 22.1.8 |

当前未构建 / 安装 Android APK，也未提交或推送这些改动。完整联机实现不在本阶段范围内。
