[English](README.md) | **简体中文**

# desktop-todo-widget

一个本地优先的 Windows 待办小组件，围绕三种窗口模式构建：**Sidebar**、**Floating** 和 **Desktop**。

它专注于轻量任务管理、以托盘为主的交互方式、可自定义外观、Quick Links、天气以及原生 Windows 集成。

`Rust` · `Tauri` · `Vue 3` · `TypeScript` · `WebView2` · `SQLite`

## Screenshots

<!-- Add Sidebar screenshot here -->
<!-- Add Floating (Acrylic) screenshot here -->
<!-- Add Desktop screenshot here -->
<!-- Add Orb screenshot here -->

截图会在发布素材整理完成后补充到这里。上面的占位注释是有意保留的，这样就不会出现凭空猜测的图片路径。

## Features

**待办生命周期** — 添加、编辑、完成、重新打开、取消、顺延、删除和重新排序。顺延会保留历史：原始记录的 status 被标记为 `carried`，并在同一个 SQLite 事务中为下一个任务日创建一条带关联的后续任务。

**窗口模式** — Sidebar、Floating 和 Desktop 共用同一个窗口和同一个 WebView；Floating 可以折叠为 56 DIP 的头像 Orb。参见[窗口模式](#窗口模式)。

**外观配置** — 每种窗口模式各自保持一份外观配置：玻璃、纯色、两段渐变、本地托管图片，或当前 Windows 壁纸，以及色调、不透明度、模糊、遮罩、图片适配方式与位置，还有一个可选的自定义文字颜色。

**Quick Links** — 添加、编辑、重新排序和删除你自己的链接；只接受 `http://` 和 `https://` URL。

**天气** — 可选的当前天气与今日最高/最低气温，带本地缓存快照，并有明确的“未配置”状态。

**回顾** — 只读的每日、每周和每月回顾，基于本地任务历史汇总。不提供评分、趋势或建议。

**语言** — 英文、简体中文或跟随系统（跟随 Windows 显示语言）。

**Windows 集成** — 常驻托盘、不显示任务栏按钮、不出现在普通 Alt+Tab 列表中、小组件和 Orb 上都有右键上下文菜单，并且每种模式的窗口几何信息在重启后依然保留。已在 150% 显示缩放比例下验证。

## Window modes

**Sidebar** — 面向屏幕边缘的窗口模式。占满显示器工作区高度，停靠在左边缘或右边缘，并记住停靠侧和宽度。把 Floating 拖到边缘附近即可进入 Sidebar。

**Floating** — 可移动的无边框窗口，也是首次运行的默认模式。它可以折叠为 Orb 再展开，并且把展开后的尺寸和 Orb 锚点作为两个相互独立的值保存下来。Enhanced 渲染后端在该模式下支持原生 Acrylic。

**Desktop** — 由桌面宿主的无边框小组件。它会被重新挂载为 Windows 桌面宿主的子窗口，因此其周围的壁纸、桌面图标和原生桌面右键菜单仍然可用。解锁状态下可移动、可调整大小，几何信息独立于 Floating，且不提供始终置顶。

### Desktop 模式下无法使用原生 Acrylic

这是设计上与平台上的限制，不是功能退化：

- Desktop 以子窗口形式托管在 Windows 桌面层级之下（`SHELLDLL_DefView`），而这并不是一个公开文档化的嵌入 API。
- 原生 Acrylic 路径需要顶层 HWND 语义；合成托管的背景无法附加到该子窗口上。
- 因此 Desktop 使用已文档化的半透明 Graphite 回退外观，原生菜单中将其标注为 `Desktop (Acrylic unavailable)`。

挂载/卸载生命周期与宿主发现细节参见 [docs/desktop-mode.md](docs/desktop-mode.md)。

## Rendering backends

WebView2 的宿主方式在设置中选择，重启后生效。它与窗口模式相互独立：Sidebar、Floating 和 Desktop 在两种后端上都能工作。**Standard** 是 v1 的默认选项。

| 后端 | 宿主方式 | 说明 |
| --- | --- | --- |
| **Standard** | 普通窗口化 WebView2（`ICoreWebView2Controller`） | 以兼容性为取向；在不需要合成特性时是推荐选择。会把 WebView 内容暴露给 Windows UI Automation。 |
| **Enhanced** | 合成托管的 WebView2（`ICoreWebView2CompositionController`） | 在 Floating 中启用原生 Acrylic。是更深入的 Windows 特定实现，其常规 UI Automation / 辅助功能行为也更受限。 |

参见 [docs/phase-7c3b4-dual-backend-release-decision.md](docs/phase-7c3b4-dual-backend-release-decision.md) 与 [docs/native-composition.md](docs/native-composition.md)。

## Appearance profiles

外观按窗口模式区分：编辑 Sidebar 的材质不会影响 Floating。每份配置保存背景类型、色调、不透明度、模糊和遮罩数值、图片适配方式与位置、文字对比度模式，以及自定义文字颜色。

- 背景类型：玻璃、纯色、渐变、本地托管图片、当前 Windows 壁纸。
- 文字对比度：自动、浅色、深色或自定义。自动模式会直接采样纯色和渐变端点，并对图片/壁纸数据按 32×32 采样一次。
- 图片缺失、损坏或过大时会回退到安全的玻璃外观，而不是让整个界面出错。

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

### Internal compatibility identifiers

一些随产品发布的标识符有意保留 `alan-desktop` 这一发布前的拼写，因为改名会让已有的用户数据无法关联或破坏升级路径。它们是内部名称，不是产品名：

- `alan-desktop` — Rust crate 名、私有的 `package.json` 名称，因此也是构建出的可执行文件 `alan-desktop.exe`。
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
- **WebView2** — 窗口化（`Standard`）或合成托管（`Enhanced`）的渲染表面。
- **SQLite** — 本地任务/历史/分类/天气存储；`app_settings` 中存放带类型的 JSON 设置。
- **Windows Composition APIs** — Enhanced 后端的合成宿主、Desktop Acrylic 控制器和视觉树。

逐模块的细节参见 [ARCHITECTURE.md](ARCHITECTURE.md) 与 [docs/](docs/)。

## Building from source

环境要求：Windows 11、Node.js 20+、pnpm、带 MSVC target 的 Rust stable、Windows SDK，以及 WebView2 Runtime。

```powershell
pnpm install
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --no-bundle
```

Windows 构建还会把自包含的 Windows App SDK payload 部署到与可执行文件相同的目录中。如果该 payload 缺失，`build.rs` 会直接失败并给出确切的命令，因为 Enhanced（合成托管）后端在运行期需要它：

```powershell
powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1
```

该 payload 不会被提交到仓库；其来源与版本策略的权威说明见 [docs/windows-app-sdk-runtime.md](docs/windows-app-sdk-runtime.md)。

`pnpm tauri:dev` 会为前端开发启动 Vite 开发服务器。生产构建会把编译好的前端嵌入其中，并通过 Tauri 自定义协议（`custom-protocol` 特性）提供，因此发布构建从不依赖开发服务器。v1 没有安装程序或更新程序流水线：`pnpm tauri build --no-bundle` 产出可执行文件及其运行时 payload，且 `bundle.active` 为 `false`。

## Known limitations

- **Desktop Acrylic** — 设计上不可用。Desktop 以子窗口形式托管在 Windows 桌面窗口层级之下，而原生 Acrylic 路径需要顶层 HWND 语义；因此 Desktop 改用半透明的 Graphite 回退外观。
- **Enhanced 的辅助功能** — 合成托管意味着 Enhanced 后端在常规 Windows UI Automation 行为上比 Standard 更受限。如果你依赖屏幕阅读器或自动化工具，请使用 Standard。
- **Windows 特定实现** — Enhanced 依赖 Windows/WebView2/Tauri 的特定行为，随着这些平台演进可能需要进行兼容性更新。
- **Desktop 宿主未公开文档化** — `Progman`、`WorkerW` 和 `SHELLDLL_DefView` 的拓扑结构可能随 Windows 更新和 Explorer 重启而变化。
- **范围** — 仅支持 Windows 11，没有安装程序/更新程序，没有同步，也没有英文和简体中文之外的其他本地化。

## Roadmap

计划在 v1 稳定之后进行：

- 评估把 Windows Composition / Acrylic 托管相关工作抽取为可独立复用的项目或库。
- 报表：在现有任务历史之上提供更丰富的事实性回顾界面。
- 组件化：让前端与 Rust 的边界更小、更清晰。
- 设置组织：更审慎地对当前设置界面进行分组。
- 安装程序与更新机制的改进。

已推迟的工作记录在 [FUTURE.md](FUTURE.md) 中。本节内容目前都尚未实现。

## Contributing

欢迎贡献。简要版：

- 让每个 pull request 只聚焦一处改动。
- 说明平台特定的行为，以及其背后的 Windows/WebView2 假设。
- 在可行的范围内补充测试；原生窗口模式的改动仍然需要手工执行模式切换检查。
- 保护用户数据。迁移必须是幂等的，不接受破坏性迁移。
- 在修改 Enhanced 时不要使 Standard 模式退化。
- 欢迎 AI 辅助的 pull request，但你必须审查、测试并理解你提交的内容。

参见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## License

MIT — 参见 [LICENSE](LICENSE)。Copyright (c) 2026 Alan Floyd.

依赖许可证与来源说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
