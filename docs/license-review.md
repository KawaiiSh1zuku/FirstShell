# 依赖与许可证审查

本文件记录 M0 对实际依赖和后续高风险候选依赖的许可证结论。自动门禁由 `node scripts/check-licenses.mjs` 对 `Cargo.lock` 和 `apps/desktop/package-lock.json` 的完整依赖树执行；新增依赖必须同时更新锁文件并通过该门禁。

## 当前依赖

| 范围 | 主要依赖 | 许可证 | 结论 |
| --- | --- | --- | --- |
| 桌面框架 | Tauri 2 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容 |
| Web UI | React、Vite | MIT | 与 AGPL-3.0-only 兼容 |
| UI 与测试 | `lucide-react`、Vitest、Testing Library、jsdom | MIT；间接依赖含 BSD-2-Clause、BlueOak-1.0.0 | 与 AGPL-3.0-only 兼容，BSD/BlueOak 文本进入发布 NOTICE |
| SSH/SFTP | `russh`、`russh-sftp` | Apache-2.0 | 与 AGPL-3.0-only 兼容，保留 NOTICE 要求 |
| M3 Connector 与校验 | `base64`、`url`、`sha2`、`winreg`、`tokio-util` | MIT OR Apache-2.0；MIT | 与 AGPL-3.0-only 兼容；均已存在于或通过兼容许可证进入锁树 |
| M4 ZMODEM | `zmodem2` 0.7.2 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容；采用 caller-owned I/O 状态机，不引入 GPL CLI 运行时依赖 |
| 终端 | `@xterm/xterm`、`@xterm/addon-fit`、`portable-pty` | MIT | 与 AGPL-3.0-only 兼容；分发时保留上游许可证文本 |
| 内置终端配色 | Solarized、Nord、Dracula、One Dark；Gruvbox（开发例外） | 前四项 MIT；Gruvbox 待确认 | 仅复用公开配色值，不引入运行时依赖；前四项发布 NOTICE 保留对应 MIT 声明；Gruvbox 在 M7 发布前必须取得明确授权或移除 |
| 终端编码 | `encoding_rs` | (Apache-2.0 OR MIT) AND BSD-3-Clause | 与 AGPL-3.0-only 兼容；保留 Apache/MIT/BSD 文本与适用 notice |
| 原生秘密输入 | `tinyfiledialogs` | zlib-acknowledgement | 与 AGPL-3.0-only 兼容；保留源文件声明，并在产品 NOTICE 中致谢 tinyfiledialogs |
| 密码学 | RustCrypto XChaCha20-Poly1305、HKDF、SHA-2 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容 |
| 本地凭据 | `keyring` 及平台后端 | MIT OR Apache-2.0 | 与 AGPL-3.0-only 兼容 |
| 数据库 | `rusqlite`、bundled SQLite | MIT / public domain SQLite | 与 AGPL-3.0-only 兼容 |

完整锁树包含 MPL-2.0 的 Tauri/WebView 解析依赖。MPL-2.0 是文件级义务，分发时必须保留相应许可证和被修改 MPL 文件的可用源码；FirstShell 不修改这些上游文件。含 `OR` 的表达式按其中兼容的 MIT/Apache-2.0 选项使用。

## 已采用与高风险候选

| 能力 | 首选 | 许可证 | 选择依据与门槛 |
| --- | --- | --- | --- |
| 终端 | `@xterm/xterm` 6 | MIT | 已在 M2 引入并复核 fit addon、`portable-pty` 和编码依赖；后续 addon 仍需独立复核 |
| ZMODEM（已采用） | `zmodem2` 0.7.2 | MIT OR Apache-2.0 | 已进入锁树并通过许可证门禁、内存 sender/receiver 往返、取消与受控 I/O 审查；真实 lrzsz 互操作延期 M7 |
| ZMODEM 备选 | `rzsz` 0.1 | Apache-2.0 | CLI 形态、版本较早；不作为默认嵌入方案 |
| ZMODEM 排除候选 | `lrzsz2` 0.7 | GPL-3.0-or-later | 可与 AGPLv3 组合但义务更强，且 CLI 形态不符合当前边界 |
| Windows RDP | FreeRDP helper | Apache-2.0 | 协议与编解码覆盖成熟；只允许在隔离 helper 内通过 FFI/子进程使用，需单独清点 codec 依赖 |
| RDP 备选 | IronRDP 0.17 | MIT OR Apache-2.0 | 纯 Rust 且 MSRV 1.89；窗口集成、兼容性和性能验证不足前不替换首选 |

FreeRDP 的 Apache-2.0 结论来自上游 LICENSE；实际打包前还必须审查启用的 FFmpeg/OpenH264 等可选组件及专利风险，不能把 FreeRDP 主项目许可证视为所有 codec 的结论。

## 发布义务

- 分发桌面端、`syncd` 或 `relayd` 时提供对应 AGPL-3.0 源码获取方式。
- 生成第三方 NOTICE/SBOM，保留 Apache、MIT、BSD、BlueOak、MPL、Unicode、zlib-acknowledgement 和其他适用文本；对 tinyfiledialogs 给出产品致谢。
- 禁止引入无许可证、仅源代码可见、自定义非商业条款、SSPL、BUSL 或与 AGPL 不兼容的依赖。
- 候选依赖只有在真正加入锁文件并重新通过自动与人工审查后才算获准使用。
- 配色集合仓库的统一许可证不能替代单个主题许可证。Gruvbox 按项目负责人决定作为开发阶段临时例外保留，不阻塞当前依赖扫描；M7 发布门禁不得忽略，必须取得明确授权或移除该预设。
