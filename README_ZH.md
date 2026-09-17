[English](README.md) | **简体中文**

# desktop-todo-widget

一个本地优先的 Windows 待办小组件，围绕三种窗口模式构建：**Sidebar**、**Floating** 和 **Desktop**。

它专注于轻量任务管理、以托盘为主的交互方式、可自定义外观、Quick Links、天气以及原生 Windows 集成。

`Rust` · `Tauri` · `Vue 3` · `TypeScript` · `WebView2` · `SQLite`

## Screenshots

<p align="center">
  <a href="docs/assets/demo.gif"><img src="docs/assets/demo.gif" width="420" alt="desktop-todo-widget 演示"></a>
</p>

[观看 MP4 录屏](docs/assets/demo.mp4)

<table>
  <tr>
    <th align="center">Orb</th>
    <th align="center">Sidebar</th>
    <th align="center">Desktop</th>
  </tr>
  <tr>
    <td align="center"><a href="docs/assets/orb.png"><img src="docs/assets/orb.png" width="72" alt="悬浮 Orb"></a></td>
    <td align="center"><a href="docs/assets/sidebar.png"><img src="docs/assets/sidebar.png" width="190" alt="Sidebar 模式"></a></td>
    <td align="center"><a href="docs/assets/desktop.png"><img src="docs/assets/desktop.png" width="220" alt="Desktop 模式"></a></td>
  </tr>
</table>

## 下载

正式版本发布在 [GitHub Releases](https://github.com/alanfloyd-dev/desktop-todo-widget/releases)。

**当前发布版本：v1.0.1** —— `desktop-todo-widget-v1.0.1-windows-x64.zip`。解压后运行：

`desktop-todo-widget.exe`

v1.0.1 压缩包同时包含 `install.ps1` 与 `uninstall.ps1`，可选用于当前用户的安装与卸载（见[安装](#安装)）。

便携使用方式仍然保留：解压 ZIP 后直接运行可执行文件即可，无需安装。

**上一版本：v1.0.0** —— `desktop-todo-widget-v1.0.0-windows-x64.zip`。

v1.0.1 的变更见 [RELEASE_NOTES.md](RELEASE_NOTES.md)。

### Windows 兼容性

- 已在 Windows 11 上测试。
- 根据底层平台要求，Windows 10 1809+ 预计可以运行，但目前尚未完成完整验证。
- 需要 WebView2 Runtime。

## 安装

`install.ps1` 与 `uninstall.ps1` 随发布 payload 一起放置在可执行文件旁（v1.0.1 及以后），用于安装或卸载一份属于当前用户的副本。它们是普通的 PowerShell 脚本（Windows PowerShell 5.1 或更高版本），不需要管理员权限。

```powershell
# 在解压出的发布文件目录中执行，install.ps1 与可执行文件位于同一目录
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

`-ExecutionPolicy Bypass` 只对该次调用生效，没有理由为了运行这些脚本而修改计算机或用户的全局执行策略。

`install.ps1` 会：

- 把可执行文件复制到 `%LOCALAPPDATA%\Programs\desktop-todo-widget\`；
- 无论构建产物内部的名称是什么，都安装为 `desktop-todo-widget.exe`；
- 创建一份当前用户的开始菜单快捷方式，除非传入 `-NoStartMenuShortcut`；
- 支持重复执行即原地升级：先停止从该目录启动的正在运行的实例，再替换程序文件，并清理新版本不再随附的 payload 文件。

它有意不做的事：不写入 Program Files 或任何全机范围的位置，不修改注册表或 `PATH`，也不删除用户数据。指定的 payload 目录必须同时包含可执行文件与随附的运行时文件；payload 不完整时安装会直接失败，而不是装出一份残缺的程序。

用户数据继续保存在 `%APPDATA%\net.alanfloyd.desktop\`（数据库、设置、外观配置、托管图片、天气缓存），安装与卸载都不会影响它。

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1                  # 保留你的数据
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -RemoveUserData  # 先警告，再一并删除数据
```

`uninstall.ps1` 会在需要时停止正在运行的实例，删除开始菜单快捷方式和程序文件，并在未传入 `-RemoveUserData` 时保留 `%APPDATA%\net.alanfloyd.desktop\`。该参数会在删除前打印确切路径并要求确认；`-Force` 用于脚本化场景跳过确认，而在无法进行交互提示的宿主中会保留数据。删除范围是严格受限的：每个文件都必须位于解析出的安装目录内，并且必须是程序文件（可执行文件、文档、日志文件，或旧版本可能安装过的运行时 payload 文件）；一旦出现意外内容 —— 陌生的文件或子目录 —— 卸载会停止，而不是把它删掉。

两个脚本都由一个模拟验证脚本在一次性沙箱中运行验证：使用伪造的 `%LOCALAPPDATA%`、`%APPDATA%`、payload 和用户数据目录，覆盖全新安装、覆盖升级、两种卸载方式，以及各项拒绝路径。

```powershell
pwsh -File scripts/verify-install-scripts.ps1
```

便携 ZIP 仍然是主要分发方式；解压即运行的流程没有任何变化。

## Features

**待办生命周期** — 添加、编辑、完成、重新打开、取消、顺延、删除和重新排序。顺延会保留历史：原始记录的 status 被标记为 `carried`，并在同一个 SQLite 事务中为下一个任务日创建一条带关联的后续任务。

**窗口模式** — Sidebar、Floating 和 Desktop 共用同一个窗口和同一个 WebView；Floating 可以折叠为 56 DIP 的头像 Orb。参见[窗口模式](#窗口模式)。

**可见的设置入口** — 展开状态的页脚会在个人资料身份旁边显示一个设置齿轮，分别出现在 Floating 展开、Sidebar 和 Desktop 中（Orb 中不会出现）。它与右键菜单和托盘中的入口打开同一个设置界面，因此无需知道上下文菜单也能发现设置。

**外观配置** — 每种窗口模式各自保持一份外观配置：玻璃、纯色、两段渐变、本地托管图片，或当前 Windows 壁纸，以及色调、不透明度、模糊、遮罩、图片适配方式与位置，还有一个可选的自定义文字颜色。

**Quick Links** — 添加、编辑、重新排序和删除你自己的链接；只接受 `http://` 和 `https://` URL。

**天气** — 可选的当前天气与今日最高/最低气温，带本地缓存快照，并有明确的“未配置”状态。

**回顾** — 只读的每日、每周和每月回顾，基于本地任务历史汇总。不提供评分、趋势或建议。

**语言** — 英文、简体中文或跟随系统（跟随 Windows 显示语言）。

**Windows 集成** — 常驻托盘、不显示任务栏按钮、不出现在普通 Alt+Tab 列表中、小组件和 Orb 上都有右键上下文菜单，并且每种模式的窗口几何信息在重启后依然保留。已在 150% 显示缩放比例下验证。

## Window modes

**Sidebar** — 面向屏幕边缘的窗口模式。占满显示器工作区高度，停靠在左边缘或右边缘，并记住停靠侧和宽度。把 Floating 拖到边缘附近即可进入 Sidebar。

**Floating** — 可移动的无边框窗口，也是首次运行的默认模式。全新（从未使用过的）配置会以**展开**状态打开它，因此首次启动就能看到小组件本身 —— 日期行、任务、页脚和设置齿轮 —— 而不是屏幕角落里一个容易被忽略的 56 DIP Orb。它可以折叠为 Orb 再展开，并且把展开后的尺寸和 Orb 锚点作为两个相互独立的值保存下来。

**Desktop** — 由桌面宿主的无边框小组件。它会被重新挂载为 Windows 桌面宿主的子窗口，因此其周围的壁纸、桌面图标和原生桌面右键菜单仍然可用。解锁状态下可移动、可调整大小，几何信息独立于 Floating，且不提供始终置顶。

### 首次运行

首次运行是产品唯一一次自行决定展示状态的时机：当配置文档尚不存在时，新建的文档以 Floating 展开、默认尺寸、**渐变（Gradient）**材质开始。渐变使用的就是项目既有的石墨色板（`#11191e` → `#213747`，135°）与既有不透明度；把它作为全新配置的默认材质，是因为在任意壁纸之上只做一层色调，会和组件自身的文字争夺可读性，而渐变让界面保持一个确定的形状，在暗色、浅色、高饱和和复杂纹理桌面上观感一致。已存在的配置永远不会被重新套用默认值 —— 它会原样加载，包括它的 Floating 展示状态与材质 —— 因此升级不会改变用户上次离开时的状态。可见的设置齿轮从第一帧展开界面起就存在。

### Desktop 的材质回退

Desktop 始终使用已文档化的半透明 Graphite 外观。这是设计上与平台上的限制，不是功能退化：

- Desktop 以子窗口形式托管在 Windows 桌面层级之下（`SHELLDLL_DefView`），而这并不是一个公开文档化的嵌入 API。
- 窗口背景效果需要顶层 HWND 语义；`SHELLDLL_DefView` 子窗口不具备。
- 因此 Desktop 始终使用已文档化的半透明 Graphite 回退外观。

挂载/卸载生命周期与宿主发现细节参见 [docs/desktop-mode.md](docs/desktop-mode.md)。

## 渲染

产品只有一个窗口化 WebView2 后端（`ICoreWebView2Controller`）：只有一条宿主路径，没有用户可选的渲染后端，并完整暴露 Windows UI Automation。窗口材质由 Tauri 窗口效果请求加上 CSS 材质层共同决定；参见[外观配置](#appearance-profiles)。

## Appearance profiles

外观按窗口模式区分：编辑 Sidebar 的材质不会影响 Floating。每份配置保存背景类型、色调、不透明度、模糊和遮罩数值、图片适配方式与位置、文字对比度模式，以及自定义文字颜色。

- 背景类型：玻璃、纯色、渐变、本地托管图片、当前 Windows 壁纸。
- 文字对比度：自动、浅色、深色或自定义。自动模式会直接采样纯色和渐变端点，并对图片/壁纸数据按 32×32 采样一次。
- 图片缺失、损坏或过大时会回退到安全的玻璃外观，而不是让整个界面出错。
- 支持的图片文件为 PNG、JPEG 和 WebP。作为背景选择的图片按原始尺寸保存；而**个人资料头像**会先做归一化 —— 解码、按方向校正、居中裁剪为正方形、缩放到 256×256，并以保留透明通道的 PNG 保存 —— 因此多兆字节的照片不会进入配置。文件过大无法读取（超过 24 MB）或解码器无法处理时，会在打开选择器的按钮旁边给出提示并拒绝。

参见 [docs/appearance.md](docs/appearance.md)。

## Quick Links

Quick Links 是用户自行管理的“名称 / URL”组合，作为产品的一个区域展示：可添加、编辑、重新排序、删除。名称和 URL 都属于你自己的数据，永远不会被翻译。保存时会校验链接是带主机名的 `http`/`https` 地址，在即将打开前会再校验一次。列表为空时该区域直接隐藏。

## Weather and Review

天气是可选的氛围信息，而不是启动依赖。地点搜索只会在你明确触发后运行，且必须由你自己选择一条结果，之后预报才使用已保存的坐标和时区。缓存快照的新鲜度是分级判定的，刷新失败时会保留最后一份有效缓存。天气数据来自 [Open-Meteo.com](https://open-meteo.com/)，遵循 [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)；署名信息显示在设置中和 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 里。

回顾是对原始任务历史的只读投影：每日（一个任务日）、每周（周一至周日）或每月（自然月）。它会报告计划、已完成、顺延、取消和待处理的数量，以及任务日与分类分布，并且不保存任何报告快照。

参见 [docs/weather.md](docs/weather.md) 与 [docs/reviews.md](docs/reviews.md)。

## Data and privacy

- **本地优先。** 任务、设置、外观配置、Quick Links、个人资料身份以及天气缓存都保存在本机。
- **没有账户。** 没有登录、托管身份或同步服务器。
- **不会主动上传。** 任务、个人资料、外观和 Quick Link 数据不会被应用程序发送到任何地方。
- **天气是唯一的远程调用。** Open-Meteo 会收到为解析你配置的地点所需的 geocoding 与预报请求。未配置地点时，不会发起任何天气请求。
- **诊断信息需主动开启。** 设置 → 开发者 → 复制诊断信息会生成一份经过允许列表筛选、隐私最小化的报告，其中包含运行时/窗口元数据和非敏感的 appearance 状态。它不包含任务内容、个人资料值、天气地点、素材文件名/路径，以及确切的数据库路径。
- **卸载会保留你的数据。** `uninstall.ps1` 只删除程序文件；只有在显式传入 `-RemoveUserData` 时，才会在打印警告并要求确认之后删除 `%APPDATA%\net.alanfloyd.desktop\`。

### Internal compatibility identifiers

一些随产品发布的标识符有意保留 `alan-desktop` 这一发布前的拼写，因为改名会让已有的用户数据无法关联或破坏升级路径。它们是内部名称，不是产品名：

- `alan-desktop` — Rust crate 名、私有的 `package.json` 名称，因此也是构建出的可执行文件 `alan-desktop.exe`。`install.ps1` 会把它安装为 `desktop-todo-widget.exe`，这只是一个文件名：产品自身不读取自己的可执行文件名称。
- `alan-desktop.sqlite3` — 本地数据库文件，位于应用数据目录 `net.alanfloyd.desktop` 下。
- `alan-desktop-tray` — 托盘图标 id。

公开产品名是 `desktop-todo-widget`，托盘提示、托盘/上下文菜单标题以及窗口标题使用的都是它。参见 [docs/data-model.md](docs/data-model.md)。

## Development note

本项目在开发中使用了大量 AI 辅助。

我是地质学专业的学生，而不是计算机科学专业的学生，主要技术方向是 Python 以及面向科研工作的数据分析。Windows 底层机制、Rust、Tauri 和 WebView2 都不是我的主要技术栈，因此我并不声称对每一处实现细节都具备深厚的专业能力。

不过，这并不是一个由 AI 一键生成的仓库。我仍然参与产品设计、架构决策、测试、调试、手工 QA 和发布审核。代码库中包含注释、测试和技术文档，目的就是让实现更容易被审查和维护。

项目在持续维护，随着我不断学习，我也会继续改进它。

非常欢迎贡献 —— 尤其是缺陷报告、代码审查、Windows 平台方面的专业知识，以及对可改进之处给出更清晰的实现。

也欢迎 AI 辅助的贡献，但请务必审查、测试并理解你提交的改动。

## Architecture

总体而言：Vue 3 + TypeScript 负责产品界面渲染并持有展示状态；Rust 负责操作系统路径、持久化、窗口/托盘生命周期以及 Win32 边界；Win32/WebView2 层被限制在一层很窄的适配器之后。UI 从不直接操作 HWND 或 SQLite。

- **Tauri** — 应用外壳、窗口/托盘生命周期、IPC 命令与事件。
- **Rust** — 产品设置、SQLite 仓储与迁移、任务生命周期、回顾、天气适配器、外观校验。
- **Vue 3 + TypeScript** — 界面呈现、各模式布局、设置、回顾、天气展示。
- **WebView2** — 窗口化的渲染表面。
- **SQLite** — 本地任务/历史/分类/天气存储；`app_settings` 中存放带类型的 JSON 设置。

逐模块的细节参见 [ARCHITECTURE.md](ARCHITECTURE.md) 与 [docs/](docs/)。

### Windows 特定代码的维护说明

部分 Windows 特定的窗口宿主与窗口模式管理代码刻意保持了保守的实现方式，目前的集中程度也高于理想状态。这部分代码主要集中在 `src-tauri/src/window_mode.rs` 与 `src-tauri/src/product_window.rs` 两个文件中。

这是已知的技术债，而不是没人注意到的问题：这些文件里的行为，是在真实缺陷修复与 QA 过程中围绕平台兼容性、生命周期、输入、DPI、桌面宿主、窗口样式、Win+D 行为、任务栏 / Alt+Tab 语义以及恢复路径逐步固化下来的。

重构计划安排在 v1 稳定之后，但行为稳定性优先于结构上的清理。在这类区域改动时，最好保持小步且不改变行为；相关约束记录在 [CONTRIBUTING.md](CONTRIBUTING.md) 与 [docs/desktop-mode.md](docs/desktop-mode.md) 中。

## Building from source

环境要求：Windows 11、Node.js 20+、pnpm、带 MSVC target 的 Rust stable、Windows SDK，以及 WebView2 Runtime。

```powershell
pnpm install
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --no-bundle
```

### 验证

有四个脚本覆盖单元测试无法触达的部分。它们都会先停止正在运行的实例，并且在没有经过校验的副本之前，绝不会改动任何配置目录：

```powershell
pwsh -File scripts/verify-no-console-window.ps1   # 发布版无控制台窗口；调试版仍有
pwsh -File scripts/verify-first-use-ux.ps1        # 全新配置的首次运行、材质、设置齿轮、重启、退出
pwsh -File scripts/verify-avatar-flow.ps1         # 头像选择矩阵、归一化、持久化与错误反馈
pwsh -File scripts/verify-install-scripts.ps1     # 在一次性沙箱中验证安装 / 升级 / 卸载
```

- `verify-no-console-window.ps1` 检查两种构建的 PE subsystem，并以 `explorer.exe` 执行一次真实的外壳启动（即双击路径），同时把调试构建作为阳性对照，因此通过即说明检测器本身能识别控制台窗口。
- `verify-first-use-ux.ps1` 通过 WebView2 DevTools 端点驱动构建好的应用。产品通过 Windows 已知文件夹解析数据目录，因此无法用环境变量把“全新配置”隔离出来：该包装脚本会先把真实的 `%APPDATA%\net.alanfloyd.desktop` 复制到一旁，在删除任何东西之前按大小与 SHA-256 校验副本，结束后再恢复并重新校验。
- `verify-avatar-flow.ps1` 采用同样的方式，并通过 `scripts/avatar-picker-drive.ps1`（UI Automation）驱动真实的原生文件选择对话框，配合生成的合成图片，逐张检查实际保存、渲染和报错的结果。
- `verify-install-scripts.ps1` 使用伪造的 `%LOCALAPPDATA%`、`%APPDATA%`、payload 和用户数据目录运行 `install.ps1` 与 `uninstall.ps1`，其中包括各项拒绝路径。

`pnpm tauri:dev` 会为前端开发启动 Vite 开发服务器。生产构建会把编译好的前端嵌入其中，并通过 Tauri 自定义协议（`custom-protocol` 特性）提供，因此发布构建从不依赖开发服务器。

发布构建会以 Windows GUI 子系统链接（`src-tauri/src/main.rs` 中的 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`），因此双击构建出的可执行文件不会再弹出控制台窗口。调试构建仍保留控制台，开发期间的 `eprintln!` 诊断信息依旧可见。在任何构建配置下，应用都会把诊断信息写入可执行文件旁边的 `qa-diagnostics.log`，panic 报告也会追加到同一个文件，因此即使没有控制台，发布版本的崩溃依然会被记录。

`pnpm tauri build --no-bundle` 产出可执行文件，且 `bundle.active` 为 `false`：Tauri 打包器处于关闭状态，安装方式为[安装](#安装)中描述的按用户脚本，而不是生成的 MSI/EXE 安装包。

## Known limitations

- **Desktop 材质** — Desktop 始终使用半透明 Graphite 回退外观。它以子窗口形式托管在 Windows 桌面窗口层级之下，而窗口背景效果需要顶层 HWND 语义。
- **Windows 特定实现** — 产品依赖 Windows/WebView2/Tauri 的特定行为，随着这些平台演进可能需要进行兼容性更新。
- **Desktop 宿主未公开文档化** — `Progman`、`WorkerW` 和 `SHELLDLL_DefView` 的拓扑结构可能随 Windows 更新和 Explorer 重启而变化。
- **范围** — 已验证 Windows 11，没有更新程序，也没有打包的 MSI/EXE 安装包（安装方式为[安装](#安装)中的按用户脚本），没有同步，也没有英文和简体中文之外的其他本地化。

## Roadmap

计划在 v1 稳定之后进行：

- 报表：在现有任务历史之上提供更丰富的事实性回顾界面。
- 组件化：让前端与 Rust 的边界更小、更清晰。
- 设置组织：更审慎地对当前设置界面进行分组。
- 打包与更新机制：在现有的按用户安装/卸载脚本之上，补充签名、打包安装程序与更新机制。

已推迟的工作记录在 [FUTURE.md](FUTURE.md) 中。除[安装](#安装)中描述的安装与卸载脚本外，本节内容目前都尚未实现。

## Contributing

欢迎贡献。简要版：

- 让每个 pull request 只聚焦一处改动。
- 说明平台特定的行为，以及其背后的 Windows/WebView2 假设。
- 在可行的范围内补充测试；原生窗口模式的改动仍然需要手工执行模式切换检查。
- 保护用户数据。迁移必须是幂等的，不接受破坏性迁移。
- 欢迎 AI 辅助的 pull request，但你必须审查、测试并理解你提交的内容。

参见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## License

MIT — 参见 [LICENSE](LICENSE)。Copyright (c) 2026 Alan Floyd.

依赖许可证与来源说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
