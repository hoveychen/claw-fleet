// 锁缩放的 JS 兜底层。
//
// 背景：<meta viewport user-scalable=no/maximum-scale> 从 Chrome 48+ 起被有意
// 忽略（保留无障碍缩放），index.css 里的 `touch-action: pan-x pan-y` 是给遵守规范
// 的引擎准备的第二道防线。但鸿蒙 NEXT 的 ArkWeb 引擎（com.huawei.hmos.browser）
// 连 touch-action 也不完全遵守，双指/双击仍能缩放。
//
// preventDefault 于 touch 事件是比 touch-action 更底层的钩子——它决定手势是否
// 被浏览器消费成滚动/缩放，是 Web 控制手势的根机制，几乎所有引擎（含 ArkWeb）
// 都必须遵守。因此这里用它做最后兜底：
//   - 多指 touchmove → 屏蔽 pinch-zoom（单指滚动不受影响）
//   - 300ms 内二次 touchend → 屏蔽 double-tap-zoom
//   - Safari 私有 gesture* 事件 → 屏蔽触控板/旧 iOS 捏合
//   - ctrl+wheel → 屏蔽桌面端触控板捏合与 ctrl+滚轮缩放
//
// 全部用 { passive: false } 注册，否则 preventDefault 无效。

export function lockZoom(): void {
  document.addEventListener(
    "touchmove",
    (e) => {
      if (e.touches.length > 1) e.preventDefault();
    },
    { passive: false },
  );

  let lastTouchEnd = 0;
  document.addEventListener(
    "touchend",
    (e) => {
      const now = e.timeStamp;
      if (now - lastTouchEnd <= 300) e.preventDefault();
      lastTouchEnd = now;
    },
    { passive: false },
  );

  // Safari/WebKit 私有捏合事件
  for (const type of ["gesturestart", "gesturechange", "gestureend"]) {
    document.addEventListener(type, (e) => e.preventDefault(), {
      passive: false,
    });
  }

  // 桌面端触控板捏合 / ctrl+滚轮缩放
  document.addEventListener(
    "wheel",
    (e) => {
      if (e.ctrlKey) e.preventDefault();
    },
    { passive: false },
  );
}
