#!/bin/zsh
# ClawFleet 一键 构建 + 安装 + 启动(真机)。
#
# 用法:
#   zsh scripts/install.sh                # 构建后装到第一台已连接设备并启动
#   zsh scripts/install.sh <hdc-target>   # 指定设备(hdc list targets 里的串号)
#   zsh scripts/install.sh --no-build     # 跳过构建,直接装当前产物
#   zsh scripts/install.sh --no-web       # 跳过 web 同步(只重编 ArkTS 时用)
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
SYNC_WEB=1
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --no-web) SYNC_WEB=0 ;;
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

if (( BUILD && SYNC_WEB )); then
  # 必须在 assembleHap 之前:rawfile 是构建的输入,晚同步就会把旧 web 打进包里
  zsh scripts/sync-web.sh
fi

if (( BUILD )); then
  echo "→ 构建 assembleHap …"
  # 日志走文件而不是管道:`hvigorw | grep` 会把退出码换成 grep 的,构建失败也
  # 看起来成功,然后把上一次的旧 hap 装上去 —— 查这种"改了没生效"要命。
  set +e
  node "$DEVECO/hvigor/bin/hvigorw.js" assembleHap --mode module -p product=default --no-daemon \
    > /tmp/hvigor-install.log 2>&1
  BUILD_EXIT=$?
  set -e
  grep -iE "Error Message|ArkTS:ERROR|BUILD FAILED" /tmp/hvigor-install.log | tail -10
  if (( BUILD_EXIT != 0 )); then
    echo "✗ 构建失败(exit $BUILD_EXIT),完整日志 /tmp/hvigor-install.log" >&2
    exit 1
  fi
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
