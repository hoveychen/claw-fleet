# 官网 SEO：机制与收录操作

本文件是仓库内部参考，**不随官网发布**（`scripts/site/stage_pages.py` 的白名单只放行真正属于站点的文件）。

## 站点有两个域，各自 canonical

| 域 | 谁在发布 | 面向 |
|---|---|---|
| `https://hoveychen.github.io/claw-fleet` | `.github/workflows/pages.yml` | Google / Bing |
| `https://fleet.eternizedlab.com` | 深圳自托管机上的 `fleet-site-update.timer` → `selfhost.py` → `distribute.py` | 百度 / 国内访问 |

两个域发布的是**同一批字节相同的文件**，而 canonical、hreflang、`og:url`、JSON-LD、sitemap、robots 都需要绝对 URL。把某一个域写死会让镜像宣称 github.io 是自己的 canonical——国内用户打不开 github.io，等于主动退出百度收录。

所以生成器写占位符 `__SITE_ORIGIN__`（`scripts/site/site_origin.py`），两条发布路径各自替换成自己的域：

- Pages：`stage_pages.py --origin https://hoveychen.github.io/claw-fleet`
- 镜像：`distribute.prepare()` 用它已有的 `public_url` 参数替换

同一套机制还带一个 `__SITE_VERSION__`：JSON-LD 的 `softwareVersion` 也是发布时才填的。Pages 侧直接读 `pages_manifest.py` 刚生成的 `docs/downloads.json`（不用往工作流里再传一个参数）；镜像侧用 `distribute.py` 已经在校验的那个 release tag。镜像还多一步：它 re-publish 的是**上一次发布过的 HTML**，里面版本号已经是解析过的值，所以会被重定向到当前 tag，并由 `assert_version()` 断言真落地了（regex 静默 no-op 会把陈旧版本号悄悄放回去）。

`site_origin.assert_no_token()` 在两条路径上都会兜底：残留一个未替换的 token 页面照样能渲染、只有机器可读的那一半是错的，所以它必须是硬错误。

### 镜像上的 HTML 不会自动更新

服务器上的 `fleet-site-update.timer` 调 `selfhost.sync()`，而它是用 `site_root=current`（**上一次已发布的部署**）来跑 `distribute.prepare()` 的。也就是说：定时器只推进 release 产物和 `downloads.json`，**站点 HTML 从来不会自己从仓库刷新**。

所以本次 SEO 改动要落到镜像上，必须按 `china-selfhost.md` 那条手动发布路径走一次（仓库里跑 `build.py` + `distribute.py --public-url https://fleet.eternizedlab.com --output <新目录>`，上传后切软链接）。这条路径用的是**仓库里的** `scripts/site/`，所以能直接拿到 `site_origin.py`，不需要提前 scp。

反过来：`/opt/fleet-site-updater/` 下的服务器副本是独立的一份。哪天要更新那份的 `distribute.py`，**必须连 `site_origin.py` 一起 scp**（前者 `import` 后者，缺了就是 ImportError，更新器会停在原地）：

```
scp scripts/site/{distribute,selfhost,site_origin}.py own-api-sz:/tmp/
ssh own-api-sz 'sudo install -o fleet-site -g fleet-site -m 644 /tmp/{distribute,selfhost,site_origin}.py /opt/fleet-site-updater/'
ssh own-api-sz 'sudo systemctl start fleet-site-update.service && journalctl -u fleet-site-update.service -n 40'
```

## robots.txt 只在镜像上生效

爬虫只从**域根**读 robots.txt。Pages 把站点放在 `/claw-fleet/` 子路径下，`hoveychen.github.io/robots.txt` 不是我们的文件。所以：

- 镜像（裸域）：`/robots.txt` 正常生效，里面声明了 sitemap。
- Pages：靠在 Search Console 里直接提交 sitemap URL。

## sitemap 的 lastmod 是发布时盖的

`docs/sitemap.xml` 提交进仓库时**不带** `lastmod`。日期由 `scripts/site/sitemap_lastmod.py` 在发布前盖上，取每个页面**自己最后一次提交**的日期。

为什么不能在 `build.py` 里写：build 生成页面，那一刻改动它们的提交还不存在，所有日期都会晚一个提交——而晚的恰好是刚改过的那个页面，也就是唯一值得爬虫关心的页面。

两个陷阱都已挡住：
- **shallow clone**：`actions/checkout` 默认 depth 1，那种仓库里每个文件的「最后提交」都是 HEAD，盖上去等于宣称四个页面刚刚全改了。所以 `pages.yml` 里加了 `fetch-depth: 0`，脚本自己也拒绝在 shallow 仓库上盖（宁可不带 lastmod）。
- **没有 git / 页面查不到提交**：同样是整条不带 `lastmod`，而不是编一个。Google 抓到 lastmod 与内容不符会直接不再信这个字段。

镜像手动发布时也要先跑一次这个脚本（它改的是仓库里的 `docs/sitemap.xml`，然后再生成镜像站点树）。

## 站长验证怎么放

两种都已支持，任选其一：

1. **meta 标签**（推荐，进 git、不会丢）：把验证码填进 `scripts/site/seo.py` 的 `VERIFICATION` 字典，例如
   ```python
   VERIFICATION = {
       'google-site-verification': 'xxxxxxxxxxxx',
       'baidu-site-verification': 'codeva-xxxxxxxx',
   }
   ```
   重跑 `python3 scripts/site/build.py`。只有两个首页会带这些 meta——验证是对你提交的那个 URL 做的，全站重复没意义。
2. **HTML 文件**：把控制台给的文件（`google*.html`、`baidu_verify_*.html` 等）直接放到 `docs/` 根下即可，`stage_pages.VERIFICATION_GLOBS` 会放行。文件名不在那几个模式里的，往 `EXTRA_FILES` 加一行。

## 两个域的上线状态（2026-09-08 实测）

| 域 | canonical | sitemap / robots | softwareVersion |
|---|---|---|---|
| `hoveychen.github.io/claw-fleet` | 指向自己 ✓ | 200 / 200 ✓ | 2.7.0 ✓ |
| `fleet.eternizedlab.com` | 指向自己 ✓ | 200 / 200 ✓ | 2.7.0 ✓ |

github.io 侧由 push 后的 Pages 工作流发布；镜像侧走了一次手动发布（`deployments/manual-v2.7.0-seo-20260908`，随后 `--rebuild-current` 又生成了 `auto-v2.7.0-4606ce1f58e3`）。`/site-review.md` 与 `/deploy/*.conf` 在 Pages 上已是 404。

服务器副本 `/opt/fleet-site-updater/` 已同步到含 `site_origin.py` 的新版（旧版备份在同目录 `*.bak-preseo`），并用 `--rebuild-current` 真跑过一次：新生成的 auto deployment 带上了 sitemap.xml 与 robots.txt，说明下次发版不会把它们丢掉——这正是旧副本会犯的错（它的 assets 基线里没有这两个文件，而没有页面 href 引用它们）。

## 收录动作清单（需要老板的账号，代码侧已就绪）

1. **Google Search Console**：加两个资源。github.io 是子路径，只能用 URL 前缀资源 `https://hoveychen.github.io/claw-fleet/`；镜像用域名资源 `fleet.eternizedlab.com`（要加 DNS TXT）。各自提交 `sitemap.xml`。
2. **Bing Webmaster Tools**：可从 GSC 一键导入。
3. **百度搜索资源平台**：只加镜像域（github.io 在国内不可达，加了也抓不到）。验证 → 提交 `https://fleet.eternizedlab.com/sitemap.xml`。
4. **百度主动推送**：拿到 `site` + `token` 后，发版时把四个 URL 推一遍：
   ```
   curl -H 'Content-Type: text/plain' --data-binary @urls.txt \
     'http://data.zz.baidu.com/urls?site=https://fleet.eternizedlab.com&token=<TOKEN>'
   ```
   `urls.txt` 就是 sitemap 里那四条 `<loc>`。想自动化的话挂在镜像的 `fleet-site-update.service` 后面，或做成 `fleet loop`。
5. **IndexNow**（Bing / Yandex / Seznam 共用）：需要在站点根放一个 `<key>.txt`，属于要往 `EXTRA_FILES` 加一行的那种文件。

## 已经做到的（不必重复检查）

- 四页都有自指 canonical、完整含自身的 hreflang 三元组（缺自指会让整簇 hreflang 被 Google 丢弃）。
- `sitemap.xml` 用 Google 的 sitemap-hreflang 形式；不写 `lastmod`/`changefreq`/`priority`——守不住的字段比没有更糟。
- JSON-LD：首页 `Organization` + `WebSite` + `SoftwareApplication`（免费、四平台、AGPL）+ `FAQPage`；实测报告页 `Organization` + `BreadcrumbList` + `FAQPage`。**没有 `aggregateRating`**：没有真实评分，编一个会让整站结构化数据被忽略。
- FAQ 的结构化数据由页面渲染的同一份 pairs 生成，markup 与可见内容不可能漂移（`test_seo.py` 有守门测试）。
- `softwareVersion` 由发布路径回填真实 tag，构建产物里只有 token——不会出现「结构化数据说 2.5.0、下载按钮给 2.7.0」。
- 标题与描述覆盖真实搜索意图：英文面向 Google（Claude Code GUI / desktop app），中文面向百度（桌面图形界面 / 客户端），平台与「免费开源」写进描述。
- `docs/` 里的设计稿、产品调研、站点评审、镜像的 systemd 单元与 nginx 配置**不再随官网发布**（此前 33 个文件是公开的）。

## 还没做、需要单独定夺的

- **自定义顶级域**。github.io 子路径站点的权重天花板明显低于独立域名，而且 robots.txt 也拿不到。真要往上做，绑一个自己的域是最大的一步。
- **面向查询的新内容**。现在的关键词覆盖只动了 title/description；「为什么需要 GUI」「和裸 CLI 的差别」这类页面能吃到长尾流量，但那是要新写的内容，属于产品文案决定，不是技术项。
- **截图转 WebP**。12 张 PNG 共 2.6MB，首屏已 `fetchpriority=high`、宽高已内联，LCP 目前不是瓶颈；真要压再说。
