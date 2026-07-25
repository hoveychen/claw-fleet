# Fleet Cloud —— OpenAI Responses 兼容 API（v2）

- **状态：** v2 精简对外 API（lean-cloud-v2，建设中）
- **定位：** 外部服务集成 Fleet agent 的**唯一对外面**。兼容 OpenAI Responses API——集成方拿标准 OpenAI SDK 指向 `<host>/v1`、`api_key=$FLEET_PUBLIC_TOKEN` 即可驱动 Fleet 的 claude/codex agent。
- **真相来源：** `claw-fleet-core/src/hooks_server/responses.rs`。
- **为什么不用裸 `fleet serve`：** v1 用 scoped token 白名单直接开放内部路由，但 `/spawn_session` 的 `workspacePath` 无约束（可指向凭证目录）、`/sessions` 返回裸 `SessionInfo`（泄露 pid/宿主机路径）、内部契约不稳定。v2 用一层投影根治：请求无 `workspacePath`、响应只含干净字段、契约稳定。

## 鉴权 & 部署

- 一客户一容器；`Authorization: Bearer $FLEET_PUBLIC_TOKEN`（SSE 用 `?token=`）。
- workspace 由容器服务端绑定（`FLEET_PUBLIC_WORKSPACE`，默认 `/workspace`）——**请求里没有 workspace 路径**，这是 confinement 的根。

## 端点

| 端点 | 用途 |
|---|---|
| `POST /v1/responses` | 创建一个 response（启动/继续一个 agent 任务）。`{model?, input, instructions?, stream?, background?, previous_response_id?}` |
| `GET /v1/responses/{id}` | 检索 response（投影 output + status + usage） |
| `POST /v1/responses/{id}/cancel` | 取消（→ 内部 interrupt/stop） |
| `GET /v1/responses/{id}/files` | 列出该 response 产出的文件（Fleet 扩展） |
| `GET /v1/files/{id}/content` | 下载产物内容（Fleet 扩展，限 `/workspace` 内） |

## 请求 `input`

- 字符串：`"input": "修复 issue #12"`。
- 消息数组：`[{"type":"message","role":"user","content":[{"type":"input_text","text":"..."}]}]`。
- 答决策卡：`[{"type":"function_call_output","call_id":"call_1","output":"allow"}]` 配 `previous_response_id`。

## `response` 对象

snake_case，兼容 OpenAI：

```json
{
  "id": "resp_<uuid>",
  "object": "response",
  "created_at": 1741369938,
  "status": "in_progress",
  "model": "claude-opus-5",
  "output": [
    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"..."}]}
  ],
  "usage": {"input_tokens":15,"output_tokens":12,"total_tokens":27}
}
```

`id` 是不透明的 `resp_<uuid>`——不暴露内部 session id/jsonl 路径。`status`：`queued` / `in_progress` / `completed` / `failed` / `cancelled` / `incomplete`。

## 决策卡 → `function_call`（Fleet 扩展点，走 OpenAI 原生形状）

Fleet 六类决策卡出现在 `output` 里作为 `function_call` 项，集成方用标准 OpenAI tool-call 循环作答：

| Fleet 卡 | function name |
|---|---|
| guard | `fleet_guard` |
| elicitation | `fleet_elicitation` |
| fleet-ask | `fleet_ask` |
| plan-approval | `fleet_plan_approval` |
| permission-prompt | `fleet_permission` |
| a2ui | `fleet_a2ui` |

卡的富文本/预览（如 a2ui 的 messageTree、guard 的命令分析）放在 `function_call.arguments` 的 JSON 里。作答：`POST /v1/responses` 带 `previous_response_id` + `function_call_output`{call_id, output}，内部路由到对应 `/*/respond`。

## 流式（`stream: true`）

SSE 事件：`response.created`、`response.output_text.delta`、`response.completed`、`response.failed`（+ 中途 `error`）。由内部 tail/live_thinking 增量映射。

## 范围外（v2 不做）

OpenAI tools 透传给 agent 当工具、image/file input、code interpreter；真按 token 出账/限额；`GET /v1/responses` 列表。
