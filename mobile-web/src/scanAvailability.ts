// 这台浏览器现在能不能在页面里扫码 —— 以及扫不了的话，是哪一种扫不了。
//
// 分两种说的理由：它们对用户是完全不同的两件事。相机被拒可以去系统设置里放行；
// 而地址不是 https 时，浏览器压根不暴露 `getUserMedia`（安全上下文限制），去设置
// 里翻一遍也找不到任何开关 —— 那种情况必须**当场**告诉他改走粘贴，而不是让他点
// 进取景器、撞一句笼统的「这台设备用不了摄像头」然后去系统设置里白找一圈。
//
// 纯函数 + 环境入参，与 push-classify.ts 同一套做法：模块加载期不碰任何全局，
// 于是 node 环境里可以直接对分类逻辑下断言。

export type ScanAvailability = "ok" | "insecure-origin" | "no-camera-api";

/** classify 依据的环境 —— 纯数据，不读全局。 */
export interface ScanEnv {
  /** `window.isSecureContext`。https、localhost，以及原生壳的自定义 scheme 都为真。
   *
   *  **拿不到时要传 `true`**：这个属性缺席只说明浏览器太老，不说明页面不安全。
   *  反过来默认成 `false` 会把扫码入口从所有这类浏览器上撤掉，代价比误留大得多。 */
  secureContext: boolean;
  /** `navigator.mediaDevices?.getUserMedia` 在不在。 */
  hasGetUserMedia: boolean;
}

export function classifyScan(env: ScanEnv): ScanAvailability {
  // 顺序要紧：非安全上下文里 `mediaDevices` 本来就不存在，先判它才能给出那条有用
  // 的理由 —— 否则每个 http 页面都会被归成「这台设备没有摄像头」，而那是假话。
  if (!env.secureContext) return "insecure-origin";
  if (!env.hasGetUserMedia) return "no-camera-api";
  return "ok";
}

/** 从当下的浏览器环境读一次。 */
export function scanAvailability(): ScanAvailability {
  return classifyScan({
    // `!== false` 而不是 `=== true`：见 ScanEnv.secureContext 的注释。
    secureContext: typeof window === "undefined" || window.isSecureContext !== false,
    hasGetUserMedia:
      typeof navigator !== "undefined" && typeof navigator.mediaDevices?.getUserMedia === "function",
  });
}
