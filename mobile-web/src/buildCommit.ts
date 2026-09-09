/**
 * 这个 bundle 的构建 commit，截前 7 位——`transportRelay` 已经把同一个
 * `__APP_COMMIT__` 放进 hello 帧交给桌面端比对新旧，「关于」页只是把它显示给
 * 人看。没有 git 来源时 vite 的 define 给的是字符串 "unknown"，那不是 commit，
 * 空串让那一行整个不渲染。
 *
 * 单独成模块而不是留在 `MoreView` 里，是因为 `MoreView` 一 import 就拉起
 * `theme.ts`，那里在模块顶层调 `window.matchMedia` —— 一个纯字符串函数不该为了
 * 被测试而要求一个 DOM。
 */
export function shortBuildCommit(raw: string | undefined): string {
  return raw && raw !== "unknown" ? raw.slice(0, 7) : "";
}

export const BUILD_COMMIT = shortBuildCommit(
  typeof __APP_COMMIT__ === "string" ? __APP_COMMIT__ : undefined,
);
