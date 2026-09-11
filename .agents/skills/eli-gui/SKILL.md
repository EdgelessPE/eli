---
name: eli-gui
description: 开发、修改或审查 Eli 的 Slint GUI，覆盖业务界面、可复用组件、Rust 互操作和 Windows PE 实机验收。
---

# Eli GUI

此 skill 补充 Eli 的 Slint 项目约束。涉及 `.slint`、GUI Rust 控制器、主题、组件、浮层或原生窗口行为时使用；同时读取可用的 Slint skill，并先从 `eli-cli/Cargo.toml` 确认项目锁定版本。

## 目录与职责

GUI 必须按业务领域组织，并让 Slint 与 Rust 目录相互对应：

```text
ui/
├─ <领域>/<界面>.slint
├─ components/
└─ theme/

eli-cli/src/ui/
├─ <领域>/<界面>.rs
└─ window.rs
```

例如插件加载界面固定对应：

- `ui/plugin/load.slint`：声明 `PluginLoadWindow`、属性、回调和布局。
- `eli-cli/src/ui/plugin/load.rs`：保存业务状态，绑定回调并调用 `eli-lib`。
- `eli-cli/src/command/plugin.rs`：只解析参数并转发到 `crate::ui::plugin::load::run`。

遵守以下边界：

- 不在 `eli-cli/src/command/` 中嵌入 Slint、窗口状态、后台任务或原生窗口代码。
- 不创建 `ui/slintcn/` 或 `eli-cli/ui/`；通用组件放在 `ui/components/`，主题放在 `ui/theme/`。
- 组件专用 SVG、动画帧等资源放在该组件目录内，不散落到 `ui/` 根目录。
- Slint import 使用相对当前文件的路径，不依赖进程工作目录。
- 需要由 Rust 实例化的新顶层界面必须同步加入 `eli-cli/build.rs` 的 Slint 编译入口，并在对应 Rust 模块使用生成类型；不能假定新增文件会自动生成绑定。
- Windows 专用 GUI 模块继续通过条件编译隔离，Linux 和 macOS 构建不得解析 Win32 依赖。

## 窗口

普通 Eli 小窗口继承 `ui/components/window/window.slint` 中的 `EliWindow`。新建对话框前读取并按需改写 [基础对话框模板](references/gui-dialog-template.slint)。

`EliWindow` 统一负责窗口表面、边框、圆角、标题栏、关闭按钮和 `close.svg`；业务页面不得重复绘制这些元素。页面只处理自己的内容，并将 `close-requested` 映射到取消或关闭行为。

Slint 无法完成的 Windows 原生样式、圆角区域和 `WM_NCHITTEST` 统一放在 `eli-cli/src/ui/window.rs`。通过 `WindowAdapter` 将 `EliWindow` 的标题栏高度、左右排除区和圆角参数传给 Rust，禁止在业务控制器中再次硬编码同一组尺寸。只有标题栏中未被控件占用的空白区域可以拖动。

## 通用组件

优先复用 `ui/components/` 和 `ui/theme/tokens.slint`：

- 进行中状态使用 `ui/components/spin/spin.slint` 的 `Spin`。业务页面只设置真实的 `running` 状态和必要尺寸，不得复制帧索引、计时器或逐帧图片选择代码。
- Spin 的预渲染帧必须保留在 `ui/components/spin/frames/`，以兼容 Windows PE 软件渲染器；不要改成依赖图片旋转的实现。
- 成功使用 `Tokens.color-affirm`，失败使用 `Tokens.color-destructive`，进行中使用 `Tokens.color-progress`。
- 颜色、边框、圆角、间距和字体使用 `Tokens`。新增语义颜色先进入 palette/token，再由组件引用。
- 明暗主题由 `Theme.mode` / 系统 `Palette.color-scheme` 驱动；组件不得写死只适用于单一主题的表面或文字色。
- 关键状态不用 Emoji、Unicode 勾号或依赖目标字体的字形；使用可着色 SVG 或基础图形。

只有确有跨界面复用价值时才新增通用组件。业务专用结构留在 `ui/<领域>/`，不要把完整页面流程塞进组件目录。

## 状态、布局与并发

- Rust 是业务状态的唯一来源。Slint 只呈现数据、运行动画并转发用户操作，不伪造加载进度或结果。
- 后台线程只能通过 `slint::invoke_from_event_loop` 更新 UI。
- 并行任务的可见状态必须对应真实的开始、完成和失败事件；单个任务失败不得篡改其他任务状态。
- 普通布局使用 `VerticalLayout`、`HorizontalLayout`、`GridLayout` 和 stretch；`x/y` 仅用于标题栏、自绘图形和浮层等确需绝对定位的区域。
- 长列表使用 `ListView` 或有明确可视高度的滚动容器，并为滚动条和行尾状态留出空间。
- Tooltip、错误详情等浮层放在窗口根层最后绘制，不能依赖列表行的绘制顺序；锚点必须随滚动偏移同步。

## 验证

修改后按风险完成以下验证：

1. 使用与项目版本一致的 `slint-viewer --check` 检查受影响的入口和新组件。
2. 使用软件渲染器截取浅色、深色以及长文本/多行数据场景；必须实际查看截图，不能只看编译结果。
3. 运行相关 Rust 单元测试、`cargo fmt --all -- --check`、无默认特性检查和 Clippy。
4. 业务行为变化时更新 `tests/e2e/`；CI 仅负责编排，并确保 `ui/**` 变更会触发三平台构建。
5. 涉及 Windows PE 的窗口、滚动、浮层、动画、主题或输入时，读取 `lcr` skill，在目标机验证文字裁切、对齐、明暗主题、Spin、浮层、按钮点击、窗口拖动和圆角边界。本机桌面截图不能代替 PE 验收。
