# 国内自托管官网与下载

状态：DONE_WITH_CONCERNS。2026-09-07 已按 Boss 授权上线，公网页面和下载验收通过；国内住宅/手机网络测速与原生安装不在本次实测范围内。

## 核查结果（2026-09-07）

- `ssh own-api-sz` 可访问，root；阿里云深圳，公网 DNS 指向 `47.106.173.51`；剩余约 24 GB。
- Nginx 为 `www.eternizedlab.com` 在本机 `127.0.0.1:8443` 终止 TLS；外层 443 按 SNI 转发。主站「一个人的机房」根目录为 `/var/www/eternizedlab`。
- 现有证书覆盖 `eternizedlab.com` 与 `www.eternizedlab.com`，有效至 2026-10-27；不覆盖 `fleet.eternizedlab.com`。
- 部署前，其他域名（含 fleet 子域名）走境外转发。因此 DNS 指向深圳本身不代表网站内容从国内出站。
- 初始提议复用 www 主站的 `/fleet/` 子目录；Boss 选择独立子域名，最终按下节配置部署。

## Boss 选定的方案

使用 `https://fleet.eternizedlab.com/zh/`（中文）与 `https://fleet.eternizedlab.com/`（英文）。为该域名单独申请 Let’s Encrypt 证书，新增 `/etc/nginx/sites-available/fleet.conf`，并在 `/etc/nginx/stream.d/muvee-forward.conf` 的 SNI map 中只新增 `fleet.eternizedlab.com local_tls_443;`。复用现有 certbot timer 和 deploy hook。独立证书有效至 2026-12-06，续期钩子先执行 `nginx -t` 再 reload。完整虚拟主机配置保存在 `deploy/fleet-selfhost.nginx.conf`。

站点根目录 `/srv/claw-fleet-site/current`，指向 `/srv/claw-fleet-site/deployments/20260907-v2.6.0`。旧主站保持原样。原 SNI 配置已备份在 `/root/fleet-site-backup-20260907/`。

## 发布内容

本地 `/tmp/fleet-site-selfhost/`：中英页面、本地静态资源、当前产品截图、`downloads.json` 与 `releases/v2.6.0/` 的全部 7 个官方文件。安装包共 223,557,517 字节，逐个与 GitHub SHA-256 一致。

发布站点专用配置：镜像基址 `https://fleet.eternizedlab.com`；清单 provider `Eternized Lab · Shenzhen`；分享预览图片改为同域名；页脚沿用既有站点的 `粤ICP备2026103741号-1`。这些配置只应用于部署产物，不改变 GitHub Pages 的站点配置。官方包原封不动。

## 站点文件部署命令（证书与路由另见上述配置）

```bash
ssh own-api-sz 'install -d -m 755 /srv/claw-fleet-site/deployments/20260907-v2.6.0'
rsync -a /tmp/fleet-site-selfhost/ own-api-sz:/srv/claw-fleet-site/deployments/20260907-v2.6.0/
ssh own-api-sz 'cd /srv/claw-fleet-site/deployments/20260907-v2.6.0 && sha256sum -c DEPLOY-SHA256SUMS'
ssh own-api-sz 'test ! -e /srv/claw-fleet-site/current && test ! -L /srv/claw-fleet-site/current && ln -s /srv/claw-fleet-site/deployments/20260907-v2.6.0 /srv/claw-fleet-site/current'
```

先完整上传到未公开目录，服务器校验全部文件通过后才建立入口软链接。新增 Fleet 虚拟主机的 `try_files` 提供该目录。首次配置需要 `nginx -t` 后 reload；后续仅切换站点入口不需要 reload。官网主目录与现有服务不改动。发布不会 git push，不创建 COS 资源。

## 验证与回退

- 公网 HTTPS 打开两种语言，实际浏览器确认中文默认国内下载，英文可切换国内线路，四个按钮指向本站 `releases/v2.6.0/`。
- 公网下载安装包并比对 SHA-256；从深圳服务器通过公网域名探测响应，记录实际下载字节数及耗时。服务器自身网络不代表国内住宅网络，不能冒充用户端测速。
- 验证 Range 请求返回 206，确认支持断点下载；主站标题与状态保持原样。
- 首次发布回退：确认 `/srv/claw-fleet-site/current` 仍指向本次目录后，移除该软链接即可下线新入口，保留发布文件供调查。后续更新用新部署目录，全量校验后原子替换入口，回退时切回上次目录。

## 后续更新

运行仓库 `scripts/site/build.py` 和 `scripts/site/distribute.py --tag <稳定版本> --public-url https://fleet.eternizedlab.com --output <新本地目录>`（不要加 `--publish`，该开关专用于 COS）。工具先下载并验证官方文件，再生成清单。随后按本节发布配置应用 provider、分享图片和备案页脚，生成全站 SHA-256 清单，上传到新部署目录，校验通过后切换入口。保留已发布的历史 `releases/<版本>/` 路径，避免旧链接因站点更新失效。

当前 GitHub 自动工作流仅支持 COS；自托管暂用上述 SSH 发布流程，未声称已实现 SSH 自动发版。


## 部署产物的域名配置

每次准备新目录后，在该目录运行以下 Python。它只调整站点配置，不修改官方发行包；重复执行不会重复添加备案号。

```python
from pathlib import Path
import hashlib, json
root = Path.cwd()
origin = "https://fleet.eternizedlab.com"
p = root / "downloads.json"
manifest = json.loads(p.read_text())
manifest["china"]["provider"] = "Eternized Lab · Shenzhen"
p.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
for p in [root / "index.html", root / "zh/index.html"]:
    text = p.read_text().replace(
        "https://hoveychen.github.io/claw-fleet/screenshots/current/work-en.png",
        origin + "/screenshots/current/work-en.png",
    )
    if "粤ICP备2026103741号-1" not in text:
        text = text.replace("</footer>", '<p><a href="https://beian.miit.gov.cn/" rel="noopener noreferrer">粤ICP备2026103741号-1</a></p></footer>')
    p.write_text(text)
paths = sorted(p for p in root.rglob("*") if p.is_file() and p.name != "DEPLOY-SHA256SUMS")
(root / "DEPLOY-SHA256SUMS").write_text("".join(
    f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(root).as_posix()}\n"
    for p in paths
))
```

新目录必须包含仍需保留的旧版本包，然后再生成全站清单。用普通 rsync 复制旧 `releases/`，不要使用 `--delete` 清除历史包。HTML 不设置长期缓存；安装包版本路径设置一年 immutable；`downloads.json` 必须重新验证。新增版本不要复用已公开的 tag 路径来放不同字节。

首次路由的撤销：从 SNI map 中移除仅属于 Fleet 的那一行，再移除 Fleet 虚拟主机的启用软链接，`nginx -t` 通过后 reload。不要直接整份覆盖原备份，否则可能覆盖别的会话之后新增的路由。证书和发布目录保留，便于再次上线。


## 上线验收结果

- 中文：https://fleet.eternizedlab.com/zh/ ；英文：https://fleet.eternizedlab.com/ 。HTTPS 证书有效，中文默认本站下载，英文可切换国内线路。
- 服务器上全部 25 个发布文件通过全站 SHA-256 校验，含全部 7 个官方发行文件。
- 公网实际浏览器 23 项断言通过：双语、默认下载线路、四个国内按钮、三幅产品面板、手机/桌面布局、备案页脚、语言切换保留锚点；页面实际加载资源全部来自 fleet.eternizedlab.com，无 HTTP 错误。
- 从执行环境通过公网完整下载 macOS、Windows、Linux x64、Linux ARM64，大小与 SHA-256 全部符合 GitHub 官方摘要。实测并发下载分别 7.882、4.963、6.043、5.458 秒。这是本次执行环境的测量，不代表国内家庭网络速度。
- 深圳服务器自身通过公网域名下载 macOS 包：70,734,592 字节，1.802504 秒，HTTP 200；日志确认本次请求 upstream 为 `127.0.0.1:8443`。此项用于验证本地路由，不是国内终端用户测速。
- macOS Range 请求 `bytes=0-1023` 返回 206、Content-Range `bytes 0-1023/70734592`。下载清单要求重新验证，版本安装包 immutable 缓存生效。
- 原主站「一个人的机房」首页 SHA-256 在切换前后保持 `cbc4d6f7461e8761a59a9335625306e7d85c1afdbc34771018d0564167e32777`。
- `docs/downloads.json` 已写入实际可访问的国内镜像，后续 Pages 发布可直接提供该入口。此次没有 git push，也没有触发 GitHub Pages 发布。

证据保存于 `/Users/hoveychen/.codex/artifacts/consumer-site-bilingual-v2/`：`live-download-verification.json`、`qa-live.js`、正式站点中英截图和 `live-verification.txt`。
