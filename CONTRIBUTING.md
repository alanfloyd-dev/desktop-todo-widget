# Contributing

English first; [简体中文](#简体中文贡献指南) below. Both sections say the same thing.

Thanks for helping improve desktop-todo-widget. This is a small, Windows-first personal project: Rust + Tauri 2 + Vue 3 + TypeScript + SQLite. Bug reports, Windows platform expertise, and focused fixes are all genuinely useful.

One historical note so nobody rediscovers old plans: an experimental Enhanced rendering backend (composition-hosted WebView2, Native Acrylic/HostBackdrop, WinAppSDK runtime payload, vendored Wry patch) existed once and has been retired. The product is Standard-only now. Please don't bring it back.

## Contribution philosophy

- Fix concrete bugs, compatibility problems, and real product needs first.
- Do not submit high-risk refactors because code "looks tidier". File size alone is not a reason to split a file.
- Keep changes small, verifiable, and revertible. One commit should carry one clear responsibility; do not mix feature work, large refactors, and dependency upgrades.
- Do not upgrade dependencies unless the change at hand requires it.

## Windows native layer policy

`window_mode.rs`, `product_window.rs`, `platform/windows/widget_frame.rs` and the rest of the native window layer are **frozen by default**.

Only touch them for:

- a concrete bug,
- a Windows compatibility requirement,
- a Tauri / WebView2 / OS upgrade incompatibility,
- a clear product need.

Not allowed:

- splitting a file because it is large,
- reordering Win32 calls without a behavior baseline and QA,
- drive-by cleanups in these modules.

Shell attach/detach, DPI, taskbar/Alt+Tab semantics, window styles, reparenting, and geometry are all sensitive to call ordering. Much of what is there reflects verified platform behavior, not taste. If a structural change in these files seems necessary, open an issue first and bring a behavior baseline: focused tests plus the manual checks listed under [Testing expectations](#testing-expectations).

## Backend policy

The product has exactly one rendering backend: the ordinary windowed WebView2 controller. There is no user-selectable backend and no second hosting path.

Do not reintroduce a second backend. If a future change genuinely needs one, it must first demonstrate:

- a concrete product capability that the Standard backend cannot provide,
- acceptable maintenance cost,
- controlled Windows environment dependencies,
- a complete test and QA plan.

Do not add a high-complexity native rendering path for a visual effect alone.

## Architecture boundaries

- **Vue / TypeScript** — UI, interaction, pages, components.
- **Rust / Tauri** — business commands, state, persistence, platform bridging.
- **SQLite** — local data.
- **Win32** — only the window/shell integration the framework cannot cover reliably.

Keep product logic out of the Win32 adapter, and do not grow the native layer without a concrete reason. Do not add telemetry, accounts, or a remote service without an explicit project decision.

## Data and migrations

- Preserve user data. Migrations must be idempotent and compatible with an existing user database.
- Add migrations in ascending order and record the version in the same transaction as the schema mutation.
- Destructive migrations are not acceptable, and neither is renaming the app-data directory, database filename, or tray id — those keep their internal `alan-desktop` spelling so existing installs keep their data.
- Do not place user databases or copied diagnostic output in the repository.

## Code and comments

- Comment non-obvious constraints, not routine syntax.
- In Win32 code, explain invariants, call ordering, undocumented Shell assumptions, and failure modes.
- Every new `unsafe` block needs a nearby `SAFETY:` comment describing why pointers, handles, buffers, callbacks, or lifetimes are valid.
- Preserve the same outer Tauri HWND and WebView2 controller across Desktop transitions.
- Do not replace conditional lifecycle recovery with polling, a global Win+D shortcut, or always-on-top.

## Testing expectations

`cargo test` must pass before you propose anything. For changes that touch product behavior, also walk through what applies:

- Sidebar
- Floating Expanded
- Floating Orb
- Desktop
- mode transitions (including Floating → Sidebar → Floating and Floating → Desktop → Floating; Desktop interaction and Win+D gates are documented in [docs/desktop-mode.md](docs/desktop-mode.md))
- startup / restart restore
- DPI (at least 100% and 150%)
- taskbar / Alt+Tab
- tray
- Glass / Solid / Gradient materials
- install / uninstall, if the change touches the install chain (`scripts/verify-install-scripts.ps1`)
- legacy settings compatibility, if the change touches settings parsing

For Windows native changes, state in the PR which of these you ran by hand and on which Windows version.

## Repository hygiene

- Do not use `git add -A`. Stage what the change actually touches.
- Do not delete existing untracked QA / evidence / inquiry directories (`.rc0-*`, `inquiry/`, `scripts/rc0-qa/`) — they are long-lived working material.
- Check `git status` before anything destructive.
- No unrelated dependency upgrades.
- One commit, one clear responsibility.

## AI-assisted contributions

AI-assisted work is welcome, but you own the change: understand the boundaries it touches, verify actual behavior, and read the diff before submitting. PRs are judged by behavior and test results, not by whether a tool called the result elegant. Large unverified refactors proposed by an AI are not acceptable. Say so in the description when a change was AI-assisted.

## Pull request expectations

- State the purpose of the change.
- State the affected scope (which modes, which layers).
- List the tests you ran (automated and manual).
- For Windows native changes, describe the manual QA.
- If compatibility is involved, name the target Windows versions / environment.

## Development

```powershell
pnpm install
pnpm build
pnpm tauri:dev            # dev server + hot reload
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --no-bundle   # release build (needs --features custom-protocol via tauri build; see README)
```

Additional gates:

```powershell
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

The codebase carries some pre-existing lint debt. Keep the lines you touch clean; do not run repo-wide `cargo fmt` and do not fix unrelated warnings in the same PR.

The verify scripts cover what unit tests cannot reach — see [README.md](README.md#verification). Requirements: Windows 11, Node.js 20+, pnpm, Rust stable with the MSVC target, the Windows SDK, and the WebView2 Runtime.

## Reporting bugs

Settings → Developer → **Copy diagnostics** produces an issue-ready text report. Review it before posting. The report is built from an explicit allowlist and excludes tasks, profile values, weather location, Quick Link URLs, asset filenames/paths, wallpaper paths, and precise database paths.

## Licensing and provenance

Contributions are accepted under the repository [MIT License](LICENSE). Do not copy code from repositories without a compatible license. When adapting a substantial external implementation, record the source, license, and what was changed.

---

# 简体中文贡献指南

感谢参与改进 desktop-todo-widget。这是一个小型、Windows 优先的个人项目：Rust + Tauri 2 + Vue 3 + TypeScript + SQLite。缺陷报告、Windows 平台经验、聚焦的修复都非常有用。

先说一句历史，免得有人重新翻出旧方案：项目曾有一个实验性的 Enhanced 渲染后端（合成托管的 WebView2、Native Acrylic/HostBackdrop、WinAppSDK runtime payload、vendored Wry 补丁），现已退役。当前产品是 Standard-only。请不要把它带回来。

## 贡献理念

- 优先修复具体的 bug、兼容性问题和真实产品需求。
- 不要因为"代码看起来更整齐"就提交高风险重构。文件行数本身不是拆分文件的理由。
- 变更尽量小、可验证、可回退。一个 commit 只承载一个明确责任；不要把功能修改、大重构和依赖升级混在一起。
- 除非当前改动确实需要，否则不做依赖升级。

## Windows 原生层策略

`window_mode.rs`、`product_window.rs`、`platform/windows/widget_frame.rs` 以及其余原生窗口层**默认视为冻结（frozen-by-default）**。

只有以下情况才应修改：

- 具体的 bug；
- Windows 兼容性要求；
- Tauri / WebView2 / 操作系统升级导致的不兼容；
- 明确的产品需求。

不允许：

- 仅因为文件较大就拆分；
- 在没有行为基线和 QA 的情况下重排 Win32 调用顺序；
- 在这些模块里做顺手清理。

shell 挂载/卸载、DPI、任务栏 / Alt+Tab 语义、窗口样式、重新挂载（reparent）、geometry 都对调用时序敏感。这些代码里的很多写法反映的是已验证的平台行为，而不是个人口味。如果确实需要结构性改动，请先开 issue，并带上行为基线：聚焦的测试加[测试期望](#测试期望)中列出的手工检查。

## 后端策略

产品只有一个渲染后端：普通的窗口化 WebView2 controller。没有用户可选的后端，也没有第二条宿主路径。

不要重新引入第二后端。如果未来确实需要，必须先证明：

- 存在 Standard 后端无法满足的明确产品能力；
- 维护成本可接受；
- Windows 环境依赖可控；
- 有完整的测试与 QA 方案。

不要为了视觉效果单独增加高复杂度的原生渲染路径。

## 架构边界

- **Vue / TypeScript** — UI、交互、页面与组件。
- **Rust / Tauri** — 业务命令、状态、持久化、平台桥接。
- **SQLite** — 本地数据。
- **Win32** — 只处理框架无法可靠覆盖的窗口/Shell 集成。

不要把产品逻辑放进 Win32 适配层；没有具体理由不要扩大原生层。未经明确的项目决策，不要加入遥测、账号或远程服务。

## 数据与迁移

- 保护用户数据。迁移必须幂等，并与已有用户数据库兼容。
- 按版本号递增添加迁移，并在与 schema 变更同一个事务里记录版本。
- 不接受破坏性迁移；也不允许重命名应用数据目录、数据库文件名或 tray id —— 它们保留内部的 `alan-desktop` 拼写，以保住现有安装的数据。
- 不要把用户数据库或复制的诊断输出放进仓库。

## 代码与注释

- 注释写"非显而易见的约束"，不要复述语法。
- Win32 代码要解释不变量、调用顺序、未文档化的 Shell 假设和失败模式。
- 每个新的 `unsafe` 块附近必须有 `SAFETY:` 注释，说明指针、句柄、缓冲区、回调或生命周期的有效性依据。
- Desktop 模式切换过程中保持同一个外层 Tauri HWND 和 WebView2 controller。
- 不要用轮询、全局 Win+D 快捷键或始终置顶来替代条件化的生命周期恢复。

## 测试期望

提出任何改动之前 `cargo test` 必须通过。涉及产品行为的修改，还要按适用范围走一遍：

- Sidebar
- Floating Expanded
- Floating Orb
- Desktop
- 模式切换（包括 Floating → Sidebar → Floating、Floating → Desktop → Floating；Desktop 交互与 Win+D 门禁见 [docs/desktop-mode.md](docs/desktop-mode.md)）
- 启动 / 重启恢复
- DPI（至少 100% 与 150%）
- 任务栏 / Alt+Tab
- 托盘
- Glass / Solid / Gradient 材质
- 安装 / 卸载（若改动涉及安装链，跑 `scripts/verify-install-scripts.ps1`）
- 旧配置兼容（若改动涉及 settings 解析）

Windows 原生层的改动，请在 PR 里说明手工跑了哪些项、在哪个 Windows 版本上。

## 仓库卫生

- 不要用 `git add -A`。只 stage 改动真正触及的文件。
- 不要删除既有的 untracked QA / evidence / inquiry 目录（`.rc0-*`、`inquiry/`、`scripts/rc0-qa/`）—— 它们是长期工作材料。
- 危险操作前先看 `git status`。
- 不做无关的依赖升级。
- 一个 commit，一个明确责任。

## AI 辅助开发

欢迎 AI 辅助，但你要对改动负责：理解它触及的边界、验证真实行为、提交前读一遍 diff。PR 以实际行为和测试结果为准，而不是以"工具认为更优雅"为依据。不接受仅凭 AI 建议的大规模未验证重构。改动若由 AI 辅助完成，请在描述中说明。

## Pull request 期望

- 说明改动目的。
- 说明影响范围（哪些模式、哪些层）。
- 列出已运行的测试（自动 + 手工）。
- Windows 原生层改动要说明手工 QA。
- 涉及兼容性时，写明目标 Windows 版本 / 环境。

## 开发环境

```powershell
pnpm install
pnpm build
pnpm tauri:dev            # 开发服务器 + 热更新
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build --no-bundle   # 发布构建（custom-protocol 由 tauri build 传入；见 README）
```

附加门禁：

```powershell
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

代码库存在一些既有的 lint 债务。保证你触及的行是干净的即可；不要跑全仓 `cargo fmt`，也不要在同一个 PR 里顺手修无关警告。

verify 脚本覆盖单元测试触达不到的部分 —— 见 [README.md](README.md#verification)。环境要求：Windows 11、Node.js 20+、pnpm、带 MSVC target 的 Rust stable、Windows SDK、WebView2 Runtime。

## 报告缺陷

设置 → 开发者 → **复制诊断信息** 会生成一份可直接贴进 issue 的文本报告。发布前请先自查。报告基于显式白名单构建，不包含任务内容、配置项取值、天气位置、Quick Link URL、资产文件名/路径、壁纸路径和精确数据库路径。

## 许可与来源

贡献按仓库的 [MIT License](LICENSE) 接受。不要从没有兼容许可证的仓库复制代码。改写自外部的重要实现，请注明来源、许可证与改动内容。
