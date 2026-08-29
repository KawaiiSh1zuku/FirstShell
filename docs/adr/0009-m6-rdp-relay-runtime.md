# ADR-0009：M6 RDP helper 与 relay 复用受控连接边界

- 状态：Accepted
- 日期：2026-08-29
- 影响范围：RDP、SSH Connector、proxy profile、SQLite migration、keyring、relayd

## 背景

M6 同时引入本机原生 RDP 引擎和远程网络中继。两者都处理高风险凭据与不可信网络输入，但不能获得 Vault Key，也不能把原生崩溃、任意端口转发或新全局状态带入 SSH 核心。

## 方案比较

| 方案 | 复杂度 | 扩展性 | 风险 | 成本 | 结论 |
| --- | --- | --- | --- | --- | --- |
| helper 内直接链接 FreeRDP ABI | 高 | 高 | ABI/codec 崩溃仍隔离，但构建和升级耦合重 | 高 | 暂不采用 |
| helper 监督同目录 `wfreerdp.exe` | 中 | 中高 | 需维护参数协议和发布清单 | 中 | 采用 |
| relay 新建独立 profile/引用体系 | 高 | 高 | 重复连接引用、CRUD 与迁移 | 高 | 拒绝当前阶段 |
| relay 扩展现有 proxy route | 中 | 中 | 必须严格区分 token 与代理密码语义 | 低到中 | 采用 |

## 决策

RDP 继续使用 ADR-0004 的独立 helper。桌面与 helper 使用版本 2 bounded named-pipe 协议和一次性 stdin token；helper 只允许同目录 FreeRDP，参数通过 `/args-from:stdin` 传递。窗口默认 companion，IPC 保留 parent HWND 嵌入模式以避免将来改变秘密与生命周期边界。

relay 作为 `ProxyKind::Relay` 进入既有 connection `proxy_id` 引用。profile 的加密 payload 只保存 relay host、QUIC/TLS port、TCP/TLS fallback port、TLS server name、证书路径和服务端 mapping；token 保存到独立 keyring 项。Connector 固定执行 direct -> QUIC/TLS -> TCP/TLS，SSH host-key 和认证仍在建立字节流后由既有 `russh` 路径处理。

`relayd` 配置声明 target mapping 和 client policy。客户端不能提交目标 host/port；server 在连接目标前完成 token、mapping 和并发授权，并在传输期间执行时长、总字节与带宽限制。server 只记录脱敏结构化审计。

## 结果与兼容

schema 4 只放宽 `proxy_profiles.kind` 并以 serde 默认字段扩展密文 payload，不新增明文目标表。schema 3 到 4 在单事务内复制原记录并有保留测试；含 relay profile 的工作区不支持直接降级到 M4。协议不承诺跨 major version 猜测兼容，版本不匹配必须失败关闭。

该实现完成了代码与自动化边界，但 M6 只有在真实 FreeRDP 产物、RDP 服务、QUIC 网络、降级与长时资源测试通过后才能从 `In progress` 改为 `Complete`。
