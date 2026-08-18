#!/bin/zsh
# ClawFleet 一键 构建 + 安装 + 启动(真机)。
#
# 用法:
#   zsh scripts/install.sh                # 构建后装到第一台已连接设备并启动
#   zsh scripts/install.sh <hdc-target>   # 指定设备(hdc list targets 里的串号)
#   zsh scripts/install.sh --no-build     # 跳过构建,直接装当前产物
#
# 依赖 DevEco Studio 默认安装路径;签名材料来自 build-profile.json5(本地
# 未提交的 signingConfigs,发布仓里是剥离的——没有它产物是未签名 hap,装不上)。

set -e
cd "$(dirname "$0")/.."

DEVECO="/Applications/DevEco-Studio.app/Contents/tools"
export DEVECO_SDK_HOME="/Applications/DevEco-Studio.app/Contents/sdk"
export JAVA_HOME="/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home"
export PATH="$DEVECO/node/bin:$DEVECO/ohpm/bin:$JAVA_HOME/bin:$PATH"
HDC="$DEVECO_SDK_HOME/default/openharmony/toolchains/hdc"
BUNDLE="com.atomicservice.6917610791622358675"
HAP="entry/build/default/outputs/default/entry-default-signed.hap"

TARGET=""
BUILD=1
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    *) TARGET="$arg" ;;
  esac
done

if [[ -z "$TARGET" ]]; then
  TARGET=$("$HDC" list targets | head -1)
fi
if [[ -z "$TARGET" || "$TARGET" == "[Empty]" ]]; then
  echo "✗ 没有已连接的设备(hdc list targets 为空)——插线、解锁并确认 USB 调试授权" >&2
  exit 1
fi
echo "→ 设备: $TARGET"

if (( BUILD )); then
  echo "→ 构建 assembleHap …"
  node "$DEVECO/hvigor/bin/hvigorw.js" assembleHap --mode module -p product=default --no-daemon 2>&1 \
    | grep -iE "ERROR|BUILD|ArkTS:ERROR|Failed" | tail -10
  # grep 会吞退出码,以产物存在且比构建开始新来判定
fi

if [[ ! -f "$HAP" ]]; then
  echo "✗ 找不到签名产物 $HAP(构建失败或签名配置缺失)" >&2
  exit 1
fi

echo "→ 安装 $HAP"
"$HDC" -t "$TARGET" install "$HAP"

echo "→ 启动 $BUNDLE"
if ! "$HDC" -t "$TARGET" shell aa start -b "$BUNDLE" -a EntryAbility 2>&1 | grep -q success; then
  echo "⚠ 启动失败(常见原因:锁屏)。手动解锁后打开 Fleet 即可,新包已装好。"
fi
echo "✓ 完成"
