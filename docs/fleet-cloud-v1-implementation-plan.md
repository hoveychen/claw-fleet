# Fleet Cloud v1 实施计划

- **日期：** 2026-07-18
- **对应宏计划：** `fleet-cloud-v1`（workspace `TASKS.md`）
- **设计基线：** [[arch/fleet-cloud-api-v1]]
- **机器契约：** [`fleet-cloud-api-v1.openapi.yaml`](fleet-cloud-api-v1.openapi.yaml)
- **目标：** 4–6 周内让内部研发工单系统通过公开 Task API 完成 100 个真实任务试点
- **建议团队：** 3 人（Rust/平台 2，React/产品工程 1）；2 人时按 6–8 周排期

## 1. 已锁定边界

1. 首个试点是内部研发工单。
2. v1 只支持客户自托管 Runner。
3. 业务身份先用 Project API Key。
4. transcript、tool output、Event 默认全量上云。
5. UX 交付 Hosted Console + iframe embed。
6. 试点计量不收费；正式方向是 Runner 并发 + 平台用量。
7. 单区部署，具体地域随试点输入冻结。

## 2. 目标仓库结构

```text
fleet-cloud-wire/
  Cargo.toml
  src/lib.rs
  src/event.rs
  src/runner.rs
  src/task.rs
  tests/json_compat.rs

fleet-cloud-control-plane/
  Cargo.toml
  src/main.rs
  src/app.rs
  src/auth.rs
  src/config.rs
  src/db.rs
  src/error.rs
  src/idempotency.rs
  src/routes/
    artifacts.rs
    decisions.rs
    embed.rs
    events.rs
    runners.rs
    tasks.rs
    webhooks.rs
  src/runner_gateway/
    connection.rs
    protocol.rs
    registry.rs
  src/services/
    artifacts.rs
    decisions.rs
    events.rs
    tasks.rs
    webhooks.rs
  migrations/
    0001_core.sql
    0002_idempotency.sql
    0003_runner.sql
    0004_transcripts_artifacts.sql
    0005_webhooks_usage.sql
  tests/
    api_auth.rs
    api_tasks.rs
    decision_cas.rs
    event_replay.rs
    runner_reconnect.rs
    tenant_isolation.rs
    webhook_delivery.rs

fleet-cloud-runner/
  Cargo.toml
  src/main.rs
  src/config.rs
  src/identity.rs
  src/journal.rs
  src/outbox.rs
  src/redaction.rs
  src/supervisor.rs
  src/transport.rs
  tests/
    command_replay.rs
    outbox_recovery.rs
    redaction.rs
    supervisor_e2e.rs

mobile-web/src/data/
  FleetDataClient.ts
  CloudApiClient.ts
  RelayClientAdapter.ts
  cloudSse.ts
mobile-web/src/embed/
  EmbedApp.tsx
  embedAuth.ts
mobile-web/src/data/__tests__/
  CloudApiClient.test.ts
  cloudSse.test.ts
  embedAuth.test.ts

scripts/
  check-openapi.sh
  cloud-e2e.sh
docs/
  fleet-cloud-pilot-baseline.md
  fleet-cloud-api-v1-rfc.md
  fleet-cloud-api-v1.openapi.yaml
  fleet-cloud-v1-implementation-plan.md
```

## 3. 依赖与并行关系

```text
P1 ──> P2 ──> P3 ──> P4 ──> P5 ──> P6 ──> P10
              │       │       │       │
              ├───────┴──> P7 ┤
              └──────────> P8 ├──> P9 ──> P10
```

- P1/P2 必须先完成。
- P3 完成稳定 API 和 Event 后，P4、P7、P8 可由三人并行。
- P5 依赖 P4 的可靠 Runner 链路。
- P6 依赖 P5 的 harness 接线。
- P9 依赖 P3 的 Event/审计模型与 P8 的 Hosted Web。
- P10 是唯一上线门，不接受用单元测试代替真实工单。

## 4. 工期与负责人建议

| P | 主责 | 估算 | 可并行窗口 | 交付物 |
|---|---|---:|---|---|
| P1 | 产品工程 + 平台 | 1–2 天 | 无 | 签字试点基线 |
| P2 | Rust/平台 A | 2–3 天 | 无 | 三 crate 骨架、迁移、契约 CI |
| P3 | Rust/平台 A | 4–5 天 | P4/P7/P8 的前置 | Task API/Event/幂等 |
| P4 | Rust/平台 B | 4–5 天 | 与 P7/P8 并行 | Runner 身份、长连接、journal |
| P5 | Rust/平台 B | 4–5 天 | P7/P8 可继续 | harness→Run/Event |
| P6 | Rust/平台 A+B | 3–4 天 | P8 并行 | 六类决策闭环 |
| P7 | Rust/平台 A | 4–5 天 | P4/P8 并行 | 全量记录、Artifact、删除 |
| P8 | React/产品工程 | 5–7 天 | P4/P7 并行 | Console + iframe |
| P9 | Rust/平台 A + React | 3–4 天 | P6 后半段 | webhook、计量、治理 |
| P10 | 全员 | 3–5 天 | 无 | 100 工单与故障演练 |

合计约 34–45 人日；3 人并行对应 4–6 周。

## 5. P1 — 试点输入冻结

**文件：**

- Create: `docs/fleet-cloud-pilot-baseline.md`
- Read: `docs/fleet-cloud-api-v1-rfc.md`

**原子步骤：**

- [ ] 运行 `fleet plan get fleet-cloud-v1`，确认输出第一项是未勾选 P1。
- [ ] 新建 `docs/fleet-cloud-pilot-baseline.md`，写入工单系统名称、API 文档地址、测试项目 ID 和业务负责人。
- [ ] 在同一文件列出允许试点的仓库 URL、默认 ref、Runner 可见的 secret 名称；不写 secret 值。
- [ ] 写入首批 Runner 的 OS/arch、CPU/内存、最大并发、出站域名和升级负责人。
- [ ] 写入 API Key 的签发人、project scope、90 天轮换周期、泄漏吊销流程。
- [ ] 写入脱敏规则：Authorization、Cookie、API key、provider key、SSH private key、常见 `.env` secret 值。
- [ ] 写入 transcript、tool output、Event、Artifact 各自的保留天数与按 Task 删除责任人。
- [ ] 写入 Hosted Console 域名、允许 iframe 的精确 HTTPS origin、embed token TTL。
- [ ] 写入主云地域、备份地域、对象存储地域和恢复演练窗口。
- [ ] 写入六项计量口径：Task 数、Run 秒、峰值并发、Event bytes、Artifact bytes、Decision 数。
- [ ] 写入成功产物定义：PR URL、最终报告 Artifact、Task 终态和工单回写字段。
- [ ] 运行 `rg -n '待定|T[B]D|T[O]DO|以后补|待确认' docs/fleet-cloud-pilot-baseline.md`，预期无输出且退出码 1。
- [ ] 让业务负责人、安全负责人、Fleet 负责人各自确认基线，记录姓名和时间。
- [ ] 提交 P1 分支变更，提交信息：`docs(cloud): freeze pilot baseline`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P1`，预期输出下一项 P2。

**完成门：** 七类实例参数都有具体值和责任人；没有占位词。

## 6. P2 — 工程骨架与契约门禁

**文件：**

- Modify: `Cargo.toml`
- Create: `fleet-cloud-wire/Cargo.toml`
- Create: `fleet-cloud-wire/src/lib.rs`
- Create: `fleet-cloud-control-plane/Cargo.toml`
- Create: `fleet-cloud-control-plane/src/main.rs`
- Create: `fleet-cloud-runner/Cargo.toml`
- Create: `fleet-cloud-runner/src/main.rs`
- Create: `scripts/check-openapi.sh`

**原子步骤：**

- [ ] 按 Rule 3 创建 `.worktrees/fleet-cloud-v1` 和分支 `prd/fleet-cloud-v1`；后续生产代码只在该 worktree 修改。
- [ ] 先检查 main checkout 的 `.gitignore` 是否已有 `.worktrees/`；缺失时暂停并向 Boss 请求首次添加授权。
- [ ] 在 workspace `members` 加入 `fleet-cloud-wire`、`fleet-cloud-control-plane`、`fleet-cloud-runner`。
- [ ] 创建 `fleet-cloud-wire/Cargo.toml`，依赖固定为 workspace 当前版本兼容的 `serde`、`serde_json`、`chrono`、`uuid`、`thiserror`。
- [ ] 创建 `fleet-cloud-wire/src/lib.rs`，仅声明 `event`、`runner`、`task` 三个公开 module；禁止依赖 core、Axum、SQLx。
- [ ] 创建 control-plane crate，依赖 `axum`、`tokio`、`sqlx` PostgreSQL、`tower-http`、`tracing`、`serde`、wire crate。
- [ ] 创建 runner crate，依赖 core、wire crate、`tokio-tungstenite`、`rustls`、`rusqlite`、`tracing`。
- [ ] 在两个 binary 的 `main` 仅解析配置、初始化 tracing、调用 `run()`；业务逻辑不写入 main。
- [ ] 创建 `scripts/check-openapi.sh`，顺序执行 Redocly lint、Rust/YAML `$ref` 完整性检查、生成 client 后的 clean-diff 检查。
- [ ] 运行 `cargo fmt -p fleet-cloud-wire -p fleet-cloud-control-plane -p fleet-cloud-runner -- --check`，预期退出码 0；既有 crate 不纳入本 P 的格式重写。
- [ ] 运行 `cargo check -p fleet-cloud-wire -p fleet-cloud-control-plane -p fleet-cloud-runner`，预期三个 crate 全部完成。
- [ ] 运行 `bash scripts/check-openapi.sh`，预期包含 `Woohoo! Your API description is valid.` 且退出码 0。
- [ ] 新建契约 CI job，仅在 OpenAPI、wire、cloud crates 变化时执行上述命令。
- [ ] 提交 P2，提交信息：`feat(cloud): scaffold control plane and runner`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P2`。

**完成门：** 三 crate 可单独编译；OpenAPI 变更无法绕过 lint/生成差异门禁。

## 7. P3 — 控制面核心

**文件：**

- Create: `fleet-cloud-control-plane/migrations/0001_core.sql`
- Create: `fleet-cloud-control-plane/migrations/0002_idempotency.sql`
- Create: `fleet-cloud-control-plane/src/{app,auth,config,db,error,idempotency}.rs`
- Create: `fleet-cloud-control-plane/src/routes/{events,tasks}.rs`
- Create: `fleet-cloud-control-plane/src/services/{events,tasks}.rs`
- Create: `fleet-cloud-control-plane/tests/{api_auth,api_tasks,event_replay,tenant_isolation}.rs`

**数据库不变量：**

```sql
UNIQUE (organization_id, project_id, external_id)
UNIQUE (organization_id, endpoint, principal_id, idempotency_key)
UNIQUE (task_id, sequence)
CHECK (task_status IN ('queued','running','waiting_input','paused','succeeded','failed','cancelled'))
```

**原子步骤：**

- [ ] 在 `api_auth.rs` 写 `missing_key_is_401`、`wrong_project_is_403`、`revoked_key_is_401` 三个失败测试。
- [ ] 运行 `cargo test -p fleet-cloud-control-plane --test api_auth`，预期三项因路由未实现而失败。
- [ ] 实现 Project API Key 解析；token 只在创建响应出现，数据库保存带 server pepper 的 hash 和可检索 prefix。
- [ ] 重跑 `api_auth`，预期 3 passed。
- [ ] 在 `0001_core.sql` 创建 organizations、projects、api_keys、tasks、runs、events、audit_records；每张业务表必须有 `organization_id`。
- [ ] 在 PostgreSQL 测试库执行 `sqlx migrate run`，预期应用 0001 和 0002。
- [ ] 在 `api_tasks.rs` 写创建 Task 返回 202、相同幂等键返回同一 Task、同键不同 body 返回 409 三个测试。
- [ ] 实现 `POST /tasks`：同一事务插 Task、首个 Run、`task.created` Event、idempotency response。
- [ ] 运行 `cargo test -p fleet-cloud-control-plane --test api_tasks create_task`，预期三项通过。
- [ ] 写 Task 状态合法转换表测试，覆盖 RFC 中全部允许边和 7 个禁止边。
- [ ] 实现状态投影服务；状态更新与 Event append 同一事务。
- [ ] 实现 `GET /tasks`、`GET /tasks/{id}` 和 cursor pagination。
- [ ] 实现 cancel/pause/resume/append-message 只创建 durable Command，不直接联系 Runner。
- [ ] 在 `event_replay.rs` 写 100 Event 分页、cursor 续拉、SSE `Last-Event-ID` 恢复测试。
- [ ] 实现 Event list 与 SSE；SSE 首先补历史，再订阅 live notification。
- [ ] 在 `tenant_isolation.rs` 写跨组织 Task/Event 查询均返回 404 的测试。
- [ ] 运行 `cargo test -p fleet-cloud-control-plane --test api_tasks --test event_replay --test tenant_isolation`，预期全绿。
- [ ] 运行 `cargo sqlx prepare --workspace --check`，预期 query metadata 无差异。
- [ ] 提交 P3，提交信息：`feat(cloud): add task event control plane`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P3`。

**完成门：** Task API、状态投影、Event replay、SSE、幂等、租户过滤在 PostgreSQL 集成测试中通过。

## 8. P4 — Runner 身份与可靠链路

**文件：**

- Create: `fleet-cloud-control-plane/migrations/0003_runner.sql`
- Create: `fleet-cloud-control-plane/src/routes/runners.rs`
- Create: `fleet-cloud-control-plane/src/runner_gateway/{connection,protocol,registry}.rs`
- Create: `fleet-cloud-runner/src/{config,identity,journal,outbox,transport}.rs`
- Create: `fleet-cloud-control-plane/tests/runner_reconnect.rs`
- Create: `fleet-cloud-runner/tests/{command_replay,outbox_recovery}.rs`

**协议不变量：**

```text
Cloud Command: unique command_id, monotonic assignment sequence, deadline, expected_version
Runner journal: persist before accepted; completed/rejected/failed is terminal
Runner Event: unique (runner_id, source_event_id), monotonic local outbox sequence
```

**原子步骤：**

- [ ] 写一次性 registration token 使用一次后失效的集成测试。
- [ ] 实现 registration token hash、10 分钟默认 TTL、Runner identity 签发和吊销。
- [ ] 写被吊销 Runner 无法握手、其他 Runner 不受影响的测试。
- [ ] 定义 wire `ClientHello`、`ServerHello`、`Command`、`CommandAck`、`RunnerEventBatch`、`BatchAck` tagged enum。
- [ ] 为每个 frame 增加固定 JSON fixture round-trip 测试；未知字段可忽略，未知 frame type 拒绝并记录协议错误。
- [ ] 实现 Runner 主动出站 mTLS WebSocket；握手发送版本、能力、容量、last cloud cursor、outbox range。
- [ ] 在 Runner 建 SQLite journal 表 `commands` 和 `outbox`，启用 WAL 与 `synchronous=FULL`。
- [ ] 写进程在 `persist command` 后、`accepted ack` 前崩溃的重放测试。
- [ ] 写控制面重发同一 command_id 时 Runner 不重复执行的测试。
- [ ] 写 Runner 离线产生 100 Event、重连批量补传、控制面去重且连续 ack 的测试。
- [ ] 实现 15 秒 heartbeat、45 秒 offline 判定、drain 后拒绝新 assignment。
- [ ] 写 capability 缺失时调度器返回 `runner_capability_missing` 的测试。
- [ ] 运行 `cargo test -p fleet-cloud-wire -p fleet-cloud-runner -p fleet-cloud-control-plane runner`，预期全绿。
- [ ] 提交 P4，提交信息：`feat(cloud): add reliable runner transport`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P4`。

**完成门：** command/event 在断线和双方重启下不丢失、不重复执行；Runner 可吊销和 drain。

## 9. P5 — Harness 接线

**文件：**

- Create: `fleet-cloud-runner/src/supervisor.rs`
- Modify: `claw-fleet-core/src/backend.rs`
- Modify: `claw-fleet-core/src/session_launch.rs`
- Modify: `claw-fleet-core/src/handoff.rs`
- Modify: `claw-fleet-core/src/pending_message.rs`
- Create: `fleet-cloud-runner/tests/supervisor_e2e.rs`

**映射：**

| Harness 事实 | Cloud 投影 |
|---|---|
| Fleet Task plan/handoff chain | 一个 Task，多 Run |
| provider session | Run 内部 provider ref |
| session status | Run status，再投影 Task status |
| JSONL assistant/user/tool | TranscriptRecord + message/tool Event |
| token/cost fields | Usage |
| stop/interrupt/resume/enqueue | durable Command result |

**原子步骤：**

- [ ] 在 core 定义无网络依赖的 `HarnessEventSink` trait；默认实现为空操作，现有桌面行为不变。
- [ ] 写 LocalBackend 使用默认 sink 时 session 扫描结果逐字节不变的回归测试。
- [ ] Runner 实现 sink，把事实先写本地 outbox，不在 core callback 内做网络 I/O。
- [ ] 写 `start_run` 创建隔离 workspace、记录 Task/Run 环境变量、调用现有 agent_source spawn 的测试。
- [ ] 写 Claude 与 Codex 各一个 fake binary fixture；fixture 输出三条 transcript 并以 0 退出。
- [ ] 实现 fake Claude/Codex 的 Run started、message、usage、Run succeeded Event 映射。
- [ ] 写 handoff 注册后当前 Run succeeded、继任 Run created、Task 保持 running 的测试。
- [ ] 写 cancel 发送终止、Run cancelled、Task cancelled 的测试。
- [ ] 写 pause 只在安全边界阻止下一个 Run/消息的测试。
- [ ] 写 resume 创建新 Run 并继承 model/effort/permission policy 的测试。
- [ ] 写 append_message 对运行中 session 进入 pending queue、空闲 Task 创建新 Run 的测试。
- [ ] 禁止任何 Cloud response 返回 PID、JSONL absolute path、workspace absolute path。
- [ ] 运行 `cargo test -p claw-fleet-core -p fleet-cloud-runner supervisor`，预期全绿。
- [ ] 提交 P5，提交信息：`feat(cloud): bridge harness runs and events`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P5`。

**完成门：** fake provider 端到端走完 create→run→handoff→complete，且现有桌面 core 回归测试无变化。

## 10. P6 — 六类决策闭环

**文件：**

- Create: `fleet-cloud-control-plane/src/routes/decisions.rs`
- Create: `fleet-cloud-control-plane/src/services/decisions.rs`
- Create: `fleet-cloud-control-plane/tests/decision_cas.rs`
- Modify: `fleet-cloud-runner/src/supervisor.rs`
- Modify: `fleet-cloud-wire/src/event.rs`

**原子步骤：**

- [ ] 建 decision 表，唯一 source key 为 `(runner_id, source_decision_id)`。
- [ ] 为 guard、elicitation、fleet_ask、plan_approval、permission_prompt、a2ui 各保存一个脱敏 JSON fixture。
- [ ] 写六个 fixture 到 Decision wire schema 的 round-trip 测试。
- [ ] 实现 `decision.created` Event ingestion 与 pending projection。
- [ ] 写两个 principal 同时回答同一 version，仅一个 202、另一个 409 的 PostgreSQL 并发测试。
- [ ] 用 `UPDATE ... WHERE status='pending' AND version=$expected RETURNING` 实现 compare-and-set。
- [ ] 实现 If-Match 缺失返回 428、旧 version 返回 412、已回答返回 409。
- [ ] 回答落库与 `decision.resolved` Event、Runner command 在同一事务创建。
- [ ] Runner 收到 resolve command 后写本地 response IPC；重复 command_id 不重复写。
- [ ] 写 Runner 离线时 Decision 已回答、重连后投递并解除 agent 阻塞的测试。
- [ ] 写控制面重启后 pending Decision 仍可列出和回答的测试。
- [ ] 写 Decision deadline 到期转 expired，迟到回答返回 409 的测试。
- [ ] 运行 `cargo test -p fleet-cloud-control-plane --test decision_cas` 和 Runner decision 测试，预期全绿。
- [ ] 提交 P6，提交信息：`feat(cloud): close decision response loop`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P6`。

**完成门：** 六类卡均可从 harness 出现、云端展示、单次作答、离线补投并解除阻塞。

## 11. P7 — 全量记录与 Artifact

**文件：**

- Create: `fleet-cloud-control-plane/migrations/0004_transcripts_artifacts.sql`
- Create: `fleet-cloud-control-plane/src/routes/artifacts.rs`
- Create: `fleet-cloud-control-plane/src/services/artifacts.rs`
- Create: `fleet-cloud-runner/src/redaction.rs`
- Create: `fleet-cloud-runner/tests/redaction.rs`

**脱敏顺序：**

```text
parse structured record
  -> redact known headers and env keys
  -> redact PEM/private-key blocks
  -> redact configured literal secret fingerprints
  -> preserve content shape and add redaction counters
  -> serialize and enqueue
```

**原子步骤：**

- [ ] 写 Authorization、Cookie、`.env`、PEM、GitHub token、OpenAI/Anthropic key 的 redaction fixtures。
- [ ] 写每个 fixture 输出不包含原 secret、包含 `[REDACTED:<kind>]` 和计数的测试。
- [ ] 实现结构化字段优先脱敏，再执行有界 regex；单记录处理设置 50 ms 上限与 8 MiB 输入上限。
- [ ] 写恶意超长行和灾难性 regex 输入不会阻塞 Runner 的测试。
- [ ] 建 transcript_records、artifacts、artifact_objects、retention_jobs 表。
- [ ] transcript record 使用 `(run_id, source_sequence)` 去重并保持顺序。
- [ ] 实现 multipart object upload、SHA-256、MIME allowlist、单文件和 Task 总量限制。
- [ ] 实现 5 分钟签名下载 URL；URL scope 绑定 Artifact 和 principal。
- [ ] 写跨组织 Artifact metadata 与签名 URL 均返回 404 的测试。
- [ ] 实现按 Task 删除：先写审计 tombstone，再删对象，最后删 transcript 内容并保留最小计费汇总。
- [ ] 写删除中途对象存储失败可重试且最终无孤儿对象的测试。
- [ ] 实现每日 retention job；使用数据库 lease 防止多实例重复跑。
- [ ] 写 KMS envelope key ID 保存、密文可解、数据库泄漏不含 plaintext 的测试。
- [ ] 运行 Runner redaction、control-plane artifact、retention 集成测试，预期全绿。
- [ ] 提交 P7，提交信息：`feat(cloud): upload redacted records and artifacts`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P7`。

**完成门：** 默认全量记录能回放；secret fixtures 零泄漏；按 Task 删除和保留清理通过故障注入。

## 12. P8 — Hosted Web 与 iframe

**文件：**

- Create: `mobile-web/src/data/{FleetDataClient,CloudApiClient,RelayClientAdapter,cloudSse}.ts`
- Create: `mobile-web/src/embed/{EmbedApp,embedAuth}.tsx`
- Create: `mobile-web/src/data/__tests__/{CloudApiClient,cloudSse,embedAuth}.test.ts`
- Modify: `mobile-web/src/App.tsx`
- Modify: `mobile-web/src/relay.ts`
- Modify: `mobile-web/vite.config.ts`

**FleetDataClient 必须暴露：**

```ts
interface FleetDataClient {
  listTasks(input: ListTasksInput): Promise<Page<Task>>;
  getTask(taskId: string): Promise<Task>;
  createTask(input: CreateTaskInput, idempotencyKey: string): Promise<CreateTaskResponse>;
  appendMessage(taskId: string, input: AppendMessageInput, idempotencyKey: string): Promise<CommandReceipt>;
  controlTask(taskId: string, action: "cancel" | "pause" | "resume", version: number, idempotencyKey: string): Promise<CommandReceipt>;
  listDecisions(input: ListDecisionsInput): Promise<Page<Decision>>;
  respondToDecision(decisionId: string, version: number, input: DecisionResponseInput, idempotencyKey: string): Promise<DecisionResponse>;
  listRunMessages(runId: string, after?: string): Promise<Page<TranscriptRecord>>;
  streamEvents(input: StreamEventsInput): EventSubscription;
}
```

**原子步骤：**

- [ ] 将上述 interface 与 OpenAPI 生成类型接入 `FleetDataClient.ts`；业务组件不得 import raw generated fetch function。
- [ ] 写 `CloudApiClient` 为每个 mutation 发送 Idempotency-Key、Decision 发送 If-Match 的测试。
- [ ] 实现 bearer token 注入、request ID 提取、统一 ErrorEnvelope 转换。
- [ ] 写 SSE 保存 cursor、断线指数退避、重连带 `after`、重复 Event ID 去重的测试。
- [ ] 实现 `cloudSse.ts`，收到 Event 后更新 keyed Task/Decision cache。
- [ ] 用 `RelayClientAdapter` 包装现有 RelayClient，保证迁移期原移动模式不回归。
- [ ] 将 TasksView 数据源改为 FleetDataClient；先让现有 Relay adapter 测试绿，再接 Cloud adapter。
- [ ] 将 SessionDetail 的 Cloud 路由改为 Task/Run/messages，不在 URL 暴露 jsonl path。
- [ ] 将 DecisionsView 回答改为 Decision id + version + idempotency key。
- [ ] 实现 Hosted Console 的 project/task deep link。
- [ ] 实现 `POST /embed-tokens` 对应的 bootstrap：token 只存内存，不写 localStorage/sessionStorage。
- [ ] iframe 启动时校验父窗口 origin 与 token claims；`postMessage` 明确 targetOrigin，拒绝 `*`。
- [ ] 配置 CSP：`frame-ancestors` 来自 token allowlist，rich card 使用独立 sandbox origin。
- [ ] 写 embed token 过期、错误 origin、越权 Task、刷新后 token 丢失四个测试。
- [ ] 运行 `pnpm --dir mobile-web test` 和 `pnpm --dir mobile-web build`，预期全绿。
- [ ] 用 desktop viewport 与 mobile viewport 各完成 Task list/detail/Decision/Usage 四页截图验收。
- [ ] 提交 P8，提交信息：`feat(cloud): add hosted and embedded fleet web`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P8`。

**完成门：** 同一 CloudApiClient 驱动 Hosted 与 iframe；现有 relay 模式测试继续通过。

## 13. P9 — Webhook、计量与治理

**文件：**

- Create: `fleet-cloud-control-plane/migrations/0005_webhooks_usage.sql`
- Create: `fleet-cloud-control-plane/src/routes/webhooks.rs`
- Create: `fleet-cloud-control-plane/src/services/webhooks.rs`
- Create: `fleet-cloud-control-plane/tests/webhook_delivery.rs`
- Modify: `fleet-cloud-control-plane/src/app.rs`

**原子步骤：**

- [ ] 建 webhook_endpoints、webhook_deliveries、usage_hourly、quota_counters 表。
- [ ] 写签名 fixture，固定 timestamp/body/secret 和期望 HMAC hex。
- [ ] 实现签名字符串 `<timestamp>.<raw-body>` 与 `Fleet-*` 四个 header。
- [ ] 写接收方 500 三次后成功，delivery 次数为 4 且 Event ID 不变的测试。
- [ ] 实现指数退避、24 小时截止、手动 replay 创建新 delivery ID。
- [ ] 写 webhook URL 解析拒绝 loopback、link-local、metadata IP、非 HTTPS 的 SSRF 测试。
- [ ] 实现 DNS resolve 后再次检查目标 IP，并限制 redirect 次数为 0。
- [ ] 每小时汇总 Task、Run seconds、峰值 Runner concurrency、Event bytes、Artifact bytes、Decision count。
- [ ] 写同一 Event 重放不重复计量的测试。
- [ ] 实现 project rate limit、并发配额、Artifact 配额，返回 OpenAPI 中稳定错误码。
- [ ] 所有 mutation 写 audit record：principal、request ID、resource、action、before/after hash。
- [ ] 暴露 `/health/live` 与 `/health/ready`；ready 检查 PostgreSQL、object store、KMS，不要求任一 Runner 在线。
- [ ] 增加 API latency/error、runner heartbeat age、command age、outbox lag、event lag、webhook backlog 指标。
- [ ] 运维页面展示 failed webhook、offline Runner、stale command、retention failure，不展示 secret。
- [ ] 运行 webhook、usage、quota、audit、health 集成测试，预期全绿。
- [ ] 提交 P9，提交信息：`feat(cloud): add webhook usage and governance`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P9`。

**完成门：** webhook 至少一次投递可重放；平台用量可对账；治理故障有指标和运维入口。

## 14. P10 — 试点验收与上线门

**文件：**

- Create: `scripts/cloud-e2e.sh`
- Create: `docs/fleet-cloud-pilot-acceptance.md`
- Modify: `docs/fleet-cloud-api-v1-rfc.md`（仅记录验收结果与偏差）

**原子步骤：**

- [ ] `cloud-e2e.sh` 创建 Project/API Key、注册 Runner、创建 Task、收 SSE、回答 Decision、下载 Artifact、验证 webhook。
- [ ] 脚本每个 mutation 使用固定幂等键并重复执行一次，断言资源 ID 不变。
- [ ] 在 staging 跑脚本，预期 Task succeeded、Event sequence 连续、Artifact hash 匹配。
- [ ] 选取 100 个真实内部研发工单，记录 ticket ID、Task ID、仓库、开始/结束时间、终态、人工介入和产物。
- [ ] Runner 断网 10 分钟后恢复，断言期间 Event 全部补传且 source event 无重复投影。
- [ ] 在 SSE 客户端记录 cursor 后重启控制面，断言重连补拉无缺口。
- [ ] 对 Task create、message、cancel、Decision response 各重放 100 次，断言副作用各一次。
- [ ] 20 个并发请求回答同一 Decision，断言一个 202、十九个 409/412。
- [ ] 使用组织 B 的 key 枚举组织 A 的 Task/Run/Decision/Event/Artifact/Runner，断言全部 404/403 且审计有记录。
- [ ] 创建含六类 secret fixture 的 Task，下载完整 transcript，断言原 secret 字节串不存在。
- [ ] 删除一个含 transcript 与 Artifact 的 Task，断言 API 不可读、对象不存在、最小计费汇总仍可审计。
- [ ] 强制 webhook 接收方失败 30 分钟后恢复，断言最终投递成功且 Event ID 不变。
- [ ] 核对 Task create p95、command accepted p95、Event ingest p95、Decision ack p95 达到 RFC SLO。
- [ ] 统计 100 Task 成功率、P50/P95 Run 时长、峰值并发、Event/Artifact bytes、Decision 数，形成计量建议。
- [ ] 在验收文档逐条引用 RFC 第 18 节十项标准，附命令、时间和证据链接。
- [ ] 运行全 workspace Rust 测试、mobile-web test/build、OpenAPI lint，预期全绿。
- [ ] 运行 `git status --ignored`，检查未跟踪与忽略产物；不可复现产物在移除 worktree 前向 Boss 报告。
- [ ] 提交 P10，提交信息：`test(cloud): complete internal pilot acceptance`。
- [ ] 运行 `fleet plan check fleet-cloud-v1 P10`。
- [ ] 向 Boss 提交 ready-to-merge 摘要并等待明确合并许可；未获许可不 merge。
- [ ] 获许可后在 main 执行 `git merge --no-ff prd/fleet-cloud-v1`，跑 post-merge smoke test。
- [ ] 确认无遗留 artifact 后移除 `.worktrees/fleet-cloud-v1` 并删除本地 feature branch。

**完成门：** RFC 十项验收全部有真实证据；测试、SLO、隔离、删除和恢复无红灯。

## 15. 每个 P 的统一节奏

```text
先写失败测试
  -> 运行并记录预期失败
  -> 最小实现
  -> 目标测试通过
  -> 相关回归通过
  -> git diff --check
  -> worktree 分支提交
  -> fleet plan check
  -> 立即进入下一 P
```

测试出现红灯时只修复一次；仍红则按 Fleet PRD 规则停止并向 Boss 提交证据。最终 merge 前必须单独走验收卡。

## 16. 计划自检

- 每个 P 都列出精确文件和完成门。
- 每个测试步骤都有精确命令或精确测试名与预期结果。
- 生产代码从 P2 起只在 `prd/fleet-cloud-v1` worktree 修改。
- P10 覆盖 100 个真实任务，不用 mock 替代上线证据。
- 合并使用 `--no-ff`，不主动 push。
