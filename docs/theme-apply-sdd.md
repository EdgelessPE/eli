# `eli theme apply` 软件设计文档（SDD）

## 1. 文档状态

- 状态：待评审（重构提效策略、EdgelessRuntime、ELS 与 ESC 策略已确认）
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
- `EdgelessExit` 在插件脚本和内置快捷方式创建完成后再次运行 `setDesktopIcon.exe`；这是旧启动流程中的最终一致性补扫事实，但其未来替代时机不属于 `theme apply` 的兼容契约。

### 2.2 原版自检行为

除 `autoESS` 外，已解密 `setTheme.cmd` 会执行两级自检：

1. 查询 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OEMInformation\Manufacturer`，要求值为 `Edgeless`。
2. 在 `systemcpl.dll.mun` 或 `systemcpl.dll` 中搜索 `Edgeless` 字样。这两个文件承载 Windows“系统/系统属性”控制面板的代码或本地化资源，旧镜像会在其中显示 OEM 品牌。

该检查不仅用于识别运行环境，也作为 Edgeless 有意保留的**防迁移门禁**：即使相同技术动作能够在其他 Windows PE 中执行，也不得因此把该检查退化为一般性的能力探测。`theme apply` 必须保留这道门禁，但不能让主题处理器自行复制注册表和二进制文本搜索逻辑。统一依赖管理模块提供 `EdgelessRuntime` 环境能力：先确认 `WindowsPE`，再按原版兼容规则检查 OEM `Manufacturer=Edgeless`，并确认 `systemcpl.dll.mun` 或回退的 `systemcpl.dll` 中包含 `Edgeless`。任一步失败都必须在副作用前返回明确的“不属于受支持 Edgeless PE”错误。

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
| 原版 `Pecmd.ini` | `setTheme.cmd auto` 后仍会创建系统快捷方式，`EdgelessExit` 最后再次运行 `setDesktopIcon.exe` | 记录为旧启动流程事实；本 SDD 只定义 `theme apply` 自身的即时图标更新，不冻结未来 Loader 或其他命令的调用时机 |
| FirPE 示例 `.eth` | 六类组件齐全，但 ELS 只有 `load0.jpg`，ESC 为 UTF-16，EMS 含 15 类光标文件 | 验收必须覆盖单帧 ELS 跳过、UTF-16 ESC 透传和 15 槽 EMS |
| 鼠标刷新讨论记录 | 旧调用 `call_dll('user32.dll','SystemParametersInfoW',0x57,0,0,2)` 中 `0x57` 即 `SPI_SETCURSORS`、`2` 即 `SPIF_SENDCHANGE`；风险点是调用前注册表配置是否已完整就绪 | 使用同步注册表写入、检查 Win32 返回值并关闭句柄，再以具名常量调用 `SPI_SETCURSORS`；不靠固定延时，也不把全量二次读回当作正常路径屏障 |

主题规范网页用于确定资源扩展名、标准组件名和归档形态；Win32 API 的调用约束以 Microsoft 文档为准。共享会话中的推断只作为定位线索，不作为平台行为的唯一依据。

### 2.5 兼容性审查基线

本设计的兼容目标是**主题包格式兼容和最终效果兼容**，而不是逐步复刻 `setTheme.cmd` 的内部实现。对于符合原版主题规范且能够成功应用的输入，新实现必须保持规范组件名、资源映射、注册表语义以及用户最终看到的主题效果；以下旧实现细节不构成兼容契约，可以为了可靠性和效率重构：

- 组件内部的具体执行顺序，只要不存在真实数据依赖且最终效果一致。
- Explorer/Shell 刷新的次数、精确位置以及旧脚本为补救刷新所使用的额外步骤。
- `Path/*.txt`、`RunMSTip`、`DelayRefresh`、`NoESSTip` 等脚本间临时协议。
- EMS 的 `DDHHMMSS` 临时命名、原版遗漏 `Pin`/`Person` 的行为以及控制面板模拟确认路径。
- EIS 对整个桌面的全量扫描、`setDesktopIcon.exe` 调用方式和“一个链接失败回滚全部链接”的脚本式事务粒度。
- `takeown.exe`/`icacls.exe`、固定 `Sleep`、按进程名杀 Explorer 等旧批处理实现方式。
- 进程异常退出时旧脚本留下半成品的偶然行为。

以下边界仍属于必须保留的产品或格式契约：

- `theme apply <PACKAGE>` 自动按扩展名路由；`apply` 只处理当前 PE 会话，不写启动盘。
- `WindowsPE` 与 `EdgelessRuntime` 门禁必须在副作用前通过。`EdgelessRuntime` 的 OEM + `systemcpl.dll(.mun)` 双重检查是有意保留的防迁移标识，不得以“其他 WinPE 也具备能力”为理由删除。
- ELS 在 `apply` 中只告警并跳过；其持久化或格式迁移由独立设计负责。
- ESC 继续交给经过依赖管理模块验证的 PECMD 解释；Eli 不实现 PECMD/WCS 子集解释器，也不通过简单命令黑名单伪装成安全沙箱。
- EIS 由 Eli 直接修改 `.lnk`，不运行主题包中的 `setDesktopIcon.exe` 或其他可执行文件。
- 归档安全检查、资源上限、路径穿越防护和 Windows 不区分大小写碰撞检查属于新实现的安全基线，不因兼容旧脚本而放宽。

错误路径允许采用比旧版更明确的失败和回滚策略。新设计可以合并重复刷新、减少扫描和外部进程调用，并对不同组件采用与风险相匹配的事务粒度，只要不改变合法主题包的最终可观察效果。
## 3. 目标

1. 通过一个明确的路径参数自动识别并应用 `.eth`、`.eis`、`.ems`、`.esc`、`.ess`、`.els` 和 `.jpg`。
2. 在任何副作用前验证当前环境为 `WindowsPE` 且满足 `EdgelessRuntime` 防迁移门禁，并通过条件编译保证 Windows、Linux、macOS 均可构建。
3. 统一使用依赖管理模块解析和探测 `7z.exe`、`pecmd.exe`，不硬编码安装路径。
4. 对组合主题执行完整预检，尽量在修改系统前发现损坏包、危险归档条目或缺失资源；纯读取、解码和校验允许有限并行。
5. 直接在 Rust 中实现 EIS 的桌面快捷方式图标替换，不再调用 `setDesktopIcon.exe`，并按图标映射定点查找候选 `.lnk`，避免全桌面扫描。
6. 修复 EMS 应用时序：同步写入光标配置、检查 Win32 写入结果、关闭句柄后调用 `SPI_SETCURSORS`，不通过固定延时、控制面板和模拟按键完成刷新；同时支持规范中的 `Pin`/`Person` 可选槽位。
7. 识别旧式 ELS，打印迁移警告并明确跳过；`apply` 不解压、不转换、不发布启动画面资源。
8. 用 Windows named mutex 串行化同机主题提交，避免多个 Eli 进程交叉覆盖全局状态。
9. 由统一 `RefreshPlan` 聚合各组件的 Shell 刷新需求，执行满足最终效果所需的最小刷新集合，避免 ESS、ESC、EIS 重复刷新 Explorer/Shell。
10. 按风险划分事务边界：ESS 保持强回滚和 Shell 恢复保证；EMS 保留当前进程内注册表/文件回滚；EIS 快捷方式采用逐链接 best-effort；不为非关键资源引入跨进程崩溃恢复协议。
## 4. 非目标

首版不包含：

- 主题包预览、混搭选择或图形化主题管理器。
- 将主题保存为启动盘默认主题。`theme apply` 只影响当前 PE 会话，不写入 `Edgeless\Default` 或 `Edgeless\wp.jpg`；该职责留给未来的 `eli theme store`。
- 执行 `.eth` 中的 `Intro.wcs`。它属于“打开主题包时的交互控制面板”，不是应用载荷。
- 用 Rust 重新实现 PECMD/WCS 解释器；`.esc` 仍由经过依赖管理模块验证的 `pecmd.exe` 执行。
- 从文件内容猜测一个没有受支持扩展名的包类型。
- 支持 `.jpeg`、`.png` 作为独立壁纸入口；首版保持旧规范的 `.jpg` 接口。
- 对正在运行的 LoadScreen 强制播放预览动画。
- 跨整个 `.eth` 提供绝对原子性。`.esc` 是交给 PECMD 的兼容脚本载荷，无法可靠撤销其全部副作用。
- 在本 SDD 中定义 `theme store`、ELS→LSBP 迁移或未来 Loader 的具体调用时序；这些职责由各自设计单独确定。

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

命令采用“识别 → 预检 → 准备 → 提交 → 聚合刷新”的五阶段模型。核心原则是组件只声明自身产生的状态变更和刷新需求，由编排器决定最小化后的实际刷新动作：

```text
CLI 薄转发层
    ↓
类型识别与环境检查
    ↓
归档/文件完整预检
    ↓
PreparedTheme（无系统副作用）
    ↓
获取 theme apply named mutex
    ↓
按数据依赖提交组件并累计 RefreshRequest
    ↓
RefreshPlan::execute_minimal()
    ↓
ApplySummary
```

`RefreshRequest` 至少能够表达：

```text
CursorReload
ShortcutNotify
IconCacheInvalidate
ExplorerRestart
```

合并规则以“更强刷新覆盖更弱刷新”为原则。例如计划中已经存在 `ExplorerRestart` 时，EIS 的普通 `ShortcutNotify` 可以省略；ESS 与 ESC 同时要求 Explorer 重启时只执行一次。`CursorReload` 与 Explorer 重启语义不同，EMS 成功后仍通过 `SPI_SETCURSORS` 明确刷新光标。

### 6.1 公开入口

`eli-lib` 只提供一个公开的主题应用入口，输入为 `Ctx` 和源路径，输出为结构化 `ApplySummary`。CLI 不解析归档、不直接操作注册表、不调用 Win32 API，也不拼装外部程序命令。

`ApplySummary` 至少包含：

- 识别出的外层类型。
- 每个组件的 `Applied`、`AppliedWithWarnings`、`Skipped` 或 `Failed` 状态。
- 迁移和兼容性警告。
- 最终实际执行的刷新动作，例如是否重启 Explorer、是否清理图标缓存、是否刷新光标。
- `.els` 是否因当前会话无法使用而被跳过。
- `.eis` 成功更新、未找到目标和修改失败的图标映射数量。

### 6.2 建议目录结构

```text
eli-cli/src/command/theme.rs

eli-lib/src/command/theme/
  mod.rs
  apply.rs
  apply/
    archive.rs
    transaction.rs
    refresh.rs
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

该结构遵守“父命令目录 + 同名子命令文件”的项目约束：`theme/mod.rs` 只公开并组织主题子命令，`theme/apply.rs` 是 `apply` 子命令的公开编排器；它通过显式 `#[path = "apply/..."]` 或等价的私有模块声明使用 `theme/apply/` 下的实现细节。未来新增其他主题子命令时再各自增加模块，不提前把其职责混进 `apply`。

`theme/apply/refresh.rs` 负责聚合各组件的刷新请求和管理当前会话 Explorer 生命周期。`theme/apply/eis.rs` 负责 EIS 资源发布和本次应用时的快捷方式修改；可复用的 Shell Link 读取/改写能力放在 `eli-lib/src/shell/desktop_icon.rs`。`theme/apply/windows.rs` 只封装其余主题专用的注册表、系统参数、named mutex、进程和文件安全描述符操作。非 Windows 构建保留统一接口，在环境检查阶段返回明确的 `WindowsPE` 不支持错误。

### 6.3 运行时路径

不得写死 `X:`。通过 `%SystemRoot%` 的父目录解析当前 PE 系统卷，并由一个 `ThemeRuntimePaths` 结构集中生成路径。建议的会话目录为：

```text
<PE 系统卷>\Users\Theme\eli\
  staging\<随机事务 ID>\
  wallpaper\current.jpg
```

Edgeless 兼容图标根目录为：

```text
<PE 系统卷>\Users\Icon
```

EMS 光标资源发布到 `%SystemRoot%\Cursors\Edgeless\<唯一 ID>\`。锁不落地为文件，由 Windows named mutex 管理。临时 staging 只服务于本次准备、当前进程内回滚和原子发布，不作为组件之间的传参协议，并在结束时尽量清理。
## 7. 环境和依赖检查

执行顺序固定为：

1. 校验扩展名和输入是普通文件。
2. 调用 `DependencyManager::require_environment(RuntimeEnvironment::WindowsPE)`。
3. 调用依赖管理模块的 `require_capability(RuntimeCapability::EdgelessRuntime)`，执行防迁移门禁。
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

`EdgelessRuntime` 属于运行环境能力，不属于外部程序依赖。它是有意保留的 Edgeless 防迁移标识，而不是“当前系统是否具备这些 Win32 能力”的一般探测。其首版规则固定为：

1. 读取 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OEMInformation` 的 `Manufacturer`，值必须为 `Edgeless`。
2. 优先检查 `%SystemRoot%\SystemResources\systemcpl.dll.mun`；文件不存在或不含目标文本时，回退检查 `%SystemRoot%\System32\systemcpl.dll`。
3. 至少一个候选文件包含 `Edgeless` 才通过。

该能力集中实现在依赖管理模块中并由主题命令声明，既保留产品支持边界，也避免业务处理器散落重复检查。除非明确修改防迁移策略，否则不得用普通 WinPE 能力探测替代该双重门禁；未来若 Edgeless 镜像品牌方式变化，只修改能力探测规则和对应测试。

## 8. 归档安全与格式校验

所有由 `theme apply` 实际打开的 7z 格式包都采用相同的安全流程；被跳过的 ELS 不进入该流程：

1. 使用 `7z l -slt -sccUTF-8` 列出条目，检查退出状态。
2. 在解压前拒绝绝对路径、盘符路径、UNC 路径、`..`、NTFS ADS、设备名和空文件名。
3. 拒绝符号链接、硬链接、junction、reparse point 和其他非普通文件/目录条目。
4. 以 Windows 不区分大小写语义检测同名碰撞；不能仅使用 Rust 的 ASCII 比较处理任意 Unicode 文件名。
5. 拒绝加密包、损坏包和超过资源限制的包。
6. 将本次应用需要的白名单条目以尽可能少的 7-Zip 进程调用解压到唯一 staging 目录；同一归档不为每个组件重复启动解压进程。
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

组合主题必须在任何系统副作用之前完成所有**可应用组件**的预检，包括嵌套 EIS/EMS/ESS 归档、图片解码、DLL PE 头、鼠标文件集合和 PECMD 依赖。嵌套 ELS 只在外层归档清单中识别并记录为 `Skipped`，不解压也不校验其内部图片。彼此独立的纯读取、解码和校验允许使用有界并行，但外部 7-Zip 进程数量必须受控，避免为了并行反而重复扫描或解压同一归档。

## 9. `.eth` 组合主题编排

`.eth` 中所有组件均为可选。没有可识别组件的包作为空操作成功，汇总为 `0 Applied / 0 Skipped`；未知根条目仍输出警告。若包内只有 `LoadScreen.els`，命令正常完成并汇总为 `0 Applied / 1 Skipped`，同时输出迁移警告。规范组件如下：

- `WallPaper.jpg`
- `LoadScreen.els`
- `IconPack.eis`
- `MouseStyle.ems`
- `StartIsBackConfig.esc`
- `SystemIconPack.ess`

组合主题不再以旧 `setTheme.cmd` 的逐行落序作为兼容契约，而是根据真实依赖构建应用计划：

1. 所有可应用组件先完成预检和 staging 准备；ELS 只记录 `Skipped`。
2. 壁纸、EMS、EIS、ESC 等不要求关闭 Shell 的组件在 Explorer 仍运行时提交。它们只记录自身刷新需求，不立即执行可被后续更强刷新覆盖的 Shell 动作。
3. ESS 若存在，其 DLL 替换安排在统一刷新阶段：编排器停止当前会话 Explorer，完成 ESS 双文件替换和图标缓存失效，然后统一恢复 Explorer。
4. 若没有 ESS 但成功的 ESC 请求了 Explorer 重启，则在全部普通组件提交后统一重启一次 Explorer。
5. 若最终需要 Explorer 重启，EIS 的普通快捷方式变更通知由该重启覆盖；否则对实际修改成功的 `.lnk` 发送定点 Shell 通知。
6. EMS 的 `SPI_SETCURSORS` 不由 Explorer 重启替代，成功写入光标配置后必须执行一次明确的光标刷新。

因此 `.eth` 同时包含 ESS 与 ESC 时最多只需要一次 Explorer 停止/启动周期，而不是复刻旧批处理中两次重启的中间状态。准备阶段允许有界并行；系统状态提交和统一刷新仍在主题全局 named mutex 内串行完成。

某个组件提交失败时，按该组件自己的事务等级处理后继续执行不依赖它的后续组件，并在最终汇总中准确记录 `Applied`、`AppliedWithWarnings`、`Skipped` 和 `Failed`。ESC 不参与跨组件伪事务；ESS 失败必须恢复旧 DLL 并保证 Explorer 最终可用。错误信息必须明确说明主题可能已部分应用。
## 10. `.ems` 鼠标样式

### 10.1 文件映射

归档根目录必须提供原版主题所需的 15 个基础逻辑槽位，每个槽位恰好选择一个 `.ani` 或 `.cur` 文件。两种格式同时存在时优先 `.ani` 并打印警告；基础槽位两者都不存在时预检失败。

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
| `aero_pin` | `Pin` | 16（可选） |
| `aero_person` | `Person` | 17（可选） |

`Pin` 和 `Person` 按主题规范作为可选扩展槽位处理：存在时验证并应用，不存在时不修改对应当前值，方案字符串中的对应字段保持为空，从而继续兼容只提供 15 个基础光标的旧主题。

每个选中的文件先用 `LoadCursorFromFileW` 验证可加载。光标目录和方案名不再使用旧版 `DDHHMMSS` 作为兼容格式；Eli 为每次发布生成碰撞概率可忽略的唯一 ID，例如：

```text
%SystemRoot%\Cursors\Edgeless\<unique-id>\
```

方案显示名可以包含源包名或简短唯一后缀，但其具体格式不是公开兼容契约。实现必须在副作用前保证最终目录和 `Schemes` 键名不会覆盖另一项现存方案；若极低概率发生碰撞，内部重新生成 ID，而不是把同秒冲突暴露给用户。

EMS 不接受子目录。除上述标准基名之外的普通文件只产生警告并被忽略；可执行文件、脚本和链接直接导致预检失败。

### 10.2 注册表提交与光标刷新

不使用固定 `Sleep`，也不打开 `main.cpl` 模拟确认。正确提交顺序为：

1. 将新目录原子发布到最终位置。
2. 快照 `HKCU\Control Panel\Cursors` 中本次会改动的当前值、默认值和 `Schemes` 中目标方案值，用于当前进程内失败回滚。
3. 以 `REG_EXPAND_SZ` 写入 15 个基础当前光标路径；存在 `Pin`/`Person` 时再写入对应可选值。路径统一使用 `%SystemRoot%`。
4. 以 `REG_SZ` 写入 `Schemes\<方案名>` 的完整 17 槽逗号分隔字符串；缺失的可选槽位为空字段。
5. 将 `HKCU\Control Panel\Cursors` 默认值写为方案名。
6. 每次 Win32 注册表调用都必须检查返回值；完成写入后关闭句柄。正常路径不再为了“可见性屏障”重新打开键并逐项读回全部值。
7. 调用 `SystemParametersInfoW(SPI_SETCURSORS, 0, NULL, SPIF_SENDCHANGE)`。
8. 若任一注册表写入或 `SPI_SETCURSORS` 失败，恢复本次快照和旧光标目录；在恢复成功后再次调用一次 `SPI_SETCURSORS` 尝试恢复旧显示。

注册表写入对其他进程同步可见，因此不调用 `RegFlushKey`。需要诊断时可以在 debug/test 后端增加读回断言，但全量读回不是生产路径的提交屏障。

调用前仍要记录当前进程的用户 SID 和会话 ID。若 Explorer 已存在但属于不同用户或不同会话，拒绝修改错误的 `HKCU` 并给出上下文不匹配错误。没有 Explorer 的启动早期允许写入当前用户 hive 和调用刷新。
## 11. `.eis` 图标资源包

### 11.1 图标发布

EIS 内容递归覆盖到 Edgeless 会话图标根目录，而不是替换整个 `Users\Icon`：

```text
<PE 系统卷>\Users\Icon
```

包内只接受目录和普通静态图片；`shortcut` 下只把能完整解码的 `.ico` 纳入快捷方式映射，其他静态图像可发布供 Edgeless 自带 UI 使用。EIS 中的 `.exe`、`.dll`、脚本及其他非图像文件必须拒绝，尤其不得接受或运行包内自带的 `setDesktopIcon.exe`。

资源发布使用临时文件 + 同卷 replace，保证单个文件不会暴露半写入状态。若资源复制本身失败，在进入快捷方式修改阶段前终止 EIS；无需维护可跨进程恢复的长期 journal。已经成功发布的图标资源即使后续个别 `.lnk` 修改失败也不整体回滚，因为稳定图标根目录仍可供后续重试或其他 Edgeless UI 使用。

### 11.2 桌面根目录

不假定桌面固定为 `X:\Users\Default\Desktop`。Windows 实现通过 `SHGetKnownFolderPath` 解析：

- 当前用户 `FOLDERID_Desktop`。
- 公共桌面 `FOLDERID_PublicDesktop`。
- 若存在且与上述路径不同，再加入 PE 兼容目录 `<PE 系统卷>\Users\Default\Desktop`。

所有路径去重。首版只处理这些目录第一层的 `.lnk`，不修改 `.url`，也不递归子目录。

### 11.3 定点匹配和修改

`shortcut/<快捷方式文件名去掉 .lnk>.ico` 与 `.lnk` 按 Windows 不区分大小写语义匹配。包内存在大小写折叠后同名图标时预检失败，避免结果依赖归档顺序。

实现不再先枚举桌面全部 `.lnk`。对每一个 `shortcut/*.ico`，直接在每个桌面根目录构造 `<stem>.lnk` 候选路径并查询是否存在；这使工作量与主题提供的图标数量相关，而不是与桌面全部快捷方式数量相关。一个图标在所有桌面根目录均找不到对应 `.lnk` 时计入 `unused`，不算失败。

实际存在的匹配项在单线程 COM STA 中执行：

1. `CoInitializeEx(COINIT_APARTMENTTHREADED)`。
2. 创建 `IShellLinkW`，通过 `IPersistFile::Load` 读取现有 `.lnk`。
3. 只调用 `IShellLinkW::SetIconLocation(<绝对 ico 路径>, 0)`，不改目标、参数、工作目录、描述或热键。
4. 通过 `IPersistFile::Save` 覆盖原快捷方式；Save 成功即视为该链接提交成功，不再为了验证重复 Load 一次。

快捷方式修改采用 **per-link best effort**。单个 `.lnk` 损坏、只读或被其他进程占用时记录该链接失败并继续其他候选，不回滚已经成功修改的链接，也不回滚已经发布的图标资源。只要至少一个目标修改失败，EIS 汇总为 `AppliedWithWarnings`；如果资源发布失败或所有实际存在的目标都因同一系统性错误无法处理，则可以升级为 `Failed`。

每个成功修改的 `.lnk` 产生 `ShortcutNotify` 刷新请求。若统一 `RefreshPlan` 最终还会重启 Explorer，则这些通知被重启覆盖；否则只对成功修改的项目发送定点 `SHCNE_UPDATEITEM`/flush 通知。`theme apply` 不承担启动流程、`plugin load` 或未来 Loader 的额外扫描时机，这些跨命令策略由各自设计决定。
## 12. `.els` 在 `apply` 中的处理

LoadScreen 是 PE 启动加载过程中展示的资源。用户能运行 `theme apply` 时，本次启动画面已经进入尾声或已经结束；把 ELS 转换成当前会话文件既不会复现旧版启动时序，也没有稳定的即时消费者。因此 `apply` 对 ELS 的行为固定为“识别、告警、跳过”。

外层 `.els` 在完成扩展名识别、`WindowsPE` 与 `EdgelessRuntime` 环境检查后直接返回 `Skipped`：不解析 `SevenZip` 依赖，不创建 staging，不打开归档，不转换图片，不写入启动盘或会话目录。嵌套 `LoadScreen.els` 只通过 `.eth` 的外层清单识别，同样不解压；同一个组件只输出一次警告。稳定消息至少包含以下关键语义，具体中英文措辞可由 CLI 本地化：

```text
warning: legacy LoadScreen.els is a startup resource and cannot affect the current PE session; skipped by `eli theme apply`; startup persistence is outside this command
```

若 `.eth` 同时含有其他资源，ELS 的 `Skipped` 不阻止其他组件应用，也不使整体退出码变为非零。若 `.eth` 只有 ELS，命令返回成功，汇总为 `0 Applied / 1 Skipped`。这与旧版 `setTheme.cmd eth/auto` 跳过 ELS 的实际行为一致。

ELS 的持久化、旧格式迁移、LSBP 映射、启动盘选择和 Loader 播放契约不属于本 SDD；未来若实现对应能力，应在独立的 `theme store`/LoadScreen 设计中定义，不能由 `theme apply` 提前冻结。
## 13. `.ess` 系统图标资源包

ESS 根目录必须恰好包含普通文件 `imageres.dll` 和 `imagesp1.dll`；目录、脚本、重复大小写条目和链接均拒绝。两个文件必须具有合法 PE/DLL 头，且 machine 类型与当前 PE 架构兼容。主题资源 DLL 不要求有效 Authenticode 签名，因为自定义资源通常会破坏签名。

ESS 是本命令中唯一需要强文件事务和 Shell 生命周期保证的组件。它向 `RefreshPlan` 声明 `IconCacheInvalidate + ExplorerRestart`，实际双 DLL 替换在统一 Shell 刷新阶段执行：

1. 将两个新 DLL 准备在系统卷 staging 中，并快照当前两个目标 DLL 到本次事务目录；不维护跨主题永久“原始系统图标”备份。
2. `RefreshPlan` 停止当前会话的 Explorer；没有 Explorer 时继续，但记录原状态。
3. 先使用当前权限尝试同卷原子 replace。只有明确收到访问拒绝等权限错误时，才读取目标安全描述符并通过 Win32 安全 API 临时取得最低必要写权限；正常可写路径不额外修改 ACL，也不调用 `takeown.exe` 或 `icacls.exe`。
4. 分别替换 `imageres.dll` 和 `imagesp1.dll`，每次 replace 后检查文件大小/哈希与预检产物一致。第二个文件失败时必须恢复第一个文件，保证两个资源 DLL 始终来自同一版本。
5. 如果本次临时修改过安全描述符，无论成功失败都恢复目标原安全描述符。
6. 在当前 Shell 用户的 `AppData\Local\Microsoft\Windows\Explorer` 缓存目录中删除第一层普通 `*.db` 文件；不得递归、不得越出该精确目录。
7. 成功或回滚完成后，由统一刷新阶段启动 Explorer 并等待桌面就绪。若计划中同时有 ESC 的重启请求，这次启动已经满足该请求，不再追加第二次重启。

既然 Explorer 会在本刷新阶段重启，ESS 正常路径不再在停止 Explorer 前额外发送一次随后立刻失效的 `SHCNE_ASSOCCHANGED`。如果未来实机验证表明某个特定系统版本仍需要额外通知，应作为兼容补丁加入，而不是默认复制旧批处理步骤。

若 Explorer 已停止，无论后续成功还是失败，都必须在 finally 路径恢复 Shell。命令不得遗留一个因主题失败而没有桌面的 PE 会话。
## 14. `.esc` 开始菜单配置

`.esc` 是 PECMD/WCS 兼容脚本。为保持既有主题兼容性，继续交由 PECMD 执行，不在 Eli 内实现一个不完整的 PECMD/WCS 子集解析器，也不通过 `EXEC`/`CALL` 等简单文本 denylist 声称脚本已被安全沙箱化。

执行前只做不会改变合法 PECMD 脚本语义的**传输级/损坏文件预检**：

- 输入必须是普通、非空文件，并设置合理的最大文件大小。
- 若存在 BOM，则验证 UTF-16 等多字节编码没有明显截断；不强制要求 UTF-8，也不尝试把无 BOM 的历史脚本重新编码。
- 确保脚本已经位于稳定绝对路径，且依赖管理模块能够解析并探测 `pecmd.exe`。
- 不分析 `REGI`、`EXEC`、`CALL` 等命令的语义，不根据命令关键字决定是否允许执行。

执行方式：

- 通过依赖管理模块得到 `pecmd.exe` 绝对路径。
- 直接构造 `pecmd.exe LOAD <绝对 esc 路径>` 参数，不经过 `cmd.exe /c`。
- 工作目录固定为 `%SystemRoot%\System32`，与旧应用路径一致。
- 同步等待退出，设置统一超时，捕获退出码和标准错误。
- 非零退出、超时或无法启动均为组件失败。
- 成功后只向 `RefreshPlan` 提交 `ExplorerRestart` 请求，不立即自行重启 Explorer；独立 `.esc` 因没有其他组件，最终仍表现为执行结束前重启一次 Explorer。

`theme apply` 不执行 `Intro.wcs`。ESC 本身是规范允许的可执行配置载荷，无法在不完整实现 PECMD 的情况下证明其只含注册表命令；因此把“显式调用 apply”视为运行该主题配置的授权，并在错误汇总中标记 ESC 为不可自动回滚组件。
## 15. `.jpg` 壁纸

独立 `.jpg` 或 `.eth` 中的 `WallPaper.jpg` 必须先按内容解码为有效静态 JPEG。为保持原版 `setTheme.cmd` 的成功路径语义，壁纸仍交给 PECMD 的 `WALL` 命令应用，不用 `SystemParametersInfoW` 猜测 PECMD 的内部注册表和显示方式行为：

```text
pecmd.exe WALL <会话稳定路径的绝对 JPEG 路径>
```

PECMD 绝对路径由依赖管理模块提供，参数直接传给进程而不经过 `cmd.exe /c`，工作目录为 `%SystemRoot%\System32`。不能直接引用 `.eth` staging 中即将删除的图片，应先复制到会话稳定路径；替换前记录旧会话文件，PECMD 启动失败、超时或非零退出时恢复旧文件。成功后不额外重启 Explorer。

## 16. Shell 刷新策略

Shell 刷新由统一 `RefreshPlan` 管理，不再要求各组件按旧批处理位置立即刷新。组件提交成功后只声明需求，编排器在能够保证最终效果的前提下合并重复动作：

| 来源 | 刷新请求 |
| --- | --- |
| 已跳过的 ELS | 无 |
| 壁纸 | PECMD `WALL` 已完成显示更新，无额外 Shell 请求 |
| EMS | `CursorReload`，调用一次 `SPI_SETCURSORS` |
| EIS | `ShortcutNotify`，仅在最终不重启 Explorer 时实际发送 |
| ESC | `ExplorerRestart` |
| ESS | `IconCacheInvalidate + ExplorerRestart`，并要求在 Explorer 停止期间完成 DLL replace |

最小化规则至少包括：

1. 多个 `ExplorerRestart` 合并成一次停止/启动周期。
2. `ExplorerRestart` 覆盖普通 `ShortcutNotify`；不再为了即将被重启的 Explorer 发送全套快捷方式刷新通知。
3. `IconCacheInvalidate` 与 ESS 的 Shell-off 提交合并，在 Explorer 停止期间清理精确缓存目录后再启动 Shell。
4. `CursorReload` 不被 Explorer restart 隐式替代，EMS 成功后仍明确调用一次 `SPI_SETCURSORS`。
5. 编排器只操作当前会话、当前用户拥有的 Explorer，不按进程名杀死其他会话进程。

因此 `.eth` 同时包含 ESS 和 ESC 时只执行一次安全 Explorer 重启。重启必须等待桌面窗口就绪并设置明确超时；超时返回“资源已提交但 Shell 恢复失败”的部分成功错误。
## 17. 并发、锁和失败语义

主题资源共享 HKCU、系统 DLL、光标目录和桌面快捷方式，因此系统状态提交和统一刷新阶段不并行。纯预检与解码可以在锁外有界并行，获得全局锁后对会依赖当前系统状态的目标重新确认必要前提。

### 17.1 全局 named mutex

Windows 实现使用一个固定名称的 named mutex（例如 `Local\\Edgeless.Eli.ThemeApply`）覆盖“提交 → RefreshPlan”阶段，同时承担同进程和多进程互斥。等待采用有界超时；超时返回 `WouldBlock`。mutex 由内核句柄生命周期管理，进程异常退出后自动释放，不创建 `theme-apply.lock`，也不需要陈旧锁文件清理逻辑。

非 Windows 测试后端可以使用普通同步原语模拟同样的单提交语义，但这不是运行时锁文件协议。

### 17.2 事务等级

不同资源采用与风险匹配的事务策略，而不是所有组件统一维护长期 journal：

- **ESS（Critical）**：双 DLL 当前进程内强事务；任一替换失败恢复两个旧 DLL 和临时 ACL，并保证 Explorer 最终恢复。
- **EMS（Configuration）**：在当前进程内快照本次会改动的光标注册表值和旧目录；写入或 SPI 失败时恢复。不保存供下一次进程自动重放的 crash-recovery manifest。
- **EIS（Best effort）**：图标资源文件采用单文件原子发布；快捷方式逐链接提交，单链接失败只进入警告/统计，不回滚其他链接。
- **壁纸（Best effort with stable file）**：稳定会话文件使用原子 replace；PECMD `WALL` 失败时可以恢复旧稳定文件，但不建立跨进程恢复日志。
- **ESC（External/Non-rollbackable）**：PECMD 脚本不承诺通用回滚。
- **ELS**：无副作用，无事务。

`.eth` 在提交前仍完成全部可应用组件预检；运行期失败不阻止与其无依赖的后续组件。统一刷新阶段必须在 finally 路径保证已停止的 Explorer 恢复。staging 写入使用临时文件 + flush + 同卷 rename/replace，不把半成品暴露为活动资源。

本命令不实现“进程异常退出后下次启动自动读取主题事务清单并恢复”的通用 crash recovery。对当前 PE 会话中的非关键主题资源，重新执行 `theme apply` 即可达到可预测状态；ESS 的安全性由单次执行中的双文件回滚和 Shell finally 保证。

日志为追加事件，每条包含事务 ID、外层源、组件、阶段、结果和 Windows 错误码。日志写入失败只作为警告，不能掩盖主要应用结果。
## 18. 对旧实现问题的处理

| 旧行为/问题 | 新设计 |
| --- | --- |
| 调用方传 `eth/ems/...`，真实路径放在 `Path/*.txt` | `theme apply <PACKAGE>` 直接传路径并自动按扩展名路由 |
| 硬编码 `X:`、`%ProgramFiles%\\7-Zip` | 运行时路径解析 + 统一依赖管理 |
| `%a:~-4,3%` 字符串切片识别扩展名 | `Path::extension` + 明确白名单 + 大小写不敏感比较 |
| 无归档穿越和链接防护 | 解压前后双重校验、资源上限，并尽量减少同一归档的重复 7-Zip 调用 |
| EMS 通过控制面板和模拟回车补救 | 同步写注册表后直接调用 `SPI_SETCURSORS`；正常路径不做全量二次读回 |
| EMS 仅处理 15 个光标并用 `DDHHMMSS` 命名 | 兼容 15 个基础槽并支持可选 `Pin`/`Person`；目录和方案使用可靠唯一 ID |
| EIS 运行 `setDesktopIcon.exe` 并扫描整个桌面 | `shortcut/*.ico` 反向定点寻找候选 `.lnk`，用 `IShellLinkW` + `IPersistFile` 修改 |
| 单个 EIS 链接失败导致整包回滚 | 快捷方式 per-link best effort；失败进入 `AppliedWithWarnings` |
| EIS 保存后再次 Load 验证 | 检查 `IPersistFile::Save` 返回值即可，避免重复 COM I/O |
| ELS 解压为最多三张 `load*.jpg` | `theme apply` 明确告警并跳过；持久化/迁移由独立设计负责 |
| 壁纸通过 PECMD `WALL` 应用 | 保留 PECMD `WALL`，仅把输入先发布到稳定会话路径 |
| ESS 每次预先接管权限并单独刷新 Shell | 先用现有权限 replace，仅在 AccessDenied 时临时调整 ACL；由统一 `RefreshPlan` 清缓存并重启 Explorer |
| ESS 与 ESC 分别重启 Explorer | `RefreshPlan` 合并为一次安全 Shell 重启 |
| 多个进程可交叉覆盖全局状态 | Windows named mutex 同时覆盖线程间和进程间主题提交 |
| 所有组件维护长期 journal / crash recovery | 仅保留当前执行必要的风险分级回滚；不为非关键会话资源维护跨进程恢复协议 |
| `Intro.wcs` 随打开主题包执行 | `theme apply` 永不执行 Intro；未来 UI 命令另行设计 |
| OEM + `systemcpl.dll(.mun)` 检查看似冗余 | 明确定义为有意保留的 Edgeless 防迁移门禁，不按普通能力探测优化掉 |
| ESC 作为 PECMD 脚本难以安全做子集解析 | 保持 PECMD 兼容执行，只做非语义的损坏文件预检，不实现半套解释器或文本 denylist |
## 19. 测试设计

### 19.1 平台无关单元测试

- 所有支持扩展名的大小写组合均正确路由。
- 未知扩展名、无扩展名、目录和非普通文件在副作用前拒绝。
- `.eth` 根组件发现、重复大小写碰撞和空主题无副作用成功。
- 计划中 ESS 与 ESC 同时存在时，`RefreshPlan` 最终只产生一次 `ExplorerRestart`；存在 Explorer 重启时 EIS 的普通 `ShortcutNotify` 被覆盖。
- 独立 `.els` 在环境检查后只输出一次稳定警告并返回 `Skipped`，不解析 7-Zip、不创建 staging。
- 仅含 `LoadScreen.els` 的 `.eth` 返回成功的 `0 Applied / 1 Skipped`；含其他组件时不阻止其应用。
- 归档绝对路径、`..`、ADS、链接、设备名、加密和资源上限拒绝。
- 同一外层归档的计划解压不会因为多个组件而重复启动无必要的完整解压流程。
- 所有组件预检完成前不会调用提交后端。
- 组件运行期失败时，与其无依赖的后续组件继续执行，汇总准确标记成功、警告、跳过和失败项。
- 并发请求进入提交后端时最大并发数始终为 1。
- 任意 Windows PE 但未通过 OEM 厂商或 `systemcpl.dll(.mun)` 文本门禁时，在副作用前返回 `EdgelessRuntime` 不满足；测试明确断言该检查是必需门禁而非可选能力探测。

这些测试在 Windows、Linux、macOS CI 都运行，不需要真实注册表或 Shell。

### 19.2 EMS 测试

- 15 个基础槽位映射和方案字符串顺序。
- `aero_pin` / `aero_person` 存在时映射到第 16/17 槽；缺失时对应当前值不修改、方案字段为空。
- `.ani` 优先；基础槽缺失失败。
- 所有注册表 Win32 写入返回成功且句柄关闭后才允许调用 SPI；正常路径不要求逐项全量读回。
- 任一写入失败或 `SystemParametersInfoW` 失败时恢复快照，并尝试重新加载旧光标。
- 方案目录/名称使用唯一 ID；模拟碰撞时内部重新生成而不是返回“同秒冲突”。
- Explorer 用户/会话不匹配时拒绝写错误的 HKCU。

### 19.3 EIS 测试

- 在临时桌面创建真实 `.lnk`，验证只改变 icon location。
- Unicode、空格、大小写不同的文件名可以按 Windows 语义通过定点候选路径匹配。
- 包内大小写折叠碰撞会拒绝。
- 测试证明实现按 `shortcut/*.ico` 构造候选，而不是先枚举整个桌面的全部 `.lnk`。
- 一个损坏/只读/被占用的 `.lnk` 不回滚其他已成功链接；汇总为 `AppliedWithWarnings` 并统计失败项。
- `IPersistFile::Save` 成功后不会为了验证再次 Load 同一链接。
- 图标没有对应 `.lnk` 时进入 `unused` 统计，不算失败。
- 计划最终不重启 Explorer 时，对成功修改项发送定点 Shell 通知；计划会重启 Explorer 时不发送冗余通知。
- 不修改 `.url` 和桌面子目录中的链接。

### 19.4 ELS 测试

- 外层和嵌套 ELS 每个组件只打印一次包含稳定关键字的迁移警告。
- 不读取 ELS 归档内容；即使 ELS 内容损坏，`apply` 也保持 `Skipped`，因为它不是本命令的消费对象。
- ELS 不创建会话 `active.tar`，不调用 LoadScreen Play，也不访问启动盘。
- 本 SDD 的测试不包含 ELS→LSBP、`theme store` 或 Loader 播放契约；这些由未来独立设计覆盖。

### 19.5 ESS、ESC、刷新与壁纸测试

- ESS 缺任一 DLL、错误 machine、损坏 PE、额外条目时拒绝。
- ESS 在目标可直接写时不调用 ACL 修改后端；仅模拟 AccessDenied 时才临时调整并最终恢复安全描述符。
- 第二个 DLL 替换失败时恢复两个旧 DLL，并保证 Explorer 最终恢复。
- ESS 在 Explorer 停止期间删除精确缓存目录第一层的普通 `*.db`，不越界。
- ESS + ESC 同时成功时只有一次 Explorer 停止/启动；ESC 单独成功时仍在命令结束前重启一次 Explorer。
- Explorer 已停止时，无论 ESS 成功失败均会恢复。
- ESC 使用中台返回的 PECMD 绝对路径、固定参数、工作目录和超时。
- ESC 预检接受合法 UTF-16 示例，不对 `REGI`/`EXEC` 等关键字做 allowlist/denylist；空文件、超限文件和明显截断的 BOM 编码文件被拒绝。
- ESC 超时/非零退出产生不可回滚部分失败标记。
- 壁纸使用中台返回的 PECMD 绝对路径和 `WALL <稳定绝对路径>` 参数，成功后不额外重启 Explorer。
- 壁纸源 staging 删除后，PECMD 使用的稳定会话路径仍存在。
- `RefreshPlan` 中 `ExplorerRestart` 覆盖 EIS `ShortcutNotify`，但不会吞掉 EMS `CursorReload`。

### 19.6 E2E 与实机验收

`tests/e2e/` 增加主题用例，CI 工作流只调用脚本：

- Linux、macOS、普通 Windows：每种入口都明确报告需要 `WindowsPE`，且不会先要求本机存在 7-Zip/PECMD。
- Windows PE：用测试脚本动态构造最小 `.eth/.eis/.ems/.ess/.els`；验证可应用组件、ELS 告警/跳过和日志汇总。
- 多进程：两个 `eli theme apply` 同时启动，一个持有 named mutex 时另一个等待或超时，不产生交叉资源。
- 故障注入：在 EMS 注册表写入、SPI、EIS COM Save、ESS 第二次替换、ACL 恢复和 Explorer 恢复处分别注入失败，检查对应事务等级和结果汇总。
- 刷新聚合：构造同时含 ESS、ESC、EIS、EMS 的主题，验证 Explorer 只重启一次、EIS 不发送冗余通知、EMS 光标仍立即刷新。

FirPE 示例只用于本地兼容性验收，不提交到仓库，避免引入第三方主题资源及许可风险。实机验收至少检查：

1. 示例 `.eth` 六类资源均被识别。
2. 鼠标无需打开控制面板即可立即变化；15 槽旧主题保持兼容，含 `Pin`/`Person` 的主题能够应用可选槽位。
3. 桌面快捷方式图标无需 `setDesktopIcon.exe` 即可更新，单个坏链接不会撤销其他成功图标。
4. 示例中只有 `load0.jpg` 的 ELS 被识别、告警并标记为 `Skipped`，其他可应用组件继续应用。
5. ESS 与 ESC 共存时最终效果正确且 Explorer 只经历一次安全重启。
6. ESS 应用失败不会留下不一致的两个 DLL 或无 Explorer 的桌面。
7. 非 Edgeless Windows PE 即使具备 7-Zip、PECMD 和相同目录布局，也会被 `EdgelessRuntime` 防迁移门禁拒绝。
## 20. 依赖与构建影响

Windows 目标需要在现有 `windows-sys` 依赖上补齐 COM、Shell、Known Folder、named mutex 和安全 API 对应 feature，优先避免再引入另一套 `windows` crate。7-Zip 和 PECMD 仍是外部运行时依赖，由现有中台集中注册。

`theme apply` 不进行 ELS 转换，因此不新增 JPEG→WebP、tar 写入或 LoadScreen 播放器依赖。ELS 的持久化和迁移属于未来独立设计，不在这里预先选择实现依赖。

所有平台都编译规划、扩展名识别、归档清单校验、刷新计划和平台无关状态机。只有实际 Win32 副作用位于 `cfg(windows)` 中；运行时仍必须进一步要求 `WindowsPE` 和 `EdgelessRuntime`，不能把“编译于 Windows”当成“正在受支持 Edgeless PE”。
## 21. 分阶段实现建议

1. 建立 CLI、公开入口、类型识别、`WindowsPE` + `EdgelessRuntime` 防迁移检查、named mutex 和 fake backend 测试。
2. 实现统一安全归档层及 `.eth` 完整预检/应用计划；同一归档最小化 7-Zip 进程调用，并为纯校验加入有界并行。
3. 实现 ELS 的稳定告警/跳过语义；不引入 LoadScreen 转换、Store 或 Loader 契约代码。
4. 实现 EMS 15 基础 + `Pin`/`Person` 可选槽位、唯一方案 ID、注册表当前进程回滚和 `SPI_SETCURSORS` 刷新。
5. 实现 EIS 图标原子发布、Known Folder 根目录解析、ICO→LNK 定点匹配和 per-link best effort Shell Link 修改。
6. 实现壁纸与 ESC；ESC 保持 PECMD LOAD，只做传输级损坏预检并向 `RefreshPlan` 请求 Explorer 重启。
7. 实现 `RefreshPlan`，覆盖快捷方式通知、光标刷新、图标缓存失效和 Explorer 最小重启聚合。
8. 实现 ESS 双文件强事务、按需 ACL 提升、Shell-off 替换、精确缓存清理，并接入统一刷新阶段。
9. 补齐 Windows PE E2E、故障注入、多进程 named mutex 和 FirPE 样包实机验收。

`theme store`、ELS→LSBP 持久化迁移以及未来 Loader 的调用时序均单独立项，不纳入以上阶段。
## 22. 已确认设计决策

本轮评审已确认以下边界，后续实现不得自行改变：

1. `theme apply` 只作用于当前 PE 会话；启动盘持久化属于独立的未来设计，本 SDD 不冻结其格式迁移和 Loader 契约。
2. `theme apply` 遇到 ELS 只打印警告并跳过，不读取或转换 ELS 内容。
3. `.esc` 继续交由依赖管理中台解析出的 PECMD 执行；Eli 不实现 PECMD/WCS 子集解析器，也不通过简单关键字 allowlist/denylist 替代真实解释器。ESC 只做传输级损坏文件预检。
4. `theme apply` 必须保留 `EdgelessRuntime` 门禁：OEM `Manufacturer=Edgeless` + `systemcpl.dll.mun`/`systemcpl.dll` 中的 `Edgeless` 标识是有意保留的防迁移机制，不得改成普通 WinPE 能力探测。
5. 兼容目标是主题格式和最终效果，不再要求复刻旧批处理的组件内部落序、`DDHHMMSS` 命名、桌面全量扫描、重复 Explorer 刷新位置或旧错误路径。
6. Shell 刷新统一由 `RefreshPlan` 聚合；ESS 与 ESC 共存时只执行一次 Explorer 重启，EIS 普通通知可被该重启覆盖，EMS 的 `SPI_SETCURSORS` 仍独立执行。
7. EMS 保持 15 个基础槽兼容，并支持主题规范中的可选 `Pin`/`Person`；方案和目录使用可靠唯一 ID，不保留同秒冲突语义。
8. EIS 按 `shortcut/*.ico` 定点寻找 `.lnk`，快捷方式修改采用 per-link best effort；单个坏链接不回滚其他成功结果，不再保存后重复 Load 验证。
9. ESS 采用双 DLL 强事务和 Shell finally 保证；ACL 只在现有权限不足时临时调整，不维护跨主题永久原始 DLL 备份。
10. 主题提交使用 Windows named mutex 做同进程/多进程互斥；不使用锁文件，也不为非关键会话资源维护跨进程 crash-recovery journal。
