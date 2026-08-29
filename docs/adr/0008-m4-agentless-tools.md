# ADR-0008：M4 采用 agentless 探针、Vault 效率对象与受控远程工具

- 状态：Accepted
- 日期：2026-08-24
- 影响范围：SSH 受控命令、会话广播、SQLite schema、Vault、SFTP/ZMODEM、左侧与底部工作区、服务器二级页

## 背景

M4 需要同时交付主机观测、命令片段与历史、会话广播、远程编辑、压缩和 ZMODEM。它们共享 SSH 身份、host key、Vault 和终端生命周期；如果拆成第二个服务或在 WebView 中直接执行命令，会重复凭据与连接状态，并显著扩大远程命令和本地文件权限面。

监控和压缩需要执行远程程序，但现有安全模型明确排除通用远程命令 Tauri API。片段与历史包含私有主机和命令信息，不能进入明文数据库。广播又必须保证目标集合校验失败时没有部分会话先收到命令。

## 方案比较

| 方案 | 复杂度 | 扩展性 | 主要风险 | 开发成本 | 结论 |
| --- | --- | --- | --- | --- | --- |
| 在现有 SSH/session/Vault 模块内增加固定能力 API | 中 | 可继续复用认证、host key、actor 与迁移 | 模块接口需要保持窄而有界 | 中 | 采用 |
| 新建 monitoring/automation 服务和第二套状态总线 | 高 | 可独立部署 | 重复凭据、连接与锁定生命周期，故障面扩大 | 高 | 拒绝 |
| 在 WebView 拼接 shell、保存片段和历史 | 低 | 快速原型方便 | 注入、秘密泄露、状态分裂，无法满足信任边界 | 低 | 拒绝 |
| 自写 ZMODEM wire protocol | 高 | 可完全定制 | 互操作与边界错误难以审计 | 高 | 拒绝 |
| 使用 `zmodem2 0.7.2` caller-owned 状态机 | 中 | 可接入异步 SSH 和有界文件 I/O | 仍需远端 `rz`/`sz` 与实机互操作验证 | 中 | 采用 |

## 决策

`firstshell-ssh::monitor` 只公开五类版本化探针：host metrics、processes、listening ports、ping 和 traceroute。命令模板固定；target 只接受受限主机名/IP 字符集并拒绝 option smuggling。每次探针限时 8 秒、输出不超过 512 KiB，控制字符和表格行/列在 Rust 解析层截断。周期监控使用独立认证 SSH 连接，最多 16 个、周期 2–60 秒；每个监控只保留 300 点内存环形缓冲，发送给 WebView 前均匀降采样。连续三次失败进入 `degraded`，后续成功可以恢复。

代码片段使用独立加密记录和 optimistic revision；名称、标签、模板和参数均在 Vault payload 内。参数只允许 text、integer、choice、host 和 path 五类，placeholder 集合必须与声明完全相同，渲染在 Rust 完成。UI 的默认动作只向活动 pane 插入文本，不发送回车。命令与简单 `cd` 路径历史保存在一个有界加密效率对象内，最多 500 项，可按连接查询并支持全局清除。

session actor 增加最多 16 个目标的原子广播：持有注册表锁时先验证空集合、重复 ID、缺失会话和所有队列容量，再统一 `try_send`。桌面命令只接受单行、无控制字符、最多 4096 字节的已确认命令并追加一个回车；紧急停止向相同显式目标发送 Ctrl-C。WebView 只持有不透明 session ID。

远程编辑只接受 rooted regular file，限制 2 MiB、UTF-8/BOM 且拒绝 NUL/symlink。打开时返回 SHA-256；保存前和同目录临时文件完整关闭后各核对一次指纹。`russh-sftp` 没有 `posix-rename` 扩展，替换仍存在已记录的极短窗口，不能描述为严格原子覆盖。压缩只允许最多 64 个 rooted source 和新 `.tar.gz` destination，使用 shell quoting 的固定 `tar -czf ... --` 模板、120 秒超时和 64 KiB 输出上限。

ZMODEM 通过远端 `rz --protect`/`sz --` 与 `zmodem2` 状态机工作，单任务总时限 10 分钟、位置上限 4 GiB，并复用 256 项/2 并发传输队列、取消、进度和本地 partial 替换。上传预检查与 `--protect` 双重拒绝覆盖；下载校验远端 advertised filename。绝对本地路径仍只存在于 Rust 原生选择器和 transfer operation。

详情与动态 `ToolsWorkspace` 在左侧栏切换：持久 SSH 终端 Ready 后通过同一个受限 monitor registry 自动启动详情摘要，直接展示 CPU、内存、SWAP、负载、可切换网卡的速率和磁盘容量；工具页只保留片段、历史和广播，不提供手动监控或诊断入口。连接树、分组和搜索迁入由顶栏按钮控制的服务器二级页，打开连接后返回主工作区；主工作区仅切换可见状态而不卸载 Terminal、RDP 或 Operations。底层固定探针仍由 Rust 保留，避免破坏详情监控的共享契约，但不作为用户可见工具。自动监控不覆盖交互 terminal channel，也不为临时快速连接创建额外连接。底部 Operations 增加编辑、压缩和 ZMODEM。两者与 Terminal 保持独立动态 chunk。终端断开时前端停止对应自动监控；Vault 手动或空闲锁定时 Rust 统一关闭 sessions、operations 和 tools，DOM 中不保留解密对象。

## 结果与限制

M4 没有新增服务、通用 shell API 或第二套全局状态。schema 3 只增加 `snippets` 和 `efficiency_state` 两张密文表；监控采样、session target、SFTP editor state、传输路径和 ZMODEM wire state均为进程内资源。

`zmodem2` 的 MIT OR Apache-2.0 与 AGPL-3.0-only 兼容，内存 sender/receiver roundtrip 已自动验证。真实服务器仍需安装兼容的 `lrzsz`，工具缺失、远端不互操作或超过 4 GiB 时失败关闭；这类环境能力不由 FirstShell 自动安装或绕过。

M4 不新增终端背景图片。终端继续复用 M3 已验收的加密 ANSI palette、字体和对比度配置；背景图会引入资源来源、可读性和渲染性能风险，又不属于 M4 权威交付清单，因此不以短期视觉功能扩大边界。
