# MHP2G 存档转换器（PSP ↔ iOS）

`mhp2g-save` 在 PSP 版和 iOS 版《怪物猎人 携带版 2nd G》（MHP2G，PSP ULJM-05500，iOS bundle id `jp.co.capcom.mhp2gjp`）之间转换存档。它是 Podroid 工作区里的一个 Rust crate（`mhp2g_save`，库 + 命令行），PSP 的系统层（KIRK）加解密已内置，不需要外部工具。以后 Podroid 本体（比如安卓的导入功能）可以直接依赖这个库。

西方版《Monster Hunter Freedom Unite》（MHFU，PSP 美版 ULUS-10391、欧版 ULES-01213，iOS 全球版 bundle id `jp.co.capcom.mhp2gwd`）和 MHP2G 是同一款游戏。它的 PSP 存档也能读取并导入，只是游戏 key 和哈希盐不同（见下面的“MHFU 存档”）。

## 许可证

**这个目录是 GPL-3.0-or-later**（见 `COPYING`），和 Podroid 其余部分（MPL-2.0）不同。原因是 `src/psp.rs` 移植自 GPL-3.0-or-later 的代码：jpcsp、kirk-engine，以及 psp-savedata-endecrypter / Apollo Save Tool。原作者的版权声明保留在该文件开头。MPL-2.0 允许和 GPL 代码组合，以 GPL 分发；Podroid 的发布包本来就附带 GPL-3.0（`dev-scripts/prepare-release.sh`）。

## 在 Podroid（安卓）里直接导入

1. 在启动器里长按 MHP2G（或 MHFU）的图标，打开菜单，选「导入 PSP 存档」。
2. 选择 PSP 存档文件夹 `ULJM05500`（MHFU 的是 `ULUS10391` 或 `ULES01213`，两个游戏都接受这三种）：PPSSPP 放在它的 `memstick/PSP/SAVEDATA/` 里，真机 PSP 在记忆棒的 `PSP/SAVEDATA/` 里。选它的上一级文件夹也可以。如果系统不支持选文件夹，也可以选「选择文件」，直接选中其中的 `MHP2NDG.BIN` 和 `PARAM.SFO`。
3. 存档会被转换成 iOS 版的格式，游戏设置沿用当前存档的。原来的 `savedata.dat` 会先移到 touchHLE 文件夹的 `Podroid_save_backups/` 里。

转换由这个 crate 完成（touchHLE 通过 JNI 调用 `mhp2g_save::import_psp`）。导出为 PSP 存档目前只能用下面的命令行工具。

## 用法（命令行）

```bash
cargo build -p mhp2g_save --release    # 生成 target/release/mhp2g-save
```

```bash
# 查看存档：平台、格式、角色名、校验和、种子
mhp2g-save inspect savedata.dat
mhp2g-save inspect PSP/SAVEDATA/ULJM05500

# PSP → iOS：输入直接用 PSP/PPSSPP 的存档目录（或其中的 MHP2NDG.BIN）
mhp2g-save convert --to ios PSP/SAVEDATA/ULJM05500 savedata.dat

# iOS → PSP：需要一份现有 MHP2G PSP 存档的 PARAM.SFO 作为模板；
# 输出目录里是 MHP2NDG.BIN、更新过的 PARAM.SFO 和模板目录里的图标
mhp2g-save convert --to psp savedata.dat ULJM05500 --psp-sfo 旧存档/ULJM05500/PARAM.SFO

# 研究用：导出明文（0x16A100 字节）/ 修改后重新加密（自动重算 SHA-1 和种子）
mhp2g-save decrypt savedata.dat plain.bin
mhp2g-save encrypt --platform ios plain.bin savedata.dat
```

- 输入格式自动识别：iOS `savedata.dat`、带系统层的 PSP `MHP2NDG.BIN`（同目录有 `PARAM.SFO` 时从中读取加密模式，没有时按 MHP2G 的模式 5）、不带系统层的 PSP 游戏层文件，或明文。SHA-1 不对的文件会被拒绝。
- 默认不覆盖已有文件，要覆盖请加 `--force`。写 PSP 存档前，会先把结果按读档流程解一遍，确认无误后才写出。模板 `PARAM.SFO` 本身不会被修改。
- `--template 某个目标平台的存档`：从这个存档取开头的平台设置块（0x00–0x24）；不加时用内置默认值，取自所有已有样本。
- `--game-layer`：只写不带系统层的 PSP 文件。
- iOS 存档位置：`Podroid_sandbox/jp.co.capcom.mhp2gjp/Documents/savedata.dat`。替换前先备份原文件。
- 测试：
  - `cargo test -p mhp2g_save` 只用合成数据，包括 AES-CBC（SP 800-38A）和 AES-CMAC（RFC 4493）的标准测试向量。
  - `tests/real_saves.rs` 用真实存档核对。设置环境变量后才会运行，用法见该文件开头。

## 存档格式

两个平台的存档主体都是 0x16A100 字节，布局相同：

| 偏移 | 内容 |
|---|---|
| 0x000000–0x000024 | 平台设置块（PSP `00 03 07 07 …`，iOS `00 01 04 04 …`），转换时替换 |
| 0x000024–0x16A0DC | 存档数据，两个平台相同（0x1000 处是 UTF-16LE 角色名，三个角色槽） |
| 0x16A0DC–0x16A0E8 | 12 字节元数据，不参与哈希 |
| 0x16A0E8–0x16A0FC | `SHA-1(数据[0:0x16A0DC] + 盐)` |
| 0x16A0FC–0x16A100 | XOR 流的种子 |

Capcom 游戏层加密：对除种子外的每个 32 位小端字，`查表 → 推进 LCG → 异或 beta → 查表`。LCG 为 `lo = lo·0x215F mod 0xFF8F`、`hi = hi·0xDFA3 mod 0xFFEF`、`beta = lo | hi<<16`，从种子开始（为 0 的半字换成 0x215F / 0xDFA3）；种子本身经加密表置换两次后存放。

| | PSP（ULJM-05500） | iOS |
|---|---|---|
| 字节表 | 两个平台相同：iOS 二进制 VA 0x5F67D0（加密）/ 0x5F68D0（解密）处的表。**不是** mhef / mhtools 里那张 `A6 21 22 …` 表 | 同左 |
| 盐 | `S)R?Bf8xW3#5h9lGU8wR` | `3Nc94Hq1zOLh8d62Sb69`（与 PSP 美/欧版 MHFU 相同） |
| 外层 | PSP 存档加密 sceChnnlsv（KIRK），模式 5，日版游戏 key `cd1f2059ae70ef68dca24513b45adb0a`；16 字节文件头 + 数据，共 0x16A110 字节 | AES-128-CBC + PKCS#7（CCCrypt），key `ls6([f$]=~I(nf28`，iv `;lknc(;f_wnX98<i`，0x16A110 字节 |
| 签名 | `PARAM.SFO`：`SAVEDATA_FILE_LIST` 里的文件哈希，加上 `SAVEDATA_PARAMS` 的三个签名（都是 AES-CMAC） | 无 |

iOS 版的逻辑来自游戏二进制：`0x322704`（读档：解密 + 校验 SHA-1）、`0x322868`（游戏层加解密）、`0x322AF4`（写档：算 SHA-1、换种子、加密）。

PSP 系统层：数据和一个密钥流做 XOR。密钥流是对一串编号块做 CBC 解密得到的，编号块由文件头和游戏 key 推出。文件哈希是“文件头 + 密文”的 AES-CMAC，再经游戏 key 加密一次。

## MHFU 存档

| | MHFU（美版 ULUS-10391、欧版 ULES-01213） |
|---|---|
| 游戏 key | `4a1ff359aeb6eff81ca8cb23bca57bb3`，美欧相同 |
| 盐 | `3Nc94Hq1zOLh8d62Sb69`，也就是 iOS 的盐 |
| 游戏层 | 字节表和 LCG 与 MHP2G 相同 |
| 文件 | `MHP2NDG.BIN`（0x16A110 字节）、`PARAM.SFO`（`SAVEDATA_PARAMS[0]` 为 0x41，模式 5） |

游戏 key 没有现成的来源，是从真实存档的 `PARAM.SFO` 反推的：模式 5 下，`SAVEDATA_FILE_LIST` 里的文件哈希包含游戏 key，所以可以解出来。同样的方法用在日版存档上，得到的正是 `MHP2G_JP_KEY`。用这个 key 解开美版 1 份、欧版 2 份存档后，SHA-1 与 iOS 的盐吻合。

槽内字段的位置与日版一致：角色名、公会卡名（与角色名相同）、金钱、游戏时间、各类任务完成数都落在日版的偏移上，数值合理（欧版新角色游戏 19 分钟、完成 6 个村任务）。

iOS 全球版的 `savedata.dat` 与日版 iOS 版格式相同：AES 的 key 和 iv、游戏层、盐都一样（1 份样本，SHA-1 吻合）。设置块第一个字节是 `01`，日版 iOS 样本是 `00`，含义没有确认。导入时设置块沿用当前存档的，所以会保留。

## 验证情况

- **iOS**：4 份真实存档全部能解密、SHA-1 吻合；用原种子重新加密后与原文件逐字节一致。
- **PSP，真机存档**：k73 的 `MHP2NDG.BIN`（PSP 写出的）能直接解开，SHA-1 吻合。用原文件头重新加密，得到的文件与原文件**逐字节一致**，`SAVEDATA_FILE_LIST` 里的文件哈希也与 PSP 写的相同。`SAVEDATA_PARAMS` 里不依赖主机的两个签名（0x10、0x70）也能复现。
- **PSP，与 Apollo `endecrypter` 对照**：同样的输入和文件头下，`MHP2NDG.BIN` 逐字节一致；`PARAM.SFO` 除了 `SAVEDATA_PARAMS` 的签名以外逐字节一致，那几个签名的差别见“限制”。本工具写出的 PSP 存档 `endecrypter` 也能解开。
- **在 Podroid（macOS release 版）里用 P2G 实际读档**：
  - 原始存档：正常读取（对照组）
  - 同一存档换新种子重新加密：正常读取
  - k73 的 PSP 存档转成 iOS：正常读取，角色选择界面显示 PSP 存档里的角色「君仔」（29:51，男性）和「无双君仔」（54:31）。Rust 版的输出与这次显示出角色的那份明文完全相同，也在游戏里读档通过。
  - 改 1 字节但不重算 SHA-1：两次报「游戏存档已损坏」（反向对照，说明检测有效）
  - 原生 iOS 存档「殲滅のレウス」（3 个角色）转成完整的 PSP 存档（含系统层和 PARAM.SFO），再转回 iOS：和原档逐字节相同。在游戏里读档、检查、存档后数据齐全，游戏只更新了游戏时间。

## 限制

- **每个角色槽后半段的布局不同，转换器会逐段搬移。** PSP 版在这里比 iOS 版多出 0x28F8 字节，分几处插在中间，所以金钱、公会卡、各种标志表和计数器在两个平台上的位置不同（见 PSP_IOS_FORMAT_COMPARISON.md §3.3）。转换器按各段的偏移差搬移，公会卡里的怪物记录按两个平台各自的格式重新打包。k73 存档往返 PSP → iOS → PSP、「殲滅のレウス」往返 iOS → PSP → iOS 都逐字节不变。手机实测：金钱、公会卡名、讨伐数和武器使用次数都显示正确。
- 公会卡名原样保留，只有为空时才填入角色名（iOS 版不会自己填）。卡名可以和角色名不同：k73 第 1 个角色的卡名是「g z j」，iOS 原生存档里也有卡名「ラギア」、角色名「Ł」的例子。公会卡里另有一个猎人列表，由游戏根据任务履历生成；任务履历本身保持原样。
- **收到的公会卡没有验证。** k73 里这块是空的，联机收过公会卡的 PSP 存档转过去可能错位。
- 三个角色槽之后的全局区只有 PSP 用（疑似含下载任务），原样保留。iOS 不读写这一区，所以往返不会丢失。
- **iOS → PSP 没有在 PSP/PPSSPP 上实际读档验证过。**
- **`SAVEDATA_PARAMS` 0x20 处的签名（模式 6）无法和真机一致**：它用到写档主机的熔丝 ID（KIRK 命令 5）。本工具用熔丝 ID 0，和其他 PC 工具一样是编的值。0x70、0x10 两个签名覆盖了它，所以也会跟着不同。Apollo 的源码注释说 PSP 能接受这样的存档；PPSSPP 据我所知不检查这些签名，但都还没有实测。
- **只验证了读档。** 没有在游戏里进入任务后再存档，确认转换来的存档能正常回写。
- 平台设置块（0x00–0x24）的含义没有逐字段确认，转换时整体换成目标平台的值。字段映射和置信度见 PSP_IOS_FORMAT_COMPARISON.md，所需样本列在其 §5。
- **MHFU 的 PSP 存档只验证了读取**（见“MHFU 存档”）：转成 iOS 后还没有在游戏里实测，各区域的道具编号是否相同也没有核对。
- iOS → PSP 只会写日版 MHP2G 的格式（日版的 key 和盐），不能转回 MHFU 的 PSP 存档。
- 日版 MHP2G 只用一份 PSP 存档（k73）验证过。
