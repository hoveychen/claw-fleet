import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App, type TransportFactory } from "./App";
import { CloudApp } from "./cloud/CloudApp";
import { LightboxProvider } from "./views/Lightbox";
import { initTheme } from "./theme";
import { initWakeLock } from "./wakeLock";
import { lockZoom } from "./lockZoom";
import "./index.css";

initTheme();
initWakeLock();
lockZoom();

const cloudMode = import.meta.env.MODE === "cloud";

if (!cloudMode && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    // BASE_URL 而不是写死 "/"：同源形态挂在 `/m/` 下，注册 `/sw.js` 会去要一个
    // 根目录下并不存在的文件，SW 静默不生效。
    navigator.serviceWorker.register(`${import.meta.env.BASE_URL}sw.js`).catch(() => {
      // dev over plain http (non-localhost) has no SW; the app still works
    });
  });
}

// 整套「同源构建里不许有 relay」就落在这一对动态 import 上。`IS_WEBUI` 是编译期
// 常量（见 hostMode.ts），所以 Rollup 会把没走到的那条分支连同它拉起的整棵依赖
// 树一并消掉 —— relay 客户端、加密、配对存储，一个都不会进 webui 产物。
// 换成运行时判断的话两边都会被打进去，那正是要避免的。
// 条件里直接写 `import.meta.env.VITE_FLEET_HOST`，而不是用 hostMode 的 IS_WEBUI：
// vite 的 define 只替换字面出现的那个表达式，折叠成 `"webui" === "webui"` 之后
// Rollup 才必定消掉另一条分支。经过 hostMode 那层 const 中转时它不会消 ——
// 实测过：dist-webui 里照样落出一个 relay-*.js chunk（能搜到 resolveRelayBase
// 和 fleet-relay/hkdf/v1）。IS_WEBUI 在别处仍然好用，只有这一处对折叠敏感。
const { makeTransport }: { makeTransport: TransportFactory } =
  import.meta.env.VITE_FLEET_HOST === "webui"
    ? await import("./transportWebui")
    : await import("./transportRelay");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {cloudMode ? (
      <CloudApp />
    ) : (
      <LightboxProvider>
        <App makeTransport={makeTransport} />
      </LightboxProvider>
    )}
  </StrictMode>,
);
