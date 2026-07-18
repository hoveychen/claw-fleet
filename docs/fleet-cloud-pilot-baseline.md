# Fleet Cloud v1 内部试点基线

- **状态：** Approved with one pilot exception
- **批准时间：** 2026-07-18（Asia/Shanghai）
- **Accountable owner：** Boss
- **Responsible owner：** Fleet engineering
- **对应计划：** `fleet-cloud-v1` P1
- **RFC：** [[arch/fleet-cloud-api-v1]]
- **实施计划：** [[arch/fleet-cloud-v1-implementation-plan]]

## 1. 试点业务

| 字段 | 冻结值 |
|---|---|
| 工单入口 | GitHub Issues |
| 仓库 | `https://github.com/hoveychen/claw-fleet` |
| 外部业务 ID | 格式为 `github:hoveychen/claw-fleet#123`，末段使用真实 Issue number |
| Fleet Project slug | `fleet-cloud-pilot` |
| 任务类型 | 缺陷修复、依赖升级、代码审计、文档/调研 |
| 目标样本 | 100 个真实 GitHub Issues，不用 mock 工单计入验收 |
| Task 创建触发 | Issue 加 `fleet-task` label |
| Task 状态回写 | Issue comment + `fleet:queued/running/waiting/succeeded/failed/cancelled` label |
| 成功产物 | PR URL 或最终 Markdown/HTML report Artifact；Task 为 `succeeded` |
| 失败产物 | 稳定 error code、最后 Run、完整审计和可恢复/不可恢复说明 |

不在试点范围内：其他仓库、Jira、自研工单、外部客户、多租户计费出账。

## 2. GitHub 鉴权

采用仅安装到 `hoveychen/claw-fleet` 的 GitHub App。

权限：

```text
Issues: Read & write
Pull requests: Read & write
Contents: Read
Metadata: Read
Checks: Read
Actions: Read
```

GitHub App 运行时 secret 名称：

```text
FLEET_GITHUB_APP_ID
FLEET_GITHUB_INSTALLATION_ID
FLEET_GITHUB_PRIVATE_KEY
FLEET_GITHUB_WEBHOOK_SECRET
```

secret 值只进入本地 Docker secret 或 Muvee secret store，不写入仓库、日志、Event、transcript 或 Artifact。

## 3. 环境拓扑

### 3.1 P2–P9 本地开发环境

实测宿主：

```text
Host: Harrys-MacBook-Pro.local
OS: macOS Darwin 25.3.0 arm64
Host CPU: 10 cores
Host RAM: 24 GiB
Host free disk at freeze: 268 GiB
Docker Engine: 29.4.1
Docker assigned CPU: 10
Docker assigned RAM: approximately 8 GiB
```

Docker Compose service 名称：

```text
control-plane
postgres
minio
runner
hosted-web
pilot-host
```

本地 Runner 最大并发为 **1**。Docker 不挂载 `/var/run/docker.sock`。workspace、provider home 和数据库/对象存储分别使用 named volume。

```text
fleet_cloud_workspace
fleet_cloud_claude_home
fleet_cloud_codex_home
fleet_cloud_postgres
fleet_cloud_minio
```

Claude/Codex 凭证通过一次性交互登录写入各自专用 volume，不挂载宿主完整 `~/.claude`、`~/.codex` 或 SSH home。

### 3.2 P10 验收环境

| 字段 | 冻结值 |
|---|---|
| 区域 | 香港 |
| 控制面平台 | Muvee |
| 控制面域名 | `https://fleet-cloud.muveeai.com` |
| API Base URL | `https://fleet-cloud.muveeai.com/api/v1` |
| iframe 样板宿主 | `https://fleet-pilot.muveeai.com` |
| Runner | 香港独立 Linux VM |
| Runner 规格 | 8 vCPU / 16 GiB RAM / 200 GiB SSD |
| Runner 最大并发 | 2 |
| 容器平台 | Docker Engine + Compose |
| 数据库 | PostgreSQL 持久卷 |
| 对象存储 | MinIO 持久卷 |

Hosted Console、control-plane API 和 SSE 共用 `fleet-cloud.muveeai.com`；不使用跨域 cookie，业务 API 使用 Bearer Project API Key，iframe 使用短时 embed token。

## 4. iframe 与浏览器边界

生产 allowlist：

```text
https://fleet-pilot.muveeai.com
```

开发 allowlist：

```text
http://localhost:5173
https://fleet-pilot.muveeai.com
```

embed token 默认 TTL 为 10 分钟，最长 60 分钟；scope 必须绑定 Project、可选单一 Task、views 和 allowed origins。token 只存内存，不进入 URL query、localStorage 或 sessionStorage。

GitHub Issue 内放 Hosted Console Task deep link，不尝试在 github.com 页面内嵌 iframe。

## 5. Runner 网络边界

Runner 只发起出站连接。初始 allowlist：

```text
fleet-cloud.muveeai.com:443
api.github.com:443
github.com:443
objects.githubusercontent.com:443
api.anthropic.com:443
claude.ai:443
chatgpt.com:443
auth.openai.com:443
```

DNS 与 NTP 使用宿主/云平台默认服务。Runner 不开放公网入站端口；SSH 仅用于 Linux VM 运维，并受现有管理员密钥与防火墙控制。

## 6. Project API Key

| 字段 | 冻结值 |
|---|---|
| Key logical name | `flk_pilot_github_issues` |
| Scope | `fleet-cloud-pilot` Project |
| 权限 | Task create/read/control、Decision read/respond、Event read、Artifact read |
| 默认过期 | 无自动过期 |
| 轮换 | Boss 发起手动轮换 |
| 重叠窗口 | 新旧 Key 最多并存 7 天 |
| 泄漏响应 | 立即吊销旧 Key，签发新 Key，审计受影响 request |
| 服务端保存 | prefix + server-peppered hash，不保存明文 |
| 审计 | created_at、created_by、last_used_at、last_used_ip、revoked_at |

**风险说明：** 手动轮换可能长期不发生。Beta 前必须增加到期策略或自动轮换集成；试点运维面板持续显示 Key age。

## 7. 全量记录与脱敏

默认上云：Task/Run/Decision/Event、完整 transcript、tool input/output、usage、Artifact metadata 和选定 Artifact object。

永不上云：

```text
provider credential plaintext
GitHub App private key plaintext
Project API Key plaintext
SSH private keys
runner local absolute paths
entire repository snapshot unless explicitly packaged as an Artifact
```

上传前按以下顺序脱敏：

1. 结构化 header：`Authorization`、`Proxy-Authorization`、`Cookie`、`Set-Cookie`、`X-Api-Key`。
2. 环境变量名：包含 `TOKEN`、`SECRET`、`PASSWORD`、`PRIVATE_KEY`、`API_KEY`、`COOKIE`。
3. PEM private-key block。
4. GitHub、Anthropic、OpenAI 和 Fleet token 已知格式。
5. 启动时注入的 literal secret fingerprint。

替换格式如 `[REDACTED:authorization]`、`[REDACTED:private_key]`，记录 kind 与 count，不记录原文或可逆 hash。

## 8. 保留与删除

| 数据 | 默认保留 |
|---|---:|
| transcript / tool input / tool output | 30 天 |
| Event 明细 | 30 天 |
| 原始日志与用户附件 | 30 天 |
| PR/report/patch/验收截图 Artifact | 180 天 |
| usage 小时汇总 | 365 天 |
| 最小审计 tombstone | 365 天 |

按 Task 手动删除优先于默认保留期：立即写审计 tombstone，排队删除 object、transcript、tool body 和 rich-card asset；仅保留不含内容的计量汇总与删除审计。

## 9. 备份例外

Boss 决定：**内部试点暂不配置数据库或对象存储备份。**

接受的后果：

- PostgreSQL/MinIO 持久卷损坏时，Event、Decision、transcript 和 Artifact 不可恢复；
- 香港地域级故障不满足 RFC 的 Event RPO 0；
- P10 只验证进程/容器重启、断网补传和持久卷重挂载，不声称验证地域灾备；
- 本例外只适用于内部 dogfood，不允许带入外部 Beta。

外部 Beta 硬门槛：加密数据库备份、对象版本副本、恢复演练、明确 RPO/RTO，并由 Boss 解除本例外。

## 10. 计量口径

试点只采集，不出账单。

| Metric | 口径 |
|---|---|
| Task count | 成功创建且未被幂等去重的 Task |
| Run seconds | 每个 Run `started_at` 到 terminal timestamp 的秒数 |
| Peak concurrency | 每 1 分钟窗口内同 Project 最大 running Runs |
| Event bytes | 接受去重后 Event envelope + data 的 UTF-8 bytes |
| Artifact bytes | MinIO 当前 object size，不含已删除 object |
| Decision count | 首次创建的 Decision；重复 source ID 不计 |

同时记录 provider input/output/cache/reasoning tokens 与 provider cost，仅做成本观察；不按 provider token 对客户加价。

## 11. Key SLO 与验收

| 指标 | 试点目标 |
|---|---:|
| Task create API p95 | < 500 ms |
| 在线 Runner command accepted p95 | < 2 s |
| Runner Event 可见 p95 | < 3 s |
| Decision 回答到 Runner ack p95 | < 3 s |
| Webhook 首次投递 p95 | < 10 s |
| 控制面月可用性目标 | 99.5% |

100 个真实 Task 的成功条件：

- 业务系统只使用公开 Task API，不读取 session/PID/JSONL/local path；
- 状态、Event、Decision、Artifact 与 GitHub Issue 回写一致；
- 重复写无重复副作用；
- Runner 断网 10 分钟后连续补传；
- 六类 Decision 至少各有一条真实或受控 fixture 通过端到端；
- 全量记录中所有注入 secret fixture 的原字节串出现次数为 0；
- 删除 Task 后内容与 object 不可读；
- 无备份例外在验收报告中保持显著可见。

## 12. 责任与批准

| 决策 | Accountable | Responsible |
|---|---|---|
| 仓库与工单范围 | Boss | Fleet engineering |
| GitHub App 权限 | Boss | Fleet engineering |
| 数据上传、保留、删除 | Boss | Fleet engineering |
| Project API Key 签发/吊销 | Boss | Fleet engineering |
| Muvee 上线与域名 | Boss | Fleet engineering |
| P10 验收与上线门 | Boss | Fleet engineering |
| 外部 Beta 备份例外解除 | Boss | Fleet engineering |

批准记录：Boss 已在 2026-07-18 的 Fleet 决策卡中逐项选择本文件的冻结值；本文件把选择转为工程可执行基线。

## 13. P1 状态

`DONE_WITH_CONCERNS`：试点业务、仓库、身份、环境、域名、存储、保留、脱敏、计量和责任边界已冻结；concern：Boss 明确选择试点不做备份，地域/卷故障会造成 Event 与全量记录不可恢复，该例外禁止进入外部 Beta。
