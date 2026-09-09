import { describe, expect, it } from "vitest";
import { showsMobilePanel } from "./hostEnv";

// 「移动端」板块的可见性。三种 origin 各自的答案是这条判定的全部内容,而它们
// 之间的差别不是风格问题:给一台手机连不到的主机出配对码,扫完只会得到一台
// 停在「连接中…」的设备,而原因在界面上看不出来。
describe("showsMobilePanel", () => {
  it("桌面端一直有,不看 origin", () => {
    expect(showsMobilePanel(false, "https:", "fleet.example.com")).toBe(true);
    // Tauri webview 的页面既不是 https 也不是一个真主机名,照样要出。
    expect(showsMobilePanel(false, "tauri:", "localhost")).toBe(true);
    expect(showsMobilePanel(false, "http:", "127.0.0.1")).toBe(true);
  });

  it("云部署(https + 真主机名)⇒ 出", () => {
    expect(showsMobilePanel(true, "https:", "fleet.example.com")).toBe(true);
    expect(showsMobilePanel(true, "https:", "fleet-cloud.muveeai.com")).toBe(true);
  });

  it("本地 webui ⇒ 不出", () => {
    // `fleet webui` 默认就绑在这儿。
    expect(showsMobilePanel(true, "http:", "127.0.0.1")).toBe(false);
    expect(showsMobilePanel(true, "http:", "localhost")).toBe(false);
    // 隧道到本地也算本地:那个 origin 手机连不到。
    expect(showsMobilePanel(true, "https:", "localhost")).toBe(false);
    expect(showsMobilePanel(true, "https:", "127.0.0.1")).toBe(false);
    expect(showsMobilePanel(true, "https:", "127.1.2.3")).toBe(false);
    expect(showsMobilePanel(true, "https:", "[::1]")).toBe(false);
    expect(showsMobilePanel(true, "https:", "app.localhost")).toBe(false);
  });

  it("明文 http 的真主机 ⇒ 不出", () => {
    // 手机上那个页面是 https 发的,浏览器不允许它连明文 http。
    expect(showsMobilePanel(true, "http:", "fleet.example.com")).toBe(false);
    expect(showsMobilePanel(true, "http:", "192.168.1.5")).toBe(false);
  });

  it("主机名大小写不影响回环判定", () => {
    expect(showsMobilePanel(true, "https:", "LOCALHOST")).toBe(false);
  });
});
