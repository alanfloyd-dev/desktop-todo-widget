[English](CONTRIBUTING.md) | **简体中文**

# 参与贡献

感谢你帮助改进 desktop-todo-widget。本项目以 Windows 为先，并有意识地让产品层保持精简。

这是一个规模不大、但持续维护的项目。缺陷报告、代码审查、Windows 平台经验，以及对可改进之处给出更清晰的实现，都非常有价值。

## 报告缺陷

设置 → 开发者 → **复制诊断信息**会生成一份可直接用于 issue 的文本报告。发布前请先自行审阅。该报告基于显式允许列表构建，不包含任务、个人资料值、天气地点、Quick Link URL、素材文件名/路径、壁纸路径，以及确切的数据库路径。

## 打开改动之前

- 让每个 pull request 只聚焦一处改动。不要把不相关的功能混入文档、外观或平台相关的改动中。
- 说明任何平台特定的行为：其背后的 Win32/WebView2 假设，以及该假设不成立时会发生什么。
- 在可行的范围内补充测试。原生窗口模式的改动还需要在 Windows 上手工执行 Floating → Sidebar → Floating 与 Floating → Desktop → Floating 检查；Desktop 的改动应重复 [docs/desktop-mode.md](docs/desktop-mode.md) 中记录的交互与 Win+D 门禁检查。
- 在修改 Enhanced 时，Standard 模式不得退化。Standard 是 v1 的默认选项，也是辅助功能的路径。
- 让产品逻辑留在 Win32 适配器之外；未经明确的项目决策，不要加入遥测、账户或远程服务。

## 数据与迁移

- 保护用户数据。迁移必须是幂等的，并与已有的用户数据库兼容。
- 迁移按升序添加，并在与 schema 变更相同的事务中记录版本号。
- 不接受破坏性迁移，也不接受重命名应用数据目录、数据库文件名或托盘 id —— 它们保持内部 `alan-desktop` 的拼写，以便已有安装保留自己的数据。
- 不要把用户数据库或复制出来的诊断输出放进仓库。

## 代码与注释

- 注释用来解释不明显的约束，而不是例行语法。
- 在 Win32 代码中，说明不变量、调用顺序、未文档化的 Shell 假设，以及失败模式。
- 每个新增的 `unsafe` 块都需要在近处有一条 `SAFETY:` 注释，说明指针、句柄、缓冲区、回调或生命周期为何是有效的。
- 在 Desktop 转换过程中，保持同一个外层 Tauri HWND 与 WebView2 controller。
- 不要用 polling、全局 Win+D 快捷键或始终置顶来替代有条件的生命周期恢复。

## Windows 内部机制与大型编排模块

部分 Windows 特定模块，尤其是窗口模式与产品窗口编排相关的模块，目前比项目最终期望的更大、耦合也更紧。具体来说，`src-tauri/src/window_mode.rs` 与 `src-tauri/src/product_window.rs` 已知都很大。

这是已知的技术债，而不是没人注意到的问题。这些区域中的行为，是在围绕桌面挂载、WebView2 宿主、DPI、输入路由、窗口样式、Win+D 行为、任务栏/Alt+Tab 语义以及恢复路径的大量测试中逐步固化下来的。其中若干约束属于未文档化的平台行为，因此当前结构往往反映的是经验证可行的做法，而不是最易读的做法。

对这些文件进行大规模结构性重构不应随意提交。优先选择小步、不改变行为的改动，并配以有针对性的测试与手工验证。改变模块边界的重构应先通过 issue 讨论，并且每次重构都必须附带针对性的测试，以及[打开改动之前](#打开改动之前)与 [docs/desktop-mode.md](docs/desktop-mode.md) 中描述的手工模式切换与 Desktop 交互检查。

## 开发检查

```powershell
pnpm install
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

## AI 辅助贡献

欢迎 AI 辅助的 pull request，但你必须审查、测试并理解你提交的改动。当改动由 AI 辅助完成时，请在描述中说明；并且不要提交你无法解释或调试的 PR。

## 许可与来源

贡献按仓库的 [MIT License](LICENSE) 接受。不要从许可不兼容的仓库复制代码。当改编一份实质性的外部实现时，请记录来源、许可，以及改动内容。文档与 API 行为参考资料，若对某个平台 workaround 产生了实质性影响，应给出链接。
