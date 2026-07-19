# Fleet Cloud API v1 RFC

- **状态：** Draft（v1 关键产品方向已于 2026-07-18 锁定）
- **日期：** 2026-07-18
- **目标读者：** Fleet 核心研发、平台研发、前端、业务接入方、安全与商业负责人
- **范围：** 把 Fleet harness 以稳定 API 和完整 Web UX 的形式提供给业务系统
- **机器契约：** [`fleet-cloud-api-v1.openapi.yaml`](fleet-cloud-api-v1.openapi.yaml)（OpenAPI 3.1，Redocly recommended 零警告）
- **实施计划：** [[arch/fleet-cloud-v1-implementation-plan]]（10 个宏 P、168 个原子检查项）

## 1. 摘要

Fleet Cloud 采用“云控制面 + 客户环境 Runner”的混合架构。云控制面负责租户、身份、任务、决策、事件、审计、计费和 Web UX；Runner 在客户受控环境内负责仓库、凭证、Claude/Codex 进程、hooks、文件与产物。

公开 API 的一等资源是 `Task`、`Run`、`Decision`、`Event`、`Artifact`，而不是现有实现中的 session、PID、JSONL 路径或本地文件。现有 session 继续作为 Run 的内部执行实例存在。

本 RFC 建议先交付单租户内部试点，再扩展到多租户 Beta。首版不提供共享宿主机上的完全托管任意代码执行。

### 1.1 已锁定的 v1 产品决策

| 决策 | v1 选择 | 直接影响 |
|---|---|---|
| 首个试点 | 内部研发工单 | 以“工单/需求 → Task → PR 或报告”的 100 个真实任务验收 |
| Runner 边界 | 客户自托管 | 代码仓库和 provider 凭证留在客户环境，Runner 主动出站连接 |
| 身份 | Project API Key 先行 | 试点不等待完整 OIDC；多租户 Beta 再补 OIDC + RBAC |
| 上云数据 | 全量记录上云 | transcript、tool output 和结构化事件默认上传；必须同步交付脱敏、加密、保留和删除 |
| UX | Hosted Console + iframe | 同一套 Web UX 支持独立使用、深链和短时 embed token 嵌入 |
| 商业计量 | Runner 并发 + 平台用量 | 试点只计量不收费；provider 费用仍由客户直接承担 |
| 区域 | 单区跟随试点 | 试点前锁定主区域、备份区域和数据处理规则；v1 不做双区 active-active |

## 2. 背景与依据

现有 Fleet 已在超过 500 个 session 中验证：

- 长任务接力与磁盘持久计划；
- Claude Code / Codex 启动、监控、停止与恢复；
- guard、elicitation、fleet ask、plan approval、permission prompt、A2UI 六类决策；
- 桌面、移动 Web、远端 probe 和推送；
- usage、审计、wiki、报告、附件与仓库操作；
- 出站 WebSocket relay、端到端加密和移动端 request/reply。

现有服务化基础包括：

- Rust `Backend` trait，以及 LocalBackend / RemoteBackend；
- `fleet serve` 的 Bearer Token HTTP 路由和 SSE；
- mobile relay 约 40 个业务方法；
- Rust 到 TypeScript 的类型导出；
- 可脱离 Tauri 运行的移动 PWA。

这些资产显著降低 harness 和 UX 的重建成本，但目前仍是单用户、单主机模型。`~/.fleet`、`FLEET_HOME`、本地 JSONL、文件 IPC、PID 和进程全局状态不能成为公网契约。

## 3. 目标

1. 业务系统可以通过 API 创建、观察、干预和取消长任务。
2. Fleet 自带 Web UX 与业务 API 使用同一套 SDK 和资源模型。
3. Runner 断线、重启或网络切换后，事件和操作结果可恢复。
4. 所有写操作可幂等重试；所有人工决策最多生效一次。
5. 租户、项目、Runner、凭证、事件与产物均有明确隔离边界。
6. 保留 Fleet 的核心产品语义：Task 跨 Run/Session 持续存在。
7. 为后续 SSO、白标组件、用量计费和企业审计留出兼容边界。

## 4. 非目标

v1 不解决：

- 在共享云宿主机上安全运行任意客户代码；
- 将现有 mobile relay 协议直接定义为长期公开 API；
- 公开 PID、JSONL 路径、`~/.fleet` 目录或供应商 CLI 参数细节；
- 完整复刻桌面端的系统托盘、浮窗、TTS、原生终端和安装器能力；
- 第一版即支持任意 agent provider；
- 第一版即提供任意程度的 CSS 白标。

## 5. 架构决策

### 5.1 总体拓扑

```text
业务系统 ───── REST / Webhook / SSE ─────┐
                                         │
Fleet Hosted Web / Embedded SDK ─────────┼── Fleet Cloud Control Plane
                                         │   Auth / RBAC / Task / Event Log
                                         │   Decision / Artifact / Usage / Audit
                                         │
                                         └── mTLS outbound stream
                                                       │
                                               Customer Runner
                                                       │
                                      isolated workspace/container/VM
                                                       │
                                           Claude Code / Codex / Git
```

### 5.2 控制面职责

- 用户、组织、项目、API key、OIDC/SSO 和 RBAC；
- Task/Run/Decision/Event/Artifact 元数据；
- Runner 注册、心跳、能力声明和任务调度；
- 可回放事件日志、SSE、webhook 投递与重试；
- 操作幂等、审计、配额、usage 和基础计费；
- Hosted Web、嵌入式组件和管理后台的 API；
- 密钥引用和 envelope encryption，不保存客户 Runner 不需要上云的明文凭证。

### 5.3 Runner 职责

- 在客户环境内 checkout/挂载 workspace；
- 持有代码仓库和 agent provider 凭证；
- 启动、恢复、打断和停止 agent 进程；
- 注入 Fleet hooks、MCP、PRD 规则与技能；
- 把本地 session/JSONL/file IPC 归一化为云 Event，并上传完整 transcript/tool 记录；
- 把云 Command 幂等地执行并回报结果；
- 管理本地缓存、待上传完整记录、未上传产物和断线期间的 outbox；
- 实施 workspace、网络、命令和资源策略。

### 5.4 为什么首版选 Runner，而非完全托管

客户环境 Runner 可复用现有 core，避免控制面直接持有仓库密钥和供应商凭证，并把任意代码执行风险留在客户已有安全边界内。完全托管模式可以在资源模型稳定后增加，不应阻塞 API 产品验证。

## 6. 资源模型

所有资源 ID 使用不可枚举 ID，例如 UUIDv7。所有响应包含 `id`、`created_at`、`updated_at`；所有列表使用 cursor pagination。

### 6.1 Organization

租户顶层边界。用户、项目、Runner、API key、usage 和数据保留策略均归属 Organization。

### 6.2 Project

业务接入边界。包含业务 metadata schema、默认 Runner pool、默认 agent 配置、webhook 和策略引用。

### 6.3 Task

业务稳定对象，可跨多个 Run 和多个底层 session。

核心字段：

```json
{
  "id": "task_...",
  "project_id": "proj_...",
  "external_id": "ticket-4821",
  "title": "修复结算重复扣款",
  "status": "running",
  "goal": "...",
  "metadata": {"order_service": "billing"},
  "active_run_id": "run_...",
  "created_by": {"type": "api_key", "id": "key_..."},
  "created_at": "2026-07-18T08:00:00Z",
  "updated_at": "2026-07-18T08:03:10Z"
}
```

Task 状态：

```text
queued -> running <-> waiting_input
          |   |             |
          |   +-> paused ---+
          +------> succeeded
          +------> failed
          +------> cancelled
```

终态为 `succeeded | failed | cancelled`。Task 状态由 Run、Decision 和控制命令投影产生，不由客户端任意写入。

### 6.4 Run

Task 的一次执行尝试或接力段。Runner 重试、handoff、provider 切换均可产生新 Run。底层 agent session ID 仅作为 `provider_session_ref` 的内部字段，不进入默认公开响应。

Run 状态：`assigned | starting | running | waiting_input | stopping | succeeded | failed | cancelled | lost`。

关键字段包括 Runner、agent/provider、model/effort、permission policy、workspace ref、开始/结束时间、退出原因和 usage 汇总。

### 6.5 Decision

统一承载六类现有决策：

- `guard`
- `elicitation`
- `fleet_ask`
- `plan_approval`
- `permission_prompt`
- `a2ui`

状态：`pending | answered | declined | expired | cancelled`。

Decision 包含版本化 `schema_version`、展示 payload、允许的 response schema、来源 Run 和 deadline。回答使用 compare-and-set：只有 `pending` 可转为终态，冲突返回 `409 decision_already_resolved`。

### 6.6 Event

Event 是事实记录，不是当前状态。每个 Organization 有单调递增 cursor，每个 Task 也有单调递增 sequence。

事件至少包含：

```json
{
  "id": "evt_...",
  "cursor": "019c...",
  "organization_id": "org_...",
  "project_id": "proj_...",
  "task_id": "task_...",
  "run_id": "run_...",
  "type": "decision.created",
  "sequence": 42,
  "occurred_at": "2026-07-18T08:03:10Z",
  "recorded_at": "2026-07-18T08:03:11Z",
  "data": {},
  "schema_version": 1
}
```

首批事件：

- `task.created | task.status_changed | task.completed`
- `run.assigned | run.started | run.status_changed | run.finished`
- `message.created | message.delta`
- `tool.started | tool.finished`
- `decision.created | decision.resolved`
- `artifact.created`
- `usage.updated`
- `runner.connected | runner.disconnected`
- `command.accepted | command.completed | command.failed`

`message.delta` 可短期保留，服务端必须最终生成可重放的 `message.created` 完整记录。

### 6.7 Artifact

日志、补丁、图片、附件、报告和 wiki 成品统一为 Artifact。对象数据存入 object storage，数据库仅保存 metadata、hash、大小、MIME、保留策略和授权范围。下载使用短时签名 URL。

### 6.8 Runner

Runner 归属 Organization，可加入一个或多个 pool。字段包括版本、平台、能力、在线状态、并发容量、标签和最近心跳。调度按 pool、标签、能力和配额匹配。

## 7. HTTP API

Base URL：`https://api.fleet.example/v1`

### 7.1 Tasks

```http
POST   /tasks
GET    /tasks/{task_id}
GET    /tasks?project_id=...&status=...&cursor=...
POST   /tasks/{task_id}/messages
POST   /tasks/{task_id}/cancel
POST   /tasks/{task_id}/pause
POST   /tasks/{task_id}/resume
GET    /tasks/{task_id}/events?after=...&limit=...
GET    /tasks/{task_id}/artifacts
```

创建任务示例：

```http
POST /v1/tasks
Authorization: Bearer flk_live_...
Idempotency-Key: billing-ticket-4821-v1
Content-Type: application/json

{
  "project_id": "proj_...",
  "external_id": "ticket-4821",
  "goal": "定位并修复结算服务的重复扣款",
  "workspace": {
    "repository": "github:acme/billing",
    "ref": "main"
  },
  "agent": {
    "provider": "claude_code",
    "model": "claude-opus-4-8",
    "effort": "high",
    "permission_policy_id": "policy_safe_default"
  },
  "metadata": {"ticket_url": "https://..."}
}
```

响应为 `202 Accepted`，返回 Task 和首个 Run。请求进入队列，不承诺 agent 已启动。

### 7.2 Decisions

```http
GET  /decisions?status=pending&project_id=...
GET  /decisions/{decision_id}
POST /decisions/{decision_id}/responses
```

回答示例：

```http
POST /v1/decisions/dec_123/responses
Idempotency-Key: decision-dec_123-answer-1
If-Match: "decision-version-3"

{
  "action": "answer",
  "answers": {"deployment_region": "cn-east"}
}
```

### 7.3 Runs 与 Runner

业务 API 默认只读 Run：

```http
GET /runs/{run_id}
GET /runs/{run_id}/messages?after=...
GET /runs/{run_id}/usage
```

Runner 管理 API：

```http
POST   /runner-registrations
GET    /runners
PATCH  /runners/{runner_id}
POST   /runners/{runner_id}/drain
DELETE /runners/{runner_id}
```

Runner 数据面使用独立协议和凭证，不复用业务 API key。

### 7.4 Webhook

```http
POST   /webhook-endpoints
GET    /webhook-endpoints
PATCH  /webhook-endpoints/{id}
DELETE /webhook-endpoints/{id}
POST   /webhook-endpoints/{id}/rotate-secret
```

Webhook payload 使用 Event envelope。签名头：

```text
Fleet-Event-Id: evt_...
Fleet-Delivery-Id: del_...
Fleet-Timestamp: 1784361791
Fleet-Signature: v1=<hex-hmac-sha256>
```

投递至少一次。接收方按 `Fleet-Event-Id` 去重。指数退避至少 24 小时，提供 delivery 日志和手动重放。

## 8. 实时与可靠性语义

### 8.1 SSE

Hosted Web 和业务系统可订阅：

```http
GET /v1/events/stream?after=<cursor>&project_id=...
Accept: text/event-stream
```

客户端断线后用最后 cursor 补拉。SSE 是 Event Log 的实时投影，不是唯一数据源。

### 8.2 幂等

- 所有可能产生副作用的业务写 API 必须支持 `Idempotency-Key`；
- key 作用域为 Organization + endpoint + authenticated principal；
- 相同 key、相同 body 返回原结果；相同 key、不同 body 返回 `409 idempotency_mismatch`；
- 结果至少保存 24 小时，Task 创建建议保存 7 天；
- Runner Command 使用稳定 `command_id`，Runner 本地 journal 去重。

### 8.3 顺序与重复

- Event 投递为至少一次；消费者必须按 event ID 去重；
- 同一 Task 的 `sequence` 严格递增；不同 Task 不保证全局业务顺序；
- Runner 可重复发送同一 source event，控制面按 `(runner_id, source_event_id)` 去重；
- 状态投影更新与 Event append 必须处于同一数据库事务。

## 9. Runner 协议

Runner 主动连接控制面，避免客户网络开放入站端口。建议使用 mTLS WebSocket 或 gRPC stream；协议本身版本化。

### 9.1 握手

Runner 发送：

- runner ID、证书身份和 nonce；
- build version、OS/arch；
- agent providers 和功能能力位；
- 并发容量与标签；
- 本地最后确认的 cloud cursor；
- 本地 outbox 最早/最晚 sequence。

控制面返回协议版本、心跳周期、配置版本、未确认 command 和需要补传的 outbox range。

### 9.2 Command

首批 command：

- `start_run`
- `append_message`
- `resolve_decision`
- `interrupt_run`
- `cancel_run`
- `pause_runner`
- `sync_policy`

每个 Command 有 `command_id`、deadline、expected state/version 和 payload。Runner 先持久化再执行，回报 `accepted` 与最终 `completed | rejected | failed`。

### 9.3 Outbox

Runner 必须有本地 durable outbox。agent 事件先写 outbox，再异步上传；控制面确认连续 sequence 后方可压缩。断网不应丢失 Task 完成、Decision 或 Artifact metadata。

现有 mobile relay 可复用加密、能力位和 request/reply 思路，但其 best-effort 语义不能直接作为 Runner v1 的可靠性保证。

## 10. 鉴权与授权

### 10.1 用户与业务系统

- 人类用户：OIDC/OAuth 2.1，Hosted Web 使用 Authorization Code + PKCE；
- 业务系统：project-scoped API key，后续支持 OAuth client credentials；
- API key 只显示一次，数据库保存 hash，支持过期、轮换和 last-used；
- 高风险操作可要求短时 step-up auth。

### 10.2 RBAC

首版角色：

- `org_admin`
- `project_admin`
- `operator`
- `reviewer`
- `viewer`
- `service_account`

权限按 Organization/Project 作用域判定。Decision 回答、Run 取消、Artifact 下载、Runner 管理和凭证管理必须独立授权。

### 10.3 Runner 身份

- 一次性 registration token 只用于签发 Runner identity；
- 正式连接使用短期证书或可轮换 workload identity；
- 每个 Runner 绑定 Organization 和允许的 pool；
- Runner 失窃时可吊销，不影响其他 Runner；
- 控制面到 Runner 的敏感配置使用 runner public key 再加密。

## 11. 安全边界

1. 控制面不得接受或返回任意本地绝对路径作为公开资源标识。
2. v1 默认上传完整 transcript 和 tool output，但不上传完整仓库、provider credentials、secrets 或本地绝对路径；上传前执行结构化脱敏，控制面提供按 Task 删除与保留期清理。
3. Rich card HTML/A2UI 必须运行在 sandboxed iframe，禁用同源权限，资源走签名 URL。
4. Artifact 做大小、MIME、hash、恶意内容和保留策略检查。
5. 命令执行受 permission policy、guard、网络策略和 workspace 边界约束。
6. 所有管理和业务写操作写 immutable audit record。
7. 控制面数据库按 `organization_id` 强制隔离；推荐 PostgreSQL RLS 作为第二道防线。
8. 日志默认脱敏 Authorization、API key、provider key、cookie 和常见 secret 格式。

## 12. 数据与存储

推荐：

- PostgreSQL：组织、项目、Task、Run、Decision、Event、幂等、审计、usage；
- Object Storage：Artifact、富卡资源、可选 transcript chunk；
- Redis 或数据库队列：短期连接路由、限流和 webhook 调度；
- durable queue：规模扩大后承载 Runner command 和 webhook，试点可用 PostgreSQL outbox；
- KMS：API key pepper、webhook secret、envelope keys。

Event 表按时间或 Organization 分区。先保留 90 天细粒度 Event，并允许租户策略覆盖；汇总 usage 和审计的保留期单独定义。

## 13. 错误模型

统一错误响应：

```json
{
  "error": {
    "type": "conflict",
    "code": "decision_already_resolved",
    "message": "Decision has already been resolved.",
    "request_id": "req_...",
    "details": {"resolved_at": "..."}
  }
}
```

稳定 code 包括：

- `authentication_required`
- `permission_denied`
- `resource_not_found`
- `validation_failed`
- `idempotency_mismatch`
- `version_conflict`
- `decision_already_resolved`
- `runner_unavailable`
- `runner_capability_missing`
- `quota_exceeded`
- `rate_limited`
- `provider_unavailable`
- `internal_error`

所有响应返回 `Fleet-Request-Id`。`429` 返回 `Retry-After`。不向业务客户端暴露本地路径、命令行、stack trace 或 provider secret。

## 14. Web UX 与嵌入

### 14.1 统一 Data SDK

前端先抽象 `FleetDataClient`，至少包含：

- task list/detail；
- event stream；
- decisions list/respond；
- messages/tool details；
- usage/artifacts；
- create/cancel/resume/append message。

实现三种 adapter：

1. `CloudApiClient`：Hosted Web 和业务嵌入；
2. `TauriClient`：现有桌面；
3. `RelayClientAdapter`：迁移期间兼容现有移动端。

业务组件不直接调用 Tauri `invoke`，也不直接使用 mobile relay method string。

### 14.2 交付形态

- **Hosted Console：** v1 完整 Fleet Web，支持组织/项目切换和深链；试点用 Project API Key，Beta 接 OIDC/SSO；
- **Embedded Components：** `TaskPanel`、`DecisionInbox`、`DecisionCard`、`UsageBadge`；
- **Headless SDK：** TypeScript 首发，后续 Python/Go；
- **iframe embed：** v1 业务内嵌路径，使用短时、Task/Project scoped embed token；
- **Web Components/React package：** Beta 后提供，避免首版陷入任意宿主 CSS 兼容。

## 15. 可观测性与 SLO

试点指标：

- Task 创建 API p95 < 500 ms，不含 agent 排队；
- 在线 Runner 的 command accepted p95 < 2 s；
- Event 从 Runner 到控制面可见 p95 < 3 s；
- webhook 首次投递 p95 < 10 s；
- Decision 回答成功后 Runner 确认 p95 < 3 s；
- 控制面月可用性试点 99.5%，Beta 99.9%；
- RPO：控制面 Event 0；Runner 断网依赖本地 outbox；
- RTO：试点 4 小时，Beta 1 小时。

核心 telemetry：API latency/error、Runner online、command age、outbox lag、event ingest lag、webhook backlog、Task terminal rate、Decision wait time、provider errors、usage reconciliation。

## 16. 兼容与版本化

- HTTP 主版本放 URL：`/v1`；
- Event 和 Decision payload 各自带 `schema_version`；
- 只做向后兼容字段新增；删除或语义改变进入 `/v2`；
- Runner 采用 capability negotiation，控制面只下发 Runner 明确支持的 command；
- SDK 对未知 event type 必须忽略并保留 envelope；
- API 文档和 OpenAPI schema 与服务端构建绑定发布。

## 17. 从现有仓库迁移

### 阶段 A：4–6 周单租户试点

1. 定义共享 wire crate/schema：Task、Run、Decision、Event、Command。
2. 从现有 core 增加 `CloudEventSink` 和 durable runner outbox，不改动 agent harness 语义。
3. 建最薄控制面：API key、Task、Event、Decision、Runner connection、PostgreSQL。
4. 将移动 PWA 的关键页面接到 `CloudApiClient`，交付 Hosted Console 与 iframe embed。
5. 接一个真实业务系统，跑 100 个连续任务并做断线/重启演练。
6. 默认上传完整 transcript/tool 记录，完成脱敏、静态加密、保留期和按 Task 删除。
7. 记录 Task 数、Run 时长、峰值并发、事件量、存储量和 Decision 数；试点不收费。

### 阶段 B：10–14 周多租户 Beta

1. Organization/Project/RBAC/OIDC；
2. Runner pools、能力调度、证书轮换和 drain；
3. webhook、幂等存储、审计、配额、usage；
4. Hosted Console、iframe embed、TypeScript SDK；
5. HA、备份恢复、压测、渗透测试和运维后台。

### 阶段 C：企业 GA

1. SLO、区域与数据保留；
2. KMS/BYOK、SCIM、企业审计导出；
3. 更细策略、网络控制和合规；
4. 可选完全托管 Runner，必须独立完成沙箱设计评审。

## 18. 试点验收

试点必须同时满足：

1. 同一业务在 100 个连续 Task 中无 Task 状态丢失；
2. Runner 断网 10 分钟后恢复，Event 可补传且顺序正确；
3. 控制面重启后 SSE 从 cursor 补拉无缺口；
4. Task 创建、追加消息、Decision 回答和取消均通过重放测试证明幂等；
5. 并发回答同一 Decision，仅一个成功；
6. 业务系统不依赖 session ID、PID、JSONL/path 或 relay method；
7. Hosted Web 和业务 API 对同一 Task 的状态一致；
8. 每个写操作可追溯到 principal、request ID、Task 和 Runner command；
9. 租户隔离测试覆盖 API、SSE、Artifact URL、webhook 和 Runner；
10. provider 故障、Runner 丢失和配额超限都有明确终态或可恢复状态。

## 19. 商业与法律门槛

进入外部 Beta 前必须完成：

- 核实 Claude Code、Codex 及相应 provider 在客户 Runner/BYOC 与托管场景的商业条款；
- 明确凭证归属、usage 计费主体和数据处理角色；
- 审计 AGPL-3.0、CLA 覆盖和第三方依赖，决定开源 SaaS、双许可证或商业许可证路径；
- 制定隐私政策、DPA、数据保留和删除流程；
- 对上传 transcript、代码片段、Artifact 和 rich card 内容做数据分级。

## 20. 实施前仍需填写的具体参数

产品方向已经锁定，不再把架构选择留给实施阶段。开始阶段 A 前只需填写以下实例参数：

1. 内部研发工单的具体来源系统、负责人、仓库清单和成功产物定义；
2. 首批 Runner 的安装主机、并发容量、网络出口和升级责任人；
3. Project API Key 的签发人、轮换周期和试点业务权限范围；
4. transcript/tool output 的脱敏规则、默认保留天数和删除责任人；
5. Hosted Console 域名、iframe 允许的宿主 origin 和 embed token TTL；
6. 单区的具体云厂商/地域、备份地域和恢复演练窗口；
7. 并发与平台用量埋点的计量口径。试点只采集，不出账单。

## 21. 建议结论

批准阶段 A。首个里程碑不是“把所有 Tauri 命令改成 HTTP”，而是让一个真实业务只依赖公开 Task API，完整经历创建、事件、决策、接力、完成和产物读取，并通过断线与幂等验收。

只有 Task/Event/Decision 契约经过试点稳定后，才扩大为多租户 Beta 或投入完全托管沙箱。

### 18.1 验收执行状态（2026-07-19）

P10 的可执行脚本、容器资产与证据矩阵已准备；真实 staging、100 个连续 GitHub Issues 及故障演练尚未执行，当前状态为 **BLOCKED / NOT RUN**。验收结果以 [[cloud/fleet-cloud-pilot-acceptance]] 为唯一记录，任何单测或 mock 均不得替代本节十项真实证据。
