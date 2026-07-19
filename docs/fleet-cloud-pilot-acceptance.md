# Fleet Cloud v1 内部试点验收

- **状态：** BLOCKED / NOT RUN
- **验收环境：** 香港 Muvee 控制面 + 香港独立 Runner VM
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
| Runner Compose 配置展开 | NOT VERIFIED | 静态 YAML 已提供；本棒两次校验命令依次漏传 `FLEET_CLOUD_RUNNER_URL`、`FLEET_RUNNER_ID`，按一次修复上限停止，未把失败改写为 PASS |
| 全 workspace Rust 测试 | PASS | 2026-07-19：`cargo test --workspace` 零失败；真实外部/超时用例按测试声明 ignored |
| staging `cloud-e2e.sh` | NOT RUN | 尚无已部署 staging、Project Key、Runner 身份和 webhook receiver |
| 100 个真实 Issues | NOT RUN | 2026-07-19 `gh issue list` 返回空数组；仅有 GitHub 默认 9 个 label |
| Muvee 资源 | NOT RUN | 2026-07-19 复核：无 Fleet Cloud project；secret store 无 Fleet Cloud/GitHub App secrets |

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

## 100 Task 记录模板

实际试点开始后每个 Issue 追加一行，不预填、不伪造：

| ticket ID | Task ID | repository | started_at | finished_at | terminal | human intervention | artifact/PR |
|---|---|---|---|---|---|---|---|

## 结论

`BLOCKED: P10 真实试点与故障演练尚未执行 — needed: 100 个真实 GitHub Issues、香港 Runner VM、已部署 Muvee staging、可保留 Runner mTLS 的公网入口、GitHub App/Project Key/Runner TLS/webhook receiver secrets，以及 Boss 对 push/部署的当前授权。`
