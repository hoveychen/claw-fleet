#!/bin/zsh
# 把 mobile-web 构建产物同步进 rawfile，供 WebShell 离线加载。
#
# 用法:
#   zsh scripts/sync-web.sh                    # 打生产 relay
#   RELAY_URL=http://127.0.0.1:18080 zsh scripts/sync-web.sh   # 打本地 relay
#
# 两个参数是硬性的，都是真机踩出来的，别改：
#
#   --base=./  vite 默认产出 `/assets/…` 这种绝对路径，在 WebShell 的加载方式
#              下解析不到，表现是「白屏但不报错」。Capacitor 不受影响是因为它
#              用 http://localhost 服务整个目录。
#
#   VITE_RELAY_URL  WebShell 从 rawfile 起页，页面 origin 是 https://fleet.local，
#              而 relayHttpBase() 在没有该变量时会回落到 origin —— 那会让 app
#              去连自己。必须在编译期把真实 relay 烘进去。
set -e
cd "$(dirname "$0")/.."

RELAY_URL="${RELAY_URL:-https://fleet-relay.muveeai.com}"
WEB_DIR="../mobile-web"
DEST="entry/src/main/resources/rawfile/web"

if [[ ! -d "$WEB_DIR" ]]; then
  echo "✗ 找不到 $WEB_DIR —— 本脚本假定鸿蒙工程与 mobile-web 同仓" >&2
  exit 1
fi

echo "→ 构建 mobile-web (relay: $RELAY_URL)"
( cd "$WEB_DIR" && VITE_RELAY_URL="$RELAY_URL" pnpm exec vite build --base=./ >/dev/null )

echo "→ 同步进 $DEST"
rm -rf "$DEST"
mkdir -p "$DEST"
cp -R "$WEB_DIR/dist/." "$DEST/"

# 白屏最常见的两个原因就是这两样缺失，构建期挡住比真机上查便宜得多。
[[ -f "$DEST/index.html" ]] || { echo "✗ 同步后没有 index.html" >&2; exit 1; }
grep -q 'src="\./' "$DEST/index.html" || {
  echo "✗ index.html 里不是相对路径 —— --base=./ 没生效，装上会白屏" >&2; exit 1; }

echo "✓ web 已同步 ($(du -sh "$DEST" | cut -f1))"
