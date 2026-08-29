# 数据模型

## 存储原则

FirstShell 使用 SQLite 作为本地权威存储，通过 `rusqlite` 的 bundled SQLite 和单一数据库 actor 串行处理事务。连接启用 WAL、foreign keys、busy timeout 和明确的同步级别。业务代码不持有裸连接，也不引入 ORM 隐藏查询与迁移行为。

该选择强调可预测延迟、事务一致性、低部署成本和可检查的 SQL。完整决策见 [ADR-0002](./adr/0002-sqlite-encrypted-records.md)。

## 数据库边界

数据库 actor 接受带类型的请求，返回领域对象或稳定存储错误。批量导入、排序、移动、删除和同步落库在事务中执行。只读列表优先使用分页或游标，禁止一次解密无限量历史记录。

启动顺序为：打开数据库、校验 header、配置 pragma、执行迁移、验证 schema、加载最小元数据。任何迁移或完整性校验失败时停止进入工作区，保留原文件并提供恢复路径。

## 概念表

M6 当前 schema version 4 包含下列实际表。后续里程碑新增表必须通过递增迁移落地，不能把规划表当成当前 schema：

| 表 | 明文索引字段 | 加密内容 | 主要用途 |
| --- | --- | --- | --- |
| `schema_migrations` | version、checksum、applied_at | 无 | 迁移历史与篡改检测 |
| `vault_meta` | vault_id、key_version、created_at | 加密校验块 | 仓库状态和解锁校验 |
| `vault_key_slots` | slot_id、vault_id、kind、created_at | 恢复口令包装的 Vault Key | 密钥环丢失后的仓库恢复 |
| `folders` | id、parent_id、sort_order、revision、deleted_at | 名称、备注 | 连接树层级 |
| `connection_profiles` | id、folder_id、kind、revision、deleted_at | 主机、端口、用户、认证引用、标签、备注 | SSH/RDP/本地会话配置 |
| `proxy_profiles` | id、kind、revision、deleted_at | 代理地址、DNS 策略、中继 TLS 名称/证书路径/目标映射 | SOCKS5、HTTP CONNECT、系统代理、FirstShell relay |
| `terminal_profiles` | id、revision | 字体、预设主题、自定义 ANSI palette、键位、编码、行为配置 | 终端外观与交互 |
| `ui_state` | workspace_id、revision | 布局、面板尺寸、最近标签 | 工作区恢复 |
| `snippets` | id、revision、deleted_at | 名称、标签、单行模板、类型化参数 | 加密命令片段与乐观并发 |
| `efficiency_state` | workspace_id、revision | 最多 500 项命令/路径历史 | 有界本地效率状态 |

M0 的 `probe_records` 仍保留用于旧 schema 的真实升级测试，不承载产品数据。M4 选择一个 `efficiency_state` 密文对象而不是逐命令明文索引表；`tunnel_profiles`、`transfer_jobs`、`sync_state` 和 `sync_outbox` 仍属于后续里程碑。M2 的 known_hosts 是应用数据目录中的 OpenSSH 格式安全文件，不是数据库表；若后续需要同步或结构化管理，必须新增迁移和安全决策。

不使用通用 EAV 表承载核心业务实体。具体表能提供更清晰的约束、迁移和查询计划；扩展字段通过版本化 payload 演进，而不是无限制 JSON 堆积。

## 标识、版本与删除

- 业务对象使用随机稳定 UUID，不泄露创建顺序。
- 每个可同步对象维护单调 `revision`，更新必须携带预期 revision。
- 同步对象采用 tombstone 表示删除，经过保留窗口和全设备确认后才能压缩。
- 本地临时任务与可同步配置分离，传输进度默认不跨设备同步。
- 时间统一存 UTC，展示时转换为用户时区；冲突判断不依赖客户端墙钟排序。

## 加密记录

敏感字段序列化为版本化 payload，再整体认证加密：

```text
sealed_payload = {
  version,
  key_id,
  nonce,
  ciphertext
}

aad = table_name || record_id || schema_version || revision
record_key = HKDF(vault_key, table_name || record_id || key_version)
```

每次更新都生成新 nonce。解密后先校验 payload schema 和长度，再映射到领域类型。未知版本不会猜测解析，而是返回可诊断的升级错误。

Vault Key 与 Sync Key 相互独立：本地记录由 Vault Key 保护；进入同步 outbox 前，客户端将规范化对象重新封装为 Sync Key 密文。泄露同步服务数据不能用于解密本地数据库，撤销同步也不要求重写全部本地记录。

M2 不改变 schema version 2。连接加密 payload 新增认证类型、私钥路径、按主机弱算法开关和 OpenSSH 回退开关；终端 payload 新增编码和键位配置。字段使用 serde 默认值兼容 M1 已保存 payload，因此不需要数据库迁移。连接的 `terminalProfileId=null` 表示使用工作区首个终端配置，前端外观与原生编码解析必须采用相同回退；仅当工作区没有终端配置时才使用内置默认值。快速连接只存在于当前前端会话，不写入 SQLite；应用内密码框取得的 SSH 秘密不进入连接 payload，持久连接写入 OS keyring，快速连接只在单次 IPC 和 Rust 可清零内存中存在。

M3 仍不改变 schema version 2。连接加密 payload 增加有序 `jumpHostIds`，代理加密 payload 增加 `dnsMode`；两者使用 serde 默认值读取旧记录，并在 application 层验证引用、最多四跳和传递循环。代理密码仍只进入 OS keyring。SFTP workspace、隧道、传输任务和拖放批次都是当前进程的 Rust 运行时记录，不创建 `transfer_jobs` 或 `tunnel_profiles` 表；Vault 锁定会取消并清空这些记录，本地绝对路径、remote partial 名和进度不持久化。

M3 补充继续保持 schema version 2。terminal profile 加密 payload 增加可选 `palette`；旧记录缺少该字段时按 `None` 读取。`theme=custom` 必须携带完整的前景、背景、光标、选区与 16 个 ANSI `#RRGGBB` 颜色；其他预设只保存稳定 ID 并拒绝冗余 palette。companion SFTP 的 workspace ID、目录缓存和自动抑制状态仍为进程内状态，不写入 SQLite。

M4 migration 3 新增 `snippets` 与 `efficiency_state`。片段的名称、标签、模板和参数整体使用记录级 AEAD；更新与软删除继续受 expected revision 约束。历史按 workspace 聚合在单个密文 payload 中，写入前去重并截断到最近 500 项；查询可按 connection ID 过滤且最多返回请求上限。portable export/import 包含片段，命令历史作为本机效率状态不进入 portable snapshot。监控 300 点环形缓冲、probe diagnostic、广播目标、远程编辑器内容和 ZMODEM/传输状态全部留在进程内。连接进入 Ready 时自动创建的详情监控同样不持久化，断开、锁定或进程退出即释放。

M6 migration 4 在单一 SQLite 事务内重建 `proxy_profiles` 的明文 kind 约束以加入 `relay`，并原样复制 id、kind、revision、deleted_at 和加密 payload。relay payload 增加 `fallbackPort`、`relayServerName`、`relayMapping` 与 `relayCertificatePath`；所有字段使用 serde 默认值读取旧记录。中继 bearer token 与 RDP 密码不进入 payload：它们分别使用独立 OS keyring account，读取后进入 Rust 可清零容器。迁移测试从 schema 3 升级并逐字节验证旧 proxy payload 保留，同时验证新 `relay` kind 可插入。旧版应用无法解释 `relay` 枚举，降级前必须恢复升级前备份或先用 M6 删除 relay profile；不支持直接用 M4 二进制打开包含 relay profile 的 schema 4 工作区。

## 查询与搜索

密文不支持可靠的数据库全文检索。解锁后，应用按用户范围逐批解密名称、标签、片段和可选历史，建立内存搜索索引；锁定时销毁。索引设内存上限，对大历史采用按需分页，不把明文索引写回磁盘。

连接树的父子关系和排序可由明文外键查询，但 UI 只在解锁后获得解密名称。锁定画面不展示最近主机、用户名或历史标签。

## 迁移与备份

迁移文件带递增版本和 checksum，在单事务中执行；CI 从每个受支持 schema 版本升级到当前版本，并验证失败回滚。涉及破坏性变化前创建 SQLite online backup，并明确检查剩余磁盘空间。

备份导出不是简单复制数据库。M1 导出器在解锁态读取一致的领域快照与布局，使用独立 Export Key 重新封装为带版本、Argon2id 参数、salt、nonce 和认证密文的 MessagePack 容器，最大明文为 512 MiB。恢复先完整解密并验证对象关系、字段长度和布局范围，再创建 SQLite online backup，最后以单个事务替换业务记录；验证失败或事务失败时保留原数据。未来若改为临时数据库文件切换，必须另行记录迁移与崩溃恢复语义。

## 同步语义

同步以对象为粒度，协议传递不透明对象 ID、对象类型、revision、父版本、密文、设备签名和 ETag。客户端使用乐观并发：父版本匹配则提交，不匹配则拉取双方版本并生成冲突记录。

首期不引入 CRDT。连接配置、代理、密钥引用和代码片段的误合并风险高于手动解决成本，因此采用字段级差异展示和用户确认。UI 布局可使用 last-writer-wins，但必须作为对象类型策略明确声明，不能成为全局默认。

设备撤销后拒绝其新提交；若怀疑设备密钥泄露，用户可轮换 Sync Key 并重新上传活动对象。服务端删除不等于立即安全擦除，文档必须说明备份和保留策略。

## 生命周期与保留

| 数据 | 默认策略 |
| --- | --- |
| 终端滚屏 | 仅内存，受终端行数上限控制 |
| 会话日志 | 默认关闭；显式开启后写入应用数据目录，只记录远端输出并保留 7 天 |
| 监控采样 | 内存环形缓冲，默认不持久化 |
| 命令/路径历史 | 最多 500 项、加密、支持按连接查询或全局清除；不自动执行 |
| 传输任务 | M3 仅进程内保留；Vault 锁定清空，不保留本地路径或文件内容 |
| tombstone | 在同步保留窗口和设备确认后压缩 |
| 安全审计 | 只保留事件类型和脱敏上下文，按期限轮转 |

数据库损坏、密钥环不可用或认证标签失败时不得返回部分明文或自动重建空仓库。应用应进入只读恢复流程，先保存证据副本，再引导用户从加密备份恢复。

应用外观的浅色、深色和跟随系统偏好不包含敏感数据，M1 将其保存在 WebView `localStorage`，不写入 Vault 或同步对象。面板尺寸和其他可能关联工作内容的布局状态继续由加密 `ui_state` 保存。2026-08-29 的主要布局调整复用 `WorkspaceLayout.rightWidth` 保存移到左侧的详情/工具栏宽度；`leftOpen` 与 `leftWidth` 仅为旧版布局反序列化兼容字段，不再控制当前 UI，因此无需 schema migration，也不会把旧布局读取失败误报为 Vault 损坏。
