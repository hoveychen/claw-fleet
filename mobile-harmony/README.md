# Claw Fleet · HarmonyOS 客户端

Claw Fleet 的 **HarmonyOS NEXT 原生元服务（atomic service）** 移动客户端，用 ArkTS / ArkUI 编写。它把桌面端 [Claw Fleet](https://github.com/hoveychen/claude-fleet) 的移动面板整套 UI 原生重写，让你在鸿蒙手机上查看并回应正在运行的 Claude Code 会话——审批决策卡、看任务进度、读会话消息、浏览知识库与仓库、查用量。

> 这是 **客户端**：它不直接跑 Agent，而是通过中转（relay）与你电脑上的桌面端 Fleet 配对通信。所有会话、决策、文件都在桌面端产生。

## 工作原理

```
鸿蒙元服务 ──WebSocket──▶ fleet-relay ──WebSocket──▶ 桌面端 Claw Fleet
   (本客户端)              (中转服务器)                (Agent 实际运行处)
```

- **配对**：在桌面端 Fleet 的「移动端」面板生成二维码，扫码把 relay 地址 + 配对密钥（pairing secret）带进本客户端。
- **端到端加密**：业务消息（`msg` 帧）用从配对密钥 HKDF 派生的密钥做 **AES-256-GCM** 加密，relay 只转发密文、看不到明文。协议实现对齐桌面端 `fleet-relay` 的 Rust 版本（`RelayCrypto.ets` ↔ `relay_crypto.rs`）。
- **通知**：接入华为 **Push Kit**，会话需要你决策时推送到手机。

## 功能

- **决策**：审批 6 类决策卡（guard / elicitation / fleet-ask / plan-approval / a2ui / permission-prompt），作答回传桌面端。
- **任务**：会话快照列表 + 实时更新。
- **会话详情**：拉取 transcript、markdown 渲染、继续/排队消息、复制消息。
- **知识库**：浏览 / 搜索 wiki 文档，markdown 渲染，`[[slug]]` 跳转，导出文档。
- **仓库**：查看未合并 worktree 与未推提交。
- **用量**：今日花费 / 输出 token / 24h 占用率。
- **新会话**：选 workspace / 模型 / effort / 权限，草稿持久化。

## 工程结构

```
entry/src/main/ets/
  entryability/EntryAbility.ets   应用入口 + 安全区/主题/i18n 初始化
  pages/Index.ets                 根页：配对门 ↔ 主壳切换
  views/                          各页面（MainShell / TasksView / DecisionsView /
                                  SessionDetailView / WikiView / WikiDocView /
                                  RepoView / UsageView / Composer / More …）
  common/                         relay 客户端、E2E 加密、HKDF、pasteboard、
                                  文件传输、安全区、图标、i18n、主题 …
  models/Types.ets                跨 relay 边界的类型定义
```

## 构建与运行

需要 **DevEco Studio**（HarmonyOS NEXT，compatibleSdkVersion 6.1.0(23) / target 6.1.1(24)）。

在 DevEco 里打开工程即可构建运行。命令行构建：

```bash
export DEVECO_SDK_HOME=/Applications/DevEco-Studio.app/Contents/sdk
export JAVA_HOME=/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home
export PATH="$JAVA_HOME/bin:/Applications/DevEco-Studio.app/Contents/tools/node/bin:$PATH"
/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw assembleHap --no-daemon
```

### 签名配置

本仓 **不含签名材料**（`build-profile.json5` 的 `signingConfigs` 已置空）。首次在 DevEco 打开工程后，到 **Project Structure → Signing Configs** 配置你自己的调试/发布签名，DevEco 会把本地签名材料写回 `build-profile.json5`（勿提交）。

## 关联项目

- 桌面端 / 中转 / CLI：[hoveychen/claude-fleet](https://github.com/hoveychen/claude-fleet)
