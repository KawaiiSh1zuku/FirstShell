# M2 开发进度检查点

> 2026-08-24 解决状态：下述阻塞已全部解除，M2 已完成并由 [M2 验收记录](./verification/m2.md)取代为当前证据；本文后续内容保留为 2026-08-23 的历史检查点，不再表示当前工作树状态。
>
> 记录时间：2026-08-23；分支：`master`；基线提交：`925bad8`。
> 当前结论：M2 已进入集成收口阶段，但尚未完成，当前改动未提交。Windows 本地 PTY 烟测仍然失败，全量门禁、许可证复核和正式验收文档也尚未完成。

本文用于从当前工作树继续开发。里程碑的正式状态仍以 [milestones.md](./milestones.md) 为准；在退出门槛全部通过前，不应将 M2 标记为 `Complete`。

## 当前交付状态

| 工作块 | 状态 | 已实现内容 | 继续开发时的关注点 |
| --- | --- | --- | --- |
| 领域模型与策略 | 已实现，待全量复验 | SSH 认证类型、终端编码/键位、弱算法例外、OpenSSH 回退配置；旧 M1 JSON 使用 serde 默认值兼容 | 复跑 application/domain 全量测试 |
| 会话运行时 | 部分完成 | 有界会话注册表、32 会话默认上限、64 KiB 单次输入上限、状态机、连接超时、尺寸校验、显式重连且不重放输入 | Windows 本地 PTY 驱动需修复后重跑全部 session 测试 |
| 纯 Rust SSH | 已实现，已有集成证据 | password/private-key/keyboard-interactive/agent、严格 TOFU、host key 变化阻断、按主机弱算法例外、PTY、resize、保活、UTF-8/GBK/Big5 和 banner 清洗 | 完成全量 Clippy/测试及最终秘密扫描 |
| OpenSSH 兼容回退 | 已实现受限入口，待复验 | 仅在连接类失败时回退；强制 app-owned known_hosts；不把秘密放入 argv；桌面入口目前限定 agent 认证 | 增加/确认不回退 host key 与认证错误的测试 |
| Tauri 会话 IPC | 已实现，待全量复验 | 公开会话 DTO、Tauri Channel、原生凭据对话框到 OS keyring、Vault 锁定关闭会话、日志开关和 7 天清理 | 最近的 quick connect/fallback 调整后需重新执行 desktop check |
| 终端工作台 | 已实现，已有前端证据 | xterm.js、多标签、最多四分屏、每 pane 独立会话、尺寸同步、TOFU 确认、显式重连、临时快速连接、显式日志 | 进行浏览器/桌面视觉检查及真实 SSH 手工烟测 |
| 依赖与文档 | 未收口 | 已新增 Rust/前端依赖并更新锁文件 | 更新许可证白名单和审查文档；同步架构、安全、数据模型、功能矩阵、里程碑和 M2 验收记录 |

## 已落地的关键边界

1. Rust 侧是会话状态、认证材料、known_hosts 和日志策略的唯一真相源。WebView 只接收会话 ID、公开状态、host key 指纹证据和终端字节流。
2. `russh` 是默认 SSH 主路径。系统 OpenSSH 只在用户显式启用且发生连接类兼容错误时作为回退，不处理 host key 变化或认证失败。
3. 默认算法集合移除 SHA-1 `ssh-rsa`；旧算法只允许按主机显式开启，并以较低优先级追加。
4. 会话断开后不缓存或重放用户输入；恢复必须由用户显式触发，避免命令在错误的远端状态下执行。
5. 会话日志默认关闭，只记录远端输出，不记录键盘输入；日志保存在应用目录并按 7 天策略清理。
6. xterm 工作区使用动态加载，避免把约 342 KiB 的终端 chunk 合入主入口；最后一次生产构建已消除原先超过 500 KiB 的入口告警。

## 当前工作树

基于 `925bad8` 的 M2 修改涉及以下区域，全部仍未提交：

```text
Cargo.toml / Cargo.lock
crates/domain
crates/application
crates/session
crates/ssh
apps/desktop/src-tauri
apps/desktop/src
apps/desktop/package.json / package-lock.json
```

新增且尚未跟踪的产品源码包括：

```text
apps/desktop/src-tauri/src/session_commands.rs
apps/desktop/src/QuickConnectDialog.tsx
apps/desktop/src/TerminalWorkspace.tsx
apps/desktop/src/TerminalWorkspace.test.tsx
```

`work/master-route-20260823-180914/` 是既有未跟踪参考目录，不属于 M2 产品提交，后续暂存时必须继续排除。

## 验证证据

以下结果来自本次开发过程。标记“需重跑”的命令在后续代码调整后尚未形成最终门禁证据。

| Evidence | 命令或观察 | 结果 | 结论 |
| --- | --- | --- | --- |
| E-001 | `cargo test -p firstshell-ssh --locked` | 4 个单元测试与 3 个集成测试通过 | TOFU、密钥变化、认证失败、恶意 banner、PTY 回显和编码路径已有自动化覆盖 |
| E-002 | `npm test`（`apps/desktop`） | 2 个测试文件、9 项测试通过 | 工作区分屏、host key 显式确认、日志默认关闭及 M1 回归通过 |
| E-003 | `npm run build`（`apps/desktop`） | 生产构建通过；xterm 被拆分为独立动态 chunk | 前端类型检查和打包路径可用，入口体积告警已处理 |
| E-004 | `cargo check -p firstshell-desktop --all-targets --locked` | 在后续 quick connect/fallback 小改动前通过 | 桌面 adapter 曾通过编译，但不能代替当前工作树的最终复验 |
| E-005 | `windows_local_terminal_should_start_whitelisted_shell` | Command Prompt PTY 未收到预期输出；失败后曾残留 `cargo`/`whoami` 进程，已手动终止 | 本地 PTY 的阻塞读写与关闭生命周期仍不正确，M2 不可验收 |
| E-006 | `git diff --check` | 当前检查通过，仅报告 Git 的 LF/CRLF 提示 | 未发现空白错误；换行提示不是本次功能阻断 |

### 发现与判断

| Finding | 证据 | 判断 | 处理路径 |
| --- | --- | --- | --- |
| F-001 | E-001、E-002、E-003 | SSH 主路径和终端 UI 已达到可集成状态 | 保持现有 Rust/WebView 权限边界，集中完成收口验证 |
| F-002 | E-005 | `portable-pty` 当前由 Tokio worker 驱动阻塞句柄，Windows 下读写或资源释放存在生命周期问题 | 将 PTY command loop 移到专用标准线程，使 master/writer/child 在同一阻塞所有权域内关闭，再验证 EOF 和进程回收 |
| F-003 | E-004 | 最近的桌面层调整没有最终编译证据 | PTY 修复后重新运行 desktop `cargo check`、Clippy 和全量测试 |

## 继续开发路径

1. 重构 `crates/session/src/lib.rs` 的本地 PTY：专用线程持有 `master`、`writer` 和 `child`，使用 `blocking_recv` 消费命令；关闭时 kill 并释放所有句柄，保证 reader 收到 EOF。
2. 单独运行 Windows PTY 烟测，确认命令输出、关闭返回和子进程回收；再运行 `cargo test -p firstshell-session --locked`。
3. 补齐 quick connect 与 OpenSSH 回退边界测试，确认 WebView command 参数、argv 和日志均不包含 SSH 密码或私钥口令。
4. 审查 `portable-pty`、`encoding_rs`、`tinyfiledialogs`、`@xterm/xterm`、`@xterm/addon-fit` 及新增传递依赖的许可证，更新自动门禁白名单和 `docs/license-review.md`。
5. 运行完整门禁：Rustfmt、workspace Clippy `-D warnings`、workspace tests、前端 tests/build、许可证检查、文档链接检查和 `git diff --check`。
6. 完成真实桌面视觉检查、10 个活跃终端压力验证和本地/测试 SSH 烟测，保存可重复证据。
7. 同步架构、安全、数据模型、功能矩阵、里程碑与 `docs/verification/m2.md`；确认全部退出门槛后再把 M2 标记为 `Complete`。
8. 仅暂存 M2 产品文件和同步文档，排除 `work/` 与本地规划文件，复核 staged diff 后创建独立 M2 提交。

建议从以下窄测试开始恢复工作：

```powershell
cargo test -p firstshell-session windows_local_terminal_should_start_whitelisted_shell --locked -- --nocapture
cargo test -p firstshell-session --locked
cargo check -p firstshell-desktop --all-targets --locked
```

如果首个测试再次超时，应先检查并终止仅由该测试启动的 `cargo`、`cmd` 或 shell 子进程，再继续调试，避免后台进程影响后续结果。
