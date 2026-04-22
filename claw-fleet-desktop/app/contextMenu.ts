/**
 * App-wide right-click context menu.
 *
 * Installed in every webview entry point to replace the default WKWebView /
 * WebView2 context menu (which otherwise shows "Reload", "Back", "Forward",
 * etc.) with an app-relevant menu: Settings / About / Quit.
 *
 * Native menu is preserved inside text inputs so Copy/Paste still works.
 * Component-level handlers that call `preventDefault()` (e.g. the overlay
 * mascot's own context menu) are also respected.
 */

import { invoke } from "@tauri-apps/api/core";
import { getItem } from "./storage";

type LangKey = "settings" | "about" | "quit" | "about_title" | "about_version" | "about_close";

const STRINGS: Record<"zh" | "en", Record<LangKey, string>> = {
  zh: {
    settings: "设置",
    about: "关于",
    quit: "退出",
    about_title: "Claw Fleet",
    about_version: "版本",
    about_close: "关闭",
  },
  en: {
    settings: "Settings",
    about: "About",
    quit: "Quit",
    about_title: "Claw Fleet",
    about_version: "Version",
    about_close: "Close",
  },
};

function currentLang(): "zh" | "en" {
  const saved = getItem("lang");
  if (saved === "zh" || saved === "en") return saved;
  const nav = (navigator.language || "").toLowerCase();
  return nav.startsWith("zh") ? "zh" : "en";
}

function tr(key: LangKey): string {
  return STRINGS[currentLang()][key];
}

function isDark(): boolean {
  const themeAttr = document.documentElement.getAttribute("data-theme");
  if (themeAttr === "dark") return true;
  if (themeAttr === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function closeMenu(menu: HTMLElement) {
  menu.remove();
  document.removeEventListener("mousedown", onOutsideDown, true);
  document.removeEventListener("keydown", onKeyDown, true);
  window.removeEventListener("blur", onWindowBlur);
  window.removeEventListener("resize", onWindowResize);
}

let activeMenu: HTMLElement | null = null;

function onOutsideDown(ev: MouseEvent) {
  if (!activeMenu) return;
  if (!activeMenu.contains(ev.target as Node)) {
    closeMenu(activeMenu);
    activeMenu = null;
  }
}

function onKeyDown(ev: KeyboardEvent) {
  if (ev.key === "Escape" && activeMenu) {
    closeMenu(activeMenu);
    activeMenu = null;
  }
}

function onWindowBlur() {
  if (activeMenu) {
    closeMenu(activeMenu);
    activeMenu = null;
  }
}

function onWindowResize() {
  if (activeMenu) {
    closeMenu(activeMenu);
    activeMenu = null;
  }
}

function buildMenuItem(label: string, onClick: () => void, dark: boolean): HTMLElement {
  const item = document.createElement("div");
  item.textContent = label;
  item.style.cssText = [
    "padding: 6px 14px",
    "font-size: 13px",
    `color: ${dark ? "#e5e5e5" : "#1f1f1f"}`,
    "cursor: default",
    "user-select: none",
    "border-radius: 4px",
    "white-space: nowrap",
  ].join(";");
  item.addEventListener("mouseenter", () => {
    item.style.background = dark ? "#3a3a3a" : "#e8e8e8";
  });
  item.addEventListener("mouseleave", () => {
    item.style.background = "";
  });
  item.addEventListener("click", (ev) => {
    ev.stopPropagation();
    if (activeMenu) {
      closeMenu(activeMenu);
      activeMenu = null;
    }
    onClick();
  });
  return item;
}

function openContextMenu(x: number, y: number) {
  if (activeMenu) {
    closeMenu(activeMenu);
    activeMenu = null;
  }

  const dark = isDark();
  const menu = document.createElement("div");
  menu.setAttribute("data-app-context-menu", "");
  menu.style.cssText = [
    "position: fixed",
    "z-index: 2147483647",
    "min-width: 160px",
    "padding: 4px",
    `background: ${dark ? "#242424" : "#ffffff"}`,
    `color: ${dark ? "#e5e5e5" : "#1f1f1f"}`,
    `border: 1px solid ${dark ? "#2e2e2e" : "#d0d0d0"}`,
    "border-radius: 8px",
    "box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3)",
    "font: 13px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  ].join(";");

  menu.appendChild(
    buildMenuItem(tr("settings"), () => {
      invoke("open_settings_window").catch((e) => console.error("open_settings_window failed:", e));
    }, dark),
  );
  menu.appendChild(
    buildMenuItem(tr("about"), () => {
      showAboutDialog().catch((e) => console.error("showAboutDialog failed:", e));
    }, dark),
  );

  const separator = document.createElement("div");
  separator.style.cssText = `height:1px;margin:4px 6px;background:${dark ? "#333" : "#e0e0e0"};`;
  menu.appendChild(separator);

  menu.appendChild(
    buildMenuItem(tr("quit"), () => {
      invoke("quit_app").catch((e) => console.error("quit_app failed:", e));
    }, dark),
  );

  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
  menu.style.visibility = "hidden";
  document.body.appendChild(menu);

  // Clamp to viewport after we know the real size.
  const rect = menu.getBoundingClientRect();
  const pad = 4;
  if (rect.right > window.innerWidth - pad) {
    menu.style.left = `${Math.max(pad, window.innerWidth - rect.width - pad)}px`;
  }
  if (rect.bottom > window.innerHeight - pad) {
    menu.style.top = `${Math.max(pad, window.innerHeight - rect.height - pad)}px`;
  }
  menu.style.visibility = "";

  activeMenu = menu;
  document.addEventListener("mousedown", onOutsideDown, true);
  document.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("blur", onWindowBlur);
  window.addEventListener("resize", onWindowResize);
}

async function showAboutDialog() {
  let version = "";
  try {
    version = await invoke<string>("get_app_version");
  } catch (e) {
    console.warn("get_app_version failed:", e);
  }

  const dark = isDark();
  const overlay = document.createElement("div");
  overlay.setAttribute("data-app-about-overlay", "");
  overlay.style.cssText = [
    "position: fixed",
    "inset: 0",
    "z-index: 2147483646",
    "background: rgba(0, 0, 0, 0.45)",
    "display: flex",
    "align-items: center",
    "justify-content: center",
    "font: 13px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  ].join(";");

  const panel = document.createElement("div");
  panel.style.cssText = [
    "min-width: 280px",
    "max-width: 360px",
    "padding: 20px 22px 16px",
    `background: ${dark ? "#242424" : "#ffffff"}`,
    `color: ${dark ? "#e5e5e5" : "#1f1f1f"}`,
    `border: 1px solid ${dark ? "#2e2e2e" : "#d0d0d0"}`,
    "border-radius: 12px",
    "box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4)",
    "text-align: center",
  ].join(";");

  const title = document.createElement("div");
  title.textContent = tr("about_title");
  title.style.cssText = "font-size:15px;font-weight:600;margin-bottom:8px;";
  panel.appendChild(title);

  if (version) {
    const versionLine = document.createElement("div");
    versionLine.textContent = `${tr("about_version")} ${version}`;
    versionLine.style.cssText = `font-size:12px;color:${dark ? "#a8a8a8" : "#666"};margin-bottom:16px;`;
    panel.appendChild(versionLine);
  }

  const button = document.createElement("button");
  button.textContent = tr("about_close");
  button.style.cssText = [
    "padding: 6px 18px",
    "font: inherit",
    `background: ${dark ? "#3a3a3a" : "#f0f0f0"}`,
    `color: ${dark ? "#e5e5e5" : "#1f1f1f"}`,
    `border: 1px solid ${dark ? "#4a4a4a" : "#d0d0d0"}`,
    "border-radius: 6px",
    "cursor: pointer",
  ].join(";");

  const close = () => {
    overlay.remove();
    document.removeEventListener("keydown", onEsc, true);
  };
  const onEsc = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") close();
  };

  button.addEventListener("click", close);
  overlay.addEventListener("mousedown", (ev) => {
    if (ev.target === overlay) close();
  });
  document.addEventListener("keydown", onEsc, true);

  panel.appendChild(button);
  overlay.appendChild(panel);
  document.body.appendChild(overlay);
  button.focus();
}

function shouldKeepNativeMenu(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  return !!target.closest("input, textarea, [contenteditable='true'], [contenteditable='']");
}

export function installAppContextMenu(): void {
  if ((window as unknown as { __appContextMenuInstalled?: boolean }).__appContextMenuInstalled) {
    return;
  }
  (window as unknown as { __appContextMenuInstalled?: boolean }).__appContextMenuInstalled = true;

  window.addEventListener("contextmenu", (ev) => {
    if (ev.defaultPrevented) return;
    if (shouldKeepNativeMenu(ev.target)) return;
    ev.preventDefault();
    openContextMenu(ev.clientX, ev.clientY);
  });
}
