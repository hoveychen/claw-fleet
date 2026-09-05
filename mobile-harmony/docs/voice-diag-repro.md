# 鸿蒙语音输入「弹了权限但录不进」——真机诊断步骤

配套计划 `harmony-voice-diag`。本文只负责**取证**，不含修复；判定结论回填到 wiki
`mobile/voice-input-ux`，修复走 `harmony-voice-fix`。

分支 `prd/harmony-voice-diag` 已经在 ASR 与注入链路上补齐了时间线日志（纯日志，
无行为改动）。没有这些日志，四条互相同形的失败路径在真机上分不出来。

## 1. 装一个带诊断日志的包

签名材料在公开仓里被剥掉了（`build-profile.json5` 的 `signingConfigs: []`），
所以走 DevEco 装，不要手搓 HAP。

```bash
cd mobile-harmony
bash scripts/sync-web.sh        # 必须先跑：rawfile/web 不入库，缺了会白屏
```

然后 DevEco Studio 打开 `mobile-harmony`，Project Structure → Signing Configs
配好签名，真机 Run。

## 2. 抓日志

```bash
hdc shell hilog -r                       # 清一次，别混进历史
hdc shell hilog | grep -E "voice-diag|fleet-asr|fleet-web"
```

保持这条命令开着，再做下面的操作。

## 3. 三轮操作，每轮之间等 90 秒

麦克风泄漏（若存在）会持续到引擎自己超时（`maxAudioDuration: 60000`），
不等够时间的话第二轮会被上一轮的残留污染，读出来的日志是假的。

| 轮次 | 操作 | 要观察什么 |
|---|---|---|
| R1 | **首次**按住麦克风（权限弹窗会出现），抬手点「允许」 | 权限弹窗出现的瞬间，按钮的外观有没有变回空闲态？ |
| R2 | 等 90s，再次**按住**麦克风说一句「测试一二三」，松手 | 输入框里有没有出字？按钮旁那行小字有没有内容？ |
| R3 | 等 90s，**快速点一下**麦克风（锁定态），说一句话，再点一下停止 | 同上 |

每轮都记下这三问的答案：

1. 松手之后**输入框**里有没有字？
2. 松手之前按钮**右侧那行 12px 小字**有没有内容？（有 = 引擎在工作，问题在
   final/落框；全程空 = 引擎侧就没起来）
3. 录音期间手机**状态栏的麦克风指示**是否一直亮着，松手后多久熄灭？
   （松手后仍亮着 ≈ 麦克风泄漏）

## 4. 日志怎么读 —— 每条假设的判决签名

补的日志会打出这样一条时间线（R1 为例）：

```
fleet-asr  voice-diag start lang=zh-CN
fleet-asr  voice-diag permission granted=yes cost=3820ms      ← 花了 3.8s，说明弹了窗
fleet-asr  voice-diag cancel engine=null                       ← 页面在弹窗期间取消了
fleet-asr  voice-diag createEngine ok cost=210ms dead=no        ← dead 被 start 重置回 false
fleet-asr  voice-diag startListening issued cost=4100ms
fleet-asr  voice-diag engine onStart cost=4350ms
fleet-asr  voice-diag result final=no len=6 dead=no match=yes
fleet-web  voice-diag injected result=NO_HOOK                   ← 页面的 hook 已被删掉
```

| 假设 | 判决签名 | 结论 |
|---|---|---|
| **H1** 权限弹窗抢指针 | `permission cost` 上千毫秒 + 随后一条 `cancel engine=null` + 之后 `injected result=NO_HOOK` | 坐实。修法：权限前置 + `dead` 置位顺序 |
| **H2** 引擎起不来 | 有 `asr start failed` 或 `asr error <code>`（`1002200002` 重复启动 / `1002200006` 引擎忙） | 引擎侧问题，H1 可能是它的诱因（泄漏导致下一轮忙） |
| **H3** 引擎晚于开口 | `engine onStart cost` 明显大于用户按住的时长 | 前半句被吞。修法：加「准备中」态（计划 `voice-tap-toggle` D4） |
| **H4** 只有 final 落框 | 有若干 `result final=no`，但**没有** `final=yes`；且 `injected result=OK` | 引擎正常、投递正常，问题在 web 侧 partial 不进输入框（计划 `voice-live-transcript` D3） |

对照表：

- R1 出现 H1 签名、**R2 正常出字** → H1 是全部答案，只是首次授权那一次踩了。
- R1 与 R2 **都**不出字 → H1 不是全部答案，看 R2 的日志落在 H2/H3/H4 哪一栏。
- 三轮都没有任何 `voice-diag` 日志 → 桥根本没被调到，问题在 web 侧的
  `detectVoiceProvider` / `startVoice` 调用，与 ASR 无关。

## 5. 交回什么

把 `hilog` 的三轮完整输出，加上 §3 那张表的答案，贴回会话即可。
