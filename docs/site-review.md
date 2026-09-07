# 中英文消费者官网验收

状态：DONE_WITH_CONCERNS（代码与本地验收完成；尚未合并、推送、部署到国内）。

## 交付

- 英文 `docs/index.html`、中文 `docs/zh/index.html`，独立可索引的静态页面。
- 消费者叙事、产品导览（看任务 / 做决定 / 接续任务）、手机使用情境、四种实际安装包入口、上手步骤、FAQ。
- 保留 `#demo` 的既有产品视频入口。字体、样式、脚本、图片、视频资源均随站点提供。
- `scripts/site/build.py` 统一生成两份 HTML；共享样式和交互不重复维护。
- 国内推荐腾讯云 COS；Gitee 单文件和总容量限制已查官方文档。部署说明见 `docs/china-distribution.md`。
- 同步工具默认仅准备；显式 `--publish` 才写云端。现有 Release 流程可按配置自动接续，默认关闭。公开下载检查通过之后才发布新清单。

## 实测证据（2026-09-07）

1. 隔离 Chrome、1440 / 768 / 390 / 320 宽度、英文 / 中文：107 项 DOM 断言通过。包含无整页横向溢出、全部导览面板、决策示例发送与重置、键盘 End 键切换、FAQ、切换语言保留下载锚点、国内清单正常 / 不完整 / 无效 JSON。正常路径未发现本地 HTTP 错误。
2. 实下载 GitHub v2.6.0 的全部 7 个官方文件，逐个比对 GitHub SHA-256 和字节数，全部一致。此步骤不是模拟测试。文件保存在 `/tmp/fleet-site-review/prepared/releases/v2.6.0/`。
3. 5 项 Python 分发测试通过：非稳定 / 缺包 / 非官方地址、缺少摘要、损坏文件、上传顺序、公开域名失败时不更新清单；另含公开域名格式约束。
4. `node --check docs/site.js`、页面重新生成一致性、`git diff --check` 通过。`actionlint` 唯一提示 SC2129 已在未修改的 main Release 工作流上复现；排除这一原有样式提示后，三个相关工作流检查通过。
5. 中文与英文桌面 / 手机完整截图已目视检查。验收脚本、原始结果和四张截图已保存在 `/Users/hoveychen/.codex/artifacts/consumer-site-bilingual/`，不随工作树清理丢失。

6. 390 像素宽、禁用 JavaScript 的 Chrome：`enhanced=false`，中文页面标题正确、1 个 H1、4 个下载入口、无横向溢出。

## 尚未验证 / 待接入

- 未创建腾讯云资源、未配置公开域名 / 证书 / 凭证、未发生真实 COS 上传，没有声称国内下载已经可用或已测速。
- 腾讯云大陆地域需要按接入要求准备备案域名；香港地域是先行选项，但不等同大陆加速。
- 真实大陆线路速度和 macOS / Windows 安装体验要在部署之后验收。已校验安装包完整性，不等同这次重测产品安装流程。
- 本轮 OpenAI / ChatGPT 官网访问被 403 / Cloudflare 验证阻挡，设计按照用户要求的产品表达标准与独立视觉方案实现，未宣称逐页复制或实时视觉对照。
- 浏览器测试最初的请求拦截器等待卡住；停止后改用真实本地 HTTP 场景完成 107 项检查。没有为工具等待修改页面行为。

## 预览与后续

本机预览：`http://127.0.0.1:4187/zh/index.html` / `http://127.0.0.1:4187/index.html`。

停止预览后可重启：`python3 -m http.server 4187 --bind 127.0.0.1 --directory docs`（在工作树根执行）。

按 Boss 的 AGENTS.md Rule 4，最终合并前需提交可审阅结果并等待许可；批准后使用 `git merge --no-ff prd/consumer-site-bilingual`。`git push` 另须当前回合明确授权。国内上线条件见部署说明。
