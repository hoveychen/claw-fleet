// 让页面能唤起原生壳的二维码扫描。
//
// PWA 里加一台设备很自然:打开第二台桌面端的二维码链接就行(那个链接的 fragment
// 带着密钥,`devices.ts::consumeHashSecret` 会把它收下)。原生壳里没有这条路 ——
// 它从 rawfile 启动,地址栏不存在,而壳自己的扫码入口只在**尚未配对**时可达
// (WebShell 的 build() 只在 src 为空时画那张配对页)。于是一个装了 app 的用户
// 无法加入第二台机器,而这正是多设备最主要的入口。
//
// 补法是把扫码这件事从壳暴露给页面:壳注册 `fleetNative.scanPairing()`,页面在
// 设备列表里给出一行「扫码添加设备」。壳扫到之后按老路子重新加载 WebView 并注入
// `#k=…`,web 侧照常把它收成一台新设备 —— 所以 web 这边除了这一行入口之外不需要
// 任何鸿蒙分支。
//
// Capacitor 壳目前走 App Links(deepLink.ts),不需要这条;等它接上扫码能力时,
// 只要注册同名方法就自动生效。

/** 壳注入的原生桥对象名(mobile-harmony 的 WebShell.ets: javaScriptProxy)。 */
const BRIDGE = "fleetNative";

interface NativeBridge {
  scanPairing?: () => void;
}

function bridge(): NativeBridge | undefined {
  return (window as unknown as Record<string, NativeBridge | undefined>)[BRIDGE];
}

/** 这个壳能不能唤起扫码。浏览器/PWA 里恒为 false —— 那里不需要,也没有壳。 */
export function canScanPairing(): boolean {
  return typeof bridge()?.scanPairing === "function";
}

/** 唤起扫码。壳扫到后自己重新加载页面并注入新配对,所以这里没有回调 ——
 *  「加进来了没有」由随后那次加载告诉页面。 */
export function scanPairing(): void {
  bridge()?.scanPairing?.();
}
