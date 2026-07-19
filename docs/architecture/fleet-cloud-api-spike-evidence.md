# Fleet Cloud API + Hosted UX Spike 验收报告

日期：2026-07-19
结论：**Go to private beta（Spike 范围）**

本报告是 [[architecture/fleet-cloud-api-spike|Fleet Cloud API + Hosted UX 两周 Spike]] 的最终证据摘要。仓内可复验 harness 在全新 PostgreSQL 16 容器上运行 Rust 测试、浏览器测试和 Cloud production build，再导出并校验结构化证据。最终结果为 **G1–G9 全部通过**。

## 复验入口

```bash
./scripts/fleet-cloud-spike-e2e.sh
```

成功标准：命令退出码为 0，输出 `GO criteria: 9/9 passed`，并生成：

- `target/fleet-cloud-spike/evidence.json`
- `target/fleet-cloud-spike/rust-tests.log`
- `target/fleet-cloud-spike/browser-tests.log`
- `target/fleet-cloud-spike/browser-build.log`
- `target/fleet-cloud-spike/browser-portability-scan.log`
- `target/fleet-cloud-spike/validation.log`

这些 `target/` 产物未纳入版本控制，可由上述命令完整重建；校验逻辑位于 `scripts/fleet-cloud-spike-validate-evidence.sh`。

## 本次实测

证据生成时间：2026-07-19T03:22:54Z。

| 指标 | 结果 | Spike 门槛 |
|---|---:|---:|
| Task 创建 p95（真实 PostgreSQL，20 次） | 7.28 ms | < 1,000 ms |
| Event 持久化 p95 | 0.00 ms | < 2,000 ms |
| Decision 往返 | 12.26 ms | < 2,000 ms |
| Runner SQLite spool 文件 | 32,768 bytes | > 0 |
| 模拟断网 | 300 s | 300 s |

功能流证据：一个 Task 最终为 `succeeded`；云端事件序列为连续的 1–7；handoff 前后生成 ordinal 1、2 两个 Attempt；Decision 恰好进入一次 `answered`；同一 launch 命令重复投递仍只启动一次；Webhook 经失败、重试和人工 replay 后为 `delivered`，且 Event ID 保持不变。

`event_persist_p95_ms` 在本地同一数据库时钟粒度下测得 0.00 ms，只能证明没有越过 Spike 的 2 秒红线，不代表生产网络传播延迟为零。生产 beta 必须用跨进程/跨主机 telemetry 重新建立延迟基线。

## G1–G9 证据

| Gate | 结果 | 自动化证据 |
|---|---|---|
| G1 Create idempotency | Pass | PostgreSQL 集成测试验证同 key/同 body 返回同一 Task，变更 body 冲突 |
| G2 Launch dedupe | Pass | 同一命令 20 次投递并重启 spool，真实 launch 计数仍为 1 |
| G3 Ordered durability | Pass | 云端 7 条事件序列连续；Runner SQLite spool 在重启后仍保留未确认事件 |
| G4 Disconnect recovery | Pass | 300 秒分区模型下恢复后不重复 Attempt，3 条 lifecycle/decision/transcript 事件均保留且有序 |
| G5 Decision unblock | Pass | Hosted CAS 回答只生成一个 Runner command；重复 command 20 次只调用一次 Fleet core answer 边界 |
| G6 Tenant isolation | Pass | foreign-scope 读取返回 404；PostgreSQL 写入 tenant denial audit 记录 |
| G7 Browser portability | Pass | mobile-web Cloud mode 26 个文件、162 个测试通过；production build 通过；Cloud 源码无 Tauri/RelayClient import |
| G8 Handoff continuity | Pass | 同一 Task 中存在 ordinal `[1, 2]` 两个有序 Attempt，最终成功 |
| G9 Webhook replay | Pass | 首次 500 后进入 retry，人工 replay 后 204/delivered；投递 Event ID 不变 |

## 故障注入与边界变化

- 浏览器 reducer 现在严格要求下一条 SSE sequence 等于 `event_cursor + 1`；发现 gap 后停止错误增量并拉取完整 Task detail。
- Runner 使用持久 command claim 交付 Decision；重复投递返回 `AlreadyApplied`，不会二次唤醒 Fleet core。
- Runner event ingestion 以 dedupe key 去重，在事务中分配连续云端 sequence，同时投影 Task、Attempt、Decision 并写入 `task.event` outbox。
- PostgreSQL migration 增加 tenant denial audit；Webhook worker 覆盖失败、重试和人工重放。
- harness 每次创建独立 PostgreSQL 容器，拒绝复用同名容器，避免旧数据污染验收。

## 非阻塞事项与范围边界

- Vite production build 有既存的大 chunk 警告；不影响 Spike 功能通过，private beta 前应按实际加载性能决定是否拆包。
- Rust 报告 `conch-parser` 与 `sqlx-postgres` future-incompat 警告；当前 toolchain 构建测试均绿，依赖升级应进入 beta hardening。
- 按既定验收决定，本 Spike 未把 `clippy` 作为门禁，因为当前 workspace 会递归触发大量既存 core warning；本次门禁为 rustfmt、目标 crate tests/examples、mobile-web tests/build、真实 PostgreSQL harness 和 diff hygiene。
- 这是架构 Spike 的 Go，不是生产上线批准。组织管理、Runner 安装升级、密钥轮换、生产 mTLS、保留策略、可观测性、限流/配额、Webhook 运维 UI 与真实客户网络验证仍属于 private beta 工作。

## 决策

九个强制 gate 全部通过，且没有发现需要否定核心架构的安全边界问题。建议进入 BYO Runner private beta，把当前 adapter、durable spool、PostgreSQL event/outbox、Hosted Cloud mode 作为实现基线；生产化工作继续由 Fleet Cloud v1 计划承接。
