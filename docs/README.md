# FirstShell 设计文档

FirstShell 是一个面向运维、开发和远程支持场景的开源桌面工作台。项目采用 Rust + Tauri，目标是覆盖 FinalShell 的核心工作流，但不会复制其代码、资源、私有协议或存储实现。

当前仓库已完成 **M2 可日常使用的 SSH 终端**，下一阶段为 M3 SFTP、代理、跳板与隧道。本文档同时描述已实现边界和后续推荐方案，不能据此推断尚未验收的产品功能已经完成；实际进度以 [里程碑](./milestones.md) 为唯一真相源。

## 已确认决策

| 决策项 | 结论 |
| --- | --- |
| 兼容基线 | 参考提供的 FinalShell JAR 的交互方式，并以 HostBuf v4.6.3 功能清单校验能力覆盖 |
| 实现原则 | 安全第一、性能优先、实现优雅，不追求内部实现或像素级 1:1 复刻 |
| 桌面平台 | Windows 优先，随后支持 macOS 与 Linux |
| 技术栈 | Rust、Tauri、React、TypeScript、xterm.js、SQLite |
| 开源协议 | AGPL-3.0 |
| 服务端 | 同步服务和中继服务均提供可自托管的开源实现 |
| 参考方式 | Clean-room：只提取功能、布局和可观察行为，不复用反编译代码或资源 |

## 阅读顺序

1. [架构设计](./architecture.md)：模块边界、运行时模型、连接体系和性能目标。
2. [安全设计](./security.md)：威胁模型、密钥体系、SSH 策略和服务端信任边界。
3. [数据模型](./data-model.md)：SQLite、加密记录、迁移、备份和同步语义。
4. [功能矩阵](./feature-matrix.md)：能力范围、目标里程碑和明确排除项。
5. [里程碑](./milestones.md)：交付顺序、依赖、验收门槛和文档同步规则。
6. [依赖与许可证审查](./license-review.md)：自动门禁、候选依赖与发布义务。
7. [M0 验收记录](./verification/m0.md)：技术 spike、性能基线和已知限制。
8. [M1 验收记录](./verification/m1.md)：Vault、数据层、桌面 UI 自动化证据和实机延期项。
9. [M2 验收记录](./verification/m2.md)：SSH、终端、会话运行时、安全与视觉证据。
10. [M2 开发进度检查点](./2026-08-23_m2-development-progress.md)：2026-08-23 的历史检查点及其后续解决状态。
11. [架构决策记录](./adr/0001-modular-monolith.md)：重要方案选择及其取舍。


## 文档治理

- `docs/milestones.md` 维护里程碑状态；其他文档只引用，不自行声明进度。
- 功能范围变化时，同步更新功能矩阵和对应里程碑。
- 数据结构、安全边界、依赖方向或主要 UI 布局变化时，必须新增或更新 ADR。
- 实现偏离设计时，在合并实现前记录偏离原因、影响和迁移方式。
- 里程碑只有在功能、测试、安全和性能门槛同时满足后才能标记完成。

## 参考资料

- [本地参考分析摘要](../work/finalshell-reference/report/reference-summary.md)
- [本地静态证据记录](../work/finalshell-reference/evidence/E-triage.md)
- [HostBuf FinalShell 功能介绍](https://www.hostbuf.com/t/988.html)

参考 JAR 的产品版本未被可靠识别，因此文档不会把它错误标注为某个确定版本。
