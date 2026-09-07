# 官网语言检测与 Pages 状态

2026-09-07 已实际读取 https://hoveychen.github.io/claw-fleet/ ，确认是「Your personal AI workspace」新版官网，使用当前产品截图。最近一次 Pages 工作流成功：https://github.com/hoveychen/claw-fleet/actions/runs/34160652260 ，对应提交 75ccd88b。

## 语言规则

- 第一次打开默认入口：浏览器首选语言为 zh（包括 zh-CN、zh-HK、zh-TW 等）时进入中文；其他语言使用英文。
- 手动切换优先并在允许时保存到 localStorage。语言链接携带 `?lang=en` / `?lang=zh`，所以禁用存储或新标签打开时也不会跳回浏览器默认语言。
- 直接打开 `/zh/` 是明确的中文入口，不被浏览器英文偏好或已保存英文选择覆盖。显式 `?lang=en` 可强制进入英文。
- 自动跳转保留原有查询参数和 `#download` 等锚点。使用 replace，不额外插入一次重定向历史。
- 脚本在正文绘制前运行，无第三方依赖。禁用 JavaScript 时不自动检测，双语链接和下载基础功能仍可用。
- 同时适配深圳根域名与 GitHub Pages 的 `/claw-fleet/` 子路径。

实现：`docs/locale.js`；两份生成页面在 head 加载；`scripts/site/build.py` 负责语言链接中的显式选择。COS 和深圳更新器的静态资源清单都包含 locale.js，升级发行版后不会丢失语言检测。

## 验证

隔离浏览器 28 项断言通过：两个部署前缀 × zh-CN、zh-HK、en-US、fr-FR 首选语言，以及手动英文、保存偏好、直接中文 URL、禁用存储、显式 query、参数/锚点和无脚本页面可用性。浏览器脚本保存在 `/Users/hoveychen/.codex/artifacts/consumer-site-bilingual-v2/qa-language.js`。

深圳语言检测已使用更新器同一把锁发布到独立部署目录，完整验证后才原子替换 current。现有下载清单不改，旧部署与安装包保留。GitHub Pages 的语言检测需本次源码合并、推送后，由 Pages workflow 发布；原有新版官网已在线，二者状态分开记录。
