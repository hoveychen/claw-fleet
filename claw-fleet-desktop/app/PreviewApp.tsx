import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import { safeMarkdownComponents } from "./markdown/safeLinks";
import { resolveTheme, useUIStore } from "./store";
import styles from "./PreviewApp.module.css";

type PreviewPayload = {
  markdown: string;
  title?: string | null;
};

function PreviewApp() {
  const { theme } = useUIStore();
  const [content, setContent] = useState<string>(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get("markdown") ?? "";
  });
  const [title, setTitle] = useState<string>(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get("title") ?? "Preview";
  });

  // Apply theme to the subwindow so it matches the main window.
  useEffect(() => {
    const apply = () => {
      const resolved = resolveTheme(theme);
      document.documentElement.setAttribute("data-theme", resolved);
      getCurrentWindow()
        .setTheme(resolved === "dark" ? "dark" : "light")
        .catch(() => {});
    };
    apply();

    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);

  // Listen for content updates pushed from the main window.
  useEffect(() => {
    let disposed = false;
    const off = listen<PreviewPayload>("preview://update", (event) => {
      if (disposed) return;
      setContent(event.payload.markdown ?? "");
      if (event.payload.title) setTitle(event.payload.title);
    });
    return () => {
      disposed = true;
      off.then((fn) => fn()).catch(() => {});
    };
  }, []);

  return (
    <div className={styles.root}>
      <div className={styles.header} data-tauri-drag-region>
        <span className={styles.title}>{title}</span>
      </div>
      <div className={styles.body}>
        {content ? (
          <ReactMarkdown remarkPlugins={[remarkGfm]} components={safeMarkdownComponents}>
            {content}
          </ReactMarkdown>
        ) : null}
      </div>
    </div>
  );
}

export default PreviewApp;
