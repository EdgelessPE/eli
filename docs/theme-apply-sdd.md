# `eli theme apply` 软件设计文档（SDD）

## 1. 文档状态

- 状态：待评审（命令边界、ELS 与 ESC 策略已确认）
- 目标命令：`eli theme apply`
- 实现范围：仅 Windows PE 运行时
- 本文档只定义设计，不包含实现代码

## 2. 背景与现状

`eli theme apply` 用于替代旧版 `setTheme.cmd` 的“应用到当前 PE 会话”能力。命令接收一个主题包、资源包或壁纸文件，根据扩展名自动选择处理逻辑，不再要求调用方额外传入 `eth`、`eis`、`ems` 等类型参数。

本设计参考了以下材料：

- 已解密旧实现：`D:\Download\setTheme.cmd`，SHA-256 为 `59F071F7ADDB4C9E40ED4E51D38EBE0623D4B837D9AC6816EFDB4693F595FA03`。
- 原版周边脚本：`D:\Desktop\Projects\EdgelessPE\wimbuilder-component\_vendor\File_Project\Program Files\Edgeless\theme_processer`。
- PE 启动集成：`D:\Desktop\Projects\EdgelessPE\wimbuilder-component\_vendor\Files_pecmd\Pecmd.ini`。
- 示例主题：`D:\Download\FirPE Experience_1.1.0.0_Cno.eth`，SHA-256 为 `DD6E089B975F9283EFAE6A75FD43384AB9B656806FA33189FBD6A40BAE537848`。
- [Edgeless 主题包规范](https://wiki.edgeless.top/v2/develop/theme.html)。
- [Edgeless 3.x `Pecmd.ini` 原版启动流程](https://github.com/EdgelessPE/Edgeless/blob/master/PE_Core_Version_3/Windows/System32/Pecmd.ini)。
- [鼠标刷新问题的前期讨论记录](https://chatgpt.com/share/6aa68cb2-e468-83ea-aa7d-5c0a08e001f5)；该会话只作为问题线索，Win32 行为以 Microsoft 文档为准。
- [Microsoft `SystemParametersInfoW` 文档](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfow)。
- [Microsoft Shell Link 文档](https://learn.microsoft.com/en-us/windows/win32/shell/links)。
- [Microsoft Known Folders 文档](https://learn.microsoft.com/en-us/windows/win32/shell/known-folders)。
- [Microsoft `SHChangeNotify` 文档](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shchangenotify)。
- [Microsoft `RegFlushKey` 文档](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regflushkey)。

示例 `.eth` 是一个 7z 归档，根目录包含：

| 条目 | 实际存在 | 说明 |
| --- | --- | --- |
| `IconPack.eis` | 是 | 包含 `shortcut/*.ico` |
| `Intro.txt` | 是 | GB 系编码的主题介绍 |
| `LoadScreen.els` | 是 | 本样包只包含 `load0.jpg` |
| `MouseStyle.ems` | 是 | 包含 15 类 `aero_*.cur/.ani` |
| `StartIsBackConfig.esc` | 是 | 本样包为 UTF-16 PECMD/WCS 脚本 |
| `SystemIconPack.ess` | 是 | 包含 `imageres.dll` 和 `imagesp1.dll` |
| `WallPaper.jpg` | 是 | JPEG 壁纸 |

旧实现同时承担“打开资源管理器 UI”“预览”“设为默认”和“应用”多种职责，并通过 `X:\Users\Theme\Path\*.txt`、`RunMSTip`、`DelayRefresh`、`NoESSTip` 等临时文件在脚本之间传参或协调时序。`theme apply` 只接管应用动作，不保留这些临时文件协议。

### 2.1 原版调用链和时序

原版各文件的职责不能只从 `setTheme.cmd` 单独推断，完整调用链如下：

```text
文件关联
  → processTheme.cmd：识别扩展名、解压 eth、打开对应 WCS 管理界面
      → proc*.wcs：让用户选择预览或设为默认
          → setTheme.cmd：应用到当前 PE 会话
          → instTheme.cmd：写入启动盘 Default/wp.jpg，供以后启动使用
```

`Pecmd.ini` 还包含两条启动时路径：

- Shell 启动前调用 `setTheme.cmd autoESS`，只处理默认 `SystemIconPack.ess`。
- 启动后段调用 `setTheme.cmd auto`，处理默认壁纸、ESC、EMS 和 EIS，跳过 ESS 与 ELS，随后由 PECMD 统一重启 Explorer。
- `EdgelessMSConfirm` 根据 `RunMSTip` 打开鼠标控制面板并模拟确认键；这是旧版 EMS 刷新的补救路径。
- `EdgelessExit` 在插件脚本和内置快捷方式创建完成后再次运行 `setDesktopIcon.exe`；因此 EIS 不能只在主题应用瞬间扫描一次桌面。

### 2.2 原版自检行为

除 `autoESS` 外，已解密 `setTheme.cmd` 会执行两级自检：

1. 查询 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OEMInformation\Manufacturer`，要求值为 `Edgeless`。
2. 在 `systemcpl.dll.mun` 或 `systemcpl.dll` 中搜索 `Edgeless` 字样。这两个文件承载 Windows“系统/系统属性”控制面板的代码或本地化资源，旧镜像会在其中显示 OEM 品牌。

该检查用于阻止脚本在非 Edgeless 环境运行。`theme apply` 没有对应旧版 `autoESS` 的内部启动特例，因此首版必须保留这道门禁，但不能让主题命令自行复制注册表和二进制文本搜索逻辑。统一依赖管理模块新增 `EdgelessRuntime` 环境能力：先确认 `WindowsPE`，再按原版兼容规则检查 OEM `Manufacturer=Edgeless`，并确认 `systemcpl.dll.mun` 或回退的 `systemcpl.dll` 中包含 `Edgeless`。任一步失败都必须在副作用前返回明确的“不属于受支持 Edgeless PE”错误。

### 2.3 ELS 在原版中的真实语义

原版 `setTheme.cmd` 并未把 ELS 作为可在当前会话中即时生效的主题组件：

- `setTheme.cmd els` 只将 `load0.jpg..load2.jpg` 解压到预览目录，实际展示由 `procLoadScreen.wcs` 的 `LOGO` 预览流程完成。
- `setTheme.cmd eth` 和 `setTheme.cmd auto` 都跳过 ELS。
- 持久化由 `instTheme.cmd` 完成，它把 ELS 解压到启动盘的 `Edgeless\Default\LoadScreen`，留给下一次 `Pecmd.ini` 启动流程读取。

因此新 `theme apply` 对 ELS 只报告迁移警告并返回 `Skipped`。旧格式转换及写入启动盘属于未来 `eli theme store` 的职责。

### 2.4 参考材料中的关键观察

为避免后续实现只参考单个批处理文件，下面记录本设计从用户提供材料中采用的事实：

| 参考材料 | 观察到的行为 | 对本设计的约束 |
| --- | --- | --- |
| `processTheme.cmd` | 通过扩展名选择 `proc*.wcs`，`.eth` 会先解包后进入主题管理界面 | CLI 改为可靠的扩展名路由，但不复刻 UI 和 `Path/*.txt` 传参 |
| 解密版 `setTheme.cmd` | `eth/auto` 跳过 ELS；EIS 复制资源后启动 `setDesktopIcon.exe`；EMS 写注册表后通过 `RunMSTip` 请求控制面板补刷新 | ELS 在 `apply` 中跳过；EIS/EMS 的最终效果由 Rust/Win32 直接实现 |
| `procLoadScreen.wcs` | `setTheme.cmd els` 只为 `LOGO` 预览解包三张图片 | ELS 预览不属于 `theme apply` |
| `instTheme.cmd` | ELS 写入启动盘 `Edgeless\Default\LoadScreen`，壁纸写入启动盘默认位置 | 持久化统一留给未来 `theme store` |
| 原版 `Pecmd.ini` | 在三个启动节点以 `LS_INDEX=0/1/2` 调用 `Edgeless_LoadScreen`；文件缺失时不切换画面 | Store 迁移保留帧顺序和“缺帧维持上一画面”语义 |
| 原版 `Pecmd.ini` | `setTheme.cmd auto` 后仍会创建系统快捷方式，`EdgelessExit` 最后再次运行 `setDesktopIcon.exe` | 启动流程完成全部快捷方式生产后，由未来 Loader 的收尾点调用共享图标刷新服务；独立 `plugin load` 不调用 |
| FirPE 示例 `.eth` | 六类组件齐全，但 ELS 只有 `load0.jpg`，ESC 为 UTF-16，EMS 含 15 类光标文件 | 验收必须覆盖单帧 ELS 跳过、UTF-16 ESC 透传和 15 槽 EMS |
| 鼠标刷新讨论记录 | 旧调用 `call_dll('user32.dll','SystemParametersInfoW',0x57,0,0,2)` 中 `0x57` 即 `SPI_SETCURSORS`、`2` 即 `SPIF_SENDCHANGE`；风险点是调用前注册表配置是否已完整就绪 | 使用同步写入、关闭句柄、逐项读回，再以具名常量调用 Win32 API；不靠固定延时 |

主题规范网页用于确定资源扩展名、标准组件名和归档形态；Win32 API 的调用约束以 Microsoft 文档为准。共享会话中的推断只作为定位线索，不作为平台行为的唯一依据。

### 2.5 兼容性审查基线

本节只约束符合原版主题规范且能够成功应用的输入：新实现必须保留组件落序、注册表和文件映射、EIS 调用时机以及 Explorer 刷新次数/位置。在这些成功路径中，仅允许以下已经由用户明确确认的差异：

- 由 `theme apply <PACKAGE>` 自动按扩展名路由，不再使用类型参数和 `Path/*.txt` 临时传参。
- `apply` 只处理当前 PE 会话，启动盘持久化留给未来 `theme store`。
- ELS 在 `apply` 中告警并跳过，未来 Store 才负责迁移。
- EMS 用同步注册表写入、关闭句柄、读回和 `SPI_SETCURSORS` 代替控制面板模拟确认。
- EIS 由 Eli 直接修改 `.lnk`，不调用 `setDesktopIcon.exe`。
- ESC 继续交给 PECMD 解释。
- 独立 `plugin load` 后不执行图标替换；只保留 EIS 应用瞬间和启动流程收尾两个原版调用时机。

错误路径采用单独策略：目录重构、跨平台条件编译、并发锁、原子写入、回滚和归档安全校验可以让损坏、缺失或恶意输入更早失败，但不得改变上述合法主题成功应用后的可观察结果。本设计不要求复现旧脚本在命令失败、文件缺失或进程崩溃时留下半成品的偶然行为；这不是对成功路径兼容性的额外豁免。

## 3. 目标

1. 通过一个明确的路径参数自动识别并应用 `.eth`、`.eis`、`.ems`、`.esc`、`.ess`、`.els` 和 `.jpg`。
2. 在任何副作用前验证当前环境为 `WindowsPE` 且满足原版 `EdgelessRuntime` 门禁，并通过条件编译保证 Windows、Linux、macOS 均可构建。
3. 统一使用依赖管理模块解析和探测 `7z.exe`、`pecmd.exe`，不硬编码安装路径。
4. 对组合主题执行完整预检，尽量在修改系统前发现损坏包、危险归档条目或缺失资源。
5. 直接在 Rust 中实现 EIS 的桌面快捷方式图标替换，不再调用 `setDesktopIcon.exe`。
6. 修复 EMS 应用时序：同步写入当前方案、读回验证，然后调用 `SPI_SETCURSORS`，不通过打开控制面板和模拟按键完成刷新。
7. 识别旧式 ELS，打印迁移警告并明确跳过；`apply` 不解压、不转换、不发布启动画面资源。
8. 串行化同进程和多进程主题应用，避免主题组件交叉覆盖。
9. 对可回滚的资源采用事务式发布；不可完全回滚时给出准确的部分成功结果。

## 4. 非目标

首版不包含：

- 主题包预览、混搭选择或图形化主题管理器。
- 将主题保存为启动盘默认主题。`theme apply` 只影响当前 PE 会话，不写入 `Edgeless\Default` 或 `Edgeless\wp.jpg`；该职责留给未来的 `eli theme store`。
- 执行 `.eth` 中的 `Intro.wcs`。它属于“打开主题包时的交互控制面板”，不是应用载荷。
- 用 Rust 重新实现 PECMD/WCS 解释器；`.esc` 仍由经过依赖管理模块验证的 `pecmd.exe` 执行。
- 从文件内容猜测一个没有受支持扩展名的包类型。
- 支持 `.jpeg`、`.png` 作为独立壁纸入口；首版保持旧规范的 `.jpg` 接口。
- 对正在运行的 LoadScreen 强制播放预览动画。
- 跨整个 `.eth` 提供绝对原子性。`.esc` 可以执行注册表脚本，无法可靠撤销其全部副作用。

## 5. CLI 设计

```text
eli theme apply <PACKAGE>
```

示例：

```text
eli theme apply "D:\Themes\FirPE Experience_1.1.0.0_Cno.eth"
eli theme apply "D:\Themes\IconPack.eis"
eli theme apply "D:\Themes\MouseStyle.ems"
eli theme apply "D:\Themes\WallPaper.jpg"
```

首版只接受一个普通文件，不接受目录、通配符或多个输入。主题会修改同一组全局状态，批量并发应用没有稳定语义；需要应用多个资源时由调用方按顺序多次调用，或将资源组成 `.eth`。

扩展名使用 ASCII 大小写不敏感比较。未知扩展名在读取文件内容、解析依赖和创建临时目录前返回 `InvalidInput`。合法扩展名与处理器的对应关系如下：

| 扩展名 | 类型 | 处理器 |
| --- | --- | --- |
| `.eth` | 组合主题包 | 解析根目录组件并生成应用计划 |
| `.eis` | 图标资源包 | 发布图标文件并更新桌面 `.lnk` |
| `.ems` | 鼠标样式包 | 发布光标文件、写注册表、刷新系统光标 |
| `.esc` | 开始菜单配置 | 通过 PECMD 同步加载脚本 |
| `.ess` | 系统图标包 | 事务替换系统资源 DLL 并刷新 Shell |
| `.els` | 旧 LoadScreen 包 | 打印迁移警告并返回 `Skipped`，不执行其他操作 |
| `.jpg` | 壁纸 | 发布会话壁纸并调用 PECMD `WALL` |

成功时输出逐组件结果和一行总计；警告写入标准错误流。任何错误都必须包含外层源路径、组件类型和失败阶段。退出码保持简单：全部成功为 `0`，发生任何失败为非零。

`--bootdisk` 对本命令没有作用，因为本命令不修改启动盘。未来的持久化功能设计为独立的 `eli theme store`，并遵守多启动盘候选时必须显式选择目标的规则。

## 6. 总体架构

命令采用“识别 → 预检 → 准备 → 提交 → 刷新”的五阶段模型：

```text
CLI 薄转发层
    ↓
类型识别与环境检查
    ↓
归档/文件完整预检
    ↓
PreparedTheme（无系统副作用）
    ↓
获取主题全局独占锁
    ↓
按计划提交各组件
    ↓
按原版时机执行组件刷新和必要的 Explorer 重启
    ↓
ApplySummary
```

### 6.1 公开入口

`eli-lib` 只提供一个公开的主题应用入口，输入为 `Ctx` 和源路径，输出为结构化 `ApplySummary`。CLI 不解析归档、不直接操作注册表、不调用 Win32 API，也不拼装外部程序命令。

`ApplySummary` 至少包含：

- 识别出的外层类型。
- 每个组件的 `Applied`、`Skipped` 或 `Failed` 状态。
- 迁移和兼容性警告。
- 是否通知或重启了 Explorer。
- `.els` 是否因当前会话无法使用而被跳过。
- `.eis` 更新、未匹配和失败的快捷方式数量。

### 6.2 建议目录结构

```text
eli-cli/src/command/theme.rs

eli-lib/src/command/theme/
  mod.rs
  apply.rs
  apply/
    archive.rs
    transaction.rs
    wallpaper.rs
    eis.rs
    ems.rs
    esc.rs
    ess.rs
    windows.rs

eli-lib/src/shell/
  mod.rs
  desktop_icon.rs
  desktop_icon/
    windows.rs
```

该结构遵守“父命令目录 + 同名子命令文件”的项目约束：`theme/mod.rs` 只公开并组织主题子命令，`theme/apply.rs` 是 `apply` 子命令的公开编排器；它通过显式 `#[path = "apply/..."]` 或等价的私有模块声明使用 `theme/apply/` 下的实现细节。未来新增 `theme store` 时再增加 `theme/store.rs` 和按需的 `theme/store/` 私有辅助模块，不提前把持久化职责混进 `apply`。

`theme/apply/eis.rs` 负责 EIS 的发布、事务和刷新策略，跨命令复用的快捷方式枚举/改写能力放在 `eli-lib/src/shell/desktop_icon.rs`，其 Win32 后端再由 `shell/desktop_icon/windows.rs` 条件编译隔离。这样未来 Loader 无需依赖 `theme apply` 的私有模块。`theme/apply/windows.rs` 只封装其余主题专用的注册表、系统参数、进程和文件安全描述符操作。非 Windows 构建保留统一接口，在环境检查阶段返回明确的 `WindowsPE` 不支持错误。

### 6.3 运行时路径

不得写死 `X:`。通过 `%SystemRoot%` 的父目录解析当前 PE 系统卷，并由一个 `ThemeRuntimePaths` 结构集中生成路径。建议的会话目录为：

```text
<PE 系统卷>\Users\Theme\eli\
  staging\<随机事务 ID>\
  backup\
  locks\theme-apply.lock
  locks\desktop-icon-refresh.lock
```

Edgeless 兼容图标根目录为：

```text
<PE 系统卷>\Users\Icon
```

临时事务目录不得成为组件之间的传参机制；它只服务于本次进程内的准备、备份和回滚，并在结束时清理。

## 7. 环境和依赖检查

执行顺序固定为：

1. 校验扩展名和输入是普通文件。
2. 调用 `DependencyManager::require_environment(RuntimeEnvironment::WindowsPE)`。
3. 调用依赖管理模块的 `require_capability(RuntimeCapability::EdgelessRuntime)`，执行原版品牌门禁。
4. 根据外层类型解析最低必要程序依赖。
5. 完成依赖探测和全部可执行前置检查。
6. 才允许创建工作目录或修改系统状态。

依赖表：

| 场景 | 依赖 |
| --- | --- |
| 所有 `theme apply` 输入 | `WindowsPE` + `EdgelessRuntime` |
| `.eth/.eis/.ems/.ess` | `ProgramDependency::SevenZip` |
| `.esc/.jpg` | `ProgramDependency::Pecmd` |
| 含 `.esc` 或 `WallPaper.jpg` 的 `.eth` | `SevenZip` 和 `Pecmd` |
| `.els` | 无；只检查扩展名、打印警告并跳过 |

`.eth` 必须先通过 7-Zip 列出根目录，才能根据 ESC 或壁纸组件确定是否还需要 PECMD。`7z.exe` 和 `pecmd.exe` 都从 `PATH` 解析、执行中台定义的测试参数并返回绝对路径。

`EdgelessRuntime` 属于运行环境能力，不属于外部程序依赖。其首版兼容规则固定为：

1. 读取 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OEMInformation` 的 `Manufacturer`，值必须为 `Edgeless`。
2. 优先检查 `%SystemRoot%\SystemResources\systemcpl.dll.mun`；文件不存在或不含目标文本时，回退检查 `%SystemRoot%\System32\systemcpl.dll`。
3. 至少一个候选文件包含 `Edgeless` 才通过。

该能力集中实现在依赖管理模块中并由主题命令声明，既保留原版支持边界，也避免业务处理器散落重复检查。未来若 Edgeless 镜像品牌方式变化，只修改能力探测规则和对应测试。

## 8. 归档安全与格式校验

所有由 `theme apply` 实际打开的 7z 格式包都采用相同的安全流程；被跳过的 ELS 不进入该流程：

1. 使用 `7z l -slt -sccUTF-8` 列出条目，检查退出状态。
2. 在解压前拒绝绝对路径、盘符路径、UNC 路径、`..`、NTFS ADS、设备名和空文件名。
3. 拒绝符号链接、硬链接、junction、reparse point 和其他非普通文件/目录条目。
4. 以 Windows 不区分大小写语义检测同名碰撞；不能仅使用 Rust 的 ASCII 比较处理任意 Unicode 文件名。
5. 拒绝加密包、损坏包和超过资源限制的包。
6. 只将计划使用的白名单条目解压到唯一 staging 目录。
7. 解压后重新遍历并确认每个解析路径仍位于 staging 根目录内。

限制值集中为常量并纳入测试。首版建议：

| 包类型 | 最大条目数 | 最大展开总量 |
| --- | ---: | ---: |
| `.eth` | 64 | 1 GiB |
| `.eis` | 4096 | 512 MiB |
| `.ems` | 64 | 128 MiB |
| `.ess` | 8 | 256 MiB |
| `.els` | `apply` 不打开归档，无展开上限 |

`.eth` 根目录只识别规范组件名。组件名采用 ASCII 大小写不敏感匹配，但同一规范名出现多个大小写变体时必须拒绝。`Intro.txt`、`Intro.wcs` 和 `Intro/` 可存在但不进入应用计划；其他未知根条目打印警告并忽略。嵌套资源包仍要按自身规则独立预检。

组合主题必须在任何系统副作用之前完成所有**可应用组件**的预检，包括嵌套 EIS/EMS/ESS 归档、图片解码、DLL PE 头、鼠标文件集合和 PECMD 依赖。嵌套 ELS 只在外层归档清单中识别并记录为 `Skipped`，不解压也不校验其内部图片。这样既避免因为最后一个可应用组件损坏而留下半套主题，也不为本命令不会消费的 ELS 引入无意义工作。

## 9. `.eth` 组合主题编排

`.eth` 中所有组件均为可选。没有可识别组件的包按原版行为作为空操作成功，汇总为 `0 Applied / 0 Skipped`；未知根条目仍输出警告。若包内只有 `LoadScreen.els`，命令正常完成并汇总为 `0 Applied / 1 Skipped`，同时输出迁移警告。规范组件如下：

- `WallPaper.jpg`
- `LoadScreen.els`
- `IconPack.eis`
- `MouseStyle.ems`
- `StartIsBackConfig.esc`
- `SystemIconPack.ess`

提交顺序固定为：

1. 应用壁纸。
2. 在计划结果中记录 ELS 为 `Skipped` 并输出一次警告；该步骤无系统副作用。
3. 执行 ESC 开始菜单配置；在 `.eth` 中只记录“末尾重启 Explorer”请求，此时不重启。
4. 应用 ESS 系统图标，并按原版时机完成 ESS 自身的缓存刷新和 Explorer 重启。
5. 应用 EMS 鼠标样式。
6. 发布 EIS 图标并立即刷新当时已有的快捷方式。
7. 若第 3 步实际执行过 ESC，则在 EIS 完成后重启一次 Explorer；没有 ESC 时不追加该次重启。

该顺序直接对应解密版 `setTheme.cmd` 从 `setWallPaper` 到 `setIconPack` 的落序。若同时存在 ESS 和 ESC，Explorer 可能发生两次重启：一次属于 ESS 自身刷新，一次属于组合主题末尾的 ESC 刷新。首版保留这一可观察时序，不擅自合并。准备阶段允许并行做纯读取、解码或校验，但首版默认串行准备；提交阶段始终串行。

某个组件提交失败时，先在该组件自身事务范围内尽量回滚，然后继续执行后续组件，保持原版组合主题的 best-effort 落序。命令最终汇总所有 `Applied`、`Skipped` 和 `Failed`，只要存在失败就返回非零；不跨越已经成功执行的 `.esc` 做伪事务。错误信息必须明确说明主题可能已部分应用。

## 10. `.ems` 鼠标样式

### 10.1 文件映射

归档根目录必须提供以下 15 个逻辑槽位，每个槽位恰好选择一个 `.ani` 或 `.cur` 文件。两种格式同时存在时优先 `.ani` 并打印警告；两者都不存在时预检失败。

| 文件基名 | 注册表当前值 | 方案槽位 |
| --- | --- | ---: |
| `aero_arrow` | `Arrow` | 1 |
| `aero_helpsel` | `Help` | 2 |
| `aero_working` | `AppStarting` | 3 |
| `aero_busy` | `Wait` | 4 |
| `aero_cross` | `Crosshair` | 5 |
| `aero_beam` | `IBeam` | 6 |
| `aero_pen` | `NWPen` | 7 |
| `aero_unavail` | `No` | 8 |
| `aero_ns` | `SizeNS` | 9 |
| `aero_ew` | `SizeWE` | 10 |
| `aero_nwse` | `SizeNWSE` | 11 |
| `aero_nesw` | `SizeNESW` | 12 |
| `aero_move` | `SizeAll` | 13 |
| `aero_up` | `UpArrow` | 14 |
| `aero_link` | `Hand` | 15 |

首版只处理原版定义的 15 个槽位。方案字符串仍保留第 16、17 个空字段，即以 `,,` 结束；不写 `Person`、`Pin` 当前值。包内出现 `aero_person` 或 `aero_pin` 时作为未知文件告警并忽略，避免未经确认扩展旧格式语义。

每个选中的文件先用 `LoadCursorFromFileW` 验证可加载。普通 `theme apply` 按原版生成本地时间串 `DDHHMMSS`，同时作为方案名和目录名：

```text
<SystemRoot>\Cursors\13094527\
```

其中示例表示 13 日 09:45:27。原版在同名目录已存在时进入交互式改名；CLI 首版不增加交互提示，改为在任何注册表副作用前返回 `AlreadyExists`，让调用方稍后重试。这只改变极少见冲突的错误路径，不改变正常成功路径的方案名、目录名和注册表映射。未来启动默认主题由 `theme store`/Loader 使用时，保留原版固定方案名 `Edgeless_Default`。

EMS 不接受子目录。除上述 15 个标准基名之外的普通文件只产生警告并被忽略；可执行文件、脚本和链接直接导致预检失败。

### 10.2 注册表提交屏障

不使用固定 `Sleep`，也不打开 `main.cpl` 模拟确认。正确提交顺序为：

1. 将新目录发布到最终位置。
2. 快照 `HKCU\Control Panel\Cursors` 中所有会改动的当前值、默认值和 `Schemes` 中目标方案值。
3. 以 `REG_EXPAND_SZ` 写入 15 个当前光标路径；路径统一使用 `%SystemRoot%`，不修改 `Person` 和 `Pin`。
4. 以 `REG_SZ` 写入 `Schemes\<方案名>` 的完整逗号分隔槽位字符串。
5. 将 `HKCU\Control Panel\Cursors` 默认值写为方案名。
6. 关闭写句柄后重新打开键，以相同数据类型逐项读回并与计划值比较。
7. 检查每个读回路径展开后仍指向已经发布的文件。
8. 只有全部读回一致后，调用 `SystemParametersInfoW(SPI_SETCURSORS, 0, NULL, SPIF_SENDCHANGE)`。
9. Win32 调用失败或读回不一致时恢复注册表快照和旧光标目录，再调用一次 `SPI_SETCURSORS` 恢复旧显示。

注册表写入对其他进程是立即可见的，因此不把 `RegFlushKey` 当作可见性屏障。Microsoft 文档明确指出它只用于强制持久化且会阻塞整个 hive；当前 PE 会话应用不需要这项昂贵操作。真正的屏障是同步 Win32 写入、关闭句柄和精确读回。

调用前还要记录当前进程的用户 SID 和会话 ID。若 Explorer 已存在但属于不同用户或不同会话，拒绝修改错误的 `HKCU` 并给出上下文不匹配错误。没有 Explorer 的启动早期允许写入当前用户 hive 和调用刷新，但 Loader 必须在创建 Shell 的同一用户上下文中调用本入口。

## 11. `.eis` 图标资源包

### 11.1 图标发布

EIS 内容递归覆盖到 Edgeless 会话图标根目录，而不是替换整个 `Users\Icon`：

```text
<PE 系统卷>\Users\Icon
```

发布过程使用文件级 journal：覆盖前复制旧文件，新文件记录为“本次创建”。任一复制、快捷方式保存或后续验证失败时，按逆序恢复旧文件并删除本次创建的文件。包内只接受目录和普通静态图片；`shortcut` 下只把能完整解码的 `.ico` 纳入快捷方式映射，其他静态图像可发布供 Edgeless 自带 UI 使用。EIS 中的 `.exe`、`.dll`、脚本及其他非图像文件必须拒绝，尤其不得接受或运行包内自带的 `setDesktopIcon.exe`。

### 11.2 桌面发现

不假定桌面固定为 `X:\Users\Default\Desktop`。Windows 实现通过 `SHGetKnownFolderPath` 解析：

- 当前用户 `FOLDERID_Desktop`。
- 公共桌面 `FOLDERID_PublicDesktop`。
- 若存在且与上述路径不同，再加入 PE 兼容目录 `<PE 系统卷>\Users\Default\Desktop`。

所有路径去重后，只枚举目录第一层的普通 `.lnk` 文件。首版不修改 `.url`，也不递归子目录。

### 11.3 匹配和修改

`shortcut/<快捷方式文件名去掉 .lnk>.ico` 与 `.lnk` 按 Windows 不区分大小写语义匹配。包内存在折叠后同名图标时预检失败，避免结果依赖归档顺序。

每个匹配项在单线程 COM STA 中执行：

1. `CoInitializeEx(COINIT_APARTMENTTHREADED)`。
2. 创建 `IShellLinkW`，通过 `IPersistFile::Load` 读取现有 `.lnk`。
3. 只调用 `IShellLinkW::SetIconLocation(<绝对 ico 路径>, 0)`，不改目标、参数、工作目录、描述或热键。
4. 通过 `IPersistFile::Save` 覆盖原快捷方式。
5. 重新加载并读回图标路径和索引，确认保存成功。

修改前保存 `.lnk` 原始字节用于回滚。单个链接损坏或被其他进程占用视为该 EIS 应用失败，执行完整 EIS 回滚，而不是静默跳过。没有对应 `.ico` 的快捷方式保持不变，并进入 `unmatched` 统计；没有对应 `.lnk` 的图标进入 `unused` 统计，但不算失败。

完成后对变更的快捷方式发送 `SHCNE_UPDATEITEM`，再发送一次带 flush 语义的 Shell 变更通知。纯 EIS 应用不应为了刷新图标强制终止 Explorer。

### 11.4 启动收尾阶段的刷新时机

旧版并非只在应用 EIS 时运行一次 `setDesktopIcon.exe`。本次检查原版代码只找到两个调用点：`setTheme.cmd` 应用 EIS 后的即时调用，以及 `Pecmd.ini` 的 `EdgelessExit` 启动收尾调用。后者位于启动期插件脚本和内置快捷方式创建流程之后，能覆盖**启动过程中加载**的插件所新建的快捷方式；没有找到 PE 已进入桌面后，单独热加载插件时专门再次运行该程序的调用点。

Rust 重构严格保留这项调用边界和时序语义，但不保留该二进制文件：

1. `theme apply` 应用 EIS 后立即刷新一次，处理当时已经存在的快捷方式。
2. 未来 Loader 在启动期插件、LocalBoost 和内置快捷方式生产者全部结束后执行一次同样的刷新；旧 `Pecmd.ini` 中 `link_system_folder.cmd` 之后的 `EdgelessExit` 可作为集成位置参考。
3. 独立执行的 `eli plugin load` 完成后不触发图标刷新，也不因 EIS 的存在追加桌面扫描，保持与原版一致。

`eli-lib` 应提供一个可复用的 `DesktopIconRefreshService` 内部能力。服务每次从稳定的 `<PE 系统卷>\Users\Icon\shortcut` 重新建立图标映射，再执行幂等桌面扫描，因此未来 Loader 不依赖 `theme apply` 的内存状态或临时文件。只有 `theme apply` 和未来 Loader 使用这项能力；`plugin load` 不接入，也不保留常驻轮询进程。

主题应用时的即时扫描属于 EIS 事务：任一匹配快捷方式保存失败会回滚 EIS；`theme apply` 结束时不再扫描。只有 Loader 启动收尾阶段的扫描属于最终一致性刷新：逐链接记录成功/失败，失败产生警告并进入汇总，不回滚启动期已经加载的插件。

桌面图标刷新另设同进程互斥量和跨进程 `desktop-icon-refresh.lock`，避免主题应用与 Loader 同时改写同一 `.lnk`。锁顺序固定为“主题全局锁 → 桌面图标刷新锁”；Loader 只获取桌面图标刷新锁，不反向获取主题锁，从而避免死锁。

## 12. `.els` 在 `apply` 中的处理与未来 `store` 契约

### 12.1 `theme apply` 行为

LoadScreen 是 PE 启动加载过程中展示的资源。用户能运行 `theme apply` 时，本次启动画面已经进入尾声或已经结束；把 ELS 转换成当前会话文件既不会复现旧版启动时序，也没有稳定的即时消费者。因此 `apply` 对 ELS 的行为固定为“识别、告警、跳过”。

外层 `.els` 在完成扩展名识别、`WindowsPE` 与 `EdgelessRuntime` 环境检查后直接返回 `Skipped`：不解析 `SevenZip` 依赖，不创建 staging，不打开归档，不转换图片，不写入启动盘或会话目录。嵌套 `LoadScreen.els` 只通过 `.eth` 的外层清单识别，同样不解压；同一个组件只输出一次警告。稳定消息至少包含以下关键语义，具体中英文措辞可由 CLI 本地化：

```text
warning: legacy LoadScreen.els is a startup resource and cannot affect the current PE session; skipped by `eli theme apply`; use `eli theme store` after LSBP migration support is available
```

若 `.eth` 同时含有其他资源，ELS 的 `Skipped` 不阻止其他组件应用，也不使整体退出码变为非零。若 `.eth` 只有 ELS，命令返回成功，汇总为 `0 Applied / 1 Skipped`。这与旧版 `setTheme.cmd eth/auto` 跳过 ELS 的实际行为一致。

### 12.2 未来 `theme store` 的迁移规则

本节只冻结兼容契约，**不属于本次 `theme apply` 的实现范围**。未来 `eli theme store` 负责把主题持久化到启动盘时，才解压并迁移 ELS：

- 根目录只接受 `load0.jpg`、`load1.jpg`、`load2.jpg`，至少一张、最多三张，并按内容验证为完整静态 JPEG。
- 固定映射为 `load0 → lsbp_0000.webp`、`load1 → lsbp_0500.webp`、`load2 → lsbp_1000.webp`。
- 只转换实际存在的旧帧，不用其他图片补齐任何缺失标记。缺 `load0` 时继续显示进入 LoadScreen 前的既有界面，缺 `load1/load2` 时维持上一张，与旧 `Pecmd.ini` 和 `procLoadScreen.wcs` 一致。
- 所有帧应用 EXIF 方向并规范化到同一尺寸，以质量 `90` 转换为 WebP，按标记升序写入根目录无外层文件夹的标准未压缩 tar；复用现有 LoadScreen 编解码能力。

旧 `Pecmd.ini` 是按三个启动节点调用 `Edgeless_LoadScreen`，而新 Loader 的进度来自动态任务权重，无法也不应把旧节点精确换算成固定百分比。`0/50/100` 映射保留的是三张图片的先后顺序和缺帧时维持既有画面的行为，而不是伪造旧脚本不存在的连续进度语义。

现有通用 LSBP 播放契约要求同时存在 `0000` 和 `1000`，与上述旧 ELS 缺帧语义冲突。未来 Store 在要启用迁移前，必须先为 Legacy ELS 增加“首帧出现前沿用既有画面、末帧之后保持最后一帧”的兼容播放模式；若该模式尚未实现，则拒绝迁移缺少 `load0` 或 `load2` 的 ELS，不能通过复制别的帧悄悄改变表现。

`theme store` 写入启动盘前必须执行启动盘发现：候选唯一时可使用该目标，存在多个候选且用户未通过全局 `--bootdisk` 显式指定时必须拒绝。迁移输出使用 Store 与 LoadScreen 共同定义的启动盘路径；不得复用或恢复本设计已删除的会话 `active.tar` 路径。

## 13. `.ess` 系统图标资源包

ESS 根目录必须恰好包含普通文件 `imageres.dll` 和 `imagesp1.dll`；目录、脚本、重复大小写条目和链接均拒绝。两个文件必须具有合法 PE/DLL 头，且 machine 类型与当前 PE 架构兼容。主题资源 DLL 不要求有效 Authenticode 签名，因为自定义资源通常会破坏签名。

提交步骤：

1. 将两个新 DLL 准备在系统卷 staging 中。
2. 快照当前 DLL 内容和安全描述符；会话内的“原始系统图标”备份只创建一次，后续主题不得覆盖它。
3. 通过 Win32 安全 API 临时取得最低必要写权限，不调用 `takeown.exe` 或 `icacls.exe`。
4. 保持 Explorer 当前状态，分别原子替换两个文件并立即回读大小、哈希和 PE 头；与原版一样先完成 DLL 替换，再处理 Shell。
5. 任一个失败时恢复两个旧文件及安全描述符。
6. 成功后恢复目标文件应有的安全描述符。
7. 调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST | SHCNF_FLUSH, NULL, NULL)`，使 Shell 丢弃关联和命名空间图标状态；这是等价替代原版 `ENVI @@DeskTopFresh=clearicon;1` 对桌面、“此电脑”和图标缓存的刷新请求。
8. 请求 ShellController 停止当前会话的 Explorer；没有 Explorer 时继续。
9. 在当前 Shell 用户的 `AppData\Local\Microsoft\Windows\Explorer` 缓存目录中删除所有普通 `*.db` 文件，与原版 `setImageRes.wcs` 的范围一致；不得递归、不得越出该精确目录。
10. 安全启动 Explorer 并等待桌面就绪，再返回编排器继续 EMS/EIS；该刷新不与 ESC 的末尾刷新合并。

若 Explorer 已停止，无论后续成功还是失败，都必须在 finally 路径恢复 Shell。命令不得遗留一个因主题失败而没有桌面的 PE 会话。

## 14. `.esc` 开始菜单配置

`.esc` 是 PECMD/WCS 脚本，保持规范兼容性并交由 PECMD 执行：

- 通过依赖管理模块得到 `pecmd.exe` 绝对路径。
- 直接构造 `pecmd.exe LOAD <绝对 esc 路径>` 参数，不经过 `cmd.exe /c`。
- 工作目录固定为 `%SystemRoot%\System32`，与旧应用路径一致。
- 同步等待退出，设置统一超时，捕获退出码和标准错误。
- 非零退出、超时或无法启动均为组件失败。
- 独立 `.esc` 成功后立即安全重启当前会话的 Explorer。
- `.eth` 中的 ESC 成功后只设置待处理标记，在 EIS 应用完成后重启 Explorer，与原版 `needKE` 时序一致。

`theme apply` 不执行 `Intro.wcs`。ESC 本身是规范允许的可执行配置载荷，无法在不完整实现 PECMD 的情况下证明其只含注册表命令；因此把“显式调用 apply”视为运行该主题配置的授权，并在错误汇总中标记 ESC 为不可自动回滚组件。

## 15. `.jpg` 壁纸

独立 `.jpg` 或 `.eth` 中的 `WallPaper.jpg` 必须先按内容解码为有效静态 JPEG。为保持原版 `setTheme.cmd` 的成功路径语义，壁纸仍交给 PECMD 的 `WALL` 命令应用，不用 `SystemParametersInfoW` 猜测 PECMD 的内部注册表和显示方式行为：

```text
pecmd.exe WALL <会话稳定路径的绝对 JPEG 路径>
```

PECMD 绝对路径由依赖管理模块提供，参数直接传给进程而不经过 `cmd.exe /c`，工作目录为 `%SystemRoot%\System32`。不能直接引用 `.eth` staging 中即将删除的图片，应先复制到会话稳定路径；替换前记录旧会话文件，PECMD 启动失败、超时或非零退出时恢复旧文件。成功后不额外重启 Explorer。

## 16. Shell 刷新策略

首版保留原版各组件的刷新位置，不跨组件合并 Explorer 重启：

| 场景 | 动作 |
| --- | --- |
| 已跳过的 ELS | 无 |
| 壁纸 | 调用 PECMD `WALL`，不额外重启 Explorer |
| EMS | 注册表读回后调用 `SPI_SETCURSORS`，不重启 Explorer |
| EIS | 修改匹配快捷方式并发送 Shell 变更通知，不重启 Explorer |
| 独立 ESC | PECMD 成功后立即重启 Explorer |
| `.eth` 中的 ESC | 延迟到 EIS 完成后重启 Explorer |
| ESS | 替换资源、清理目标缓存后立即重启 Explorer 并等待恢复 |

若 `.eth` 同时包含 ESS 和 ESC，保留 ESS 阶段和组合主题末尾各一次 Explorer 重启。编排器只处理当前会话、当前用户拥有的 Explorer，不按进程名杀死其他会话进程。每次重启都必须等待桌面窗口就绪并设置明确超时；超时返回“资源已提交但 Shell 恢复失败”的部分成功错误。`theme apply` 不在末尾追加第二次 EIS 扫描；另一次扫描只存在于启动流程的 Loader/`EdgelessExit` 收尾点。

## 17. 并发、锁和失败语义

主题资源共享 HKCU、系统 DLL、光标目录和桌面快捷方式，因此提交阶段没有安全并行空间。ELS 不提交资源，不参与锁内事务。

### 17.1 同进程并发

`eli-lib` 内部使用进程级 `Mutex` 包围整个“获取文件锁 → 提交 → 刷新”阶段。准备阶段可在锁外完成，但获得锁后必须重新验证所有目标快照，避免准备期间目标被其他操作修改。

### 17.2 多进程并发

在 `<PE 系统卷>\Users\Theme\eli\locks\theme-apply.lock` 上获取独占文件锁。锁由 OS 句柄生命周期释放，不通过“删除锁文件”判断陈旧状态。等待采用有界超时；超时返回 `WouldBlock` 并显示持锁目标，不擅自打断另一个应用进程。

### 17.3 事务边界

- EIS、EMS、ESS、壁纸各自有独立 journal 和回滚；ELS 没有副作用，不创建 journal。
- ESC 不能承诺通用回滚。
- `.eth` 先完整预检、后按原版顺序串行提交；单个组件运行期失败不阻止后续组件，并在最终统一汇总。
- Shell 刷新在统一 finally 路径执行，确保 Explorer 不因中途错误永久退出。
- staging 写入使用“临时文件 + flush + 同卷 rename/replace”，不把半成品暴露为活动资源。
- 进程异常退出后，下次运行根据事务清单恢复未完成的文件级提交；清单不记录或回放 PECMD 脚本。

日志为追加事件，每条包含事务 ID、外层源、组件、阶段、结果和 Windows 错误码。日志写入失败只作为警告，不能掩盖主要应用结果。

## 18. 对旧实现问题的处理

| 旧行为/问题 | 新设计 |
| --- | --- |
| 调用方传 `eth/ems/...`，真实路径放在 `Path/*.txt` | `theme apply <PACKAGE>` 直接传路径并自动按扩展名路由 |
| 硬编码 `X:`、`%ProgramFiles%\7-Zip` | 运行时路径解析 + 统一依赖管理 |
| `%a:~-4,3%` 字符串切片识别扩展名 | `Path::extension` + 明确白名单 + 大小写不敏感比较 |
| 无归档穿越和链接防护 | 解压前后双重校验和资源上限 |
| EMS 通过控制面板和模拟回车补救 | 按原版写 15 个当前值和末尾两个空方案槽，读回后调用 `SPI_SETCURSORS` |
| 用固定延时猜测注册表是否就绪 | 同步写入、句柄关闭、精确读回；不依赖 sleep |
| EIS 运行 `setDesktopIcon.exe` | `IShellLinkW` + `IPersistFile` 直接修改 `.lnk` |
| 遍历固定桌面路径 | Known Folder + PE 兼容目录 |
| ELS 解压为最多三张 `load*.jpg` | `theme apply` 明确告警并跳过；未来 `theme store` 再迁移为 LSBP |
| 壁纸通过 PECMD `WALL` 应用 | 保留 PECMD `WALL`，仅把输入先发布到稳定会话路径 |
| ESS 直接通配删除 Explorer `*.db` | 在解析并校验当前 Shell 用户的精确 Explorer 缓存目录后，删除该目录第一层全部普通 `*.db`，保持原版失效范围 |
| 多个进程可交叉覆盖全局状态 | 进程内 mutex + 跨进程独占文件锁 |
| 旧脚本直接按进程名终止 Explorer | 保留原版刷新次数与先后位置，但只安全重启当前用户/会话的 Explorer，并保证失败时恢复 |
| `Intro.wcs` 随打开主题包执行 | `theme apply` 永不执行 Intro；未来 UI 命令另行设计 |

## 19. 测试设计

### 19.1 平台无关单元测试

- 所有支持扩展名的大小写组合均正确路由。
- 未知扩展名、无扩展名、目录和非普通文件在副作用前拒绝。
- `.eth` 根组件发现、原版 `壁纸→ELS(skip)→ESC→ESS→EMS→EIS→ESC末尾重启` 顺序、重复大小写碰撞；空主题作为无副作用成功。
- 独立 `.els` 在环境检查后只输出一次稳定警告并返回 `Skipped`，不解析 7-Zip、不创建 staging。
- 仅含 `LoadScreen.els` 的 `.eth` 返回成功的 `0 Applied / 1 Skipped`；含其他组件时不阻止其应用。
- 归档绝对路径、`..`、ADS、链接、设备名、加密和资源上限拒绝。
- 所有组件预检完成前不会调用提交后端。
- 组件运行期失败时后续组件继续执行，汇总准确标记全部成功、跳过和失败项。
- 多线程同时调用时，假的提交后端最大并发数始终为 1。
- 任意 Windows PE 但未通过 OEM 厂商或 `systemcpl.dll(.mun)` 文本门禁时，在副作用前返回 `EdgelessRuntime` 不满足。

这些测试在 Windows、Linux、macOS CI 都运行，不需要真实注册表或 Shell。

### 19.2 EMS 测试

- 15 个标准槽位映射和方案字符串顺序。
- `.ani` 优先、缺槽失败；方案末尾保留两个空字段且不修改 Person/Pin 当前值。
- 所有注册表值读回成功后才允许调用 SPI。
- 任意读回不一致时不调用 SPI，并完整恢复快照。
- `SystemParametersInfoW` 失败时回滚后再次加载旧光标。
- 正常应用生成原版格式的 `DDHHMMSS` 方案名和目录；同秒冲突在副作用前返回 `AlreadyExists`。
- Explorer 用户/会话不匹配时拒绝写错误的 HKCU。

### 19.3 EIS 测试

- 在临时桌面创建真实 `.lnk`，验证只改变 icon location。
- Unicode、空格、大小写不同的文件名可以按 Windows 语义匹配。
- 包内大小写折叠碰撞会拒绝。
- 损坏 `.lnk`、只读文件和分享冲突触发完整 EIS 回滚。
- 未匹配/未使用统计正确。
- `theme apply` 只执行 EIS 即时扫描，命令末尾不会再调用刷新服务。
- Loader 等到启动期全部快捷方式生产者进入终态后才执行一次收尾扫描，并能处理启动插件新建的快捷方式。
- 独立 `plugin load` 完成后不会调用桌面图标刷新服务。
- 主题应用和 Loader 同时请求刷新时由同进程/跨进程锁串行化，且遵守固定锁顺序。
- 不修改 `.url` 和桌面子目录中的链接。

### 19.4 ELS 与未来 Store 契约测试

本次 `theme apply` 必须实现并测试：

- 外层和嵌套 ELS 每个组件只打印一次包含稳定关键字的迁移警告。
- 不读取 ELS 归档内容；即使 ELS 内容损坏，`apply` 也保持 `Skipped`，因为它不是本命令的消费对象。
- ELS 不创建会话 `active.tar`，不调用 LoadScreen Play，也不访问启动盘。

以下是未来 `theme store` 实现时必须补充的契约测试，本次不创建占位实现：

- 三张完整输入映射为 `0000/0500/1000`。
- 缺少哪张就不生成对应标记，不从其他图片补帧。
- Legacy ELS 兼容播放模式在首帧出现前维持既有界面，在最后一帧后维持该帧。
- 兼容播放模式未实现时，缺少 `load0` 或 `load2` 的输入会被 Store 拒绝而不是改变表现。
- 不同尺寸和 EXIF 方向被规范化为同一尺寸。
- 输出为未压缩 tar，根目录条目有序且均可解码为 WebP。
- 多启动盘且没有显式 `--bootdisk` 时拒绝写入。

### 19.5 ESS、ESC 和壁纸测试

- ESS 缺任一 DLL、错误 machine、损坏 PE、额外条目时拒绝。
- 第二个 DLL 替换失败时恢复两个旧 DLL 和安全描述符。
- ESS 在替换 DLL 后、停止 Explorer 前发出带 flush 语义的 `SHCNE_ASSOCCHANGED`，覆盖原版 `DeskTopFresh=clearicon;1` 的刷新效果。
- ESS 停止 Explorer 后删除精确缓存目录第一层的全部普通 `*.db`，不越界也不缩小原版失效范围。
- Explorer 已停止时，无论成功失败均会恢复。
- ESC 使用中台返回的 PECMD 绝对路径、固定参数、工作目录和超时。
- ESC 超时/非零退出产生不可回滚部分失败标记。
- 独立 ESC 成功后立即重启 Explorer；`.eth` 中 ESC 延迟到 EIS 后重启。
- `.eth` 同时包含 ESS 与 ESC 时按原版时序发生两次安全重启，不被聚合。
- 壁纸使用中台返回的 PECMD 绝对路径和 `WALL <稳定绝对路径>` 参数，成功后不额外重启 Explorer。
- 壁纸源 staging 删除后，PECMD 使用的稳定会话路径仍存在。

### 19.6 E2E 与实机验收

`tests/e2e/` 增加主题用例，CI 工作流只调用脚本：

- Linux、macOS、普通 Windows：每种入口都明确报告需要 `WindowsPE`，且不会先要求本机存在 7-Zip/PECMD。
- Windows PE：用测试脚本动态构造最小 `.eth/.eis/.ems/.ess/.els`；验证可应用组件、ELS 告警/跳过和日志汇总。
- 多进程：两个 `eli theme apply` 同时启动，一个持锁时另一个等待或超时，不产生交叉资源。
- 启动集成：Loader 在启动期插件及内置快捷方式创建完成后统一替换图标；独立 `plugin load` 的 E2E 断言不会触发该行为。
- 故障注入：在发布、注册表读回、COM 保存、DLL 第二次替换、Explorer 恢复处分别注入失败，检查回滚和结果汇总。

FirPE 示例只用于本地兼容性验收，不提交到仓库，避免引入第三方主题资源及许可风险。实机验收至少检查：

1. 示例 `.eth` 六类资源均被识别。
2. 鼠标无需打开控制面板即可立即变化。
3. 桌面快捷方式图标无需 `setDesktopIcon.exe` 即可更新。
4. 示例中只有 `load0.jpg` 的 ELS 被识别、告警并标记为 `Skipped`，没有生成会话 LSBP 文件；其他五类组件继续应用。
5. ESS 应用失败不会留下缺失 DLL 或无 Explorer 的桌面。

## 20. 依赖与构建影响

Windows 目标需要在现有 `windows-sys` 依赖上补齐 COM、Shell、Known Folder 和安全 API 对应 feature，优先避免再引入另一套 `windows` crate。7-Zip 和 PECMD 仍是外部运行时依赖，由现有中台集中注册。

`theme apply` 不进行 ELS 转换，因此不新增 JPEG→WebP、tar 写入或 LoadScreen 播放器依赖。未来 `theme store` 实现迁移时必须复用现有 LoadScreen 图像处理能力，并将通用编解码部分与 `loadscreen bake` 的并发进度展示解耦。

所有平台都编译规划、扩展名识别、归档清单校验和事务状态机。只有实际 Win32 副作用位于 `cfg(windows)` 中；运行时仍必须进一步要求 `WindowsPE`，不能把“编译于 Windows”当成“正在 WinPE”。共享桌面图标服务同样提供跨平台统一接口，非 Windows 后端返回明确的不支持环境错误。

## 21. 分阶段实现建议

1. 建立 CLI、公开入口、类型识别、环境检查、全局锁和 fake backend 测试。
2. 实现统一安全归档层及 `.eth` 完整预检/应用计划。
3. 实现 ELS 的稳定告警/跳过语义，不引入 LoadScreen 转换与发布代码。
4. 实现 EMS 文件验证、注册表快照/读回屏障和 SPI 刷新。
5. 实现共享桌面图标服务、EIS 图标事务、Known Folder 枚举和 Shell Link 修改；为未来 Loader 暴露启动收尾入口，但不接入 `plugin load`。
6. 实现壁纸与 ESC，并按独立/组合场景保留原版 Explorer 刷新时机。
7. 实现 ESS 权限、双文件回滚、精确目录缓存清理和 Explorer 生命周期。
8. 补齐 Windows PE E2E、Loader 启动图标收尾、故障注入、并发测试和 FirPE 样包实机验收。

未来 `theme store` 及 ELS→LSBP 持久化迁移单独立项，不纳入以上阶段。

## 22. 已确认设计决策

本轮评审已确认以下边界，后续实现不得自行改变：

1. `theme apply` 只作用于当前 PE 会话；“保存为启动盘默认主题”属于未来的 `eli theme store`。
2. `theme apply` 遇到 ELS 只打印警告并跳过。未来 Store 迁移采用固定 `0%/50%/100%` 映射，只转换实际存在的帧，不补缺失端点，并以缺帧维持既有画面作为旧 `Pecmd.ini` 行为兼容标准。
3. `.esc` 交由依赖管理中台解析出的 PECMD 执行；不在 Eli 内实现 PECMD/WCS 解释器。
4. `theme apply` 保留原版 Edgeless 品牌门禁；实现集中在依赖管理模块的 `EdgelessRuntime` 能力中。
5. `.eth` 保留原版组件顺序和 Explorer 刷新位置；`theme apply` 不在末尾追加第二次 EIS 扫描。
6. 独立 `plugin load` 后不刷新桌面快捷方式图标；只保留 EIS 应用后的即时刷新和 Loader 启动收尾刷新。
