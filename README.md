# Podroid

[简体中文](README.md) | [English](README.en.md)

[![GitHub Release](https://img.shields.io/github/v/release/shawnw67/Podroid?style=flat&logo=github&color=1f6feb)](https://github.com/shawnw67/Podroid/releases)
[![GitHub Stars](https://img.shields.io/github/stars/shawnw67/Podroid?style=flat&logo=github&color=gold)](https://github.com/shawnw67/Podroid)
[![GitHub Issues](https://img.shields.io/github/issues/shawnw67/Podroid?style=flat&logo=github&color=lightgrey)](https://github.com/shawnw67/Podroid/issues)
[![License](https://img.shields.io/github/license/shawnw67/Podroid?style=flat&logo=github&color=blue)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Android-blue?style=flat)](https://github.com/shawnw67/Podroid/releases)

**Podroid** 是 [touchHLE](https://github.com/touchHLE/touchHLE) 的非官方分支，面向早期 iOS 游戏，增加了特定游戏兼容修复、Android 启动器、存档导入导出、模拟机型选择及原生手柄支持。

本分支由 [shawnw67](https://github.com/shawnw67) 维护，包含 AI 辅助开发的代码，与上游 touchHLE 独立维护。

## 分支功能

- **游戏适配**：针对 P2G、罪恶都市、Inotia 1 / 2、Resident Evil Mercenaries VS. 和 Dead Rising Mobile 等游戏补充接口、修复兼容问题。
- **Android 启动器**：导入游戏、长按管理游戏、查看和分享日志，设置页面提供模拟机型、游戏联网和更新检查等选项。
- **存档迁移**：通过 ZIP 导入、导出游戏存档，导入前备份已有存档。
- **机型与分辨率选择**：按屏幕比例自动选择，也可设置全局机型或为单个游戏指定机型。
- **手柄支持**：实现游戏使用的 GameController 接口，已对 P2G 和罪恶都市进行实机测试；保留触屏映射和虚拟光标等输入方式。
- **P2G 画面扩展**：可选扩展横向视野，触控布局保持在中间的 16:9 操作区域；默认模式支持摇杆拖入黑边后继续操作。
- **音频与生命周期修复**：包含 P2G 背景音乐循环、罪恶都市部分音效噪声，以及退出和 Android 切后台恢复等路径的修复。

各版本改动见 [Release 说明](https://github.com/shawnw67/Podroid/releases)。[CHANGELOG.md](CHANGELOG.md) 目前主要保留上游历史记录，不是 UF 分支完整的发布日志。

## 兼容范围

Podroid 使用高层模拟：执行游戏的 ARM 代码，并自行实现游戏需要的 iOS 系统接口，不需要启动完整的 iOS 系统。

仅支持 **32 位 ARM（armv6 / armv7）应用**。基础兼容范围来自上游的 iPhone OS 2–4 时代应用，本分支为部分 iOS 5 / 6 时代游戏增加了支持；这不代表这些系统版本的所有应用都能运行。

选择 iPhone 5s、iPhone 6 等模拟机型，改变的是设备信息、屏幕尺寸和渲染配置，**不会启用 arm64 模拟，也不会将模拟器升级为对应版本的 iOS**。宽屏和高清资源能否启用，取决于具体 IPA 版本及游戏实现。

| 重点适配游戏 | UF 分支的适配内容 |
| --- | --- |
| 怪物猎人 Portable 2nd G（P2G） | Retina、角色命名输入、存档相关兼容、原生手柄、可选扩展视野、音乐循环 |
| Grand Theft Auto: Vice City | OpenGL ES 2.0、1.4 版本兼容、分辨率适配、音频、原生手柄及退出处理 |
| Inotia 1 / 2 | 游戏运行所需的兼容修复 |
| Resident Evil Mercenaries VS. | 游戏运行、角色渲染及退出相关修复 |
| Dead Rising Mobile | 游戏运行及渲染相关兼容修复 |

同名游戏的不同版本、语言包或修改包可能有不同表现。上述列表不表示所有流程、联网功能和所有设备均已验证。上游的 [应用兼容数据库](https://appdb.touchhle.org/) 可作参考，但不会完整反映本分支的新增适配。

## 下载与安装

从本仓库 [Releases](https://github.com/shawnw67/Podroid/releases) 下载 Android ARM64 APK。新版发布文件使用 `Podroid_vX.Y.Z_Android_AArch64.apk` 命名；历史版本仍使用 UFtouchHLE 名称。

目前仅发布 Android 安装包。macOS、Windows 和 Linux 用户可自行从源码构建。

项目不提供游戏 IPA。请自行准备合法取得且可用于模拟的已解密应用；发行包不包含游戏文件。

### Android

1. 安装 APK 并打开应用。
2. 点击“导入游戏”，选择 `.ipa` 文件。
3. 在列表中点击游戏启动，长按游戏可打开管理菜单。
4. 通过设置图标调整全局选项，或长按游戏单独选择模拟机型。

Podroid 包名为 `org.podroid.android`，数据目录为：

```text
/storage/emulated/0/Android/data/org.podroid.android/files/
```

常用子目录：

```text
touchHLE_apps/          游戏文件
touchHLE_sandbox/       游戏沙盒与存档
touchHLE_save_exports/  存档导出目录（选择应用内目录导出时）
touchHLE_save_backups/  导入前备份的旧存档
```

从旧版 UFtouchHLE 迁移时，请先保留旧应用：在旧版长按每款游戏并将存档导出到共享位置，然后在 Podroid 中重新导入游戏和对应存档。设置需要重新配置。确认迁移完成后再考虑卸载旧版；新旧包名不同，存档不会自动迁移。较新的 Android 系统可能限制文件管理器访问 `Android/data`，优先使用应用内导入、导出功能。

“允许游戏联网”控制被模拟游戏的网络访问；“自动检查更新”是独立设置。检测到新发行版后，可打开对应下载页面。

### Windows / macOS

打开发布包中的应用，将 `.ipa` 或 `.app` 放入 `touchHLE_apps` 目录后，通过内置游戏选择器运行。Windows 请先完整解压 ZIP，保留附带的字体、动态库和配置文件。

也可以在终端指定游戏路径：

```powershell
# Windows
.\touchHLE.exe "路径\游戏.ipa"
```

```sh
# macOS：在包含 touchHLE 可执行文件和配套资源的目录中运行
./touchHLE "路径/游戏.ipa"
```

更多参数见 [OPTIONS_HELP.txt](OPTIONS_HELP.txt)，目录安排及构建方式见 [构建文档](dev-docs/building.md)。

## 存档迁移

Android 启动器中长按游戏，选择“导出存档”生成 ZIP。在另一台设备导入同一游戏后，长按游戏选择“导入存档”，再选择该 ZIP。

导入时已有存档会先移入 `touchHLE_save_backups`。迁移时尽量使用相同游戏版本及相同 Bundle ID；不同地区或版本的存档不保证通用。存档 ZIP 不包含游戏本体。

桌面版游戏数据保存在 `touchHLE_sandbox` 中。手动迁移前请退出游戏并备份原目录。

## 模拟机型与画面设置

Android 设置支持全局选择“自动”或指定机型，也可以长按游戏单独设置。当前配置包括：

| 机型 | 横屏像素尺寸 |
| --- | --- |
| iPhone 3GS | 480 × 320 |
| iPhone 4 / 4S | 960 × 640 |
| iPhone 5 / 5s | 1136 × 640 |
| iPhone 6 | 1334 × 750 |
| iPhone 6 Plus | 2208 × 1242 |
| iPad 2 | 1024 × 768 |
| iPad Retina | 2048 × 1536 |

这些是模拟屏幕配置，实际游戏渲染尺寸可能不同；不支持对应屏幕的游戏会使用兼容尺寸。更高分辨率会增加渲染开销，也不会增加 IPA 中没有的纹理资源。

`touchHLE_default_options.txt` 的游戏默认配置和 `touchHLE_options.txt` 的自定义参数仍然生效，启动器传入的机型选择也会参与最终配置。请以启动日志中的 `Device:` 信息为准。

P2G 的扩展视野可在长按菜单的“画面与按键布局”中选择，通过扩展视野利用更宽的屏幕，并将触控操作区保持在中间。遇到画面或操作异常时可切回默认布局。

## 手柄

通过蓝牙或 USB 将手柄连接到运行模拟器的设备，再启动游戏。已用实际手柄验证 P2G 和罪恶都市原生输入；其他蓝牙或有线手柄取决于系统及 SDL 是否能正确识别其按键和轴。

### P2G：Xbox / 标准布局

| 按键 | 游戏操作 |
| --- | --- |
| 左摇杆 | 移动 |
| 右摇杆 | 镜头 |
| A | 回避（对应 ×） |
| B | 攻击／互动；收武器后采集、剥取（对应 ○） |
| X | 收武器／使用道具（对应 □） |
| Y | 攻击（对应 △） |
| LB | L 键：镜头归位及组合操作 |
| RB | R 键：奔跑及武器相关动作 |
| LT | Select |
| RT | Start |

推满左摇杆是普通移动，奔跑需配合 RB。具体动作随武器和当前状态变化，采集、剥取前通常需要先用 X 收武器。

### 罪恶都市

支持游戏原生手柄路径，左摇杆控制移动，右摇杆控制镜头。其余操作随步行、驾驶等状态变化，以游戏提示为准。支持方向键下 + B 打开地图、方向键上 + B 切换镜头。

其他未使用原生手柄接口的游戏，可以使用触屏映射、虚拟光标或倾斜控制映射，参数见 [OPTIONS_HELP.txt](OPTIONS_HELP.txt)。

## 问题反馈

请向 [本仓库 Issues](https://github.com/shawnw67/Podroid/issues) 提供：

- Podroid 版本、设备型号和系统版本。
- 游戏名称、版本、Bundle ID，以及模拟机型和画面布局。
- 复现步骤、`touchHLE_log.txt`，必要时附上 `touchHLE_log_previous.txt`。
- 画面问题附截图，手柄问题附型号及蓝牙／USB 连接方式。

Android 启动器可查看、分享运行日志；罪恶都市还提供性能和声音对比测试入口。

## 构建与开发

依赖安装及完整流程见 [dev-docs/building.md](dev-docs/building.md)。获取本分支源码：

```sh
git clone --recurse-submodules https://github.com/shawnw67/Podroid.git Podroid
cd Podroid
```

macOS 本地编译与测试：

```sh
export CMAKE_POLICY_VERSION_MINIMUM=3.5
cargo build --release
cargo test --release --lib
python3 dev-scripts/macos-integration-tests.py
```

准备好 Android SDK、NDK、Rust 目标及 Gradle 后，可运行签名脚本：

```sh
dev-scripts/build-android-uf.sh /path/to/your.keystore
```

脚本交互式询问签名密码。请使用自己的密钥；覆盖安装已有发行版需要相同包名和签名证书。

当前只发布 Android APK：本地签名构建，上传到 Release 草稿，审核并填写英文说明后手动发布。

本分支包含 AI 辅助代码。上游 touchHLE 不接受 AI 生成的代码，请勿将本分支代码作为贡献提交到上游。本分支的修改和讨论请留在本仓库。

## 许可证与致谢

touchHLE © 2023–2026 touchHLE project contributors。本分支保留上游及各依赖的版权与许可证声明。

touchHLE 自身源码采用 **Mozilla Public License 2.0**；由于依赖的许可证兼容要求，发行二进制采用 **GNU GPL v3 或更高版本**。附带动态库和字体的条款分别见 [touchHLE_dylibs](touchHLE_dylibs/) 和 [touchHLE_fonts](touchHLE_fonts/)。完整依赖版权信息可在应用设置或通过 `--copyright` 参数查看。

感谢 [上游 touchHLE 及贡献者](https://github.com/touchHLE/touchHLE/graphs/contributors)、dynarmic、SDL、OpenAL Soft、Symphonia、Rust 生态及所有相关开源项目，也感谢协助测试、反馈问题的用户。

本项目与 Apple Inc. 无关联，也未获其背书。iPhone、iOS、iPod touch 和 iPad 等名称属于其各自权利人。请仅运行合法取得的软件。
