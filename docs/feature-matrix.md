# 功能矩阵

本矩阵用于控制 FirstShell 的产品范围。兼容指覆盖用户工作流和可观察能力，不代表复制 FinalShell 的内部实现、资源、私有协议、云服务或数据格式。

功能实际状态以 [里程碑](./milestones.md) 为准；本表的“目标”表示计划落在哪个验收门，不表示已经实现。

## 状态定义

| 状态 | 含义 |
| --- | --- |
| Planned | 已进入路线图，尚未通过对应里程碑验收 |
| In progress | 正在实现，仍可能调整接口或交互 |
| Verified | 已通过对应功能、安全和性能验收 |
| Deferred | 明确延后，不阻塞当前发布目标 |
| Excluded | 不属于项目范围或存在不可接受风险 |

M1 基础工作区、M2 SSH 终端、M3 SFTP/复杂网络和 M4 主机监控/效率工具已通过各自的实现与自动化验收，详见 [M1](./verification/m1.md)、[M2](./verification/m2.md)、[M3](./verification/m3.md) 和 [M4 验收记录](./verification/m4.md)。项目负责人于 2026-08-24 决定跳过本轮安装后人工流程、真实远端与 lrzsz 互操作，将其作为未通过的延期项移入 M7；`Verified` 不表示这些人工项已经执行。

项目负责人于 2026-08-24 决定暂缓 M5；以下 M5 能力保留在产品范围内，但恢复前不进入设计、验证或实现流程。

## 基础工作区

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 连接配置 CRUD | M1/M4 补充 | Verified | SSH/RDP/本地类型清晰，详情按钮与连接树右键菜单复用 revision-safe 编辑/删除/撤销 |
| 文件夹、排序、标签与搜索 | M1 | Verified | 密文名称可在解锁态搜索，树操作可撤销 |
| 本地加密仓库 | M1 | Verified | OS keyring、自动锁定、损坏恢复 |
| 多工作区布局持久化 | M1/M4 补充 | Verified | 左侧详情/工具栏宽度加密保存；服务器树由顶栏进入二级页，导航不卸载活动工作区 |
| 加密导入、导出与备份 | M1 | Verified | 独立导出口令、完整性验证、事务恢复 |
| 快速连接 | M2 | Verified | 临时连接不被意外持久化 |

## SSH 与终端

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 密码、公钥、keyboard-interactive | M2 | Verified | 主题化密码框；秘密不进入前端持久状态、argv 或日志，持久凭据进入 OS keyring |
| SSH agent | M2 | Verified | 平台能力探测、转发默认关闭 |
| known_hosts 与首次信任 | M2 | Verified | 密钥变化 fail-closed |
| 多标签与多分屏终端 | M2 | Verified | 布局稳定、键盘焦点正确、会话独立 |
| 保活、断线检测与显式重连 | M2 | Verified | 重连不自动重放敏感输入 |
| 终端主题、字体、编码与键位 | M2/M3 补充 | Verified | 多预设、加密自定义 ANSI palette、实时预览、字体回退和 CJK 输入可验证 |
| 本地终端 | M2 | Verified | 平台 shell 白名单和受控环境继承 |
| 会话日志 | M2 | Verified | 默认关闭、应用目录和 7 天保留策略 |
| 命令历史与补全 | M4 | Verified | 加密、500 项上限、可清除、补全只插入不执行 |
| 命令广播 | M4 | Verified | 16 个目标上限、逐次确认、原子校验、Ctrl-C 紧急停止 |

## SFTP、代理与隧道

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 同工作区 SFTP 文件管理器 | M3 | Verified | 终端 ready 自动 companion、同连接多 pane 共享、故障隔离且不争抢交互带宽 |
| SFTP 目录树与地址跳转 | M3 补充 | Verified | 远端系统 `/` 起始、懒加载目录树、地址直达、`..` 与符号链接穿越拒绝 |
| 上传、下载、暂停、取消、重试 | M3 | Verified | 有界队列、临时文件、失败可恢复 |
| 覆盖、续传与冲突策略 | M3 | Verified | 远端变化检测；新目标同目录 rename，既有目标记录替换窗口 |
| 拖放与多文件队列 | M3 | Verified | 路径规范化、批量错误清晰 |
| 权限、重命名、删除与新建 | M3 | Verified | 工具栏与上下文菜单复用同一动作、危险操作确认、符号链接策略 |
| 在线远程编辑 | M4 | Verified | 2 MiB UTF-8、双重版本指纹、保存冲突和 symlink 拒绝 |
| 压缩打包传输 | M4 | Verified | rooted source、固定 tar 模板、shell quoting 与超时上限 |
| ZMODEM | M4 | Verified | `zmodem2` 状态机、受控队列与内存往返通过；真实 lrzsz 互操作延期 M7 |
| Direct、SOCKS5、HTTP CONNECT | M3 | Verified | 认证、DNS 策略、超时一致 |
| 系统代理 | M3 | Verified | 每次连接固化解析结果 |
| SSH 跳板链 | M3 | Verified | 逐跳 host key、循环和深度限制 |
| 本地、远程、动态端口转发 | M3 | Verified | 默认最小监听范围、明确暴露警告 |

## 监控与效率工具

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| CPU、内存、SWAP、负载、磁盘、网络监控 | M4 | Verified | SSH Ready 后自动启动并显示在详情页，可切换网卡；agentless 独立连接、8 秒超时、300 点环形缓冲 |
| 代码片段分组、标签与搜索 | M4 | Verified | Vault 加密记录、标签搜索与 revision 冲突 |
| 参数化代码片段 | M4 | Verified | 五类变量、placeholder 严格匹配、默认只插入不执行 |
| 路径历史与自动补全 | M4 | Verified | 按连接隔离、500 项有界、可清除 |
| 终端调色板和视觉配置 | M3/M4 | Verified | 复用 M3 加密 ANSI palette；M4 不引入影响可读性与性能的背景图片 |

## 同步、中继与远程桌面

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 自托管同步服务 | M5 | Deferred | 端到端密文、服务端不可解密 |
| 连接、代理、片段和 UI 同步 | M5 | Deferred | 分类型冲突策略和离线 outbox |
| 多设备注册与撤销 | M5 | Deferred | 设备签名、撤销和密钥轮换 |
| 冲突检测与人工解决 | M5 | Deferred | 不静默覆盖高风险配置 |
| RDP 工作区集成 | M6 | Implemented / pending real backend | 隔离 helper、伴随窗口、崩溃隔离、最小凭据；待真实 FreeRDP 验收 |
| 自托管加速中继 | M6 | Implemented / partial verification | 直连优先、QUIC/TLS 主路径、真实 TCP/TLS 回退；待 QUIC 环境互操作 |
| 中继目标映射与配额 | M6 | Implemented / partial verification | 令牌鉴权、服务端目标白名单、时长/流量/带宽/并发配额与审计 |

## 跨平台与发布

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| Windows 安装、更新与卸载 | M7 | Planned | 签名更新、数据保留、失败回滚 |
| macOS 支持 | M7 | Planned | keychain、签名、公证、shell 与字体差异 |
| Linux 支持 | M7 | Planned | Secret Service、Wayland/X11、发行格式 |
| SBOM 与可复现发布记录 | M7 | Planned | 锁定工具链、依赖来源和许可证 |
| 数据库跨版本升级 | M7 | Planned | 所有受支持 schema 升级矩阵 |

## 明确排除项

| 项目 | 状态 | 原因 |
| --- | --- | --- |
| 复制 FinalShell 代码、图像、字体或主题资源 | Excluded | 违反 clean-room 与知识产权边界 |
| 兼容未公开的 FinalShell 私有云协议 | Excluded | 安全、稳定性和合法性不可控 |
| 服务端解密用户凭据 | Excluded | 破坏端到端信任模型 |
| 通用远程命令执行 Tauri API | Excluded | 权限面过大，WebView 风险不可接受 |
| 默认开放公网端口映射 | Excluded | 容易形成未授权暴露和滥用 |
| 首期插件市场 | Deferred | 会显著扩大权限、签名和供应链范围 |
| 多人实时共享终端 | Deferred | 需要独立的授权、审计和协作模型 |

功能来源通过 [本地参考分析摘要](../work/finalshell-reference/report/reference-summary.md) 和 HostBuf v4.6.3 公开功能清单交叉核对；最终交互以 FirstShell 的安全模型和可维护性为准。
