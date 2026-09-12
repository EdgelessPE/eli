---
name: lcr
description: Use Lite Command RPC (lcr) on Windows to execute commands, manage asynchronous processes, inspect or control the desktop, and transfer files over HTTP.
---

# LCR

使用 [Lite Command RPC](https://github.com/Cnotech/lite-command-rpc) 在需要被调试的 Windows 主机上通过 HTTP 执行操作，服务默认端口为 `9527`。

所有接口均为 `POST`。以下示例省略重复的 `curl -X POST http://127.0.0.1:9527` 前缀。

| 接口简介 | 请求示例 | 响应示例 |
| --- | --- | --- |
| `/exec`：执行命令并等待完成。请求可选 `cwd`、`timeout`、`interpreter`、`script_mode`、`detached`、`output_encoding`。 | `/exec -H "Content-Type: application/json" --data-raw '{"command":"echo hello","interpreter":"cmd"}'` | `{"ok":true,"exit_code":0,"stdout":"...","stderr":"","timed_out":false,"error":null}` |
| `/exec/stream`：流式执行命令，返回 NDJSON 事件。请求字段同 `/exec`。 | `/exec/stream -H "Content-Type: application/json" --data-raw '{"command":"ping 127.0.0.1 -n 4"}'` | `{"type":"stdout","data":"..."}`，最终为 `exit`、`timeout` 或 `error` 事件。 |
| `/spawn`：异步启动命令，立即返回会话。请求字段同 `/exec`。 | `/spawn -H "Content-Type: application/json" --data-raw '{"command":"ping 127.0.0.1 -n 10"}'` | `{"session_id":"1234-1","pid":5678,"status":"running"}` |
| `/spawn/result`：查询异步任务状态及新增输出；轮询时传回 `*_next_offset`。 | `/spawn/result -H "Content-Type: application/json" --data-raw '{"session_id":"1234-1","stdout_offset":0,"stderr_offset":0}'` | `{"session_id":"1234-1","status":"exited","exit_code":0,"stdout":"done\r\n","stderr":"","stdout_next_offset":6,"stderr_next_offset":0}` |
| `/spawn/terminate`：终止异步任务及其进程树。 | `/spawn/terminate -H "Content-Type: application/json" --data-raw '{"session_id":"1234-1"}'` | 与 `/spawn/result` 相同，主动终止时 `status` 为 `terminated`。 |
| `/screenshot`：截取主屏幕。 | `/screenshot --output screenshot.png` | PNG 二进制数据。 |
| `/windows`：枚举顶级窗口并标识前台窗口。 | `/windows` | `{"foreground_hwnd":"0xA12BC","windows":[{"hwnd":"0xA12BC","title":"Command Prompt","foreground":true}]}` |
| `/control`：按顺序聚焦窗口或模拟键盘、鼠标输入。 | `/control -H "Content-Type: application/json" --data-raw '{"actions":[{"type":"focus_window","hwnd":"0xA12BC"},{"type":"text","text":"hello"}]}'` | `{"ok":true,"completed_actions":2}` |
| `/download`：下载 Windows 主机上的文件。 | `/download -H "Content-Type: application/json" --data-raw '{"path":"D:\\Desktop\\test.7z"}' --output test.7z` | 文件二进制数据。 |
| `/upload`：上传文件；目标已存在时不会覆盖。 | `/upload -H "Content-Type: application/octet-stream" -H "X-File-Path: D:\Desktop\uploaded.7z" --data-binary "@D:\Download\source.7z"` | `{"ok":true,"path":"D:\\Desktop\\uploaded.7z","bytes":123456}` |
| `/version`：查询当前 LCR 版本和 PID。 | `/version` | `{"ok":true,"version":"0.8.2","pid":2424}` |
| `/update`：上传新的 `lcr.exe` 并自更新。 | `/update -H "Content-Type: application/octet-stream" --data-binary "@D:\Download\lcr.exe"` | HTTP 202，`{"ok":true,"status":"restarting","current_version":"0.8.2","candidate_version":"0.8.3","pid":2424,"bytes":987654}` |

需要管理员权限时，可尝试向 `/exec`、`/exec/stream` 或 `/spawn` 传入 `"require_admin": true`。服务未提升时会弹出 UAC；该能力默认关闭，拒绝或取消提权会返回错误。

在通过 WCS 启动的 WinPE 中，`/control` 会把控制动作放到以完整允许权限打开的当前输入桌面上的隔离辅助进程中执行；只请求窗口枚举和对象写入权限会导致 `SendInput` 返回 `ERROR_ACCESS_DENIED`。若系统仍因 UIPI 等外部限制阻止全局 `SendInput`，前台目标为经典 `cmd.exe` 等 Windows 控制台时，`keyboard` 和 `text` 会自动写入控制台输入缓冲区；该后备方式不适用于非控制台窗口或鼠标动作，失败时仍返回 `409 Conflict`。

自更新时直接把候选 exe 作为 `/update` 的请求体，文件上限为 64 MiB。LCR 会先运行候选程序的内置探针，通过后才返回 202、替换当前程序并按原参数重启；新进程启动失败时会恢复旧版本。接口不校验 SHA256，同版本更新也允许。收到 202 后轮询 `/version`，以 PID 已变化且版本符合预期作为切换完成的判断。探针失败返回 400，服务保持运行。

服务默认监听 `127.0.0.1:9527`，监听地址按命令行 `--listen`、配置文件 `listen`、内置默认值的优先级确定。服务按 `--config PATH`、当前工作目录下的 `lcr.toml`、`lcr.exe` 同目录下的 `lcr.toml` 这一优先级读取配置；自动查找时只有文件不存在才会继续下一位置，其他读取错误会拒绝启动。配置可通过 `work_dir` 将命令工作目录、上传和下载限制在允许目录内，并通过 `command_allowlist` 限制 `/exec`、`/exec/stream` 和 `/spawn`；普通条目为忽略大小写的前缀，`/…/` 条目为忽略大小写的正则。正则按 UTF-8 字节匹配，仅支持 ASCII 字符类和 ASCII 大小写匹配；`.*` 等表达式仍可覆盖 Unicode 参数。`work_dir` 为字符串时可作为省略 `cwd` 和相对文件路径的默认根目录；为字符串数组时命令必须显式指定位于任一允许根目录内的 `cwd`，文件传输必须使用绝对路径。Windows 上路径目录句柄会保持到操作完成，以防重解析点竞态。白名单启用时只允许 `program` 和 `args`，禁用 `command`，并拒绝 CMD、PowerShell、Python、Node 等命令或脚本解释器；匹配文本由程序名和 JSON 引号化的全部参数组成。请求被策略拒绝时返回 HTTP 403，JSON `msg` 字段说明原因并列出允许的 program 规则或工作目录。

需要完整字段、限制或错误语义时，查看 [README.md](https://github.com/Cnotech/lite-command-rpc/blob/master/README.md)。
