# FirstShell

FirstShell 是一个 Windows 优先的开源远程运维桌面工作台，采用 Rust、Tauri、React 和 SQLite。项目以 clean-room 方式覆盖常用 SSH、SFTP、同步与远程桌面工作流，不复制参考产品的代码、资源或私有协议。

当前已完成 M2 可日常使用的 SSH 终端，下一阶段为 SFTP、代理、跳板与隧道；产品功能按 [`docs/milestones.md`](./docs/milestones.md) 逐个里程碑交付。

## 开发环境

- Rust 1.89.0 stable
- Node.js 24
- Windows 10 或更新版本
- WebView2 Runtime

```powershell
npm --prefix apps/desktop ci
npm --prefix apps/desktop run build
cargo test --workspace --all-targets --locked
node scripts/check-licenses.mjs
```

运行桌面开发环境：

```powershell
npm --prefix apps/desktop run tauri dev
```

M0 的固定基准可通过 `powershell -ExecutionPolicy Bypass -File scripts/run-m0-benchmarks.ps1` 重放，结果写入忽略版本控制的 `target/m0-benchmarks/latest.json`。

M1 自动化验收与延期到 M4 后的实机检查见 [`docs/verification/m1.md`](./docs/verification/m1.md)。
M2 的终端运行时、安全边界、自动化与双视口视觉验收见 [`docs/verification/m2.md`](./docs/verification/m2.md)。

设计、安全模型、数据模型、许可证结论与验收状态见 [`docs/README.md`](./docs/README.md)。项目以 [AGPL-3.0-only](./LICENSE) 发布。
