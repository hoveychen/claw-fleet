import { useEffect } from "react";
import "./App.css";
import { SessionDetail } from "./components/SessionDetail";
import { SessionList } from "./components/SessionList";
import { resolveTheme, useUIStore } from "./store";

function App() {
  const { theme, viewMode } = useUIStore();

  useEffect(() => {
    const apply = () => {
      document.documentElement.setAttribute("data-theme", resolveTheme(theme));
    };
    apply();

    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);

  return (
    <div className="app">
      <SessionList />
      {viewMode === "list" && <SessionDetail />}
    </div>
  );
}

export default App;
