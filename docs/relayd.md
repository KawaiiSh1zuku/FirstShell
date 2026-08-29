# relayd 自托管部署

`firstshell-relayd` 是独立进程，不读取桌面 Vault 或数据库。部署者必须提供 TLS 证书、私钥、固定 target mappings 和逐 client 配额。

## 配置

```json
{
  "quicListen": "0.0.0.0:9443",
  "tlsListen": "0.0.0.0:9444",
  "certificatePem": "C:/FirstShell/relayd/cert.pem",
  "privateKeyPem": "C:/FirstShell/relayd/key.pem",
  "maxConcurrent": 128,
  "mappings": [
    { "name": "ssh-prod", "targetHost": "10.0.0.10", "targetPort": 22 }
  ],
  "clients": [
    {
      "id": "ops-laptop",
      "tokenSha256": "REPLACE_WITH_64_LOWERCASE_OR_UPPERCASE_HEX_CHARACTERS",
      "allowedMappings": ["ssh-prod"],
      "maxConcurrent": 4,
      "maxDurationSeconds": 3600,
      "maxBytes": 10737418240,
      "bandwidthBytesPerSecond": 10485760
    }
  ]
}
```

`tokenSha256` 保存 bearer token 的 SHA-256，不保存原 token。可用 PowerShell 生成随机 token 并计算散列；命令输出包含一次明文 token，应直接写入客户端 OS keyring，不要写入配置、shell history 或工单：

```powershell
$bytes = [byte[]]::new(32)
[Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
$token = [Convert]::ToHexString($bytes)
$hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($token))).ToLowerInvariant()
[pscustomobject]@{ Token = $token; TokenSha256 = $hash }
```

## 启动与网络

```powershell
cargo build --release -p firstshell-relayd --locked
target\release\firstshell-relayd.exe --config C:\FirstShell\relayd\config.json
```

防火墙需允许配置的 UDP `quicListen` 和 TCP `tlsListen`。证书 SAN 必须覆盖桌面 relay profile 的 `TLS 服务名`；桌面 profile 的证书路径指向受信任 PEM/DER 证书，连接不会回退到系统任意 CA。QUIC 与 TCP/TLS 使用同一版本化 hello 和 target policy。

服务输出一行一个 JSON 审计事件。日志可能暴露 client ID、mapping、连接时间和流量大小，应按运维数据分类限制访问和保留；日志不应包含 token、SSH/RDP 凭据或转发内容。

更新前备份配置与密钥并滚动重启。协议版本不匹配会失败关闭；先部署兼容 relayd，再更新桌面客户端。当前没有自动证书轮换协议，轮换时需让新旧证书有明确的运维重叠窗口并同步更新客户端 profile。
