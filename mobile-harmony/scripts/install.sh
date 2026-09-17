#!/usr/bin/env bash
# ClawFleet one-command build + install + launch (physical device).
#
# Usage:
#   bash scripts/install.sh                # Build, install on first connected device, launch
#   bash scripts/install.sh <hdc-target>   # Specify device (serial from hdc list targets)
#   bash scripts/install.sh --no-build     # Skip build, install current artifact
#   bash scripts/install.sh --no-web       # Skip web sync (use when only recompiling ArkTS)
#
# Requires DevEco Studio default install path; signing material from build-profile.json5 (local
# uncommitted signingConfigs, stripped in release repo—without it, artifact is unsigned hap, won't install).

set -e
cd "$(dirname "$0")/.."

# DevEco comes with Node 18, but pnpm needs 22+. Below we'll put DevEco's node first in PATH
# (hvigor needs it), so we save system PATH for web build first, else sync-web fails with
# "pnpm requires at least Node.js v22".
SYSTEM_PATH="$PATH"
DEVECO="/Applications/DevEco-Studio.app/Contents/tools"
export DEVECO_SDK_HOME="/Applications/DevEco-Studio.app/Contents/sdk"
export JAVA_HOME="/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home"
export PATH="$DEVECO/node/bin:$DEVECO/ohpm/bin:$JAVA_HOME/bin:$PATH"
HDC="$DEVECO_SDK_HOME/default/openharmony/toolchains/hdc"
# Read from app.json5, not hardcoded—change bundle name in one place.
BUNDLE=$(sed -n 's/.*"bundleName"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' AppScope/app.json5 | head -1)
# Old bundle name from Element Services era. After switching to regular app, it won't be
# overwritten on install, leaving a zombie icon on desktop that can't hold new data.
# Clean it before each install (silent if never installed).
LEGACY_BUNDLE="com.atomicservice.6917610791622358675"
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
  # Must run before assembleHap: rawfile is build input, late sync bakes old web into package.
  # Use system PATH to avoid DevEco's Node 18.
  PATH="$SYSTEM_PATH" bash scripts/sync-web.sh
fi

if (( BUILD )); then
  echo "→ 构建 assembleHap …"
  # Log to file, not pipe: `hvigorw | grep` changes exit code to grep's, failed build looks
  # successful, old hap from last time gets installed—debugging "no effect" issues is a nightmare.
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

if [[ -z "$BUNDLE" ]]; then
  echo "✗ 没能从 AppScope/app.json5 解析出 bundleName" >&2
  exit 1
fi

"$HDC" -t "$TARGET" uninstall "$LEGACY_BUNDLE" >/dev/null 2>&1 || true

echo "→ 安装 $HAP ($BUNDLE)"
"$HDC" -t "$TARGET" install "$HAP"

echo "→ 启动 $BUNDLE"
if ! "$HDC" -t "$TARGET" shell aa start -b "$BUNDLE" -a EntryAbility 2>&1 | grep -q success; then
  echo "⚠ 启动失败(常见原因:锁屏)。手动解锁后打开 Fleet 即可,新包已装好。"
fi
echo "✓ 完成"
