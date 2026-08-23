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

当前所有产品功能均为 `Planned`；M0 只完成设计和技术验证，不产生可发布功能。

## 基础工作区

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 连接配置 CRUD | M1 | Planned | SSH/RDP/本地类型清晰，修改事务化 |
| 文件夹、排序、标签与搜索 | M1 | Planned | 密文名称可在解锁态搜索，树操作可撤销 |
| 本地加密仓库 | M1 | Planned | OS keyring、自动锁定、损坏恢复 |
| 多工作区布局持久化 | M1 | Planned | 面板与标签恢复不泄露锁定数据 |
| 加密导入、导出与备份 | M1 | Planned | 独立导出口令、完整性验证、原子恢复 |
| 快速连接 | M2 | Planned | 临时连接不被意外持久化 |

## SSH 与终端

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 密码、公钥、keyboard-interactive | M2 | Planned | 秘密不进入 WebView、argv 或日志 |
| SSH agent | M2 | Planned | 平台能力探测、转发默认关闭 |
| known_hosts 与首次信任 | M2 | Planned | 密钥变化 fail-closed |
| 多标签与多分屏终端 | M2 | Planned | 布局稳定、键盘焦点正确、会话独立 |
| 保活、断线检测与显式重连 | M2 | Planned | 重连不自动重放敏感输入 |
| 终端主题、字体、编码与键位 | M2 | Planned | 字体回退和 CJK 输入可验证 |
| 本地终端 | M2 | Planned | 平台 shell 白名单和受控环境继承 |
| 会话日志 | M2 | Planned | 默认关闭、明确路径和保留策略 |
| 命令历史与补全 | M4 | Planned | 加密、可清除、补全不自动执行 |
| 命令广播 | M4 | Planned | 目标会话可见、逐次确认、紧急停止 |

## SFTP、代理与隧道

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 同工作区 SFTP 文件管理器 | M3 | Planned | 与终端共享逻辑会话但不争抢交互带宽 |
| 上传、下载、暂停、取消、重试 | M3 | Planned | 有界队列、临时文件、失败可恢复 |
| 覆盖、续传与冲突策略 | M3 | Planned | 远端变化检测、原子替换 |
| 拖放与多文件队列 | M3 | Planned | 路径规范化、批量错误清晰 |
| 权限、重命名、删除与新建 | M3 | Planned | 危险操作确认、符号链接策略 |
| 在线远程编辑 | M4 | Planned | 版本指纹、保存冲突和编码检测 |
| 压缩打包传输 | M4 | Planned | 受控命令模板、临时目录清理 |
| ZMODEM | M4 | Planned | 采用成熟实现并完成许可证审查 |
| Direct、SOCKS5、HTTP CONNECT | M3 | Planned | 认证、DNS 策略、超时一致 |
| 系统代理 | M3 | Planned | 每次连接固化解析结果 |
| SSH 跳板链 | M3 | Planned | 逐跳 host key、循环和深度限制 |
| 本地、远程、动态端口转发 | M3 | Planned | 默认最小监听范围、明确暴露警告 |

## 监控与效率工具

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| CPU、内存、磁盘、网络监控 | M4 | Planned | agentless、低开销、探针超时 |
| 进程与监听端口视图 | M4 | Planned | 输出上限、权限不足可诊断 |
| Ping 与 traceroute | M4 | Planned | 平台差异、取消和速率限制 |
| 代码片段分组、标签与搜索 | M4 | Planned | 端到端加密、快速检索 |
| 参数化代码片段 | M4 | Planned | 类型化变量、默认只插入不执行 |
| 路径历史与自动补全 | M4 | Planned | 按主机隔离、可清除 |
| 终端背景和视觉配置 | M4 | Planned | 可读性、对比度和性能上限 |

## 同步、中继与远程桌面

| 能力 | 目标 | 状态 | 验收重点 |
| --- | --- | --- | --- |
| 自托管同步服务 | M5 | Planned | 端到端密文、服务端不可解密 |
| 连接、代理、片段和 UI 同步 | M5 | Planned | 分类型冲突策略和离线 outbox |
| 多设备注册与撤销 | M5 | Planned | 设备签名、撤销和密钥轮换 |
| 冲突检测与人工解决 | M5 | Planned | 不静默覆盖高风险配置 |
| RDP 工作区集成 | M6 | Planned | 隔离 helper、崩溃隔离、最小凭据 |
| 自托管加速中继 | M6 | Planned | QUIC/TLS 优先、TCP/TLS 回退 |
| 中继端口映射 | M6 | Planned | 鉴权、目标白名单、时长和流量配额 |

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
