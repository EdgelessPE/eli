---
name: eli-gui
description: 开发、修改或审查 Eli 的 Slint GUI，特别是 Windows PE 下的小型对话框、状态反馈、主题、浮层和实机验收。
---

# Eli GUI

此 skill 补充项目中的 Slint GUI 实践。它不替代通用 Slint 文档：涉及 `.slint` 代码、布局、主题、事件、浮层或 Rust 互操作时，先读取可用的 Slint skill，并以项目锁定的 Slint 版本为准。当前 `eli-cli` 使用 Slint 1.17。

## 适用范围

- Eli 的 Slint 窗口、对话框和可复用组件；尤其是 Windows PE 上运行的 GUI。
- 本 skill 不定义某个具体命令的文案、按钮数量、业务流程、并行度或自动关闭时机。这些由对应功能需求决定。
- 平台特定的原生窗口处理必须用条件编译隔离；不要把 Windows API 适配误当作 Slint 的跨平台能力。

## 开始前

1. 新建小型操作对话框时，先阅读 [基础对话框模板](references/gui-dialog-template.slint)。复制其结构并替换业务状态和文案，不要把插件加载器的流程直接复制到新界面。
2. 优先复用 `ui/slintcn/theme/tokens.slint`、`ui/slintcn/components/` 和 `eli-cli/ui/` 中已有组件与 SVG 资源；只有确有跨界面复用价值时才新增组件或图标。
3. 需要自绘 Windows PE 标题栏时，令 Rust 宿主侧的窗口区域、原生样式和命中测试与 Slint 中的标题栏尺寸同步。标题栏仅有未被控件占用的空白区可拖动；关闭按钮、其他控件和内容区必须保持可点击。

## 状态、视觉与主题

- 状态表达使用一致的图形语义：成功使用绿色圆形标记（`Tokens.color-affirm`），失败使用红色圆形标记（`Tokens.color-destructive`），进行中使用蓝色连续圆环。等待或无需强调的状态保持安静，不滥用图标。
- 关键状态不用 Emoji、Unicode 勾号或依赖目标字体的字形。优先使用可着色 SVG 或基础图形；若软件渲染器不能可靠旋转图片，使用预渲染的 SVG 动画帧。
- 颜色、边框、圆角、间距和字体优先使用 `Tokens`。新增蓝色进行中状态时先增加主题 token，不能在各页面散落硬编码颜色。
- 明暗主题必须由 `Theme.mode` / `Palette.color-scheme` 自动驱动。新增组件不得只实现浅色，也不得绕过 token 写固定的浅色表面与文字颜色。
- 使用 4px、8px、12px、16px、24px、32px 的统一间距尺度。自定义交互元素必须有指针、悬浮和按下反馈，并使用短时过渡。

## 状态、数据与交互

- Rust 是业务状态的唯一来源。Slint 用 `export global`、属性与回调呈现数据和转发操作；不要在界面中伪造加载进度、成功或失败。
- 后台任务只能通过 `slint::invoke_from_event_loop` 更新 UI。并行任务的界面状态必须对应真实的开始、完成和失败事件。
- 普通布局使用 `VerticalLayout`、`HorizontalLayout`、`GridLayout` 和 stretch；`x/y` 只用于标题栏、自绘图形和浮层等绝对定位场景。
- 长列表使用 `ListView`，或为短列表提供固定可视高度的滚动容器。为滚动条、行尾状态和操作保留空间，避免与文本重叠。
- Tooltip、错误详情和其他非菜单浮层作为窗口根层中最后绘制的元素；它们不能依赖某一列表行的绘制顺序，也不能靠下推原布局避免遮挡。锚点和悬浮命中区必须随滚动偏移同步，长内容应换行并可在浮层内部滚动。

## Windows PE 兼容与验收

- Windows PE 的软件渲染、字体、输入命中和窗口样式是独立目标环境；本机桌面渲染不能代替验收。
- 修改 `.slint` 后，先完成与当前版本匹配的编译检查和截图检查；支持时可用 `slint-viewer --check` 与 `slint-viewer --screenshot` 快速验证布局与两个主题。
- 涉及 Windows PE 的窗口、滚动、浮层、动画、主题或输入行为时，读取 `lcr` skill，在目标机实际运行、截图并验证交互。至少检查：文字裁切、长内容、对齐、主题、浮层遮挡、按钮点击、窗口拖动与圆角边界。
- 业务行为变化同时补充 Rust 单元测试；GUI 验证不能只依赖编译成功。
