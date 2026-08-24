# ADR-0007：M3 补充采用 companion SFTP、启动快速解锁与版本化终端调色板

- 状态：Accepted
- 日期：2026-08-24
- 影响范围：终端会话生命周期、SFTP 工作区、Vault 启动流程、终端 profile、主要 UI 布局

## 背景

M3 首次验收后的实用性复核发现，SFTP 需要用户再次选择连接，底部 180px 面板又被三层工具栏占用，文件表缺少目录树和可输入地址栏。Vault 已有 OS keyring 快速解锁能力，但程序启动只读取锁定状态；终端 profile 也只有三个由前端硬编码的主题分支，不能表达完整 ANSI 调色板。

补充实现不能让 SFTP 故障阻塞终端、不能把恢复口令或 Vault Key 交给 WebView，也不能把终端主题拆到未加密的 localStorage。

## 方案比较

| 方案 | 复杂度 | 扩展性 | 主要风险 | 开发成本 | 结论 |
| --- | --- | --- | --- | --- | --- |
| React companion coordinator 调用既有 Rust SFTP runtime | 中 | 可按连接共享并保持终端/SFTP 故障隔离 | 需要严格处理 pane 计数、迟到 open 和关闭 | 中 | 采用 |
| Rust 原生复合 Terminal + SFTP session | 高 | 生命周期一致性最强 | 扩大 M2 session API，SFTP/host-key 失败容易耦合终端 | 高 | 暂不采用 |
| 自定义主题保存在加密 terminal profile | 中 | 可同步、导入导出并由 Rust 校验 | payload 需要兼容旧记录 | 中 | 采用 |
| 自定义主题保存在 WebView localStorage | 低 | 实现快 | 状态分裂、不能随加密配置迁移，锁定边界不清晰 | 低 | 拒绝 |

## 决策

每个持久化 SSH connection 在首个 terminal pane 进入 `ready` 后自动创建一个以 `.` 为 canonical root 输入的 SFTP workspace。同一连接的多个 pane 共享该 workspace；最后一个 ready pane 退出时关闭自动 workspace。快速连接不进入该流程，因为它不具备持久化 connection ID。用户手动关闭 SFTP 后，本轮终端生命周期不自动重开，终端全部断开并重新连接后才解除抑制。

companion 只保存不透明 workspace ID 和公开快照。凭据、host-key 校验、远端路径检查和资源回收仍在 Rust；自动 SFTP 失败只进入 Operations 错误状态，不改变终端状态。Operations 即使视觉折叠仍保持挂载以维护 companion 生命周期，但使用 `hidden` 移除布局和可聚焦控件。

SFTP UI 使用单行导航/操作栏、可提交地址栏、左侧懒加载目录树和右侧文件表。地址栏只将 canonical root 内地址转换为相对路径，`..` 或根外绝对地址在前端拒绝，Rust rooted workspace 继续作为最终安全边界。默认底部面板高度由 180 调整为 260 logical px，旧的加密布局值继续按用户设置读取。

应用冷启动读取到 initialized 且 locked 的 Vault 时自动调用既有 OS keyring 解锁。成功前保持 loading UI，失败后展示脱敏错误并直接选择恢复口令表单。恢复口令仍需用户输入，不持久化、不自动填充，也不创建新的“恢复密钥”存储旁路。

terminal profile 增加可选 `palette`：自定义主题必须提供前景、背景、光标、选区和 16 个 ANSI 色，且 Rust 只接受 `#RRGGBB`。非自定义预设不保存 palette。字段使用 serde 默认值读取旧记录，schema version 仍为 2。预设注册表由前端统一解析并提供实时预览；应用浅色/深色主题仍与终端主题分离。

## 结果

终端连接后文件工作区可立即使用，SFTP 面板的纵向空间从三层控制区缩减为一层，目录导航和直接路径跳转同时可用。自动解锁复用既有密钥体系，自定义配色随 Vault 加密、导入导出和后续同步边界演进。

代价是 companion 生命周期仍由前端公开事件编排，需要 StrictMode、迟到 open、多 pane 和隐藏面板回归测试。Gruvbox 预设是开发阶段许可证例外，M7 发布前必须取得明确授权或移除。
