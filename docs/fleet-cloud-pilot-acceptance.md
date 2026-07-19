# Fleet Cloud v1 内部试点验收

- **状态：** LOCAL PASS / EXTERNAL STAGING BLOCKED
- **验收环境：** 本地 Docker 全栈已验；目标环境仍为香港 Muvee 控制面 + 香港独立 Runner VM
- **证据规则：** 只记录实际执行命令、时间与不可变证据链接；单测和 mock 不替代 100 个真实 GitHub Issues。
- **基线：** [[cloud/fleet-cloud-pilot-baseline]]
- **RFC：** [[arch/fleet-cloud-api-v1]] §18

## 当前可复现基线

| 项目 | 状态 | 证据 |
|---|---|---|
| control-plane 测试 | PASS | 2026-07-19 本地 PostgreSQL：`cargo test -p fleet-cloud-control-plane` |
| mobile-web 测试 | PASS | 2026-07-19：28 files / 166 tests |
| mobile-web build | PASS_WITH_WARNING | 2026-07-19：构建成功；Vite 报主 chunk >500 kB |
| OpenAPI lint / 生成一致 | PASS | 2026-07-19：Spectral 零警告，openapi-typescript 7.13.0 生成一致 |
| 三类本机容器镜像 | PASS | 2026-07-19：control-plane / Runner / Hosted Web 实构建；live/ready、SPA deep link、两种 provider CLI 烟测通过 |
| linux/amd64 容器镜像 | PASS | 2026-07-19：三类镜像实构建；control-plane manifest list `sha256:014bc6d3...4995198`，Runner manifest list `sha256:54de885a...71d8eb63`，Hosted Web 构建成功 |
| Runner Compose 配置展开 | PASS | 2026-07-19：补齐 `FLEET_CLOUD_RUNNER_URL` 与 `FLEET_RUNNER_ID` 后 `docker compose ... config --quiet` 通过 |
| 全 workspace Rust 测试 | PASS | 2026-07-19：`cargo test --workspace` 零失败；真实外部/超时用例按测试声明 ignored |
| staging `cloud-e2e.sh` | NOT RUN | 尚无已部署 staging、Project Key、Runner 身份和 webhook receiver |
| 100 个真实 Issues | PASS | 2026-07-19 创建并反向核对 #3–#102；`docs/fleet-cloud-pilot-backlog.{json,csv}` 含 100 行唯一 FCP marker、Issue number/URL、源码证据；创建时未加 `fleet-task`，待 staging 在线后触发 |
| 本地 Docker 全栈 | PASS | 2026-07-19：PostgreSQL、MinIO、control-plane、Hosted Web、GitHub adapter + GitHub API stub、HAProxy、真实 Runner 同网运行；首页、live/ready、同源 API rewrite 均实测 |
| 本地 MinIO Artifact | PASS | 明文上传后 PostgreSQL `storage_backend=s3`、`ciphertext IS NULL`、加密数据键 48 bytes；MinIO 对象存在；签名下载 SHA-256 一致；删除后对象消失、metadata tombstone=`deleted` |
| 本地 GitHub adapter | PASS_WITH_SCOPE | HMAC webhook 经 Hosted Web 同源路由创建 Task；同 delivery ID 重放返回同 Task；stub 实收 installation token、状态 label 与隐藏 marker 评论请求。真实 GitHub App 未触发 |
| 本地 Runner mTLS/调度 | PASS | WebSocket Upgrade 有证书返回 101、无证书 TLS 关闭；公开 API 一次性 claim 后 Runner online；生产自动调度领取 command，sequence=1、required=codex；无 provider 凭证时 Run/Task 正确投影 failed |
| 本地故障演练 | PASS_WITH_SCOPE | 模拟最后心跳在 10 分钟前，生产 stale worker 判 offline，Runner 启动恢复 online；控制面重建后 Runner 无需重启自动恢复心跳；幂等 body 变化 409，跨租户读取 404 |
| 100 个本地真实 Issue Task | PASS_WITH_SCOPE | #3–#102 共 100 个唯一 `github:hoveychen/claw-fleet#N` Task 均通过公开 API 创建并 SQL 反向核对；Runner draining，未声称 100 个 provider 执行终态 |
| GHCR `pilot-50eaf4b` | BLOCKED | linux/amd64 control-plane 构建成功，push 被 GHCR 明确拒绝：OAuth token 缺 `write:packages`；四个远端 tag 复核均不存在 |
| Muvee 资源 | NOT RUN | 2026-07-19 复核：无 Fleet Cloud project；secret store 无 Fleet Cloud/GitHub App secrets |

## 2026-07-19 本地验收新增修复

1. `6fe7582` 修复生产调度断链：此前 Task 只生成 `runner_id=NULL` 的 pending command，全仓仅测试手工调用 `assign_command`，真实在线 Runner 永远领不到任务。现在 `runner_pool_id` 按 OpenAPI 契约持久化，Runner connect/heartbeat 使用行锁与 `SKIP LOCKED` 按 Project、pool、capability、max concurrency 原子 claim 并下发。
2. `50eaf4b` 给 Runner WebSocket 建连增加 15 秒硬超时。此前控制面容器换 IP 后，`connect_async_tls_with_config` 可卡在半连接而不进入指数重试。新增“TCP 接受但不完成 TLS”回归测试，并实测控制面重建后 Runner 自动恢复心跳。
3. `cargo test -p fleet-cloud-control-plane` 全绿（含真实 PostgreSQL 的 MinIO mock、Decision CAS、Runner mTLS、webhook、治理）；`cargo test -p fleet-cloud-runner` 全绿；`scripts/check-openapi.sh` 验证 OpenAPI 与 TypeScript bindings 一致。

## RFC §18 十项上线门

| # | 标准 | 状态 | 必需证据 |
|---:|---|---|---|
| 1 | 100 个连续 Task 无状态丢失 | NOT RUN | 100 行 ticket/task 对账表、状态时间线和终态 |
| 2 | Runner 断网 10 分钟后 Event 顺序补传 | NOT RUN | 故障窗口日志、source event 去重 SQL、连续 sequence |
| 3 | 控制面重启后 SSE cursor 无缺口 | NOT RUN | 重启前 cursor、重启后首条和全量 diff |
| 4 | create/message/Decision/cancel 重放幂等 | NOT RUN | 每类 100 次请求与单一副作用 SQL |
| 5 | 20 方并发回答仅一个成功 | NOT RUN | 1×202 + 19×409/412、单一 response/command |
| 6 | 业务不依赖内部 session/PID/JSONL/path/relay | NOT RUN | GitHub App 调用日志仅含公开 API |
| 7 | Hosted Web 与 API 状态一致 | NOT RUN | 100 Task API/UI 抽样对账 |
| 8 | 每个写操作可追溯 | NOT RUN | audit principal/request/task/command 联表查询 |
| 9 | 隔离覆盖 API/SSE/Artifact/webhook/Runner | NOT RUN | 组织 B 对组织 A 的拒绝矩阵与审计 |
| 10 | provider/Runner/配额故障有终态或恢复态 | NOT RUN | 三类演练时间线、错误码与运维面板截图 |

## P10 扩展演练

| 演练 | 状态 | 通过条件 |
|---|---|---|
| 六类 Decision | NOT RUN | 每类至少一条真实或受控 fixture 端到端 |
| 六类 secret 脱敏 | NOT RUN | 下载完整 transcript 后原字节串计数为 0 |
| Task 内容删除 | NOT RUN | API/object 不可读，最小计费与删除审计仍在 |
| webhook 失败 30 分钟后恢复 | NOT RUN | 最终 delivered，Event ID 不变 |
| SLO | NOT RUN | create <500ms；command <2s；Event/Decision <3s；webhook <10s（均 p95） |
| 计量建议 | NOT RUN | 成功率、Run P50/P95、峰值并发、Event/Artifact bytes、Decision 数 |

## 已知偏差与硬门槛

1. Boss 已选择保持 MinIO 基线。2026-07-19 已实现 S3/MinIO 后端，目标测试证明 PostgreSQL 仅保留对象键和加密信封元数据，密文上传、签名下载、单对象删除与 retention 删除通过 mock S3；真实 MinIO staging 仍为 NOT RUN。
2. 内部试点暂不做数据库/对象备份是 Boss 已接受的例外；该例外禁止进入外部 Beta。
3. `scripts/cloud-e2e.sh` 会拒绝非数字 Issue，并执行 Project/API Key bootstrap、Runner claim、Task/SSE/Decision/Artifact/webhook 闭环；只有 staging 真实运行成功后才能把对应项改为 PASS。
4. Runner claim 生成的证书是短期验收身份，正式 Runner 身份必须写入 VM secret store，不得提交 `deploy/cloud/identity/`。
5. 当前 Runner gateway 是独立 `8091` 端到端 mTLS；Muvee 标准项目只暴露 HTTP `8080` 并终止 TLS。staging 前必须确认 TCP/TLS passthrough 或提供保留客户端证书的第二入口，不能把它当作普通反代 WebSocket。
6. Boss 于 2026-07-19 选择 TLS passthrough；但冻结基线让 API 与 Runner 共用 `fleet-cloud.muveeai.com:443`，普通 L4 passthrough 无法在同一 SNI/端口同时转发 Muvee TLS 终止的 HTTP 与控制面端到端 mTLS。必须增加 Runner 专用 hostname/port，或实现可区分的 ALPN/L4 多路复用后再烟测。
7. GitHub App 适配器已在提交 `6c33417`、`37d931e`、`e383364` 实现并通过目标测试与 workspace 全测：验签后以 GitHub delivery ID 调用公开 `POST /tasks`，轮询公开 Task API，通过 installation token 幂等回写 `fleet:*` label 与带隐藏 marker 的状态评论。真实 App 安装、secret 注入和 staging webhook 仍未执行。
8. 100 个真实 backlog Issues 已创建为 #3–#102，并由 `scripts/fleet-cloud-pilot-backlog.mjs` 生成/恢复/反向对账；当前只带 `fleet-backlog`、`quality`、`test-coverage`，不会在接收器离线时丢失 `fleet-task` 触发事件。
9. 本地全栈发现并修复的自动调度与 Runner 建连超时都已由目标测试、全套测试和真实容器链路交叉验证；这两项不是 staging 环境替代物，但已从外部阻塞清单移除。
10. GHCR 发布授权已具备，但当前 `gh auth` OAuth token 只有 `repo/workflow` 等 scope，缺少 GHCR 要求的 `write:packages`。在 token scope 修复前不得把本地镜像称为已发布镜像。

## 100 Task 记录模板

实际试点开始后每个 Issue 追加一行，不预填、不伪造：

| ticket ID | Task ID | repository | started_at | finished_at | terminal | human intervention | artifact/PR |
|---|---|---|---|---|---|---|---|

## 结论

`BLOCKED: 本地 Docker 范围已完成并修复两处真实生产缺口，但外部 staging 仍未执行 — needed: 带 write:packages 的 GHCR 凭证、香港 Runner VM、已部署 Muvee staging、fleet-runner.muveeai.com 的 L4/TLS passthrough + DNS、真实 GitHub App/Project Key/Runner TLS/webhook receiver secrets。100 个 Issues 与 100 个本地 Task 已对账，但尚未加 fleet-task 触发真实 App。`
