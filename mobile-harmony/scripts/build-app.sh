#!/usr/bin/env bash
# Build release `.app` package for app store submission (HarmonyOS App Market artifact).
#
# Division of labor with install.sh:
#   install.sh   → assembleHap, debug signing, `hdc install` to device. Installs but doesn't upload.
#   build-app.sh → assembleApp, release signing, upload to AGC. Uploads but doesn't install to device.
#
# The second part is not a typo: the release profile's device list is empty, yet the system still
# verifies the signature on installation. Release-signed packages can only be distributed via
# the app market. Device verification always uses install.sh; this script's output has only one
# destination — upload via AGC "Package Management".
#
# Signing uses environment variables (see hvigorfile.ts top comment) because the repo's
# build-profile.json5 lacks credentials, and DevEco-generated secrets are platform-specific:
#   FLEET_OHOS_STORE_FILE / FLEET_OHOS_CERT_PATH / FLEET_OHOS_PROFILE_PATH
#   FLEET_OHOS_KEY_ALIAS / FLEET_OHOS_STORE_PASSWORD [/ FLEET_OHOS_KEY_PASSWORD]
#
# Usage:
#   bash scripts/build-app.sh                    # build with production relay, requires signing
#   RELAY_URL=... bash scripts/build-app.sh      # switch relay URL
#   bash scripts/build-app.sh --allow-unsigned   # validate build pipeline only, allow unsigned output
#   bash scripts/build-app.sh --no-web           # skip web sync (recompile ArkTS only)

set -euo pipefail
cd "$(dirname "$0")/.."

ALLOW_UNSIGNED=0
SYNC_WEB=1
for arg in "$@"; do
  case "$arg" in
    --allow-unsigned) ALLOW_UNSIGNED=1 ;;
    --no-web) SYNC_WEB=0 ;;
    -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
    *) echo "未知参数: $arg (试 --help)" >&2; exit 2 ;;
  esac
done

fail() { echo "✗ $*" >&2; exit 1; }

# DevEco comes with Node 18, but pnpm requires 22+ — same issue as install.sh: preserve the system
# PATH for web build, otherwise sync-web fails with "pnpm requires Node.js v22".
SYSTEM_PATH="$PATH"

# Two toolchain layout options. CI cannot install DevEco Studio (GUI IDE, macOS SDK is 5.1G,
# not a Linux build tool), so Linux runners use Huawei's command-line-tools package — it bundles
# hvigorw / ohpm / sdk / node, but the layout differs from DevEco's tools directory, so we must
# distinguish:
#
#   command-line-tools/          DevEco-Studio.app/Contents/
#     bin/hvigorw   (executable)   tools/hvigor/bin/hvigorw.js  (requires node)
#     bin/ohpm                    tools/ohpm/bin/ohpm
#     sdk/                        sdk/
#     tool/node/bin/node          tools/node/bin/node
#
# Point HARMONY_CLI_TOOLS to the root directory of command-line-tools after extraction.
HVIGOR_CMD=()
if [ -n "${HARMONY_CLI_TOOLS:-}" ]; then
  CLI="$HARMONY_CLI_TOOLS"
  [ -x "$CLI/bin/hvigorw" ] || fail "HARMONY_CLI_TOOLS=$CLI 下没有可执行的 bin/hvigorw —— 解压路径是否多了一层?"
  export DEVECO_SDK_HOME="${DEVECO_SDK_HOME:-$CLI/sdk}"
  # command-line-tools does not include JDK; CI provides it via setup-java. Without it, we cannot
  # fall back to the bundled jbr like DevEco, so we require it here.
  [ -n "${JAVA_HOME:-}" ] || fail "用 HARMONY_CLI_TOOLS 时必须自己给 JAVA_HOME(command-line-tools 不含 JDK)"
  export PATH="$CLI/tool/node/bin:$CLI/bin:$JAVA_HOME/bin:$PATH"
  HVIGOR_CMD=("$CLI/bin/hvigorw")
else
  DEVECO="${DEVECO_TOOLS:-/Applications/DevEco-Studio.app/Contents/tools}"
  export DEVECO_SDK_HOME="${DEVECO_SDK_HOME:-/Applications/DevEco-Studio.app/Contents/sdk}"
  export JAVA_HOME="${JAVA_HOME:-/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home}"
  export PATH="$DEVECO/node/bin:$DEVECO/ohpm/bin:$JAVA_HOME/bin:$PATH"
  HVIGORW_JS="$DEVECO/hvigor/bin/hvigorw.js"
  [ -f "$HVIGORW_JS" ] || fail "找不到 hvigorw ($HVIGORW_JS) —— 装 DevEco Studio,或用 HARMONY_CLI_TOOLS 指向 command-line-tools"
  HVIGOR_CMD=(node "$HVIGORW_JS")
fi
[ -d "$DEVECO_SDK_HOME" ] || fail "找不到 HarmonyOS SDK ($DEVECO_SDK_HOME)"

# ---------------------------------------------------------------- Pre-check --
# hvigor silently produces an *unsigned* package on missing signature config instead of failing,
# so we must catch it here or discover it when AGC rejects the upload.
if (( ! ALLOW_UNSIGNED )); then
  for v in FLEET_OHOS_STORE_FILE FLEET_OHOS_CERT_PATH FLEET_OHOS_PROFILE_PATH \
           FLEET_OHOS_KEY_ALIAS FLEET_OHOS_STORE_PASSWORD; do
    [ -n "${!v:-}" ] || fail "缺环境变量 $v —— 发布签名材料要从 AGC 申请(发布证书 .cer + 发布 Profile .p7b),或加 --allow-unsigned 只验构建链路"
  done
  for f in FLEET_OHOS_STORE_FILE FLEET_OHOS_CERT_PATH FLEET_OHOS_PROFILE_PATH; do
    [ -f "${!f}" ] || fail "$f 指向的文件不存在: ${!f}"
  done
fi

if [ ! -d oh_modules ]; then
  echo "→ oh_modules 缺失,跑 ohpm install"
  ohpm install --all
fi

if (( SYNC_WEB )); then
  # Must run before assembleApp: rawfile is a build input; syncing late bundles stale web.
  PATH="$SYSTEM_PATH" bash scripts/sync-web.sh
fi

# Skipping cache clear after signing change lets hvigor report UP-TO-DATE and skip the signing task,
# leaving old signatures in the output (verified 2026-08-18). This script switches signing sources
# each time, so we always clear unconditionally.
OUT=build/outputs/default

echo "→ clearing .hvigor/cache (skipping causes it to reuse old signatures)"
rm -rf .hvigor/cache
# Also clear the output directory: the check below for "was signing effective?" relies on artifact
# filenames, and leftover -signed.app from a previous run would make a build that produced nothing
# look successful. hvigor's clean task usually removes it, but only if clean actually ran — which
# we shouldn't assume.
rm -rf "$OUT"

echo "→ building assembleApp (release) …"
LOG=/tmp/hvigor-assembleapp.log
# Log to file instead of pipe: `hvigorw | grep` swallows the exit code, making build failures
# look successful, then returns old artifacts as if they were new.
set +e
# Cannot use the `--mode module` from install.sh: assembleApp is a project-level task. With module
# mode, it silently ignores the flag — clean runs but hvigor still reports BUILD SUCCESSFUL while
# the build/ directory is never created.
"${HVIGOR_CMD[@]}" clean assembleApp -p product=default -p buildMode=release --no-daemon \
  > "$LOG" 2>&1
BUILD_EXIT=$?
set -e
grep -iE "Error Message|ArkTS:ERROR|BUILD FAILED|No signingConfig" "$LOG" | tail -10 || true
(( BUILD_EXIT == 0 )) || fail "构建失败(exit $BUILD_EXIT),完整日志 $LOG"

# --------------------------------------------------------------- Artifacts --
# The -signed / -unsigned suffix in artifact names is the only reliable signal of whether signing
# succeeded — we cannot rely on hvigor's exit code. On signing success, both files **coexist**
# (unsigned is an intermediate; clean won't remove it), so look for signed first, fall back to
# unsigned if not found.
shopt -s nullglob
SIGNED=("$OUT"/*-signed.app)
UNSIGNED=("$OUT"/*-unsigned.app)
shopt -u nullglob

if (( ${#SIGNED[@]} == 1 )); then
  APP="${SIGNED[0]}"
elif (( ${#SIGNED[@]} > 1 )); then
  fail "$OUT 下有多个已签名 .app,不确定该交哪个: ${SIGNED[*]}"
elif (( ${#UNSIGNED[@]} > 0 )); then
  if (( ALLOW_UNSIGNED )); then
    APP="${UNSIGNED[0]}"
    echo "⚠ 产物未签名(--allow-unsigned):$APP —— 不能上传 AGC"
  else
    fail "只产出了未签名的 ${UNSIGNED[0]} —— 签名环境变量没被 hvigorfile.ts 认到(日志里应有 '[fleet] 已用环境变量覆盖签名配置',且不该有 'No signingConfig found'),别拿它去上传"
  fi
else
  fail "构建报告成功但 $OUT 下没有 .app,日志 $LOG"
fi

VERSION_NAME=$(sed -n 's/.*"versionName"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' AppScope/app.json5 | head -1)
VERSION_CODE=$(sed -n 's/.*"versionCode"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' AppScope/app.json5 | head -1)

echo
echo "=================================================================="
echo "  $(basename "$APP")"
echo "=================================================================="
echo "    path    $PWD/$APP"
echo "    size    $(du -h "$APP" | cut -f1)"
echo "    sha256  $(shasum -a 256 "$APP" | cut -d' ' -f1)"
echo "    version $VERSION_NAME (versionCode $VERSION_CODE)"
cat <<'NOTE'

  下一步
    - 上传到 AGC:我的应用 → 对应 HarmonyOS 应用 → 版本信息 → 软件包管理 → 上传
    - versionCode 必须比线上那版大,否则 AGC 直接拒收(改 AppScope/app.json5)
    - 这个包装不上真机(发布签名),真机验证走 scripts/install.sh
NOTE
