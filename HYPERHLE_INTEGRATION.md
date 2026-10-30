# HyperHLE-Reborn → UFtouchHLE：审计与整合记录

比较日期：2026-10-28。本文是**分批已实施成果与完整剩余覆盖队列**，
不是“全面整合已完成”的声明。文件全集已建立清单，但大量函数级行为审查、
复杂功能移植和真机回归仍未完成。

## 固定基准与原项目保护

| 项目 | URL | 分支 | SHA |
| -- | -- | -- | -- |
| UF / 本批目标基线 | https://github.com/shawnw67/UFtouchHLE | trunk | d94c48f37bb5316aa2b87692b3aa6448f1fb20d6 |
| Hyper / 参考快照 | https://github.com/KlugKlugTG/HyperHLE-Reborn | trunk | ff8eb8274130560efbafc56704e64ee7c5279572 |
| 两者共同祖先 | touchHLE 历史中的共同提交 | — | 4785c39e7b217d1e65cbc8da0f13dd048eee034c |
| 本批工作分支 | UF 独立本地工作目录 | codex/hyperhle-integration | 基于上述 UF SHA；改动未提交 |

已获取两仓 trunk 历史；UF 远端与本地基准一致。共同祖先之后，UF 有 466 个提交、
Hyper 有 4248 个提交（含合并提交）。这些数量不是代码质量指标，也不能代表提交
都由本 fork 原创：后续上游合并、revert、重复应用均需查阅历史。

原工作目录 `/Users/wesley/Documents/touchHLE` 的分支和已跟踪文件未改动。
发现的未跟踪 APK、发行包、build-android-uf.command、promo-video/ 均保留，
没有 stash、reset、覆盖或远端推送。此前联机与电话修复已存在于基准提交中，
本批不回退它们。独立工作目录构建时复用本机已有第三方依赖，未改其源码。

## 覆盖清单与来源

运行 `python3 dev-scripts/hyperhle-inventory.py` 可从固定 Git 对象重新生成：

- [全部文件差异表](dev-docs/hyperhle/INVENTORY.md)：759 个不同路径，含配置、平台、资源和子模块。
- [机器可读清单](dev-docs/hyperhle/inventory.json)：共同祖先及双方 blob SHA、来源分类、声明候选、显式依赖。
- [全局独有声明](dev-docs/hyperhle/unique-declarations.json)：跨文件比较，避免把函数搬家误记为新增。
- [Hyper 提交来源](dev-docs/hyperhle/hyper-history.txt) 与 [UF 提交来源](dev-docs/hyperhle/uf-history.txt)：共同祖先后的完整非合并提交 SHA、作者、时间、主题。

分类：双方均修改旧文件 225；仅 Hyper 修改旧文件 78；仅 UF 修改旧文件 6；
双方各自新增同名文件但内容不同 45；Hyper 新增 326；UF 新增 79。
这里的“新增”以共同祖先为准，不等于另一仓不存在该功能。
完全相同的路径不进入逐项审查表；声明列表也不等于已注册、可调用或行为正确的 API。
清单中的 pending 是真实剩余工作，不能在发布说明里当成已支持功能。

## 已作出的源码决策

路径均相对仓库根目录；Hyper 路径须在上述参考 SHA 查看。
“待审查”与“因缺陷暂不采用”是不同状态，不能用后者代替未完成工作。

| 模块 | UF 实现 | Hyper 实现 | 选择方案 | 技术依据 | 状态 |
| -- | -- | -- | -- | -- | -- |
| ARM 可变参数 | `src/abi.rs` 的 VaList::next 读取实时寄存器 | 同路径 cc5ad4af 保存入参 r0–r3 | 融合双方 | %@ 可回调客体 description，改变实时寄存器；只移植保存区，保留 UF 调度与错误处理 | 已接入；ARM 集成测试通过 |
| 命名信号量 | `src/libc/semaphore.rs::sem_open` 的 O_EXCL 判断相反，失败未设置 errno | 099b81cd 修正重开与 EEXIST/ENOENT | 融合双方 | 复用相同 Rc/句柄；仅在 O_CREAT 与 O_EXCL 同时设置时拒绝存在的名称 | 已接入；ARM 重开与共享计数测试通过 |
| Mach-O 段边界 | `src/mach_o.rs::load_from_bytes` 直接切片/后期 assert | validate_segment_file_range 先验证截断与溢出 | 融合双方 | 写入任一段前检查所有声明范围；BSS 仍允许虚拟尾部；fat 切片也使用 checked range | 已接入；截断载入回归测试通过 |
| NSUUID | 未注册 | `foundation/ns_uuid.rs` 提供类、时间种子 LCG、自制字符串解析及编码 | 融合双方 | 复用 UF uuid crate 的随机源及解析，避免 Unicode 字节边界 panic；保留不可变 copy/等值/hash 语义 | 核心值 API 已实现；NSCoding 待档案样本 |
| C UUID | 无 `libc/uuid.rs` 导出 | 同路径 C API；generate_time 实际生成 v4 | 融合双方 | 新增 10 个真实值 API；严格 canonical C 解析，错误不改输出；不冒充时间 UUID | 10 API 已接入；generate_time 待实现 |
| CFUUID | `core_foundation/cf_uuid.rs` 使用 uuid::Uuid | 同路径自制 PRNG；CreateWithBytes 接收结构指针 | 第九批融合 | 保留系统随机源，按真实 C ABI 实现独立参数、结构体和对象生命周期 | 五个真实接口已接入；类型 ID 和自定义 allocator 仍待处理 |
| AIFF / PCM | `audio.rs → symphonia_formats.rs` 未启用 AIFF，筛选只允许 S16LE | Cargo.toml 启用 AIFF，音频模块有更多容器路径 | 融合双方 | 复用 UF PCM 解码/读回通道，明确加入 S16BE；用已知大端采样检查输出字节 | 已接入；真实解码单元测试通过 |
| 音频标签探测 | default-features=false 未注册 ID3/APE readers | Cargo.toml 注册 ape/id3v1/id3v2 | 采用 Hyper 的配置方案 | 复用已有 symphonia-metadata，无新增依赖；超 1 MiB ID3 包裹合成 MP3 后解码须与原数据一致 | 已接入；ID3 回归通过；APE/ID3v1 实际文件待验证 |
| MFi 手柄 | `frameworks/game_controller.rs` + window 输入后端可读取实体手柄 | 同路径 GCController.controllers 固定空数组 | 保留 UF | 参考实现没有 HID 输入后端，会失去 UF 已有 P2G/GTA 原生手柄 | 本批不替换 |
| GameKit 匹配 | `game_kit/local_network.rs`、rooms.rs、transport/、lan/ 使用真实本地会话 | `game_kit/gk_matchmaker.rs` 回调 nil match / not-authenticated 错误 | 保留 UF | Hyper 该路径不能提供本地会话；不能覆盖 UF 已验证真实双实例集会所能力 | 本批不替换；其它 GameKit 类另审 |
| fnmatch | `libc/fnmatch.rs` 保留已有语义与测试 | 新解析器忽略 PATHNAME/PERIOD/NOESCAPE，部分分支不回溯 | 暂不采用当前解析器 | `*[ab]` 对 `ca` 在括号匹配失败处分支直接退出；末尾 * 无视路径/前导点约束 | 需正确解析器及 flags 对照测试，未宣称 UF 完整 |
| 字符串强化检查 | 原无 __strncat_chk；已有其它强化入口保留 | `string.rs::__strncat_chk` 忽略 dest_size | 重写后融合 | 写入前有界检查整个目标容量；虚拟堆映射不能替代 C 缓冲区边界 | 第四批接入；正例、哨兵与三个 ARM 拒绝进程通过 |
| strndup / 有界字符串 | 原缺少这些导出；strtok 耗尽后可能 unwrap | `string.rs::strndup` 先 strlen 再 min(n)，另有 strtok_r 等 | 重写后融合 | 限 n 扫描、溢出及可失败分配；共享分词器保留独立上下文 | 第四批七个导出及原接口修复通过，详见文末 |
| 其它字符串 / 二进制工具 | string.rs 原有 34 个导出 | 参考有 51 个导出，包括 memmem、末尾复制、显式清零等 | 第七/八批逐项融合 | 复用已锁定 memchr 的线性二进制搜索；有界末尾复制；重叠移动；volatile 清零 | 17 个新增导出候选已全部接入；不是完整 locale 支持或原有全部函数已审完 |
| strverscmp / strfry / strerror_r | 原无导出，strerror 未报告未知值错误 | UTF-8 失败当空串、数字 u64 溢出当 0；固定置换；零缓冲仍写 NUL 且总返回成功 | 第八批独立重写并注册 | 原始字节比较通过 GNU 夹具；OS 随机源加拒绝采样；错误文本按历史 Darwin 返回值及容量规则 | 三个接口已接入；历史错误号 0 / errno / 缓冲边界已有回归 |
| malloc 系列 / reallocf | `stdlib.rs` 溢出或分配失败 panic；成功清除 errno | 有乘法检查和 reallocf，但仍调用不可失败分配；malloc 有负数取反及 512 MiB 上限 | 重写后融合 | 复用真实可失败堆，NULL/ENOMEM；失败保留或按 reallocf 释放原块，不改写请求大小；预留并清理每线程 errno | 第五批接入；真实 ARM 地址空间耗尽测试通过 |
| 分配区 / 对齐分配 | `malloc.rs` 各区独立，但 calloc/valloc/memalign 函数表为空，普通 free/realloc 不寻找所属区 | 所有区改用全局堆；memalign 返回内部指针但未配套释放 | 保留 UF 区隔离并重写扩展 | 小块对齐记录真实堆块并重用空隙，大块/大对齐走 VM；普通 C 接口按所属区路由，销毁回收整区 | 第六批五个导出、对齐及生命周期测试通过 |
| CFBinaryHeap | 未实现 | `core_foundation/cf_binary_heap.rs` 忽略 comparator，first() 当最小值 | 暂不采用当前实现 | 插入顺序并不保证优先级顺序，会错误触发定时/任务排序 | 需客体 comparator 与 retain/release/context 生命周期 |
| NSCondition | `foundation/ns_lock.rs` 及 pthread 调度已有 UF 行为 | `foundation/ns_condition.rs` 手动等待队列/线程状态 | 暂不整体采用 | 直接设置 blocked_by 会耦合调度器；需比较 waitUntilDate 互斥重获、唤醒与超时 | 未完成函数级审查 |
| ES2 / ES3 | UF GLES2 guest 与 programmable 后端，已有 GTA 修复 | gles2_native/on_gl3、gles3_native/on_gl3、GLSL 转译、software_vao | 后续融合 | 大型 GLES trait/注册及上下文共享改造；仅复制后端不会接通；需 shader/FBO/上下文回归 | 已定位依赖，未移植/未测 |
| XML DOM | UF NSXMLParser 与 quick-xml 路径 | `frameworks/libxml2.rs → libxml2_wrapper → vendor/libxml2`，客体 DOM 镜像 | 后续融合 | 需句柄、客体结构布局、doc/node 释放及全部平台原生库构建；不能暴露宿主指针 | 已定位依赖，未移植/未测 |
| 摄像头/麦克风 | UF 现有权限与 AudioToolbox/AVFoundation 路径 | android_media.rs、android_jni.rs、HostMedia.java、AVCapture/音频输入 | 后续融合 | 必须成组接入 JNI 缓存、Java 权限、硬件断开及释放；有跨平台 unavailable 路径 | 已定位，未移植/未测 |
| Android 性能提示 | UF 现有帧循环和设备选项 | perf_hints.rs 的 ADPF、优先级、核亲和性 → frame_presented | 后续融合、默认策略需评估 | 必须测帧时间、温度/功耗、暂停重启和旧 Android 符号缺失，不直接全局开最高性能 | 未移植/未作性能结论 |
| CPU 未解码指令 | dynarmic A32 + UF fault 路径 | cpu/asimd_fallback.rs 的 VADDHN/VRADDHN/VSUBHN/VRSUBHN | 后续融合 | 需接 undefined instruction 入口、ARM/Thumb 解码、向量寄存器读写，禁止只加未调用模块 | 未移植；指令向量与真实 fault 测试排队 |
| 加载/异常运行时 | UF mach_o/dyld/objc 已有实现 | Mach-O LC_MAIN、外部/局部符号区分、Swift shim、异常控制流与 ObjC forwarding/weak | 后续逐项融合 | 与调度、ABI 返回寄存器和对象引用计数紧耦合；不能整文件替换 | 段校验、架构筛选和符号可见性已接入（见第二批）；其它项待审 |
| Android 启动器/发布 | UF LauncherActivity、Languages、存档导入导出、更新检查、签名和草稿流程 | 不同 Gradle/包结构、SDL Java 拷贝、ANGLE 二进制、不同 workflow | 保留 UF 工程，择取能力 | 直接替换会删除 UF 启动器及多语言、破坏安装包身份/升级；ANGLE 还需对应许可证及驱动 A/B | 本批保留；新增能力待专项整合 |
| iOS 宿主 | UF 无独立 platform/ios 宿主 | Swift/ObjC 宿主、dynamic-sdl、JIT/SDL 补丁、rust-entry | 纳入后续可行性范围 | 需独立平台构建、JIT 授权、SDL 进程入口共享；不是修改 guest 机型即可实现 | 未移植/未验证 |
| Trainer/归档商店/Picker | UF 原启动和配置界面 | trainer*.rs、app_picker/archive_store*、access_prompt 等 | 纳入后续审查 | 要审客体写入保护、目录穿越、下载/解包、许可和取消清理；避免替换 UF Android 入口 | 未移植/未验证 |

## 第一批代码变化与调用链

- `src/abi.rs`：所有 variadic CallFromGuest 创建快照；VaList::next 只读快照，栈型 va_list 保持栈读取。
- `src/libc/semaphore.rs`：原 libSystem 导出入口直接使用修正后的 sem_open，无新 stub。
- `src/mach_o.rs`：实际 load_from_bytes 在解析架构后、写入段前校验，fat 选择处增加 checked slicing。
- `src/frameworks/foundation/ns_uuid.rs`：通过 `foundation.rs::DYLIB.class_exports` 注册；
  alloc/init、UUID、initWithUUIDString/Bytes、getUUIDBytes、UUIDString、isEqual、hash、copyWithZone、description、dealloc。
- `src/libc/uuid.rs`：通过 `libc.rs::DYLIB.function_exports` 接入 uuid_clear/copy/compare/is_null/generate/
  generate_random/parse/unparse/unparse_lower/unparse_upper。无后台线程或模拟器状态外的全局 UUID 表。
- `Cargo.toml` 与 `audio/symphonia_formats.rs`：AIFF → 已有 AudioFile::read_from_vec → S16BE decoder → 16bit PCM。
- `src/audio.rs`：不含游戏资源的合成 AIFF 和大 ID3 MP3 回归。
- `tests/TestApp_source/cli_tests.m`：新增四项 ARM 客体测试，覆盖 C/ObjC 导出、引用保活、随机版本、坏输入不改输出、信号量共享和格式回调。

## 验证与限制

以下仅是本批代码的检查，不是参考项目全部功能的验收：

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS cargo build --release --offline | 通过 | 本批代码与原 UF 架构构建 |
| cargo test --release --lib --offline | 128 通过，0 失败，1 个原有忽略 | 含保存寄存器/栈混合参数的两项专门 ABI 回归、AIFF、ID3、Mach-O、新旧 UF 测试；网络项在获准本机连接后运行 |
| macos-integration-tests.py | 121/121 通过 | ARM32 客体系统接口；含新增 4 项 |
| gamekit-integration-tests.py | 通过 | ARM 客体双向 TCP/UDP、匹配错误、取消/超时、第三人入房、房主断线、自动发现；不代表真实 MHP2G 本批回归 |
| Android NDK 检查 | 通过 | NDK 28.2、API 23、aarch64-linux-android，使用 Android 对应 features；不是 APK/真机测试 |
| cargo fmt / format.sh --check | 通过 | 本地 clang-format 22.1.8；未运行 CI 的 23 或其它版本 |
| dev-scripts/lint.sh --offline | 通过 | 长注释行检查、Clippy deny warnings、rustdoc deny warnings；未添加 allow 或关闭既有测试 |
| Android ARM64 原生库完整构建 | 通过 | 同上 features，cargo ndk build --lib，产物经 file 确认为 ARM aarch64 ELF shared object；debug、未签名、未打包 APK |
| 真实游戏、实体手柄、Android 安装/运行 | 未验证 | 用户操作；本批未自动操作游戏、未签名安装 |
| Windows / iOS 宿主 | 未验证 | 未构建这些目标 |

首轮单元测试暴露了 S16BE 轨道未选中，已修复而非删测试。受限环境首次回环 socket
失败后，在允许本机 socket 的环境中重新运行全套通过。集成测试最初使用的 ObjC
字符串字面量不受当前测试 linker stubs 支持，改为已有 NSString 构造接口后全套通过。
本批未改既有忽略项。APE/ID3v1 仅启用读取器，尚无该格式实文件验收。

Mach-O 仍有 section/symbol/relocation 等其它未验证范围，不是完整的恶意文件解析器。
信号量第一批当时只修重开/errno；无效句柄错误、sem_post 溢出及等待者销毁
已在第三批继续处理，详见文末。重复 open/close 的行为以目标年代约定为准，
不能预设必须采用现代宿主或通用引用计数模型。

Android 检查复现：`cargo ndk -t arm64-v8a -p 23 check --lib --no-default-features
--features touchHLE_openal_soft_wrapper/static,sdl2/bundled --offline`。
本机 CMAKE_TOOLCHAIN_FILE 使用一个仅用于构建的 wrapper，先设置 ANDROID_ABI=arm64-v8a、
ANDROID_PLATFORM=android-23 再 include NDK 原 toolchain；独立 CARGO_TARGET_DIR 避免
此前 ARMv7 CMake 缓存串用。NDK 错误没有通过改动 CPU/SDL 逻辑或删除检查绕过。
两种宿主构建仍有基线的 Cargo 版本号/标签不一致提示；没有为清除此提示修改版本号。

实际游戏回归尚缺 P2G、GTA，以及新增 API 的真实调用游戏。此前基线已验证的游戏
行为不能自动算成本批验证。本批修改没有增加全局分配器、改变 CPU/JIT 后端、替换
GLES 或调整线程调度，性能变化尚未作基准测量。

## 许可证与代码来源

两仓根 LICENSE 均为 MPL-2.0；所有新增 Rust 源文件保留 MPL 文件头。
借鉴/重写来源标记在文件注释及本文固定 SHA 中，完整作者/历史在 history.txt 中；
本批未移除现有版权声明。参照提交包括 ABI cc5ad4af、信号量 099b81cd 及固定
Hyper 快照中的 NSUUID、uuid、Mach-O 和 Cargo feature 配置。
未带入参考仓签名证书、品牌资源、IPA、ANGLE 二进制或额外第三方源码。
第一批沿用 UF 的 uuid 与 Symphonia；第七批把已锁定的 memchr 2.5.0 直接用于
二进制搜索，第八批复用锁定的 getrandom 0.2.17 提供熵；Cargo.lock 只新增根包依赖边，
没有新增依赖包或升级版本。未来移植 libxml2、
字体、媒体及二进制驱动前仍需逐项检查其实际 LICENSE/NOTICE 和打包流程。

## 可继续执行的剩余任务

不要把以下队列解释为已拒绝移植：除表中明确错误实现外，均仍在目标范围。
文件级完整队列以 inventory.json 为准，不能只做下面举例的模块就结束。

1. P0：逐一审完 225 个双方修改及 45 个双方新增冲突文件。先处理 Mach-O 架构/外部符号、
   ObjC forwarding/weak/异常、内存分配错误和 semaphore 生命周期。每项保留前后行为及失败用例。
2. P1：Foundation 的集合、predicate/regex/sort、JSON、操作队列、URLSession/Protocol；
   CoreFoundation 的 stream/host/service/notification/collection；对照 UF 现有对象与回调上下文，
   做 API/selector 级对照，不能复制不同文件名下的重复类。UUID NSCoding 需真实 archive fixtures。
3. P1：XML DOM/SQLite/ICU/bzip2 通用库。为所有 handle ↔ guest memory、释放重用、
   回调和解析异常建立 tests；核对 native wrapper、子模块 SHA、平台链接和许可证。
4. P1：扩展音频 AudioConverter/AudioFileStream/ExtAudioFile/AUGraph、AVPlayer、CoreMedia/Video；
   保留 UF 的 P2G BGM、CAF 元数据和 GTA 爆音修复，比较真实解码数据、loop/EOS/seek。
5. P2：GLES2/3 与 shader/VAO/FBO/压缩纹理/图形状态、CoreGraphics path/gradient/pattern/font、
   CoreText 排版、CoreAnimation 动画/3D；逐模块增加差分场景，真机检查 UI/触控和帧耗时。
6. P2：UIKit table/collection/gesture/storyboard/layout/navigation/alert/web/text、传感器与摄像头麦克风。
   先实现真实状态/资源/回调，再接 Java 与权限；不要把属性 getter/setter 当完整视图行为。
7. P2：网络 BSD sockets、CFNetwork/DNS-SD/NetService、Reachability runloop callouts，
   与 UF 的局域网传输和 GameKit 同时回归；离线错误不能变成假成功。
8. P2：Accelerate/CommonCrypto/Security、CoreImage/ImageIO、缺少 Framework 与第三方 SDK。
   对每个导出验证参数布局/结果；Keychain/Trust 不得假装持久化或验证成功。
9. P2：Android ADPF/ANGLE/媒体/WebView/JNI、CPU NEON fallback；性能变更需开关和基准，
   JNI 要主栈缓存、引用及异常清理；保留 UF 包名、签名、更新、语言、存档和安全触控。
10. P3：iOS 宿主及 trainer/picker/archive 能力。独立平台/配置融合，不能用参考 UI 替换 UF。
11. 所有 batch 在真实 Android 和由用户操作的 P2G/GTA 等游戏回归后再考虑提交；
    最后全量重新运行 inventory，逐条关闭候选，执行所有平台构建和长时稳定性测试。

当前结论：已经形成可构建的分批整合代码和可复现覆盖队列；
**完整源码审计、所有可行功能移植及最终复查尚未完成，不应发布为全面整合版。**

## 第二批：架构选择与跨镜像符号可见性

仍基于本文顶部两个固定 SHA；沿用第一批全部未提交改动，未提交或推送。
主要源码为 `src/mach_o.rs`、`src/dyld.rs`。

| 模块 | UF 原行为 | Hyper 方案 | 第二批决策与依据 | 状态 |
| -- | -- | -- | -- | -- |
| ARM 子类型 | FAT 遇到第一个 ARM 就可能选中；thin 未验证 subtype，未知值日志可能 panic | arm_subtype_priority 排序/白名单 | 融合：接受 ARM_ALL、v6、v7、v7f、v7s；优先通用 v7；未知/必需 capability 位返回错误 | 已接入 load_from_bytes |
| 损坏 FAT 备用切片 | 先 OFile::parse 全文件，一个坏切片可能导致全局失败 | parse 后循环跳坏切片，仍受底层 eager parsing 限制 | 重写 select_fat_slice：在调用递归解析器前读并验证架构目录，各候选独立预检 header/segment | 已接入；保留虚拟零尾部，不把缺失文件内容伪装为零 |
| FAT64/反字节序目录 | 依赖底层 parser，反字节序路径仍按 BE 读取 | 仍依赖同一库 | 使用 20/32-byte 架构表、u64 checked ranges、目录与内容不得重叠；FAT64 仅表示容器，客体仍必须 ARM32 | 已接入；真实 arm64-only 客体不支持 |
| 定义符号和导出符号 | 同一 exported_symbols 收集 local/external，所有镜像可见 | 分成 exported_symbols / external_symbols，全局查外部表 | 融合：保留全部定义用于自身/诊断，另收集 N_EXT；symbol_for_linking 仅允许所属镜像读自己的 local | 已接入全部现有跨镜像链接入口 |
| 链接依赖范围 | 非延迟、外部重定位、延迟 stub、lazy-symbol dump 各自遍历全表 | 改查 external_symbols | 四条调用路径使用统一可见性规则，继续保留 UF 的现有 host fallback 顺序 | 已接入并增加直接写回验证 |

新增测试覆盖：

- FAT32/FAT64 × BE/LE：首选 ARMv7 内部段截断、坏 Mach-O 头、目录/内部 subtype 不一致，
  及文件范围被截断时回退 ARMv6；真正调用加载器并检查载入内存。
- 子类型顺序颠倒仍优先通用 ARMv7；v7k、ARMv8、未知值与 required capabilities 拒绝。
- 截断架构目录、过大的 count、嵌套 FAT、u64 offset 溢出、切片指回目录均不选用。
- 构造真实 Mach-O nlist 字节验证 local/external 分表和 Thumb bit。
- 真实 do_non_lazy_linking：私有同名定义不能截获非延迟指针或外部重定位；加数保持正确。
- 真实 do_lazy_link：恢复 A32 stub，写回 public 符号目标并保留 Thumb bit。
- 镜像自身的 private 符号仍能解析，外部私有同名符号不能截获。

第二批最终验证结果（原始输出保存于 `dev-docs/hyperhle/validation/batch2-*.log`）：

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS Release 构建 | 通过 | 最终源码，含嵌套 FAT 提前拒绝 |
| Rust 单元测试 | 136 通过、0 失败、1 项原有忽略 | 本批新增 8 项，含真正调用加载器及三种链接入口 |
| ARM 客体系统接口 | 121/121 通过 | 既有完整测试应用，第一批新增 API 继续通过 |
| GameKit ARM 多实例回归 | 通过 | 真实 TCP/UDP、发现、第三人加入、断线、回调；该夹具为 thin ARM，可见性修改后通过 |
| Android ARM64 原生库完整构建 | 通过 | NDK 28.2、API 23、Android features；产物为 ARM aarch64 ELF shared object，debug |
| 严格 lint / rustdoc | 通过 | deny warnings；未降低检查级别 |
| 格式检查 | 通过 | cargo fmt；本机 clang-format 22.1.8，未执行版本 23 |
| 实体 Android / 真实游戏 / Windows | 未验证 | 未签名打包 APK；所有游戏操作仍由用户完成 |

主项目 trunk 的已跟踪文件再次检查无变化；本分支第一、二批代码都保留为未提交修改。

已知边界和后续：

1. 这是架构与段范围预检，不是完整的 Mach-O verifier；section、symtab/strtab、
   relocation、vmaddr+slide 的完整校验仍在 P0 队列。候选通过初筛后，如果后续这些
   元数据有问题，目前不做加载中途的事务回滚或再次回退。
2. 保留原 host 实现优先顺序和全局外部符号顺序；未实现 Mach-O library ordinal、
   two-level namespace、weak/coalesced/reexport、N_PEXT 和 N_ABS 的完整规则。
3. 本批未移植 LC_MAIN 或修改程序入口 ABI，也未修改 ObjC 异常/forwarding；这些需要
   独立夹具及调用链测试，不能算作已支持。
4. FAT alignment/reserved 字段尚未严格验证。当前只接受文件内合法范围、可解析的
   ARM32 thin 候选，拒绝嵌套容器；未宣称可安全解析任意敌意二进制。

## 第三批：信号量错误边界、等待与生命周期

日期：2026-10-29。沿用固定基准和第一、二批改动。

| 路径/入口 | UF 原行为 | Hyper 参考 | 本批决策与接线 |
| -- | -- | -- | -- |
| libc/semaphore.rs::sem_post、sem_wait、sem_trywait | 无效句柄可能经调度器 unwrap 崩溃；trywait 空计数未设置 EAGAIN | 返回 EINVAL/EAGAIN | 融合：入口先验证；失败返回 -1/errno；成功保留 errno；接真实现有计数 |
| Environment::sem_increment | 直接增加有符号计数 | bool 表示句柄是否存在 | 改为 Result，区分 EINVAL 和 EOVERFLOW；越界不改变计数 |
| semaphore.rs::sem_close、sem_destroy | 未检查句柄/对象种类；可能先移除对象再 panic；destroy 错误返回正值 | close 验证对象种类，但等待及 destroy 边界仍不全 | 验证后才移除；误用返回 EINVAL；有等待者时 EBUSY 保留原对象，避免悬空调度键 |
| Environment 调度器 Semaphore 分支 | 移除 current_thread，而非实际被唤醒的 thread_id | 不整体替换调度器 | 精确修正 waiting.remove(&thread_id)，否则线程醒来后还残留等待记录 |
| semaphore.rs::sem_unlink | 无论名称是否存在都成功 | 仍未完整实现 errno | 不存在返回 ENOENT；移除名称不关闭旧句柄；同名新对象与旧对象独立 |
| semaphore.rs::read_name | UTF-8 unwrap 和无界 C 字符串扫描 | 仍依赖 UTF-8 字符串 | 以原始 bytes 为键；有界扫描、空指针/受保护页/空名称/长度错误有明确错误；不要求游戏改名 |
| sem_init / host_create_semaphore / dispatch_semaphore_create | POSIX 和宿主内部信号量共用宽松计数 | 沿用相同方式 | guest POSIX 上限 32767；内部 GCD 维持 i32 最大值，不把 40000 的合法 GCD 初值误拒绝；共享进程参数返回 ENOTSUP |
| sem_getvalue | 无导出 | 实际计数或负等待人数；空输出指针仍成功 | 新增真实状态读回；返回非负当前计数，验证输出地址；通过 libSystem 导出注册 |

以上源码前缀为 `src/`。关联调用链已经检查：libSystem 导出 → semaphore.rs →
Environment 计数/阻塞 → 调度器消费 post → 移除正确的等待者；
NSRunLoop 的宿主 semaphore、NSObject 的跨线程 performSelector 等待、GCD 均继续使用
同一调度机制。内部对象错误或 GCD 计数溢出仍按契约误用显式失败，不吞掉错误。

### 行为依据与明确边界

- [历史 Apple sem_open 文档](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/sem_open.2.html)
  描述同名重复打开返回相同描述符；[sem_close 文档](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/sem_close.2.html)
  描述关闭会使描述符失效。本批保留该目标约定，没有擅加“每次 open 必须匹配 close”的计数规则。
- 本机临时对照程序实际返回不同句柄，两个句柄可分别关闭。这说明现代宿主结果不能直接
  当成早期 iOS 的验收标准；此探针仅是现代 macOS 对照，**不是早期 iPhone 真机验证**。
- 上限值核对本机 SDK `sys/semaphore.h`，名称长度核对 SDK `sys/posix_sem.h`
  及 [Apple XNU 的名称复制/长度检查](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/posix_sem.c)。
  仅据接口/常量独立实现；未复制 APSL 内核实现进入 MPL 源文件。
- 与已有 sem_init 一样，sem_getvalue 是 UF 的 POSIX 兼容扩展：真实读取已存在的
  模拟器计数，不声称所有历史 Apple 系统都原生支持该接口。
- POSIX 中关闭/销毁仍被线程等待的对象属于未定义或错误用法；本批选择 EBUSY，
  保留对象供等待完成后的正确清理。这是明确的 HLE 防护策略，不冒充完整 Darwin 内核。
- 信号量仍是单个模拟器 Environment 内的对象，不实现跨 touchHLE 进程共享、权限/umask、
  命名资源配额、sem_timedwait、信号中断、pthread cancellation 或线程异常退出的完整清理。
  指针检查复用已有虚拟内存范围与空页检查，未补齐所有页权限/分配有效性验证。
- GCD 的 group、超时、signal 返回值及 dispatch_release 生命周期仍有原有简化行为；
  本批只保证计数上限分离及既有等待链路，不标记完整 GCD 已实现。

### 新增回归与验证

新增一项 Rust 名称边界测试和五项 ARM 客体测试：

1. close/reopen 保持计数，unlink 后旧句柄仍有效，同名新对象计数独立。
2. 空句柄/SEM_FAILED、误用 close/destroy、NULL 输出、受保护页存储、pshared、errno 保留。
3. 达到 SEM_VALUE_MAX 后 post 失败且计数不变，再 wait/post 可正常操作。
4. 命名/匿名对象分别真实 pthread 阻塞、EBUSY、post、join、getvalue、最终清理；
   此测试覆盖等待者记录移除错误，而非仅测试容器内容。
5. GCD 初值 40000 仍能 wait/signal，验证未套用 POSIX 的较小上限。

名称夹具明确配置客体空指针保护页，避免 Mem::new 的初始空页设置尚未完成时分配到
地址零；无效地址测试使用真实受保护页，不假设任意高地址都无效。没有删除边界断言。
pthread 测试先用独立信号量握手，再让主线程短暂休眠以允许 worker 进入等待；
不宣称这是穷尽所有线程交错的模型检查。

最终验证记录见 `dev-docs/hyperhle/validation/batch3-*.log`：

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS Release | 通过 | 本批最终实现，含 sem_init 的存储范围检查 |
| Rust 单元测试 | 137 通过、0 失败、1 个原有忽略 | 新增名称字节/长度/保护页测试；保留前两批全部回归 |
| ARM 客体系统接口 | 126/126 通过 | 新增五项信号量/GCD 测试，真实阻塞、唤醒和最终销毁 |
| GameKit ARM 多实例 | 通过 | 真实 TCP/UDP、自动发现、第三人加入、断线和回调；不代表真实游戏已回归 |
| Android ARM64 原生库 | 完整构建通过 | NDK 28.2、API 23、Android features，debug .so；未打包 APK |
| 严格 lint / rustdoc | 通过 | deny warnings，未屏蔽错误 |
| 格式检查 | 通过 | cargo fmt；本机 clang-format 22.1.8，版本 23 未验证 |
| 现代宿主对照探针 | 运行完成 | macOS 27.2；源码 batch3-native-probe.c，结果 batch3-native.log；创建的测试信号量已清理 |

源码行尾的 `sem_init` 检查之后再次运行完整单元、构建和 ARM 测试。GameKit 使用本批
调度/计数/生命周期修复通过；最后新增的 sem_init 客体存储检查不改变其 host_create 路径。
最初的名称单元测试在未配置保护页的空 Mem 中分配到了零地址，修正夹具后所有保护页与
合法名称断言均通过；没有改动运行时来允许 NULL 名称，也没有删除失败测试。

此批结束时，现代探针使用的进程专属命名信号量已移除；没有删除用户数据。
真实 Android 和 P2G/GTA 游戏回归仍未完成，Windows 未构建；未签名安装、未提交、未推送。

## 第四批：有界字符串、强化检查与可失败的底层分配

日期：2026-10-29。保留前面三批修改，不改游戏、渲染、输入或联机逻辑。

| 入口/源码 | UF 原行为 | Hyper 实现 | 选择与实际行为 |
| -- | -- | -- | -- |
| `src/libc/string.rs::strnlen`、strndup | 无导出 | strnlen 有界；strndup 先 strlen 再 min(n) | 共用 bounded_len，最多读 n 字节；strndup 检查长度加一和分配失败，返回 NULL/ENOMEM |
| 同文件 strlcat | 无导出 | 依赖 Hyper 的 GenericChar 扩展 | 独立接入 UF 内存模型；目标只扫描 size 字节，未终止时不写；返回原有效目标长度与完整源长度之和 |
| 同文件 __strncat_chk | 无导出 | 忽略 dest_size，直接普通拼接 | 写前验证目标 NUL 和剩余容量，溢出明确失败；拒绝时目标及相邻哨兵不变 |
| 同文件 strtok / strtok_r | strtok 重复读完可能 unwrap；无 strtok_r | 独立重复的分词循环 | 共用无堆分配的分隔字节表；strtok 状态保持在 Environment，strtok_r 由调用方提供，各流不互相覆盖 |
| 同文件 memrchr / index | 无导出 | 提供反向二进制搜索与别名 | 采用接口能力并按 UF 指针实现，int 转低八位；零长度反向搜索不访问输入 |
| `src/libc/generic_char.rs::strlcpy` | size=0 时计算 size-1，debug 可能溢出 | 另有 GenericChar 重构 | 仅修复零容量减法；仍完整计算源长度，零容量不访问目标，保留已有宽字符调用方 |
| `src/mem.rs::try_alloc`、`src/mem/allocator.rs` | 对 guest 空间耗尽、堆初始化/扩容失败直接 panic | malloc 有固定 512 MiB 上限及负数取反 hack，仍调用不可失败 alloc | 不采用上限/取反；新增可失败路径，先检查 guest 页/块对齐溢出，初始化/增长失败不破坏堆；原 alloc 仍显式失败 |

新增七个导出全部通过原 libSystem → string::FUNCTIONS 接线，测试从 ARM 二进制调用。
未复制 Hyper 的“内存映射足以保护小缓冲区”假设，也没有绕过实际容量检查。
`Mem::try_alloc` 复用原堆和 VM 分配器、原 2 MiB 初始化/增长策略及清零模式；
普通成功分配不引入新的内存布局。ENOMEM=12 加入现有 errno 定义。

依据：[Apple 缓冲区安全指南](https://developer.apple.com/library/archive/documentation/Security/Conceptual/SecureCodingGuide/Articles/BufferOverflows.html)、
[Apple XNU 字符串接口说明](https://github.com/apple-oss-distributions/xnu/blob/main/doc/primitives/string-handling.md)、
[LSB 接口定义](https://refspecs.linuxfoundation.org/LSB_4.1.0/LSB-Core-generic/LSB-Core-generic.pdf)。
仅参考接口语义；本批实现沿用 MPL 源文件，不引入这些项目的实现源码。

### 新增测试与边界

- 六个 Rust 回归：客体地址空间最后四字节的有界读、零上限不访问地址、
  强化失败不改目标/哨兵、对齐溢出不破坏后续分配、堆初始化失败后恢复、
  小块堆真实耗尽后原数据保留并可释放重用。
- 三个 ARM 正例：有界复制/拼接/零容量、交错分词状态、二进制反向搜索/别名。
- `tests/TestApp_source/main.c` 接三个测试专用失败入口；
  `dev-scripts/macos-integration-tests.py` 各启动独立 ARM 进程，检查容量不足、
  目标无 NUL、零容量。必须非零退出且出现准确强化错误，任意崩溃不算通过。

首次耗尽夹具取值恰好在 16 字节对齐后进入大块 VM 路径，测试正确失败；
已改为每块一页，真实填满小块堆并覆盖 grow 失败，保留数据/重用断言。
严格检查要求的 unwrap_or_default 等价写法已修正，没有添加 lint 忽略项。

最终日志见 `dev-docs/hyperhle/validation/batch4-*.log`：

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS Release 构建 | 通过 | 包括最终堆初始化/增长错误传播 |
| Rust 单元测试 | 143 通过、0 失败、1 个原有忽略 | 较第三批新增六个回归 |
| ARM 系统接口 | 129/129 通过 | 较第三批新增三项；另三个独立强化拒绝进程通过 |
| GameKit 多实例 | 通过 | 最终版本真实 TCP/UDP 及发现/回调回归；不是 P2G 游戏实测 |
| Android ARM64 原生库 | 完整构建通过 | NDK 28.2、API 23、Android features；未打 APK |
| 严格 lint / rustdoc、格式 | 通过 | deny warnings；clang-format 22.1.8，未验证 CI 23 |
| Android 真机 / 游戏 / Windows | 未验证 | 无签名安装，无自动操作真实游戏 |

此批结束时剩余 memmem、strcasestr、stpcpy/stpncpy、strverscmp 等候选；
其中七个接口已在第七批接入，见文末的逐项状态。
更正：strspn/strpbrk 在 UF 原基准已存在且有导出，先前待办文字误列，已移除；不算新增成果。
本批不宣称 string.h 已完整移植。不支持非法 C 重叠缓冲区，也未完成所有 guest 页权限保护。
try_alloc 处理客体空间/算术耗尽，不承诺恢复宿主进程实际物理内存不足；
此批当时尚未接入原 malloc/calloc/realloc/valloc 的 errno/失败语义；第五批已继续处理，见下文。
原始主目录的已跟踪源码未改；本工作分支改动仍未提交、未推送。

## 第五批：malloc 系列错误语义与 reallocf

日期：2026-10-29。针对上一批发现的实际调用链继续实施；不移植参考项目的负尺寸
取反和 512 MiB 固定限制，不用成功返回值掩盖客体地址空间耗尽。

| 入口/位置 | 原 UF / Hyper 差异 | 本批实现 |
| -- | -- | -- |
| `src/libc/stdlib.rs::malloc`、allocation_result | UF alloc 失败 panic，成功清除 errno；Hyper 改写接近 u32 最大值的尺寸并限定 512 MiB，但底层仍可能 panic | 原样解释无符号 size，接 Mem::try_alloc；客体空间或尺寸对齐失败返回 NULL/ENOMEM，成功保留已有 errno |
| 同文件 calloc | UF checked_mul(...).unwrap；Hyper 已检查乘法但仍有上限并调用不可失败 calloc | checked_mul → Mem::try_calloc，先成功分配再清零，乘法/对齐/空间不足都用同一失败约定 |
| 同文件 valloc / `src/mem.rs::try_valloc` | 原零大小断言、页对齐加法可能溢出 | 先检查页向上取整；零请求选一个最小页；成功页对齐且整块清零，失败不写入内存 |
| stdlib.rs::realloc / Mem::try_realloc_in_heap | UF 增长失败 panic；Hyper 只对无法返回的 NULL 作补救 | 成功分配后才复制/释放；失败保持原块和数据；普通缩小继续复用；C 入口零大小按历史 Darwin 新建最小块并释放原块 |
| stdlib.rs::reallocf / FUNCTIONS | UF 无导出；Hyper 有包装，但底层失败可能先 panic | 新增真实 libSystem 导出；复用 realloc，失败时释放原块；NULL 原指针不多余释放，成功行为相同 |
| stdlib.rs::free | 原成功路径清除 errno | 正常释放及 free(NULL) 保留 errno；原 UF NSObject 误用保护保留，没有新增“未知指针也成功”的分支 |
| `src/libc/errno.rs::initialize_thread` / destroy_thread；`Environment::new`、new_without_app、new_thread 和线程返回 | errno 地址原为第一次访问时懒分配 | 在客体线程开始前预留，使耗尽时仍能写入错误码；正常线程退出移除映射并释放存储；主线程随 Environment 结束 |

行为核对 [Apple 历史 malloc 手册](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/malloc.3.html)，
包括失败返回、原块保留、reallocf 释放及 realloc 非空指针的零大小特例。
只参考接口约定，按 UF 原有分配器独立实现。新增接口沿用原 MPL 文件头。
原不可失败 Mem::alloc/calloc/realloc/自定义堆入口继续对无法满足的内部请求显式失败；
没有把所有宿主内部对象分配都变成可忽略的 NULL。
无调用者的旧 Mem::valloc panic 包装改为实际被 C 导出使用的 try_valloc，
没有删掉客体 valloc 能力，也没有用 dead_code allow 掩盖重复入口。

### 测试与结果

新增三项 Rust 测试：真实 VM 空间不足时 resize 不破坏原块、页分配对齐/溢出/重用清零、
线程错误码释放及不同线程互不干扰。新增三项常规 ARM 测试：

1. u32 最大请求、calloc 乘法溢出、valloc 对齐溢出、realloc/reallocf 错误及原数据保留。
2. 零大小分配为不同有效对象、realloc(非 NULL, 0) 新对象、calloc 清零、增长复制与 errno 保留。
3. pthread 初始 errno 为零、失败后普通分配成功不清除错误、主/子线程 errno 隔离。

另增加独立 `--allocation-exhaustion` ARM 测试进程：以 32 MiB 块填满真实客体虚拟
地址空间，保留这些分配直至进程退出，避免逐块释放触碰整个地址空间。
先验证 realloc 增长失败仍保留旧块和数据，再验证 reallocf 失败后新的同尺寸分配
重新成功，实际证明原块已经释放。最多 128 次成功分配限制用于检测异常循环；
要求进程正常退出及准确 PASS 标记，不把任意失败当作通过。
输入由 volatile 函数指针送到导出，避免编译器把明显过大的请求常量折叠成结果。

最终日志：`dev-docs/hyperhle/validation/batch5-*.log`；
联机测试 manifest 同目录保存，binary SHA-256 与最终 release 可执行文件一致。

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS Release | 通过 | 最终包含线程 errno 退出清理的版本 |
| Rust 单元测试 | 146 通过、0 失败、1 个原有忽略 | 第四批 143 基础上新增三项，未删除测试 |
| ARM 系统接口 | 132/132 通过 | 加上三个强化拒绝进程与真实地址空间耗尽进程，全部通过 |
| GameKit ARM 多实例 | 通过 | 最终 release 的真实 TCP/UDP、取消、第三人加入、断线、自动发现及回调 |
| Android ARM64 原生库 | 完整构建通过 | NDK 28.2、API 23；file 确认为 AArch64 ELF；debug .so，无签名 APK |
| 格式 / 严格 lint / rustdoc | 通过 | 不增加忽略项；仍只有原 Cargo.toml 与 tag 版本不同的构建脚本提示 |
| 真实游戏、Android 安装、Windows | 未验证 | 用户操作游戏；未把合成客体测试标为游戏验收 |

此批当时只接通 libc 这些入口；分配区及较大对齐已在第六批继续处理。
自定义 allocator、NSZone 相关扩展及完整非法 free 检测仍待逐项审查。
线程创建的栈/运行时对象分配仍是原有不可失败路径，不声称 pthread_create 已处理 OOM；
线程异常退出/取消等完整清理也不在本批正常返回生命周期的验收范围。
realloc 缩小保留原分配容量，未增加原地扩容策略或性能结论。
不能恢复宿主实际物理内存 OOM，也未补齐整个虚拟内存权限系统。

### 下一轮可直接执行的顺序

1. 沿 `src/libc/malloc.rs` 的 zone 函数指针表及默认/自定义 heap 审查空入口、乘法与
   失败返回；用真正 ARM zone 调用覆盖，保留 UF 的合法区分配和销毁。
2. 审查 `posix_memalign` 的对齐/释放匹配，以及 NSZone* 是否确为历史目标的缺少接口；
   不照搬参考项目依赖隐藏头部但未完整接入 free/realloc 的方案。
3. 完成字符串其余候选、CFUUID 的值/结构体 ABI/常量生命周期，避免采用参考实现的
   指针代替 16 个 byte 参数、CGRect 冒充返回类型及 CreateString 自动释放。
4. 再推进原队列中的 ObjC forwarding/weak/异常、Foundation 集合与复杂跨模块能力。

本轮合计新增八个 C 导出（第四批七个字符串入口、第五批 reallocf），并修复既有接口。
此前三个批次成果均保留。**完整整合仍未完成**；759 路径清单中的其它待审项没有关闭。
源代码和报告保留在 codex/hyperhle-integration，未提交、未推送、未触发云端构建。

## 第六批：独立分配区、对齐分配及资源回收

日期：2026-10-29。参考仍为固定 Hyper ff8eb827，不整体替换 malloc.rs。

| 路径/关键函数 | UF / Hyper 对照 | 选择及实际接线 |
| -- | -- | -- |
| `src/libc/malloc.rs::malloc_zone_t::new` | UF calloc/valloc/memalign 为空；Hyper 仅补 memalign，version 仍为 0 | 保留 UF 独立 heap；函数表接入三个真实导出，version 5 表示已提供 memalign；batch 回调仍允许 NULL |
| 同文件 malloc_zone_calloc / malloc_zone_valloc | 双方缺少完整函数表接线 | 使用各区自己的可失败分配器，检查乘法/页取整；calloc 清零，valloc 页对齐并清零 |
| malloc_zone_memalign / stdlib.rs::posix_memalign | UF 大于默认对齐可能 assert；Hyper 用隐藏原指针头部，但 free/realloc 不配套 | 重写：以真正的对齐块登记，不返回未登记的内部地址；无额外头部，原 free/size/realloc 可识别 |
| malloc_zone_from_ptr / malloc_zone_size | UF 无所属区查询，错区 size 可能 panic；Hyper 始终给 default zone，可能解引用未知指针 | 按真实分配起点查找所属区，不读取待查询地址；错区 size 为 0；NULL/栈地址/已释放地址不归属任何区 |
| malloc.rs::free_allocation / reallocate；stdlib.rs::free / malloc_size / realloc / reallocf | UF 普通 C 入口只访问默认堆；Hyper 以合并所有区绕过 | 保留隔离，普通 C 操作找到真实所属区；失败保留原块，reallocf 在同一所属区释放；零大小沿用第五批历史 Darwin 约定 |
| malloc_create_zone / malloc_destroy_zone | UF start_size 初始化可能直接失败；Hyper 不再提供整区回收 | 检查对齐溢出、可失败创建；失败回收已申请的区结构；销毁释放该 heap 所有 backing/VM 分配及名称，不影响其它区 |
| malloc_set_zone_name / malloc_get_zone_name | UF / Hyper 重复设置丢失旧存储 | 名称复制，替换/清空/销毁释放所拥有的副本；允许当前名称作为新输入；不误释放客体直接写入字段的外来指针 |
| `src/mem/allocator.rs::SizeBucketedChunkMap::allocate_aligned` / HeapAllocator::memalign | 双方无这条完整的 UF 独立堆路径 | 小块拆出对齐段，前后空隙继续可用；较大请求/大对齐通过 VM 选择合适地址，取整用宽整数和检查运算 |
| HeapAllocator::free | UF 仅合并一侧相邻空闲块 | 同时合并两侧，避免明明全部释放却无法恢复连续块；不改变正常小块的最低对齐 |
| Mem::free_in_heap | 原按大小猜测是否为 VM 块，小 valloc 在 zero-on-alloc 模式可能不清零释放页 | 按真实来源处理；VM 释放始终清零，小堆块保留 UF 原清零模式，含既有特定游戏配置 |

新增五个导出：malloc_zone_calloc、malloc_zone_valloc、malloc_zone_memalign、
malloc_zone_from_ptr、malloc_get_zone_name；通过原 libSystem → malloc::FUNCTIONS 注册。
对齐入口返回的地址也是 allocator 元数据中的起点，普通释放和扩容无需猜测头部。
小块对齐搜索按可用块检查，最坏线性扫描空闲块；未宣称该策略比所有原生分配器更快。
大对齐 VM 选择不额外占用整段对齐间隙，允许其它合法分配重用间隙。

接口布局及功能核对本机 SDK 与
[Apple libmalloc 公开接口](https://github.com/apple-oss-distributions/libmalloc/blob/main/include/malloc/malloc.h)。
仅用于参数、版本字段和契约确认；未复制其 APSL 实现进 MPL 文件。
参考项目“两区共用全局堆也等同于独立区”的假设不成立：整区销毁及所属区查询已有
可观测行为，本批保留且测试了这些能力。

### 新增验证

- 七个 Rust 测试：VM 对齐间隙重用与回收、高位大对齐与溢出、堆两侧合并、
  小块对齐不浪费整页、500 次交错分配/释放不重叠且最终恢复完整空闲块、
  对齐指针扩容/释放、堆与 VM 在 zero-on-alloc 模式下各自的清零策略。
- 五项 ARM 测试：默认及自建区函数表真实调用；两个区隔离及整区销毁；
  名称复制/别名替换/清空/销毁；从 4 字节到 1 MiB 的 POSIX 对齐、输出/errno 保留；
  创建/乘法/页取整/对齐/扩容失败及零大小生命周期。
- 前批独立耗尽进程扩展：填满客体虚拟空间后，自建区分配失败，新区初始化失败并
  回收区结构，原 realloc/reallocf 验证仍通过。
- 首轮 ARM 测试的可变 `% alignment` 引入测试工具链未提供的 __umodsi3；
  对已知二次幂采用等价位掩码检查，保留全部对齐断言，没有增加假返回的符号。

最终日志：`dev-docs/hyperhle/validation/batch6-*.log`。

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS release 构建 | 通过 | 最终分配区及对齐实现 |
| Rust 单元测试 | 153 通过、0 失败、1 个原有忽略 | 第五批 146 基础上新增七项 |
| ARM 系统接口 | 137/137 通过 | 新增五项；三个强化拒绝进程及扩展耗尽进程通过 |
| GameKit 多实例 | 通过 | 最终 release 的真实 TCP/UDP、第三人加入、取消、断线、发现和回调 |
| Android ARM64 原生库 | 完整构建通过 | NDK 28.2、API 23、Android features；未签名打包 APK |
| 格式 / 严格 lint / rustdoc | 通过 | clang-format 22.1.8；保留原版本提示，未添加忽略项 |
| Android 真机、真实游戏、Windows | 未验证 | 未自动操作 P2G/GTA，未提交或推送 |

### 剩余范围

不声称实现整个 libmalloc：注册客体自定义 zone、introspection、可选 batch 加速、
非零创建 flags、完整 NSZone/CFAllocator、自定义页保护和非法 free 诊断仍待审查。
本批只识别本模拟器管理的默认/自建区，不执行客体自定义分配器回调。
所属区查询对自建区线性查找，未建立全局页索引；未作真实游戏帧率/内存基准。
malloc_default_zone 及首次创建函数表的 dyld stub 仍使用内部不可失败对象分配，
不承诺在运行时自身元数据也无法分配时恢复；实际客体块和新区 heap 耗尽路径已测试。
所有历史未完成项继续保留；本批结束时累计新增 24 个 C 导出，完整整合仍未完成。

## 第七批：二进制搜索、末尾复制与显式清零

日期：2026-10-29。重新按实际 `FUNCTIONS` 导出比较，而非仅凭函数名或文件差异：
UF 固定基准有 34 个 string.rs 导出，Hyper 固定基准有 51 个；多出的 17 个候选中，
第四批接入七个，本批接入七个，余下三个明确保留为待重写。
该数字仅表示本模块候选接线覆盖，不能换算为全项目完成率。

全部实现位于 `src/libc/string.rs`，通过现有 libSystem 分发表注册：

| API / 关键函数 | 参考实现及选择 | UF 实际行为与限制 |
| -- | -- | -- |
| memmem / find_binary | Hyper 双重逐字节循环；择取接口，优化搜索 | 仅访问给定二进制范围；空 needle 返回 haystack；复用 memchr::memmem::find，保留含 NUL 数据和原客体地址偏移 |
| stpcpy / copy_to_end | Hyper 独立逐字节复制 | 复用有界扫描/内存复制，返回写入的 NUL 地址；不添加额外的游戏逻辑 |
| stpncpy / copy_to_end | Hyper 支持截断及补零；融合为共享入口 | 只读至 n 或 NUL，恰写 n 字节；截断不补 terminator，短源补零，返回首个 NUL 或 dest+n；n=0 不访问源/目标 |
| strchrnul | 采用接口能力，正确使用 C int 输入 | 按低八位比较，命中返回该地址，否则返回字符串末尾 NUL；非 UTF-8 字节可正常使用 |
| strcasestr | Hyper 使用 ASCII 窗口比较；与 UF strcasecmp 一致地接入 | ASCII/C locale 范围及原字节偏移有测试；不实现任意 locale/Unicode 大小写折叠，不能标记完整 locale 兼容 |
| bcopy | 采用重叠移动语义 | 源在前、目标在后，调用原 Mem::memmove；两个方向的重叠均测试，零长度不访问地址 |
| explicit_bzero | Hyper 普通字节写不能表达显式擦除的编译器要求 | 对已检查的客体字节范围逐个 volatile 写零；避免把显式擦除折叠成无操作，零长度不访问地址 |

memmem 不再用 O(haystack_len × needle_len) 的客体读循环。
`memchr` 已存在于原 Cargo.lock，版本、校验和均保持 2.5.0，只增加直接使用的依赖边；
本机该版本的 memmem::find 文档承诺最坏线性时间与常量额外空间。
没有自行复制另一份复杂搜索器，也没有拉取新库或更新不相关依赖。
`build.rs` 自动生成的 rust_dependencies.txt 已包含 memchr 作者及 MIT OR Unlicense 声明。
原 MPL 文件头保留，未复制 GNU 库实现源码。

末尾复制的返回值与截断规则对照
[GNU 接口文档](https://sourceware.org/glibc/manual/2.29/html_node/Truncating-Strings.html)；
按接口行为独立整合。strcasestr 的 locale 限制对照
[LSB 接口说明](https://refspecs.linuxfoundation.org/LSB_3.1.0/LSB-Core-generic/LSB-Core-generic.pdf)，
因此本批只将已有 ASCII 处理范围标为已验证。

### 尚未采用的三个参考实现

下表是第七批结束时的状态；三个接口已在第八批重写接线，见下文。

| 候选 | 实际缺陷 | 后续可执行工作 |
| -- | -- | -- |
| strverscmp | cstr_at_utf8 失败替换为空串；整段数字解析 u64，溢出退化为 0；不符合含前导零的版本比较 | 按原始字节及数字段状态实现，避免定长整数解析；用 GNU 前导零、超长数字和非 UTF-8 对照夹具验证 |
| strfry | 置换索引仅由固定 i 计算，没有随机状态；乘法在 debug 可能溢出 | 使用模拟器内明确的随机源及无偏索引，验证多重集不变、NUL 不移动及连续调用的随机状态；不能只靠“变了顺序”验收 |
| strerror_r | 不复用 UF 的 errno 文本；buflen=0 仍写 NUL；截断和未知错误均返回成功 | 核对目标历史 Darwin 的 int 返回签名，复用 errno 文本，加入零容量、短缓冲及未知错误测试，再注册 |

这些接口仍属于整合范围，没有因为参考实现存在缺陷就从清单删除。
UF 原有 strspn、strpbrk 以及其余已导出函数继续保留；本批没有重复实现它们。

### 新增测试与结果

- 两项 Rust 边界测试：在客体地址空间最后四字节执行二进制查找和无 NUL 的限长复制；
  检查零容量不访问地址、补零及目标哨兵不变。
- 四项 ARM 测试：二进制 NUL/重叠输入/重复前缀；末尾指针、截断和补零；
  ASCII 大小写及非 UTF-8 原字节；两个方向的重叠复制、显式清零和哨兵。
- 新接口以 volatile 函数指针调用，避免 C 编译器把测试变成自己的 builtin 结果。

最终日志：`dev-docs/hyperhle/validation/batch7-*.log`。

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS release 构建 | 通过 | 本批最终版本 |
| Rust 单元测试 | 155 通过、0 失败、1 个原有忽略 | 第六批基础上新增两项 |
| ARM 系统接口 | 141/141 通过 | 新增四项；三个强化拒绝进程和扩展耗尽进程继续通过 |
| GameKit 多实例 | 通过 | 实际 TCP/UDP/发现/加入/退出/回调，不是真实 P2G 游戏验收 |
| Android ARM64 原生库 | 完整构建通过 | NDK 28.2、API 23、Android features，未打包 APK |
| 格式 / 严格 lint / rustdoc | 通过 | 未删除测试、未添加检查忽略项 |
| 依赖与许可证 | 已核对 | lock 只有根依赖边新增；memchr 版本不变；自动许可证清单包含作者及授权 |
| 真机游戏 / Windows | 未验证 | 未安装签名 APK、未自动操作游戏 |

限制：不会恢复非法 C 缓冲区、任意重叠字符串复制或越过客体地址空间的输入；
显式清零只擦除指定客体范围，不声称能清理游戏/宿主中已经存在的其它副本。
strcasestr 仍是窗口式 ASCII 比较，最坏二次时间，不宣称本批对其作了性能提升。

本轮第六、七批合计新增十二个 C 导出；累计为三十一个，另有 NSUUID 核心类。
下一轮可直接从上表三个字符串缺口及 CFUUID 结构体 ABI 开始，再推进 ObjC runtime
和原有复杂框架队列。完整整合、真实 Android 游戏回归及最终复查均仍未完成。
所有改动仍在 codex/hyperhle-integration，未提交、未推送、未触发 CI。

## 第八批：GNU 版本排序、随机打乱和历史 Darwin 错误文本

日期：2026-10-29。沿用全部前批代码；本批不直接复制参考项目存在缺陷的实现。

| API / 关键源码 | 决策与实际行为 | 证据与边界 |
| -- | -- | -- |
| strverscmp；`src/libc/string/version.rs::compare`、string.rs::strverscmp | 原始字节比较，数字段按长度/字典序处理，不解析定长整数；维护前导零排序规则；非数字仍按无符号字节比较 | 1,467 组固定 GNU 参考输出全部匹配，另测 4096 位数字和非 UTF-8 输入；只验证 C/ASCII 数字范围 |
| strfry；`src/libc/string/shuffle.rs::shuffle` / below / shuffle_with、string.rs::strfry | Fisher-Yates，64 字节缓冲 OS 熵，拒绝采样消除简单取模偏差；不读取或改变 srand/rand 状态；不打乱终止 NUL | 注入随机流验证不同合法置换、拒绝区间、u32 最大上限、空/单字节无需熵、首批/中途错误传播；ARM 连续调用检查 1–255 每种字节各一次及两侧哨兵 |
| strerror_r；`src/libc/errno.rs::error_message` / render_error / FUNCTIONS | 复用 UF 错误文本表；返回历史 Darwin 的 int 状态；已知错误短缓冲 ERANGE，未知值即使短缓冲仍 EINVAL；零容量不访问缓冲 | 已知值、零/一/短容量、未知负值、i32::MIN、errno 不变、线程交替与耗尽场景均有测试；通过 errno 模块注册而非重复放入 string.rs |
| errno.rs::strerror | 纠正未知值不设置 errno 的原行为；缓存命中仍报告 EINVAL；已知值成功保留 errno | 历史 Apple 源码与本机编译探针支持；错误号 0 按该年代视为未知值，与现代宿主不同 |
| `src/libc/unistd.rs::usleep` | 仅移除成功时 set_errno(0)；保留 UF 原休眠/调度和大间隔处理 | 新线程测试初轮因旧 usleep 清零失败；修复后有直接 usleep(0) 回归；未照搬 Hyper 的 >=1 秒拒绝条件，Darwin 文档允许 useconds_t 范围 |

新增三个 libSystem 导出：strverscmp、strfry、strerror_r。
前两者通过 string::FUNCTIONS，第三个通过已有 errno::FUNCTIONS，三个均有 ARM 实际调用。
全局重新检查 libc 导出，Hyper string.rs 的 51 个函数都能找到接线；其相对 UF 基准
新增的 17 个候选已全部注册。这里表示导出与所述测试范围覆盖，**不是全部 libc 或
所有 locale/历史系统行为已经完成审查**。尤其第七批 strcasestr 仍只处理 ASCII。

### 可复现的独立证据

1. GNU 对照源码固定在
   [glibc 66f3e921](https://github.com/bminor/glibc/blob/66f3e9219d8f86b977d9be04ad469b5d72af0da2/string/strverscmp.c)，
   SHA-256 `03dde07efc33255d921adbe4ce27a70aa68d5f0c397cc12398daf8d4ace3e9d6`。
   `dev-scripts/strverscmp-oracle.py` 需要用户另行提供该原始文件，校验后在临时目录
   编译对照函数并生成 `tests/fixtures/strverscmp.tsv`。再次生成与仓库夹具逐字节相同。
   默认 Rust 单元测试读取固定夹具，不下载网络、不依赖 Linux/clang 或 GNU 动态库。
   Rust 实现按规则独立编写；对照 GNU 文件保留原 LGPL 声明，未复制入仓库或链接进产品。
2. 历史 Apple 源码固定在
   [Libc-594.1.4 / 0d7a0002](https://github.com/apple-oss-distributions/Libc/blob/0d7a000256faa8e750c97ebd703e5d01a5c26fb6/string/FreeBSD/strerror.c)，
   SHA-256 `90b2598f38daa3438218857138f8bc4c57fc35af57b4a685ae804b3ca14edb64`。
   `dev-scripts/darwin-strerror-probe.py` 校验原文件并编译本地探针，与现代宿主对照；
   记录在 `batch8-darwin-probe.json`，包含返回值、errno 及缓冲/哨兵十六进制数据。
   这是历史函数在现代 macOS 上的验证，正错误文本依赖宿主 sys_errlist，
   **不是旧 iPhone 实机或整套旧 libc 验证**；历史源码与归档手册另作接口依据。
3. [Apple 归档手册](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/strerror.3.html)
   与该历史源均将 0 判为未知错误。本机现代实现却将 0 作为正常文本；因此本批不以
   当前 macOS 的 0 结果作为目标年代标准。原 UF 的 "Undefined error: 0" 因此改为
   "Unknown error: 0"，strerror 设置 EINVAL，strerror_r 返回 EINVAL 且保留 errno。

getrandom 0.2.17 已存在于原 Cargo.lock，本批仅增加直接使用的依赖边，未升级或新增包。
其 Windows/macOS/Android 后端沿用依赖本身的平台实现；自动许可证清单已包含作者与
Apache-2.0 OR MIT 声明。两个新增 Rust 子模块及分析脚本均保留 MPL 文件头。
历史 Apple/GNU 外部源仅供对照，保留各自原授权；没有带入模拟器发行包。

### 测试与验证

- 八项新增 Rust 测试：版本顺序、超长数字/非 UTF-8、GNU 夹具、拒绝采样、
  注入置换、熵失败传播、错误缓冲边界、已知表与未知文本。
- 四项新增 ARM 测试：版本比较；真实 OS 熵下的置换、哨兵、rand 独立性；
  strerror/strerror_r 返回值/容量/errno/缓存；信号量握手下的双线程隔离。
- 独立耗尽进程增加 strerror_r 检查，确认正常文本不依赖新的客体堆分配。
- 初轮跨线程测试还依赖 usleep 保留 errno，暴露旧入口清零；现在用信号量直接验证
  两线程交替及错误码隔离，并单独覆盖已修复的 usleep 行为，没有删除失败断言。
- 没有用“随机后一定不同”作为随机测试通过条件，避免合法相同排列带来的偶发失败。

最终日志：`dev-docs/hyperhle/validation/batch8-*.log`，另有对照 JSON 和 manifest。

| 检查 | 结果 | 范围 |
| -- | -- | -- |
| macOS release | 通过 | 最终新接口及既有行为修复 |
| Rust 单元测试 | 163 通过、0 失败、1 个原有忽略 | 包含 1,467 组 GNU 对照数据 |
| ARM 系统接口 | 145/145 通过 | 另有三个强化拒绝进程及扩展耗尽进程通过 |
| GameKit 多实例 | 通过 | 最终二进制的真实 TCP/UDP/发现/加入/断线/回调；SHA 与 manifest 一致 |
| Android ARM64 原生库 | 完整构建通过 | NDK 28.2、API 23、Android features；未签名打包 APK |
| 严格 lint / rustdoc / 格式 | 通过 | 未添加忽略项；clang-format 22.1.8，未验证 CI 23 |
| 两个 Python 工具 | 语法及运行通过 | GNU 夹具二次生成相同；Darwin 差异探针已保存 |
| 真机 / 真实游戏 / Windows 运行 | 未验证 | 未自动操作游戏；Windows 只沿用 getrandom 平台支持，未构建或实测 |

熵源失败时 strfry 明确记录错误、返回 NULL/EIO，不伪造随机成功；若失败发生在中途，
缓冲区仍是原字节的一个排列，可能已发生部分置换。这是 HLE 的失败策略，不宣称 GNU
定义了相同错误返回；Rust 注入测试覆盖传播，未在真实系统强制关闭 OS 熵源。
strerror 既有的按错误号缓存仍保留，未实现所有 locale 或原生静态未知错误缓冲布局。
usleep 的信号中断及完整线程取消仍未实现，本批不改变这些原有限制。

第八批结束时累计新增 34 个 C 导出及 NSUUID 核心类。下一步为 CFUUID 的真实结构体
ABI、常量对象生命周期和其它缺口，再推进 ObjC forwarding/weak/异常与原复杂模块队列。
完整审计、所有功能移植和真机最终验收仍未完成；未提交、未推送、未触发 CI。

## 第九批：CFUUID 和 UTF-16 字符串

- 新增 CFUUIDCreateWithBytes、CFUUIDGetConstantUUIDWithBytes、
  CFUUIDCreateFromUUIDBytes、CFUUIDGetUUIDBytes、CFUUIDCreateFromString，
  以及 CFStringCreateWithCharacters；累计新增 40 个 C 导出及 NSUUID 核心类。
- 按真实 17 参数调用和 16 字节结构体传参与返回实现 ARM ABI，保留所有位模式。
  普通 UUID 缓存不持有对象，释放时移除；常量持有永久根引用。原生系统常量的
  retainCount 哨兵值没有模拟，不能认为引用计数数值完全一致。
- CFUUIDCreateString 返回拥有的引用；CFEqual 对 UUID 做类型与值比较。
  UTF-16 创建保留嵌入零和未配对代理项，空字符串也返回拥有的引用。
- 字符串解析按历史 CF 源码的 UTF-16 上限与容错行为实现，并对照 25 组本机
  CoreFoundation 样例。样例来自现代 macOS，不能等同于真实旧版 iPhone 验证。
- 对照来源为 Apple CF-550.43 的 CFUUID.c，提交
  `49c349e5852474570d014ba9c7ba2df52f66d6ce`；仅用于行为研究，未复制源文件。
  保留 UF 的系统随机源；参考实现的字节构造签名和字符串所有权不能直接采用。
- CFUUIDGetTypeID 依赖尚缺的通用类型 ID 机制；自定义 allocator 尚不支持。
  参考项目的 CFUUIDRetain/Release 别名缺少公开头文件或实际调用证据，现有
  CFRetain/CFRelease 可用，待出现 SPI 调用证据再审查。
- 新增五项 Rust 测试和六项 ARM 测试，覆盖跨栈参数、结构体、生命周期、字典
  互操作及 UTF-16 边界。真实设备、真实游戏和 Windows 尚未验证。
- 最终验证：macOS release 构建通过；Rust 168 通过、1 个原有忽略；ARM 151/151
  通过，另有强化拒绝与地址空间耗尽测试通过。GameKit 合成双实例网络测试通过；
  Android ARM64 原生库构建通过；严格 lint、文档与格式检查通过。没有生成 APK。

剩余工作包括通用 CFTypeID/allocator、Objective-C forwarding、weak、异常及
其它复杂模块；完整审计和移植仍未完成。

## 第十批：Core Foundation 对象描述

| 模块 | UFtouchHLE | HyperHLE-Reborn | 决策与依据 | 状态 |
| -- | -- | -- | -- | -- |
| CFCopyDescription | 未导出 | 通过 description 并 retain | 接入同样的所有权规则，复用现有 Objective-C 对象实现 | 新增接口；UUID 描述跨自动释放池回归 |
| CF 类型识别 | 尚无通用机制 | 返回实际 Objective-C 类地址，数字和布尔 ID 相同 | 暂不采用：类簇、子类及 CFBoolean/CFNumber 区分不能靠类地址正确表达 | 待设计通用类型映射及跨类型测试 |

描述内容仍取决于各个类型已有的 `description` 实现；未实现原生 CF 自定义类型的
注册回调或 allocator。参考源码保留 MPL-2.0，现有文件的版权声明保留。

验证：macOS release 构建通过，Rust 单元测试 168 通过、1 个原有忽略，ARM
测试 152/152 通过，强化拒绝与地址空间耗尽测试通过；Android ARM64 原生库
构建及格式、lint、文档检查通过。初次 ARM 链接因测试使用 CFSTR 缺少常量符号，
改用现有 UTF-8 读取比较后重新执行通过。未进行 Android 真机、真实游戏或 Windows
验证。本批未提交、推送或发布。

## 第十一批：CFDictionary 回调及生命周期

| 模块 | UFtouchHLE | HyperHLE-Reborn | 决策与依据 | 状态 |
| -- | -- | -- | -- | -- |
| 键和值回调 | 只有键回调非空时读取两者，键非空而值为空会断言 | 分别读取键和值回调 | 吸收独立配置行为，保留已有哈希与客体回调架构 | 已接入，测试两种独立配置 |
| 删除全部与销毁 | 清空通过枚举，销毁未释放保存的键和值 | 参考项目有大量不同实现，不能简单覆盖 | 清空时移出条目并逐一调用 release，销毁同样清理，避免泄漏 | 已接入；回调计数测试覆盖单项删除、清空和销毁 |
| 集合扩展接口 | 基础字典接口已有回调对象 | 参考项目增加许多集合方法，但部分实现忽略回调或返回自对象作为副本 | 暂缓直接移植；需要逐项验证空值、复制、回调和对象所有权 | 后续待审 |

尚未补齐自定义 allocator、所有集合 API 及重入场景，不能宣称完整 CFDictionary
兼容。真实游戏和 Android 真机验证仍未完成。

验证：macOS release 和 Android ARM64 原生库构建通过；Rust 测试 168 通过、
1 个原有忽略，ARM 测试 153/153 及强化拒绝、地址空间耗尽测试通过；格式、
严格 lint、文档及 GameKit 多实例回归通过。测试不代表真实游戏或设备验收。

## 第十二批：CFDictionary 查询和空值

参考项目新增了 `CFDictionaryGetValueIfPresent` 和 `CFDictionaryContainsKey`；
本批基于 UF 的哈希和相等回调实现它们，并兼容普通 NSDictionary 桥接。
查询能区分不存在的键与值为 NULL 的键；缺失时不修改输出。`CFDictionaryAddValue`
也按键是否存在决定是否插入，避免覆盖已有 NULL 值。ARM 测试覆盖哈希碰撞、
NULL 值、可选输出、删除以及普通 NSDictionary。仍未解决回调重入和其他集合接口。
macOS release、Android ARM64 原生库构建通过；168 项 Rust 测试、154 项 ARM
测试、严格检查及 GameKit 多实例回归通过。真机、真实游戏和 Windows 未验证。

## 第十三批：CFArray 非持有元素操作

参考项目提供数组插入、替换、清空等接口，但不完整处理自定义回调。本批接入
`CFArrayInsertValueAtIndex`、`CFArraySetValueAtIndex` 和 `CFArrayRemoveAllValues`，
为现有非持有数组覆盖对应 Objective-C 方法，避免对原始指针误执行 retain/release。
不声称支持非空自定义数组回调；涉及 `CFRange`、排序和复制的接口留待单独审查。
原始指针插入、替换、末尾追加、清空和释放已有 ARM 回归。macOS release、
Android ARM64 原生库构建通过；Rust 168 项、ARM 155 项、严格检查和 GameKit
多实例回归通过。未在真机或真实游戏验证。

## 第十四批：NSMutableArray 重复元素删除

UF 按升序删除已记录的匹配下标，连续或多个相同元素会因下标移动删错或越界；
参考项目采用重建数组。本批改为倒序发送已有删除方法，保留子类分发及既有
所有权行为，复杂度仍为 O(n²)，没有直接移植参考的重建实现。新增 ARM 测试
覆盖交错和连续重复元素。macOS release、Android ARM64 原生构建、168 项 Rust、
156 项 ARM、严格检查及 GameKit 多实例回归通过；真机和真实游戏未验证。

## 第十五批：NSDictionary 重复键所有权

两个项目的普通字典在替换已有键时都会复制或保留新键，但匹配后未释放该临时
引用。本批保留原键、替换值后释放多余的键引用；ARM 回归重复设置 32 次并检查
引用计数和销毁后的余额。macOS release、Android ARM64 原生构建、168 项 Rust、
157 项 ARM、严格检查与 GameKit 多实例回归通过。未验证真机或真实游戏。

## 第十六批：CFArray 范围读取

接入参考项目已有的 `CFArrayGetValues`，使用 UF 现有的 `CFRange` 客体结构体
传参，校验非负范围并以减法避免范围加法溢出。ARM 测试验证子范围、缓冲区
哨兵以及末尾零长度范围。macOS release、Android ARM64 原生构建、168 项 Rust、
157 项 ARM、严格检查和 GameKit 多实例回归通过；未进行真机或真实游戏验证。

## 第十七批：CFDictionary 条件替换

接入参考项目的 `CFDictionaryReplaceValue`，通过已有查询判断键是否存在；已有值
为 NULL 时仍可替换，不存在的键不插入。扩展 ARM 碰撞与空值测试。macOS release、
Android ARM64 原生构建、168 项 Rust、157 项 ARM、严格检查和 GameKit 多实例
回归通过；未在真实游戏、Android 真机或 Windows 上验证。

## 第十八批：CFDictionary 值查询

接入参考项目已有的 `CFDictionaryContainsValue`。CF 字典使用其值相等回调，
没有回调时使用指针相等；普通 NSDictionary 使用对象相等判断。初次桥接测试发现
不可变字典不支持 `allValues`，已改用 `allKeys` 与 `objectForKey:` 并重新验证。
ARM 测试覆盖自定义相等回调、不同地址的相等值、NULL、普通字典桥接。
macOS release、Android ARM64 原生构建、168 项 Rust、158 项 ARM、严格检查与
GameKit 多实例回归通过；回调重入、真机、真实游戏与 Windows 尚未验证。

## 第十九批：CFArray 范围搜索

接入 ContainsValue、GetFirstIndexOfValue、GetLastIndexOfValue 和 GetCountOfValue。
参考代码无条件向元素发送 Objective-C 方法，无法安全处理 NULL 回调下的原始
指针；本批对现有非持有数组采用指针相等，对普通 NSArray 使用对象相等。
测试覆盖重复指针、NULL、范围及不同对象但内容相等的字符串。非 NULL 自定义
数组回调仍未实现，不宣称支持；搜索目前遍历完整范围，有进一步优化空间。
macOS release、Android ARM64 原生构建、168 项 Rust、158 项 ARM、严格检查及
GameKit 多实例回归通过。真机、真实游戏与 Windows 尚未验证。

## 第二十批：CFArray 回调和默认对象回调

参考实现接收非空数组回调但没有按回调管理每个元素，本批在 UF 新增专用数组
对象：创建时复制 version 0 回调，插入和替换执行 retain，删除、清空和销毁执行
release，搜索执行 equal；NULL 回调仍使用原有非持有数组。新增
`kCFTypeArrayCallBacks`，复用 CFType 默认 retain/release/equal，并补上默认
copyDescription 回调。`CFEqual` 增加 NSNumber 的相等比较。

ARM 测试覆盖自定义回调计数、不同地址的等价值、替换、清空和销毁，以及默认
回调下 UUID 引用计数和不同数字实例。macOS release、Android ARM64 原生构建、
168 项 Rust、159 项 ARM、严格检查与 GameKit 多实例回归通过。自定义 allocator、
回调重入、数组复制、其他继承的 Objective-C 批量方法及完整 CFEqual 类型仍未
验证或实现；真实游戏、Android 真机和 Windows 尚未验证。

## 第二十一批：CFArray 复制

接入 `CFArrayCreateCopy` 和 `CFArrayCreateMutableCopy`，非持有数组复制不对
原始指针执行 Objective-C 引用管理；带回调数组复制保留原回调并逐项执行 retain，
各副本释放时执行相应 release。不可变副本使用独立的不可变对象。非零 capacity
和自定义 allocator 仍未实现，回调重入和其他批量 Objective-C 方法尚未验证。
macOS release、Android ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查及
GameKit 多实例回归通过；真实游戏、Android 真机和 Windows 未验证。

## 第二十二批：CFArray 不可变创建

接入 `CFArrayCreate`，直接构建不可变数组，避免通过可变对象复制导致额外的
retain/release 回调。覆盖 NULL 回调、空数组、自定义相等回调和创建/销毁次数。
macOS release、Android ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查及
GameKit 多实例回归通过；非零 capacity、自定义 allocator、回调重入、真实游戏与
真机仍未验证或实现。

## 第二十三批：CFArray 范围遍历

接入 `CFArrayApplyFunction`，按客体 `CFRange` 校验边界并依次调用客体函数，
传递元素和上下文。ARM 回归覆盖范围顺序、上下文和零长度范围；回调中修改正在
遍历的数组未验证。macOS release、Android ARM64 原生构建、168 项 Rust、159 项
ARM、严格检查及 GameKit 多实例回归通过；真机、真实游戏和 Windows 尚未验证。

## 第二十四批：CFArray 数组追加

接入 `CFArrayAppendArray`，先保存源范围以支持自身追加，再逐项调用目标数组的
添加方法，保留其元素回调。ARM 测试覆盖非持有指针、自身追加和自定义回调的
持有/释放计数；回调中修改源数组的重入行为未验证。macOS release、Android
ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查和 GameKit 多实例回归通过；
真机、真实游戏与 Windows 尚未验证。

## 第二十五批：CFArray 元素交换

接入 `CFArrayExchangeValuesAtIndices`，通过数组实现直接交换元素位置，不增加
持有或释放回调。ARM 测试覆盖不同下标、相同下标和自定义回调计数；客体自定义
NSArray 子类尚未验证。macOS release、Android ARM64 原生构建、168 项 Rust、
159 项 ARM、严格检查和 GameKit 多实例回归通过；真机、真实游戏、Windows 未验证。

## 第二十六批：CFArray 子范围排序

接入 `CFArraySortValues`，调用客体比较器并仅交换位置，避免参考实现对原始
指针执行 Objective-C retain/release。ARM 测试覆盖自定义回调数组、比较器上下文、
部分范围、重复值和交换过程中的持有计数。当前使用插入排序，最坏 O(n²)，大数组
性能有待优化；比较器重入修改数组及客体自定义 NSArray 子类尚未验证。macOS
release、Android ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查和 GameKit
多实例回归通过；真机、真实游戏和 Windows 尚未验证。

## 第二十七批：CFArray 排序性能

将第二十六批的插入排序替换为稳定排序与索引置换，使用 O(n) 额外空间，并通过
交换应用结果，不对元素执行 retain/release。ARM 回归覆盖重复值的顺序、部分范围、
64 个乱序元素和比较次数上限。比较器修改数组的重入行为仍未验证；macOS release、
Android ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查与 GameKit 多实例
回归通过。真机、真实游戏和 Windows 尚未验证。

## 第二十八批：CFArray 二分查找

接入 `CFArrayBSearchValues`，使用半开区间避免下标下溢，支持空范围和非零
起点；依据本机 CoreFoundation 头文件的匹配与插入位置规则。macOS 原生探针确认
有序数组中目标 25 的插入位置为 2；不能据此推断旧 iOS 所有细节。ARM 回归覆盖
命中、两端未命中、空范围、比较器上下文和跨寄存器/栈传参。macOS release、
Android ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查与 GameKit 多实例
回归通过；真机、真实游戏和 Windows 尚未验证。

## 第二十九批：CFArray 范围替换

接入 `CFArrayReplaceValues`。CF 回调数组先按回调持有全部新元素，再替换范围
并释放旧元素；非持有数组保留 NULL 和原始指针。普通 NSMutableArray 桥接先临时
持有新对象，避免替换旧范围时提前销毁。ARM 测试覆盖新旧元素重叠、回调计数、
NULL 及只有数组持有的 Objective-C 对象。回调重入和客体自定义数组子类尚未验证。
macOS release、Android ARM64 原生构建、168 项 Rust、159 项 ARM、严格检查及
GameKit 多实例回归通过；真机、真实游戏和 Windows 尚未验证。

## 第三十批：CFData 复制与空范围

接入 `CFDataCreateCopy`，使用 NSData 的复制语义；从可变数据复制时保留独立
内容。修复 NSData/CFData 在空范围或范围恰好位于末尾时错误断言的问题，范围
检查使用减法避免加法溢出。ARM 测试覆盖可变原始数据修改、空数据、末尾空范围
及缓冲区哨兵。macOS release、Android ARM64 原生构建、168 项 Rust、160 项 ARM、
严格检查和 GameKit 多实例回归通过。CFData 自定义 allocator、无复制释放策略、
真机、真实游戏及 Windows 尚未验证或实现。

## 第三十一批：CFData 可变创建和追加

接入 `CFDataCreateMutable`（目前只支持 capacity 0）和 `CFDataAppendBytes`。
修复 NSMutableData 从自身缓冲区追加时，扩容可能使源指针失效的问题；仅在源和
原数据范围重叠时保存快照。长度相加检查溢出，零长度追加无需访问 NULL 指针。
ARM 测试覆盖扩容、部分重叠追加和零长度；未强制复用被释放的旧地址。macOS
release、Android ARM64 原生构建、168 项 Rust、160 项 ARM、严格检查及 GameKit
多实例回归通过。自定义 allocator、非零 capacity、真机与真实游戏尚未验证。

## 第三十二批：CFData 无复制释放策略

保留 UF 的无复制数据行为，修复默认释放器为 NULL 时错误解引用的问题：默认及
系统释放器在数据销毁时释放客体内存，`kCFAllocatorNull` 不释放。参考实现忽略
释放器并复制数据，无法保留指针与所有权语义，未直接采用。ARM 测试覆盖指针
一致性、默认释放后分配区查询和不释放选项。macOS release、Android ARM64 原生
构建、168 项 Rust、161 项 ARM、严格检查及 GameKit 多实例回归通过；自定义
allocator、真机、真实游戏和 Windows 尚未验证。

## 第三十三批：CFData 可变复制和长度

接入 `CFDataCreateMutableCopy`、`CFDataGetMutableBytePtr` 和 `CFDataSetLength`，
允许查询空数据的可写地址。测试时发现原 NSData `mutableCopy` 把可变指针类型
传给要求只读指针的 Objective-C 方法，导致类型检查崩溃；现已修复。ARM 回归
覆盖复制隔离、增长清零、缩短后增长、空长度和该崩溃路径。macOS release、
Android ARM64 原生构建、168 项 Rust、162 项 ARM、严格检查和 GameKit 多实例
回归通过；非零 capacity、自定义 allocator、真机及真实游戏仍未验证或实现。

## 第三十四批：CFData 增长填零

接入 `CFDataIncreaseLength`，修复 NSMutableData 增长时未填零的问题。Apple 的
[接口说明](https://developer.apple.com/documentation/corefoundation/cfdataincreaselength%28_%3A_%3A%29)
要求新增长的字节为零；ARM 测试覆盖缩短后再增长。macOS release、Android ARM64
原生构建、168 项 Rust、162 项 ARM、严格检查及 GameKit 多实例回归通过。真实
游戏、真机及 Windows 未验证。

## 第三十五批：CFData 范围替换与删除

接入 `CFDataReplaceBytes` 和 `CFDataDeleteBytes`。根据长度变化安排尾部移动与
扩缩容，对和原数据重叠的新字节先保存快照；范围和长度计算均检查溢出。ARM
测试覆盖重叠扩容、重叠缩短、插入、删除和清空。macOS release、Android ARM64
原生构建、168 项 Rust、163 项 ARM、严格检查和 GameKit 多实例回归通过。真机、
真实游戏、客体自定义 NSMutableData 子类及 Windows 尚未验证。

## 第三十六批：CFData 相等性与哈希

实现 NSData 按字节比较与一致哈希，并使 CFEqual 识别数据对象；空数据与不同
类型不会误判或崩溃。修正空范围读取提前跳过范围检查的问题。ARM 回归覆盖空值、
不同内容、跨类型比较、相同数据哈希和以数据对象查询字典键。哈希值是内部实现，
不宣称与 iOS 原生数值一致。macOS release、Android ARM64 原生构建、168 项 Rust、
164 项 ARM、严格检查及 GameKit 多实例回归通过；真机、真实游戏及 Windows 未验证。

## 第三十七批：CFDictionary 创建与复制

接入 `CFDictionaryCreate`、`CFDictionaryCreateCopy` 和
`CFDictionaryCreateMutableCopy`。CF 字典使用单独的不可变对象，复制时保留键、
值的独立回调及原始指针语义；修正内部 allKeys 返回对象的自动释放所有权。
参考项目直接返回可变创建结果或通过普通 NSDictionary 复制，会丢失部分回调语义，
未直接采用。ARM 回归覆盖六参数客体 ABI、空值、不可变和可变副本独立性及回调
持有/释放次数。macOS release、Android ARM64 原生构建、168 项 Rust、165 项 ARM、
严格检查和 GameKit 多实例回归通过；非零 capacity、回调重入、真机及真实游戏
尚未验证或实现。

## 第三十八批：CFDictionary 等价键替换对照

本机 SDK 头文件描述已有键只替换值；但 macOS 27.2 ARM64 原生探针中，两个不同
地址且相等的键先后设置后，原生系统保留后一个键，并再次执行键 retain/release。
这与现有 UF 行为和原有回归一致。旧版 iOS 的实际行为尚无证据，暂不改变实现；
保留原测试并增加不同地址的等价键及回调次数测试。可使用
`dev-scripts/cfdictionary-native-probe.py` 重现现代 macOS 观察，不能将其等同于
旧版 iOS 验证。macOS release、Android ARM64 原生构建、168 项 Rust、166 项 ARM、
严格检查和 GameKit 多实例回归通过；真机、真实游戏和 Windows 尚未验证。

## 第三十九批：CFDictionary 遍历回调

接入 `CFDictionaryApplyFunction`，CF 字典保留原始键和值指针及 NULL 值，普通
NSDictionary 通过其公开方法取得条目后调用客体函数。顺序不作保证；回调中修改
字典的重入行为未验证。ARM 回归覆盖空字典、NULL 值、自定义指针、上下文和
NSDictionary 桥接。macOS release、Android ARM64 原生构建、168 项 Rust、167 项
ARM、严格检查和 GameKit 多实例回归通过；真机与真实游戏尚未验证。

## 第四十批：CFDictionary 匹配计数

新增 `CFDictionaryGetCountOfKey` 和 `CFDictionaryGetCountOfValue`，值计数沿用
字典的自定义 equal 回调和 NULL 指针语义；原 ContainsValue 在找到首个匹配时仍
提前结束。ARM 回归覆盖多个等价值、缺失键和 NULL 值。macOS release、Android
ARM64 原生构建、168 项 Rust、167 项 ARM、严格检查和 GameKit 多实例回归通过；
回调重入、真机和真实游戏尚未验证。

## 第四十一批：CFLocale 复制

接入 `CFLocaleCreateCopy`，保留原地区对象的复制及持有规则，并检查分配器。
参考项目的 `CFLocaleGetIdentifier` 直接转发至现有 NSLocale，但 UF 当前解析会
丢失脚本、变体和关键字，且返回值寿命需要保证，因此暂不直接接入该接口。
ARM 测试验证释放原地区后副本仍可读取国家码。macOS release、Android ARM64
原生构建、168 项 Rust、168 项 ARM、严格检查和 GameKit 多实例回归通过；完整
地区标识、真机与真实游戏尚未验证。

## 第四十二批：CFURL scheme

接入 `CFURLCopyScheme`，通过现有 NSURL 解析并返回持有的字符串。本机 macOS
CoreFoundation 对照确认大写 `HTTP` 保留大小写、相对路径无 scheme、文件地址
为 `file`；ARM 测试覆盖这三种及自动释放池结束后的所有权。其余 URL 接口涉及
文件地址、转义和相对基址语义，尚未直接移植。macOS release、Android ARM64
原生构建、168 项 Rust、169 项 ARM、严格检查和 GameKit 多实例回归通过；真实
游戏、Android 真机和 Windows 尚未验证。

## 第四十三批：旧版 URL 百分号解码

接入 `CFURLCreateStringByReplacingPercentEscapes`，按 SDK 说明使用 UTF-8 并复用
现有编码版本的实现。ARM 测试覆盖 UTF-8、多字节字符、保留 ASCII 转义字符、
NULL 保留列表和无效 UTF-8。现有实现对非 ASCII 保留字符仍有限制，不宣称完整
编码兼容。macOS release、Android ARM64 原生构建、168 项 Rust、170 项 ARM、
严格检查及 GameKit 多实例回归通过；真实游戏、Android 真机与 Windows 未验证。

## 第四十四批：URL 非 ASCII 转义保留

修复百分号解码对保留列表中非 ASCII 字符的处理：先按 UTF-8 字符解析连续转义，
保留指定字符的原始转义及大小写。本机 macOS 对 `%C3%A9%2F` 与保留列表 `é/`
的观察结果相符；ARM 测试覆盖多字节字符、ASCII、混合及无效字节。macOS release、
Android ARM64 原生构建、168 项 Rust、170 项 ARM、严格检查及 GameKit 多实例
回归通过；其它字符编码、真实游戏和真机尚未完整验证。

## 第四十五批：URL 端口

为 NSURL 增加 `port` 并接入 `CFURLGetPortNumber`，识别用户信息、IPv6 方括号、
符号、无效端口与整数溢出；文件 URL 由字符串创建时保留其端口。本机 macOS
对照覆盖端口 0、65536、无效值、IPv6、负值、溢出和文件地址；不能将现代结果
视为旧 iOS 全面验证。macOS release、Android ARM64 原生构建、168 项 Rust、171 项
ARM、严格检查和 GameKit 多实例回归通过。文件 URL 的其他 authority、相对基址
语义仍有缺口，真机与真实游戏未验证。

## 第四十六批：URL 主机名

接入 `CFURLCopyHostName`，并在由字符串创建的文件 URL 中保留主机名。空主机
返回 NULL，IPv6 方括号与用户信息不包含在返回值中，主机名进行百分号解码；
返回值按 Copy 规则持有。本机 macOS 对照与 ARM 回归覆盖本地/远程文件 URL、
转义主机、IPv6 和无主机地址。macOS release、Android ARM64 原生构建、168 项
Rust、172 项 ARM、严格检查和 GameKit 多实例回归通过；相对基址与异常 URL
处理仍有缺口，真实游戏、真机和 Windows 未验证。

## 第四十七批：URL 查询与片段

接入 `CFURLCopyQueryString` 与 `CFURLCopyFragment`，文件 URL 由字符串创建时
保留查询和片段。NULL 保留列表保持原始转义；非空列表通过现有 UTF-8 解码器
保留指定字符。macOS 原生对照及 ARM 测试覆盖 HTTP、文件 URL、多字节转义、
空查询/片段与所有权。macOS release、Android ARM64 原生构建、168 项 Rust、
173 项 ARM、严格检查和 GameKit 多实例回归通过。文件 URL 的相对基址、其他
编码、真实游戏与真机尚未验证或完整实现。

## 第四十八批：URL 原始路径

接入 `CFURLCopyPath`，保留字符串创建的文件 URL 原始路径，并对文件系统路径
生成转义路径；不把 `%2F` 解码成路径分隔符。区分可分解路径与 `about:blank`
等不可分解 URL，保留文件目录末尾斜杠。macOS 对照及 ARM 测试覆盖转义路径、
相对路径、空路径、不可分解 URL 和文件目录。macOS release、Android ARM64 原生
构建、168 项 Rust、174 项 ARM、严格检查和 GameKit 多实例回归通过。客体自定义
NSURL 子类、完整相对基址解析及真实游戏、真机尚未验证。

## 第四十九批：URL 严格路径与可分解判断

新增 `CFURLCopyStrictPath` 和 `CFURLCanBeDecomposed`，使用 URL 的原始
转义路径。对照 macOS 系统行为：无路径时严格路径为空指针且绝对路径标志为
false；根路径 `/` 返回空字符串且标志为 true；仅去掉一个起始斜杠。ARM 回归
覆盖文件 URL、网络 URL、根路径、相对路径、不可分解 URL 和字符串所有权。
macOS release、Android ARM64 原生构建、168 项 Rust、174 项 ARM、格式、静态
检查和 GameKit 多实例测试通过。带基址的 URL、自定义 NSURL 子类以及真实 iOS、
游戏和 Android 真机行为仍未验证。

## 第五十批：URL 目录路径判断

`CFURLHasDirectoryPath` 对内置 URL 改用未解码路径判断，避免把 `%2e`、
转义斜杠等内容误认成目录；保留原有 `//` 特例和自定义 URL 对象的旧处理。
测试覆盖点目录、根目录、文件 URL 的目录标记和现有路径兼容行为。macOS
release、Android ARM64 原生构建、168 项 Rust、175 项 ARM、格式、静态检查
及 GameKit 多实例回归通过；真实游戏与 Android 真机尚未验证。当前
`CFURLCreateWithBytes` 仍存在仅按文件路径解释的既有局限，须另行处理。

## 第五十一批：字节 URL 创建与文件目录标志

参考 Hyper 的 `CFURLCreateWithBytes` 分发方式，改为按 URL 字符串初始化，
修复网络地址中的 `://` 触发断言及错误归类为文件 URL 的问题；无效长度和
空指针返回空结果。保留 ASCII 编码和空基址的原有支持范围。对文件系统路径
创建的 URL，目录判断采用显式目录标志及根目录规则；URL 字符串仍按原始路径
判断。测试覆盖 HTTP URL、转义路径、文件目录标志和既有相对路径用例。
macOS release、Android ARM64 原生构建、168 项 Rust、176 项 ARM、格式、
静态检查及 GameKit 多实例回归通过。其他字符编码、相对基址、文件路径生成的
`CFURLCopyPath` 末尾斜杠行为以及真实游戏和 Android 真机仍待验证或完善。
