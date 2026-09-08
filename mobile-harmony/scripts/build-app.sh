#!/usr/bin/env bash
# 构建上架用的 release `.app` 包(HarmonyOS 应用市场的提交产物)。
#
# 和 install.sh 的分工:
#   install.sh   → assembleHap,debug 签名,`hdc install` 装真机。装得上、传不上。
#   build-app.sh → assembleApp,发布签名,上传 AGC。传得上、装不上真机。
#
# 后半句不是笔误:发布 profile 的设备列表为空,系统装的时候仍会验签,正式签名
# 的包只能走应用市场。所以真机验证永远走 install.sh,这个脚本的产物只有一个
# 去处 —— AGC「软件包管理」上传。
#
# 签名走环境变量(见 hvigorfile.ts 顶部注释),因为仓里的 build-profile.json5
# 不带材料,而 DevEco 写的密文是平台相关的:
#   FLEET_OHOS_STORE_FILE / FLEET_OHOS_CERT_PATH / FLEET_OHOS_PROFILE_PATH
#   FLEET_OHOS_KEY_ALIAS / FLEET_OHOS_STORE_PASSWORD [/ FLEET_OHOS_KEY_PASSWORD]
#
# 用法:
#   bash scripts/build-app.sh                    # 打生产 relay,要求签名
#   RELAY_URL=... bash scripts/build-app.sh      # 换 relay
#   bash scripts/build-app.sh --allow-unsigned   # 只验构建链路,允许未签名产物
#   bash scripts/build-app.sh --no-web           # 跳过 web 同步(只重编 ArkTS)

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

# DevEco 自带 Node 18,而 pnpm 要 22+ —— 和 install.sh 同一个坑:先留一份系统
# PATH 给 web 构建,否则 sync-web 会以 "pnpm requires Node.js v22" 挂掉。
SYSTEM_PATH="$PATH"

# 两种工具链布局二选一。CI 上不可能装 DevEco Studio(它是带 GUI 的 IDE,而且
# macOS 那份 sdk 有 5.1G、也不是 Linux 的构建工具),所以 Linux runner 走华为的
# command-line-tools 包 —— 它自带 hvigorw / ohpm / sdk / node,布局却和 DevEco
# 里的 tools 目录不一样,这里必须分开认:
#
#   command-line-tools/          DevEco-Studio.app/Contents/
#     bin/hvigorw   (可执行)       tools/hvigor/bin/hvigorw.js  (要 node 跑)
#     bin/ohpm                    tools/ohpm/bin/ohpm
#     sdk/                        sdk/
#     tool/node/bin/node          tools/node/bin/node
#
# 用 HARMONY_CLI_TOOLS 指向 command-line-tools 解压后的根目录即可。
HVIGOR_CMD=()
if [ -n "${HARMONY_CLI_TOOLS:-}" ]; then
  CLI="$HARMONY_CLI_TOOLS"
  [ -x "$CLI/bin/hvigorw" ] || fail "HARMONY_CLI_TOOLS=$CLI 下没有可执行的 bin/hvigorw —— 解压路径是否多了一层?"
  export DEVECO_SDK_HOME="${DEVECO_SDK_HOME:-$CLI/sdk}"
  # command-line-tools 不带 JDK,CI 里由 setup-java 提供。缺了不能像 DevEco 那样
  # 回落到自带 jbr,所以在这儿就要求它。
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

# ------------------------------------------------------------------ 预检 --
# hvigor 对缺失签名配置的反应是产出 *unsigned* 包而不是报错,所以要么在这里
# 挡住,要么等上传 AGC 被拒才发现。
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
  # 必须在 assembleApp 之前:rawfile 是构建的输入,晚同步就把旧 web 打进包里。
  PATH="$SYSTEM_PATH" bash scripts/sync-web.sh
fi

# 换签名后不清 cache 会让 hvigor 报 UP-TO-DATE 直接跳过签名任务,产物仍带旧签名
# (2026-08-18 实测)。这个脚本每次都换签名来源,所以无条件清。
OUT=build/outputs/default

echo "→ 清 .hvigor/cache(不清会跳过签名任务,产物带旧签名)"
rm -rf .hvigor/cache
# 也清产物目录:下面判"签名有没有生效"靠的是产物文件名,而上一次跑剩下的
# -signed.app 会让一次没真产出任何东西的构建看起来成功。hvigor 的 clean 任务
# 通常会带走它,但那前提是 clean 真的跑了 —— 正是不该假设的那件事。
rm -rf "$OUT"

echo "→ 构建 assembleApp (release) …"
LOG=/tmp/hvigor-assembleapp.log
# 日志走文件而不是管道:`hvigorw | grep` 会把退出码换成 grep 的,构建失败也
# 看起来成功,然后把上一次的旧产物当成新的交出去。
set +e
# 不能带 install.sh 那句的 `--mode module`:assembleApp 是工程级任务,加了
# module 模式它会被静默忽略 —— 只有 clean 真的跑了,hvigor 仍报 BUILD
# SUCCESSFUL,而 build/ 目录根本没生成。
"${HVIGOR_CMD[@]}" clean assembleApp -p product=default -p buildMode=release --no-daemon \
  > "$LOG" 2>&1
BUILD_EXIT=$?
set -e
grep -iE "Error Message|ArkTS:ERROR|BUILD FAILED|No signingConfig" "$LOG" | tail -10 || true
(( BUILD_EXIT == 0 )) || fail "构建失败(exit $BUILD_EXIT),完整日志 $LOG"

# ------------------------------------------------------------------ 产物 --
# 产物名里带 -signed / -unsigned,这是判断签名有没有真生效的唯一可靠信号 ——
# 不能只看 hvigor 的退出码。签名成功时两个文件**并存**(unsigned 是中间产物,
# clean 不会带走它),所以先找 signed,找不到才回落到 unsigned。
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
