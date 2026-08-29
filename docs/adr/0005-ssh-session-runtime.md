# ADR-0005：SSH 会话采用 Rust actor 与受限兼容回退

- 状态：Accepted
- 日期：2026-08-24
- 影响范围：SSH 认证、host key、会话生命周期、本地 PTY、桌面 IPC 与终端 UI

## 背景

M2 需要同时满足日常交互、秘密隔离和 Windows 本地终端。纯异步 SSH 有利于统一状态与测试，但少数旧服务器可能只与系统 OpenSSH 兼容；Windows ConPTY 又提供阻塞式 master、reader、writer 和 child 句柄，若在 Tokio worker 中跨锁等待，关闭路径容易死锁。

Rust 必须继续拥有连接生命周期和认证材料的持久存储；WebView 除专用密码框的短生命周期局部值外，只持有公开句柄与输出。断线后自动重放输入会在远端状态未知时重复执行命令，因此不能作为透明恢复策略。

## 方案比较

| 方案 | 复杂度 | 扩展性 | 安全与维护风险 | 开发成本 | 结论 |
| --- | --- | --- | --- | --- | --- |
| `russh` 主路径，OpenSSH 显式受限回退 | 中 | 状态、SFTP 和后续隧道可统一演进 | 回退边界需要专门测试 | 中 | 采用 |
| 系统 OpenSSH 作为全部连接主路径 | 低到中 | 结构化事件、认证交互和跨平台行为受进程协议限制 | argv、提示解析和进程清理风险更高 | 低 | 拒绝 |
| 所有 PTY 句柄放入异步任务共享锁 | 中 | 表面统一 | 阻塞 read/wait 会占用 worker，并与关闭锁形成死锁 | 中 | 拒绝 |
| 专用阻塞线程独占 PTY 控制句柄 | 中 | 与平台 PTY API 一致 | 需要显式线程退出和错误事件 | 中 | 采用 |

## 决策

每个远程或本地终端由有界 session actor 管理。注册表默认最多 32 个会话，交互队列容量 256，单次输入最大 64 KiB，连接超时 30 秒。actor 是状态机唯一所有者；断线只进入 `Disconnected`，用户显式重连才创建新 transport，旧输入绝不缓存或重放。

SSH 默认通过 `russh` 建立，执行严格 TOFU/known_hosts 校验、认证、PTY、resize、保活和编码转换。OpenSSH 只有在连接配置显式启用且主路径返回 `SSH_CONNECTION_FAILED` 时才可回退；host key、认证和策略错误不得回退。桌面回退只允许 agent 认证，使用应用自有 known_hosts，不在 argv 中传递秘密。

Windows 本地终端由一个命名阻塞线程独占 PTY master、writer 和 child，并消费有界命令队列；reader 使用独立线程读取输出。所有 read/write/resize 失败映射为稳定脱敏事件。transport 不解释终端字节，ConPTY 的光标位置查询由 xterm 正常响应。

React 端每个 pane 维护 session generation。异步 open 迟到时立即关闭对应 Rust session，防止 StrictMode 重挂载产生泄漏或重复活动记录。xterm 模块动态加载，多标签与每标签最多四分屏只管理布局和公开句柄。

## 结果

正面结果是认证秘密不进入前端持久状态、普通业务 DTO、argv 或日志，持久秘密由 OS keyring 管理；连接失败语义可测试，本地 PTY 不阻塞 Tokio worker，断线不会重复执行命令。代价是专用密码框提交期间 WebView 内存会短暂持有秘密，同时仍需维护纯 Rust 与窄范围 OpenSSH transport，并持续回归 actor、线程关闭和前端异步生命周期。

M3 必须复用现有 actor 与 [ADR-0003](./0003-dual-lane-ssh.md) 的 lane 边界，不得让 SFTP 或代理功能绕过 host key、认证和会话配额。若扩大 OpenSSH 回退认证方式，必须先证明秘密传递不经过 argv、环境变量或临时明文文件，并更新本 ADR。
