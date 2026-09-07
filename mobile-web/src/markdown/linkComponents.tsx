// markdown 正文里的链接怎么点得开。
//
// 消息页以前把 `a` 直接换成一个 `<span>`：看着是链接（橙色 + 下划线），点下去
// 什么都不会发生 —— 老板在手机上点论文标题点不开，就是这一处。它从会话详情页
// 第一版起就是这样，不是回归，是一直没接。
//
// 为什么不是简单地一律 `target="_blank"`：两个原生壳都靠**同窗口导航**这一个
// 信号把外链交给系统浏览器 ——
//   - Capacitor Android：`BridgeWebViewClient.shouldOverrideUrlLoading` →
//     `Bridge.launchIntent()`，非本机 host 就 `ACTION_VIEW` 拉系统浏览器。
//   - 鸿蒙 WebShell：`Web.onLoadIntercept` → `ctx.openLink(url)`。
// 两边的 WebView 都没开多窗口（Android 的 `setSupportMultipleWindows` 默认
// false，ArkWeb 的 `multiWindowAccess` 同理），`target="_blank"` 会被静默丢掉。
// 所以壳里不加 target，浏览器 / PWA 里才加 —— 那里不加反而会把 SPA 导走。
import type { ComponentPropsWithoutRef } from "react";
import type { Components } from "react-markdown";
import { Capacitor } from "@capacitor/core";
import styles from "./mdLink.module.css";

/** 可以交给系统去打开的 scheme。其余（相对路径、`file:`、自造 scheme）保持不可
 *  点：在壳里点它们会导航掉整个 SPA，而不是打开任何东西。
 *
 *  只列 react-markdown 默认 urlTransform 放行的那几个 —— 它会把别的 scheme（比如
 *  `tel:`）洗成空串，那时 href 已经没了，标成可点只会得到一个点了没反应的链接。 */
const OPENABLE = /^(?:https?|mailto):/i;

/** 当前是不是跑在某个原生壳里（Capacitor 包 / 鸿蒙 WebShell）。鸿蒙那侧没有
 *  Capacitor 运行时，唯一的身份标记是壳注入的 `fleetNative` 桥，与
 *  voiceInput.ts / nativeScan.ts 用的是同一个。 */
export function inNativeShell(): boolean {
  if (Capacitor.isNativePlatform()) return true;
  if (typeof window === "undefined") return false;
  return typeof (window as unknown as { fleetNative?: unknown }).fleetNative === "object";
}

/** markdown 里的一个链接。可开的 scheme 渲染成真 `<a>`，其余保持 inert。
 *
 *  `node` 是 react-markdown 塞给每个组件的 hast 节点，不是 DOM 属性 —— 不摘出来
 *  它会被 spread 到标签上，渲染成 `node="[object Object]"`。 */
export function MdLink({
  href = "",
  children,
  node: _node,
  ...rest
}: ComponentPropsWithoutRef<"a"> & { node?: unknown }) {
  if (!OPENABLE.test(href)) {
    return (
      <span className={styles.inert} {...rest}>
        {children}
      </span>
    );
  }
  return (
    <a
      className={styles.link}
      href={href}
      // 壳里必须留在同窗口，见文件头。
      {...(inNativeShell() ? {} : { target: "_blank", rel: "noopener noreferrer" })}
      {...rest}
    >
      {children}
    </a>
  );
}

/** 每个 markdown 渲染面 spread 这一份，别再各写各的。 */
export const mdLinkComponents: Components = { a: MdLink };
