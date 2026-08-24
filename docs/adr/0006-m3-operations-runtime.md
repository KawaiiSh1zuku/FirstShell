# ADR-0006：M3 网络与文件操作采用 SSH 内部分模块和 Rust 运行时注册表

- 状态：Accepted
- 日期：2026-08-24
- 影响范围：Connector、SSH 跳板、SFTP、端口转发、桌面 IPC、传输队列与拖放

## 背景

M3 需要在 M2 的认证、host key 和双 lane 会话边界上增加 SFTP、代理、跳板链和三类隧道。网络路由和文件操作都必须复用同一套信任策略，同时避免让代理密码、本地绝对路径或任意文件系统能力进入 WebView。

系统代理还存在实现边界：完整 PAC 需要 JavaScript 引擎、PAC 下载和额外 HTTP 信任面，而 M3 只需要可预测的静态代理选择。传输和隧道是短期运行资源；如果过早持久化，会把本地路径、partial 名和易失状态引入数据迁移与 Vault 生命周期。

## 方案比较

| 方案 | 复杂度 | 扩展性 | 安全与维护风险 | 开发成本 | 结论 |
| --- | --- | --- | --- | --- | --- |
| 在 `firstshell-ssh` 内拆分 Connector、SFTP、tunnel 模块 | 中 | 可直接复用认证、host key、连接链和取消语义 | crate 内接口需要保持清晰 | 中 | 采用 |
| 新增顶层 network/transfer crates | 高 | 长期边界更细 | M3 会重复认证编排并扩大依赖倒置和回归面 | 高 | 拒绝 |
| 引入 PAC JavaScript 引擎和网络客户端 | 高 | 可执行完整企业 PAC | 扩大代码体积、供应链和远程脚本信任面 | 高 | M3 拒绝 |
| 每次连接解析一次静态系统代理 | 低 | 后续可通过新 ADR 扩展 PAC | 不能执行 PAC JavaScript | 低 | 采用 |
| 将传输、隧道和拖放任务持久化到 SQLite | 高 | 可跨重启恢复 | 会持久化敏感路径并要求 schema 迁移和清理协议 | 高 | M3 拒绝 |
| Rust 进程内运行时注册表 | 中 | 生命周期与 Vault/窗口一致 | 进程退出后任务不恢复 | 中 | 采用 |

## 决策

Connector、SFTP 和 tunnel 作为 `firstshell-ssh` 的独立模块实现。Direct、SOCKS5、HTTP CONNECT 和系统静态代理共用端点校验、超时、取消与脱敏错误；代理凭据从 OS keyring 直接进入 Rust。系统代理在每次连接开始时解析并固化为 Direct、SOCKS5 或 HTTP CONNECT，不执行 PAC JavaScript。

跳板链由首跳 Connector 和后续 `direct-tcpip` channel 组成，最多四跳。每跳使用自己的认证、host/port 和 host-key 审批证据；配置代理或跳板时禁止 OpenSSH fallback，避免改变用户选择的路由或绕过逐跳信任。

SFTP 使用服务器 canonical root 建立 rooted workspace。所有远程路径保持根内相对语义，进入目录、写入和 chmod 前检查父链并拒绝隐式跟随符号链接。上传和下载使用同目录 partial、取消令牌、续传长度检查及可选 SHA-256。`russh-sftp 2.4` 没有 OpenSSH `posix-rename` 封装：新目标可在同目录 rename；覆盖既有目标需要 remove 后 rename，存在极短替换窗口，不能称为严格原子覆盖。

本地、远程和动态隧道默认只监听 loopback，非 loopback bind 必须带显式 UI 确认。动态转发只实现 SOCKS5 CONNECT/no-auth，每个隧道最多 128 个并发连接；停止远程转发时显式撤销服务器 bind。

桌面端以独立 `DesktopOperations` 运行时注册表持有 SFTP workspace、最多 256 个传输任务、最多 2 个并发传输、隧道和拖放暂存。传输暂停状态为 `pausing -> paused -> queued/running`，取消使用独立的 `cancelling -> cancelled`；暂停保留安全 partial，恢复创建新 generation 并从已确认长度继续。Vault 锁定、workspace 关闭或应用退出会取消并清空运行资源。

原生文件对话框和系统拖放得到的绝对本地路径只保存在 Rust operation 中。WebView 只接收一次性批次 ID、文件名和大小；批次最多 8 个、每批最多 64 个常规文件、5 分钟过期，并在消费后删除。

## 结果

该方案保持 M2 的认证和信任边界，网络路由、SFTP 和隧道可在同一 SSH 实现中共享逐跳生命周期；WebView 不获得代理秘密或本地路径，Vault 锁定能够统一回收运行资源。代价是 M3 任务不能跨进程恢复，系统代理不支持 PAC JavaScript，覆盖既有远程文件也没有严格原子保证。

若后续需要 PAC、跨重启传输或服务器扩展级原子覆盖，必须分别补充网络脚本信任、加密任务持久化和协议能力协商设计，并更新本 ADR、数据模型与安全文档。
