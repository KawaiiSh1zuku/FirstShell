# ADR-0004：Windows RDP 采用隔离的 FreeRDP helper

- 状态：Accepted
- 日期：2026-08-23
- 影响范围：RDP、进程隔离、本地 IPC、凭据生命周期、Windows 打包

## 背景

RDP 客户端需要处理复杂协议、图形编解码、Windows 输入和窗口生命周期。把原生 RDP 引擎直接链接进 Tauri 主进程会扩大崩溃与内存安全影响面；直接启动系统 `mstsc` 虽然简单，却无法满足工作区嵌入、版本化控制和一致错误处理。

## 决策

Windows 首选在独立 helper 进程中使用 FreeRDP。桌面主进程与 helper 只通过版本化 Windows named pipe 协议通信；每次启动生成随机令牌，通过 stdin 交给 helper，令牌不得出现在 argv、环境变量或日志中。named pipe 只接受本机客户端并限制单帧大小。

helper 不获得 Vault Key，只接收单次连接需要的最小凭据和显示参数。M6 选择监督同目录 `wfreerdp.exe` 的 CLI 适配，而不是把 FreeRDP ABI 链接进 helper：唯一 argv 为 `/args-from:stdin`，全部连接参数和密码通过 stdin 逐行传入；helper 提供 companion 模式，并保留 `/parent-window` 嵌入协议。该方案保持崩溃边界且避免绑定不稳定 ABI，但真实窗口嵌入、DPI、codec 和发布文件仍必须在 M6/M7 验收中单独验证。

## 备选方案

| 方案 | 复杂度 | 扩展性 | 主要风险 | 结论 |
| --- | --- | --- | --- | --- |
| 系统 `mstsc` | 低 | 低 | 无法嵌入、控制和统一诊断 | 拒绝作为主路径 |
| 主进程内 FreeRDP | 中 | 高 | 原生崩溃与大依赖面进入 Tauri 信任边界 | 拒绝 |
| 隔离 FreeRDP helper | 中到高 | 高 | IPC 与窗口集成复杂 | 采用 |
| 隔离 IronRDP helper | 高 | 高 | 当前兼容性与性能验证不足 | 保留备选 |

## 结果

RDP 故障不会直接破坏 SSH、SFTP、Vault 或 WebView 进程，主应用也不绑定具体 RDP ABI。代价是需要维护 helper 生命周期、协议兼容、窗口嵌入和发布依赖清单。

若 M6 证据表明 FreeRDP 无法满足安全、兼容或打包门槛，可以在不改变 IPC 边界的情况下替换为 IronRDP；替换引擎仍需更新本 ADR 和许可证审查。
