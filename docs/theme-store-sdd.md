# 启动盘主题资源管理设计说明

## 目标

`eli theme store <PACKAGE>` 将 `.eth` 主题包、`.eis`、`.ems`、`.esc`、`.ess`、`.els`
资源包或 `.jpg` 壁纸保存到所选 Edgeless 启动盘，并复刻旧版 `instTheme.cmd` 的固定路径、
元数据和替换行为。命令属于启动盘写操作；存在多个候选启动盘时必须通过全局
`--bootdisk` 明确目标。

## 兼容布局

- `.eth` 会整体替换 `Edgeless/Default`，只写入包中存在的规范组件及五行 `Info.txt`。
- `.eis`、`.ems`、`.esc`、`.ess` 分别替换 `Default` 中对应的固定文件，并只更新自己的
  `Info.txt` 行。
- `.els` 只删除旧版 `Edgeless/Default/LoadScreen` 根目录中的 `.jpg`，保留非 JPG 文件与
  子目录内容，再以覆盖模式将归档解压到该目录。
- `.jpg` 将已有 `Edgeless/wp.jpg` 轮换为 `wp_backup.jpg`，再发布新壁纸。
- `Info.txt` 使用旧实现兼容的 GBK 编码、CRLF 和固定五行顺序；主题包缺少的组件沿用旧值，
  首次安装时为 `Unknown`。

当前版本有意保留旧版 LoadScreen 目录规范。迁移到新版 LoadScreen 规范后，`store` 应在应用
主题包时增加“旧格式到新格式”的显式映射，而不是改变或猜测当前归档内容；该映射不属于本次实现。

## 安全与并发

所有归档先通过统一依赖管理解析 7-Zip，再进行路径、链接、加密标记、条目数和展开大小检查。
内容先写入启动盘内的唯一 staging 目录，发布前不修改正式路径。发布过程记录同步刷盘的 JSONL
事务日志，并在失败时逆序回滚；进程异常退出后，下次调用会先恢复未提交事务。

同一启动盘的所有写操作复用 `eli-lib` 的启动盘全局文件锁，因此同进程线程与多进程调用均按
启动盘串行，避免 `Default`、`Info.txt` 和壁纸轮换互相覆盖。不同启动盘没有共享状态，可以并行。

## 验证范围

单元测试覆盖固定目标、GBK 五行元数据、旧版组件顺序、壁纸轮换、失败回滚和中断恢复。
Windows、Linux、macOS 端到端脚本覆盖多启动盘歧义拒绝、显式目标存储和两个进程并发写壁纸。
Edgeless PE 实机测试额外覆盖所有包类型、嵌套资源包校验和旧版 LoadScreen 解包目录。

## 列出与删除

`eli theme list` 只检查六个固定资源位置是否实际存在，输出 `Resource` 和 `Configured`
两列。它不展示 `Info.txt` 中的包名，也不判断 LoadScreen 属于新旧规范。

`eli theme delete <RESOURCE>` 接受 `icon/eis`、`system-icon/ess`、`loadscreen/els`、
`mouse/ems`、`start-menu/esc`、`wallpaper/jpg` 和 `all`。删除非壁纸资源时，对应
`Info.txt` 行会重置为 `Unknown`；删除壁纸只移除活动的 `wp.jpg`，保留
`wp_backup.jpg`。单项资源不存在时返回 `NotFound`，`all` 没有找到资源时视为成功。

列出操作也持有启动盘写锁以获得一致快照；删除复用 store 的事务日志、回滚和中断恢复机制。
所有目标均由固定白名单映射，且不会跟随重解析点或符号链接。
