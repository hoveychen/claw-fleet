> **当前已采用深圳自托管。** Boss 选择使用现有 own-api-sz 服务器；正式站点为 https://fleet.eternizedlab.com/zh/ 。更新与回退见 [自托管说明](china-selfhost.md)。下文 COS 为备选部署方案，未启用。

# 国内官网与下载分发

## 推荐：腾讯云 COS；Gitee 作补充入口

官网与二进制安装包放同一个 COS 桶，GitHub Release 仍作为唯一版本来源。支持自定义 HTTPS 域名与目录前缀，静态页面没有外部字体、图片 CDN 或 GitHub API 运行时依赖。中文 `/zh/index.html`、英文 `/index.html` 都能直接访问。

2026-09-07 查阅的官方依据：

- [腾讯云静态网站](https://cloud.tencent.com/document/product/436/14984)：需要自定义域名；2024 年之后的新桶不能用默认 COS 域名作为公开官网。大陆地域域名接入需按服务要求备案；香港地域可作为先行部署，但不能声称大陆加速或保证国内下载速度。
- [Gitee 创建 Release](https://help.gitee.com/repository/release/create)：普通单文件 <=100 MB，仓库附件累计 <=1 GB。v2.6.0 的 macOS 包 70,734,592 字节，Windows 包 32,482,625 字节，当前可用；不适合作为不限版本的长期安装包仓库。可建立 Gitee 公共仓库，发行说明链接到 COS；或者手动上传当前版本包及 SHA256SUMS。匿名下载、额度变化仍需开通后用大陆网络实测。
- CNB 未完成下载 API / 额度核实，不作为本次上线依赖。

## 已实现与尚缺的条件

已实现：双语官网、版本准备工具、GitHub 官方 SHA-256 比对、COS 上传、匿名 HEAD 验证、清单最后发布、保留 GitHub 备用入口、发布工作流自动接续。未实施：创建云资源、付费、域名备案或绑定、写入远端、国内网络测速。

Boss 开通时需要：腾讯云账号；一个专用 COS 桶和地域；已绑定证书的公开域名；只允许向该桶发布路径上传及读取元信息的子账号凭证。凭证填 GitHub Secrets，不发到对话、不提交仓库。Bucket 开启静态网站、索引 `index.html`；公开读取通过桶策略或 CDN 回源配置，写入仅限发布账号。无需写入删除权限。官网和包同域名同路径，跨域 CORS 不需要。

## 配置 GitHub

Repository variables:

| 名称 | 值 |
| --- | --- |
| CHINA_DISTRIBUTION_ENABLED | 配置完再设为 `true` |
| COS_BUCKET | 完整桶名，带 APPID 后缀 |
| COS_REGION | 桶的实际地域，如 `ap-guangzhou` |
| COS_PUBLIC_URL | 绑定好的 HTTPS 自定义域名，可带目录前缀，不带末尾斜杠 |

Repository secrets: `COS_SECRET_ID`、`COS_SECRET_KEY`。创建 `china-distribution` Environment，可配置审核人控制真实发布。

1. 先手动运行 **Publish China website and downloads**，tag 选当前稳定版，例如 `v2.6.0`。
2. 流程从官方 Release 下载包，校验大小与 GitHub 提供的 SHA-256，再上传 `releases/v2.6.0/…`。没有 SHA-256 的发行版会被拒绝，不能静默降级为只算本地摘要。
3. 所有包都通过公开域名 HEAD 检查后，依次上传静态依赖、HTML、`downloads.json`。HTML/CSS 缓存 5 分钟；清单要求重新验证；版本包按不可变内容缓存。工具不会删除历史版本。
4. 现有 **Release** 的 publish job 成功后会直接调用同一复用流程。不能依赖 `release: published` 事件，因为 GitHub Actions token 创建 Release 不一定触发新工作流。
5. 只更新官网时手动运行分发流程，tag 填 `latest`。发布指定旧 tag 会主动切回该旧稳定版，需在触发前核对 tag。
6. GitHub Pages 保持独立部署。启用国内分发后，手动运行 **Deploy Landing Page to GitHub Pages** 可把镜像清单同步到 GitHub 站；此后每次 Pages 发布会重新读取镜像清单。清单获取失败时保留 GitHub 下载。

## 本地只准备，不发布

```bash
python3 scripts/site/build.py
python3 scripts/site/distribute.py \
  --tag v2.6.0 \
  --public-url https://YOUR_CONFIGURED_DOMAIN \
  --output /tmp/claw-fleet-china-site
```

`--public-url` 必须换成实际配置的域名。默认只在本地下载、校验和生成站点，不会写云端。实际发布需安装 `cos-python-sdk-v5==1.9.38`，设置上述四个 `COS_*` 环境变量后显式加 `--publish`。`GH_TOKEN` 可选，仅用于 GitHub 元数据查询，不转发给安装包重定向地址。

`docs/downloads.json` 已配置经实测的深圳自托管镜像。尚未开通镜像的新环境应使用空镜像配置，避免显示不存在的下载。真正部署输出中的清单由脚本自动生成，包含版本、每个包的 HTTPS 地址、大小和 SHA-256。中文页面默认国内线路，英文默认 GitHub，可手动切换。每个国内按钮旁有原始 GitHub 链接。不要把测试使用的 example.com 清单提交到官网。

## 上线验收

使用实际大陆网络完成这些步骤，才能声称国内分发已可用：

1. 匿名打开 `/zh/index.html` 和 `/index.html`，检查图片、脚本无 GitHub / Google 请求、两语切换正常。
2. 中文下载区出现真实版本号和国内线路；GitHub 备用入口能切换。
3. 实下载 macOS / Windows / 两种 Linux 包，与同版本 GitHub SHA-256 比对，记录下载字节数与实际耗时。
4. macOS / Windows 真机安装并核对版本；HEAD 成功不能代替安装验收。
5. 发布下一版后检查只有全部包可下载时才更新清单；模拟缺包、错误摘要、无公开访问权限时应拒绝更新清单。

## 官网维护

双语文案：`scripts/site/content/zh.json` / `en.json`；模板：`scripts/site/build.py`；当前产品截图：`docs/screenshots/current/`（实际组件、示例数据）；样式与交互：`docs/site.css` / `docs/site.js`。执行 `python3 scripts/site/build.py` 后提交两个 HTML 产物。无需 Node 构建依赖。不要直接改生成的 HTML。

### 更新官网双语截图

示例数据在 `scripts/site/fixtures/scenes.json`，两种语言使用独立文案。`generate.py` 生成真实可预览的 HTML/Markdown 示例材料；`capture.mjs` 使用真实桌面和手机组件生成八张截图，取景在生成阶段完成，官网不再二次偏移裁剪。

分别启动 `claw-fleet-desktop` 的 Vite（端口 5299）与 `mobile-web` 的 Vite（端口 5288），从仓库根执行：

```sh
python3 scripts/site/fixtures/generate.py
node scripts/site/capture.mjs
python3 scripts/site/build.py
```

截图使用已安装 `patchwright-cli` 自带的 Patchright 与 Chrome；也可用 `PATCHRIGHT_MODULE` 指向本地 Patchright 模块。官网在 5290 端口提供服务后，执行 `node scripts/site/verify.mjs` 检查双语引用、资源加载与桌面/手机显示，再逐张打开图片眼验。
