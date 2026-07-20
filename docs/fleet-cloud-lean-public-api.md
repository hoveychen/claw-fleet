# Fleet Cloud (lean) — 公开 API 契约

- **状态：** v1 精简版已合并 main（lean-cloud-v1 P1–P5，commit 91cea2d）
- **定位：** 把现有 `fleet serve` harness 的 agent 能力，以一层 scoped token 开放给外部服务集成。**不是**新服务——就是 `fleet serve` 加了一层默认拒绝的公开白名单。
- **真相来源：** 路由常量 `claw-fleet-core/src/routes.rs`；白名单函数 `routes::is_public()`；鉴权 `claw-fleet-core/src/hooks_server/auth.rs`。

## 部署模型

一客户一容器：每个客户对应一个自托管的 Fleet 容器（完整 Linux Fleet + claude/codex + 凭证）。客户的集成服务只通过 HTTP + scoped token 访问，碰不到容器内部、凭证、宿主机。

## 快速开始

### 1. 起服务（单容器）

```sh
export FLEET_ADMIN_TOKEN=$(openssl rand -hex 32)    # 第一方，全权
export FLEET_PUBLIC_TOKEN=$(openssl rand -hex 32)   # 客户，scoped
export HOST_WORKSPACE=/srv/customer-a/repos         # 客户仓库（bind mount）
docker compose -f deploy/lean/fleet.compose.yaml up --build
# 本地开发也可直接：FLEET_PUBLIC_TOKEN=$FLEET_PUBLIC_TOKEN \
#   fleet serve --port 8080 --token $FLEET_ADMIN_TOKEN
```

服务在 `http://<host>:8080`。凭证由 cred store（foxy-switcher 远程 vault + Linux 注入器）注入，见下方 deploy/lean/README。

### 2. 集成方用 scoped token 驱动一个任务

一条任务的完整生命周期——创建 → 观察 → 答决策 → 取用量：

```sh
B=http://<host>:8080
H="Authorization: Bearer $FLEET_PUBLIC_TOKEN"

# ① 创建任务：启动一个 agent。请求体 camelCase：
#    workspacePath(必) prompt(必) model? effort? permissionMode?
curl -H "$H" -X POST $B/spawn_session -d '{
  "workspacePath": "/workspace/repo",
  "prompt": "修复 issue #12：登录页在 Safari 下白屏",
  "model": "claude-opus-4-8",
  "effort": "high"
}'

# ② 实时观察：SSE 事件流（会话/决策变化），或轮询 tail 拿 transcript
curl -N -H "$H" "$B/events"
curl -H "$H" "$B/tail?..."               # 参数同现有 serve /tail

# ③ 遇到决策卡时先看 pending，再作答（六类之一，如 fleet-ask）
curl -H "$H" "$B/fleet-ask/pending"
curl -H "$H" -X POST "$B/fleet-ask/respond" -d '{...}'

# ④ 查该客户（=该容器）的 token 用量
curl -H "$H" "$B/cloud_usage"
```

> SSE 无法设头，用 query 携带 token：`curl -N "$B/events?token=$FLEET_PUBLIC_TOKEN"`。

## 鉴权：两级 token

| Token | 来源 | 权限 |
|---|---|---|
| **admin** | `fleet serve --token <t>` | 全部路由。第一方用（desktop / RemoteBackend / mobile relay）。 |
| **scoped（公开）** | 容器 env `FLEET_PUBLIC_TOKEN` | **仅** `routes::is_public()` 白名单。外部客户集成服务用。 |

- 携带方式：`Authorization: Bearer <token>` 头，或 `?token=<token>` query（SSE `EventSource` 用后者，因为它设不了头）。
- `FLEET_PUBLIC_TOKEN` 未设或为空 → scoped 层关闭，只有 admin token 生效。
- scoped token 命中非白名单路由 → `403`；不带/带错 token → `401`（无 token）/ `403`（token 错）。

**默认拒绝**是安全边界的核心：白名单之外的一切——任意命令执行 `/proc_run`、设置与 guidance 注入（`/apply_*`、`/remove_*`）、文件浏览（`/explorer_*`、`/browse_dir`、`/scratchpad_*`）、插件/技能/源管理、LLM 配置、memories、wiki 变更、mobile-relay 配置、remote-workspace 注册表——都要 admin token。这正是让凭证与宿主机内部对客户不可见的机制。

> `/explorer_file`（读任意路径）**故意不在**白名单里——否则 scoped token 能读出凭证。客户取产物走更窄的 `/user_attachment` 与 `/decision_asset`。

## 公开端点（scoped token 可达）

请求/响应形状与现有 `fleet serve` 内部路由**完全一致**（同一 handler），本表只列出对外稳定的子集与用途。

### 观察
| 路由 | 用途 |
|---|---|
| `GET /health` | 存活探针 |
| `GET /sessions` | 该容器内会话列表 |
| `GET /session_read` | 读单个会话 |
| `GET /session_decisions` | 会话的决策卡历史 |
| `GET /handoff_chain` | 接力链 |
| `GET /tail` | 拉会话 transcript（增量） |
| `GET /messages` | 会话消息 |
| `GET /live_thinking` | 实时思考流 |
| `GET /tool-result` | 工具结构化结果 |
| `GET /events` | **SSE** 实时事件流（会话/决策变化） |

### 驱动
| 路由 | 用途 |
|---|---|
| `POST /spawn_session` | 创建/启动一个 agent 任务 |
| `POST /resume_session` | 恢复会话（可带后续 prompt） |
| `POST /enqueue_message` | 向运行中的会话追加消息 |
| `POST /cancel_pending_message` | 取消待发消息 |
| `POST /interrupt` | 中断（可恢复） |
| `POST /stop` | 停止会话 |
| `POST /stop_workspace` | 停止该 workspace 下全部 agent |

### 答决策卡（六类，pending + respond）
`guard` / `elicitation` / `fleet-ask` / `plan-approval` / `permission-prompt` / `a2ui-render`，各有 `GET /<type>/pending` 与 `POST /<type>/respond`；`elicitation` 另有 `POST /elicitation/upload`。

### 取产物（窄面，非任意路径）
| 路由 | 用途 |
|---|---|
| `GET /user_attachment` | 会话产出的用户附件 |
| `GET /file_size` | 文件大小探测 |
| `GET /decision_asset` | 决策卡关联资产（图片等） |

### 用量（计量；出账/限额 v1 不做）
一客户一容器，所以本容器的用量**就是**该客户的用量。

| 路由 | 用途 |
|---|---|
| `GET /cloud_usage` | **推荐给集成方**：单一合计视图 —— `today`（今日 input/output token + cost）+ `cumulative*`（所有留存 agent 会话的累计 token + cost，即计费基准）。 |
| `GET /today_usage` | 今日窗口（含 Fleet 自身 LLM 开销） |
| `GET /today_usage_breakdown` | 今日按模型拆分 |
| `GET /usage_summaries` | 各 source 用量摘要 |
| `GET /usage_history`、`/codex_usage_history` | 历史 |
| `GET /fleet_llm_usage/daily` | Fleet 自身 LLM 每日用量 |
| `GET /token_breakdown`、`/codex_token_breakdown` | 单任务 token 拆分 |

> `cumulative*` 受会话保留策略影响（被清理的会话会掉出合计），是*当前用量*视图，不是权威计费账本。真正的按 token 出账与超额限流是 v1 之后的工作。

## 不在 v1 范围（见 lean 计划「明确不做」）

出账/超额限流、进程内多租户、control-plane/runner 分裂、mTLS、event-log/outbox、GitHub adapter、webhooks、MinIO、Hosted Console/iframe embed、redaction 脱敏。这些是被砍掉的 `fleet-cloud-v1` 重量级方案的一部分，本精简版不重建。
