# 并入主仓说明

本目录原本是独立仓 `~/DevEcoStudioProjects/ClawFleet`，2026-08-17 并入主仓。

## 为什么并进来

鸿蒙端的 ArkTS UI 与 `mobile-web` 是两套独立实现，每次 web 加功能，鸿蒙都要
手写一遍 ArkTS 版本并且总是滞后（见 TASKS.md 的 `harmony-feature-gaps`：
P1–P4 全是「mobile-web 有、鸿蒙没有」）。同仓之后：

- `mobile-web` 的改动与鸿蒙端的跟进能落在同一个 commit 里
- 计划中的 rawfile 同步（把 web 的 `dist` 打进 hap）变成同仓操作
- 不再需要跨仓协调两份 git 历史

## 构建

macOS 自带的 `java` 是 stub，而 `PackageHap` 阶段需要真实 JDK；SDK 路径也
必须显式给出，否则报 `Invalid value of 'DEVECO_SDK_HOME'`。

```sh
export DEVECO_SDK_HOME=/Applications/DevEco-Studio.app/Contents/sdk
export JAVA_HOME=/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home
/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw assembleHap --no-daemon
```

## 签名

`build-profile.json5` 的 `signingConfigs` 在版本控制里保持为空数组。首次用
DevEco Studio 打开本目录后，到 **Project Structure → Signing Configs** 配置
自己的签名，DevEco 会把本地材料写回该文件——**那份改动不要提交**，它含有
密钥口令字段。

## 旧仓

`~/DevEcoStudioProjects/ClawFleet` 原样保留作归档，不再是开发入口。它的
提交历史（TASKS.md 里若干处按 commit hash 引用的记录）仍可在那里查阅。
