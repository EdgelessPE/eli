# ELI Plugin LocalBoost SDD

## 目标与边界

目标命令集：

```text
eli plugin localboost load <PATH>
eli plugin localboost startup
eli plugin localboost clean <PLUGIN>
eli plugin localboost clean --all
```

- `load`：将单个插件包安装到 LocalBoost 持久仓库，并加载到当前 Edgeless PE。
- `startup`：在 PE 启动阶段直接加载仓库中已有的插件，不重新解压；启动 Hook 负责决定何时调用它。
- `clean`：清理单个插件或显式清空全部仓库，不承诺逆转 CMD/WCS 已产生的进程、注册表等外部副作用，完成后提示重启 PE。
- 保持原版仓库布局、`_LocalBoost.txt`、`Plugins_info` 和 `repoPart.txt` 兼容；不创建或依赖旧脚本传参文件 `pluginPath.txt`、`unit.txt`。

## 共享加载模型

LocalBoost 只保留一套 unit 发布流程：

```text
restore_unit(unit)
  -> 复制根目录文件
  -> 为一级目录创建 junction
  -> 更新 Plugins_info
  -> 按 CMD、WCS 顺序执行脚本并归档
  -> 提交当前 PE 会话的已加载状态
```

`load` 在安装后调用它，`startup` 对仓库中的每个 unit 调用它。已加载状态仅在整个 unit 成功后原子提交；重复调用返回 `AlreadyLoaded`，不得重复执行脚本。

## `localboost load`

CLI 薄转发到唯一的包加载 service：

```rust
localboost::load(ctx, path)
```

service 在产生副作用前检查 Windows PE、`7z`/`cmd`/`pecmd` 依赖，以及 `PATH` 存在、是普通文件且能推导合法插件名。**不校验扩展名**；任意扩展名或无扩展名文件均可交给 7-Zip，无法解压时返回明确错误。

流程：

1. 解析或选择仓库，获取 LocalBoost 跨进程锁。
2. 若该插件已在当前会话成功加载，返回 `AlreadyLoaded`。
3. 解压到与仓库同盘的唯一 staging，拒绝其中的 reparse point，并写入一级目录的 `_LocalBoost.txt`。
4. 事务式合并到 `<drive>:\Edgeless\BoostRepo\<plugin-name>`；失败时回滚 ELI 可控的文件和元数据。
5. 调用共享 `restore_unit()`，最后清理 staging。

## `localboost startup`

`startup` 对齐原版 `loadOnBoot` 的“加载已有仓库”阶段：

1. 检查 Windows PE 及 `cmd`/`pecmd`，但不要求 7-Zip。
2. 读取 `repoPart.txt` 或发现已有仓库；完全没有仓库时幂等成功，不弹出选择窗口。
3. 获取与 `load`/`clean` 相同的跨进程锁，稳定排序仓库内的普通插件目录。
4. 逐个调用 `restore_unit()`；一个 unit 失败不阻止其他 unit，最后统一汇总。

启动脚本负责先调用 `localboost startup`，再调用批量 `plugin load`。无需复刻 `.7zl -> .7zb -> .7zl`：后续 `.7zl` 仍进入 `localboost::load`，service 通过当前会话已加载状态返回 `AlreadyLoaded`，不会重新解压或执行脚本。

## 批量 `plugin load` 集成

`eli plugin load` 保留 `.7zl`（大小写不敏感）作为**路由标志**：

- `.7zl` + `ignore`：跳过并记录结果。
- `.7zl` + `load`：直接调用同一个 `localboost::load(ctx, path)` service。
- 其他包：走普通插件加载。

`.7zl` 只决定选择哪个 loader，不是 `localboost::load` 的输入限制。禁止在 `plugin/load.rs` 中维护第二套 LocalBoost 生命周期，也不通过 shell 再启动 `eli`。混合批次按包隔离结果，一个包失败不回滚或阻止其他包。

## `localboost clean`

Clap 必须要求 `<PLUGIN>` 与 `--all` 恰好选择一个：

```text
eli plugin localboost clean <PLUGIN>
eli plugin localboost clean --all
```

- `<PLUGIN>` 是仓库中的目录名而不是路径，允许 Unicode，但必须是单个安全路径组件，拒绝空值、`.`、`..`、分隔符、绝对路径和前缀。
- `clean <PLUGIN>` 只处理选中仓库中的该 unit：先依据 manifest 尽力移除可确认归属的当前会话文件和 junction，再事务式移走并删除 unit，最后更新 `Plugins_info`/`List_LocalBoost.txt`。找不到插件返回明确错误，不影响其他 unit。
- `clean --all` 扫描当前电脑上的全部合法 `BoostRepo`，清理仓库和 ELI staging，并在相应仓库已删除或不存在时移除失效的 `repoPart.txt`。跨仓库允许部分成功并汇总失败，不恢复已经删除的大型缓存。
- 裸 `clean` 或同时给出 `<PLUGIN>` 与 `--all` 均为参数错误。`--all` 本身是整机清理的显式确认。

两种 clean 都不声称完整 unload；脚本副作用不可可靠逆转。当前会话清理失败或不完整时必须报告并提示重启 PE。

## 仓库发现与选择 GUI

仓库逻辑集中在 `eli-lib`，供三个命令复用：

1. 优先读取 `%SystemDrive%\Users\LocalBoost\repoPart.txt`，兼容 `D`、`D:` 等旧格式，并验证仓库不在 PE 系统盘。
2. 配置缺失或失效时扫描已挂载本地卷中的 `<drive>:\Edgeless\BoostRepo`；唯一已有仓库可直接采用并原子回写配置。
3. 首次创建仓库，或多个候选无法自动决定时，由库层返回结构化候选信息，CLI 打开 Slint 分区选择窗口；Slint 不直接访问文件系统。

选择规则对齐原版 GUI：排除 PE 系统盘、Edgeless 启动盘、不可写及剩余空间不足 2 GiB 的卷。窗口展示盘符/挂载点、卷标、可用空间和不可选原因；确认后由 `eli-lib` 再次校验并原子写入 `repoPart.txt`，防止界面展示后磁盘状态变化。

UI 文件与职责：

```text
ui/plugin/localboost-repository.slint
eli-cli/src/ui/plugin/localboost_repository.rs
```

- 顶层窗口继承 `EliWindow`，复用 `Tokens`、`Button`、`Spin` 和现有滚动组件，支持明暗主题和软件渲染；不得复制标题栏、写死颜色或用 Emoji 表示关键状态。
- Rust 是候选、选中项、忙碌和错误状态的唯一来源；后台扫描结果通过 `slint::invoke_from_event_loop` 更新。
- 新入口加入 `eli-cli/build.rs`；Windows GUI 使用条件编译隔离，Linux/macOS 不解析 Win32 依赖。
- 用户取消不写配置或创建仓库：显式 `load` 返回取消错误；批量加载将待处理 LocalBoost 项标记为取消，普通插件继续执行。
- `startup` 在没有任何仓库时直接成功；只有存在多个仓库且必须选择本次启动仓库时才显示窗口。`clean --all` 不需要选择窗口。

## 安全、事务与并发

- 对仓库、unit、staging、删除目标及既存祖先检查 Windows reparse 属性；拒绝借助 symlink、junction 或其他 reparse point 越界写入、遍历或删除。
- 解压先进入唯一 staging，再通过 merge/copy、metadata snapshot 和 rollback 发布；最终仓库路径不直接承接不受信任的解压输出。
- `load`、`startup`、`clean` 共用同一个 Windows named mutex，首版覆盖完整 LocalBoost 生命周期，处理多进程 load/startup/clean 的所有组合。
- 若还需获取普通插件 publish lock，固定顺序为 `LocalBoost lock -> publish lock`，禁止反向获取。
- 脚本副作用不可事务化；失败时回滚 ELI 可控状态，并明确报告无法自动撤销的部分。
- 原版对依赖目录内 BAT/CMD 的 LocalBoost 兼容性警告继续保留，但只作警告，不改变结果。

## 代码组织

```text
eli-lib/src/command/plugin/localboost/
  mod.rs
  load.rs
  startup.rs
  clean.rs
  repository.rs
  runtime.rs

eli-cli/src/ui/plugin/localboost_repository.rs
ui/plugin/localboost-repository.slint
```

CLI 命令模块只解析参数、调用 service、触发需要的 GUI 并格式化结果。磁盘发现、校验、锁、事务和业务状态均留在 `eli-lib`。

## 测试要点

- **load**：任意扩展名/无扩展名、缺失路径、目录、非法插件名、依赖或解压失败、reparse point、同名更新、回滚、marker 和 manifest。
- **startup**：无仓库幂等成功、稳定加载多个 unit、无需 7-Zip、单 unit 失败隔离、重复调用不重复执行脚本，以及 startup 后 `.7zl` 调用统一 load service 并返回 `AlreadyLoaded`。
- **批量路由**：`.7zl + ignore/load`、`.7z` 普通加载、混合批次独立汇总，证明没有第二套 LocalBoost 实现。
- **clean**：参数互斥、Unicode 安全目录名、单插件清理不影响其他插件、`--all` 清理多仓库、残留 staging、部分失败、失效配置和不可完整 unload 提示。
- **仓库/GUI**：有效/无效 `repoPart.txt`、唯一/多个/无仓库、系统盘/启动盘/空间/可写性过滤、确认前后二次校验、取消零副作用、无可用卷和长卷标。
- **并发**：跨进程 load/load、load/startup、startup/startup、startup/clean、load/clean、clean/clean，以及固定锁顺序。
- **跨平台与视觉**：Windows 单元测试和 `tests/e2e/`；Linux/macOS 编译并返回明确 Unsupported。使用项目 Slint 1.17 检查 UI，实际查看软件渲染的浅色、深色、长文本截图，并通过 LCR 在 Windows PE 验证裁切、滚动、按钮、窗口拖动和圆角。

## 参考

- [Edgeless 官方 LocalBoost 文档](https://wiki.edgeless.top/v2/playground/localboost.html)
- [wimbuilder-component：plugin_localboost 原版实现](https://github.com/EdgelessPE/wimbuilder-component/tree/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_localboost)
  - [GUI.wcs](https://github.com/EdgelessPE/wimbuilder-component/blob/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_localboost/GUI.wcs)
  - [installToRepo.wcs](https://github.com/EdgelessPE/wimbuilder-component/blob/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_localboost/installToRepo.wcs)
  - [loadOnBoot.wcs](https://github.com/EdgelessPE/wimbuilder-component/blob/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_localboost/loadOnBoot.wcs)
  - [loadUnit.cmd](https://github.com/EdgelessPE/wimbuilder-component/blob/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_localboost/loadUnit.cmd) / [loadUnit.wcs](https://github.com/EdgelessPE/wimbuilder-component/blob/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_localboost/loadUnit.wcs)
- [wimbuilder-component：EPT 单插件移除逻辑](https://github.com/EdgelessPE/wimbuilder-component/blob/master/_vendor/File_Project/Program%20Files/Edgeless/plugin_ept/ept-remove.cmd)
- ELI 当前实现：[CLI `plugin.rs`](https://github.com/EdgelessPE/eli/blob/master/eli-cli/src/command/plugin.rs)、[`plugin/load.rs`](https://github.com/EdgelessPE/eli/blob/master/eli-lib/src/command/plugin/load.rs)、[`plugin/localboost/load.rs`](https://github.com/EdgelessPE/eli/blob/master/eli-lib/src/command/plugin/localboost/load.rs)
- ELI 规范：[`AGENTS.md`](https://github.com/EdgelessPE/eli/blob/master/AGENTS.md)、[`eli-gui` skill](https://github.com/EdgelessPE/eli/blob/master/.agents/skills/eli-gui/SKILL.md)
