# 依赖与许可证审查

本文件记录 M0 对实际依赖和后续高风险候选依赖的许可证结论。自动门禁由 `node scripts/check-licenses.mjs` 对 `Cargo.lock` 和 `apps/desktop/package-lock.json` 的完整依赖树执行；新增依赖必须同时更新锁文件并通过该门禁。

## 当前依赖

| 范围 | 主要依赖 | 许可证 | 结论 |
| --- | --- | --- | --- |
| 桌面框架 | Tauri 2 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容 |
| Web UI | React、Vite | MIT | 与 AGPL-3.0-only 兼容 |
| UI 与测试 | `lucide-react`、Vitest、Testing Library、jsdom | MIT；间接依赖含 BSD-2-Clause、BlueOak-1.0.0 | 与 AGPL-3.0-only 兼容，BSD/BlueOak 文本进入发布 NOTICE |
| SSH/SFTP | `russh`、`russh-sftp` | Apache-2.0 | 与 AGPL-3.0-only 兼容，保留 NOTICE 要求 |
| 密码学 | RustCrypto XChaCha20-Poly1305、HKDF、SHA-2 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容 |
| 本地凭据 | `keyring` 及平台后端 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容 |
| 数据库 | `rusqlite`、bundled SQLite | MIT / public domain SQLite | 与 AGPL-3.0-only 兼容 |

完整锁树包含 MPL-2.0 的 Tauri/WebView 解析依赖。MPL-2.0 是文件级义务，分发时必须保留相应许可证和被修改 MPL 文件的可用源码；FirstShell 不修改这些上游文件。含 `OR` 的表达式按其中兼容的 MIT/Apache-2.0 选项使用。

## 高风险候选

| 能力 | 首选 | 许可证 | 选择依据与门槛 |
| --- | --- | --- | --- |
| 终端 | `@xterm/xterm` 6 | MIT | 浏览器终端成熟方案；M2 引入时复核 addon 的独立许可证 |
| ZMODEM | `zmodem2` 0.7 | MIT OR Apache-2.0 | 库形态适合接入终端流；M4 必须验证取消、错误恢复和互操作后才能采用 |
| ZMODEM 备选 | `rzsz` 0.1 | Apache-2.0 | CLI 形态、版本较早；不作为默认嵌入方案 |
| ZMODEM 排除候选 | `lrzsz2` 0.7 | GPL-3.0-or-later | 可与 AGPLv3 组合但义务更强，且 CLI 形态不符合当前边界 |
| Windows RDP | FreeRDP helper | Apache-2.0 | 协议与编解码覆盖成熟；只允许在隔离 helper 内通过 FFI/子进程使用，需单独清点 codec 依赖 |
| RDP 备选 | IronRDP 0.17 | MIT OR Apache-2.0 | 纯 Rust 且 MSRV 1.89；窗口集成、兼容性和性能验证不足前不替换首选 |

FreeRDP 的 Apache-2.0 结论来自上游 LICENSE；实际打包前还必须审查启用的 FFmpeg/OpenH264 等可选组件及专利风险，不能把 FreeRDP 主项目许可证视为所有 codec 的结论。

## 发布义务

- 分发桌面端、`syncd` 或 `relayd` 时提供对应 AGPL-3.0 源码获取方式。
- 生成第三方 NOTICE/SBOM，保留 Apache、MIT、BSD、BlueOak、MPL、Unicode 和其他适用文本。
- 禁止引入无许可证、仅源代码可见、自定义非商业条款、SSPL、BUSL 或与 AGPL 不兼容的依赖。
- 候选依赖只有在真正加入锁文件并重新通过自动与人工审查后才算获准使用。
