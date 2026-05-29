import { Component, type ErrorInfo, type ReactNode } from "react";

/**
 * A last-resort render guard. When a child throws during render, React unmounts
 * the whole tree and leaves a blank white window — the error only surfaces in
 * the DevTools console, which is unavailable in release builds. This boundary
 * catches the error and paints it into the window instead, so the failure is
 * visible (and screenshot-able) on any platform.
 *
 * Styling is fully inline on purpose: the boundary must render even when CSS,
 * fonts, or the design system are part of what broke.
 */

interface Props {
  /** Shown in the header so we know which surface failed (e.g. "Settings"). */
  label?: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
  componentStack: string | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    this.setState({ componentStack: info.componentStack ?? null });
    // Still log to console for anyone who does have DevTools open.
    console.error(`[${this.props.label ?? "app"}] render error:`, error, info);
  }

  render(): ReactNode {
    const { error, componentStack } = this.state;
    if (!error) return this.props.children;

    const detail = [
      `${error.name}: ${error.message}`,
      "",
      error.stack ?? "(no stack)",
      componentStack ? `\nComponent stack:${componentStack}` : "",
    ].join("\n");

    return (
      <div
        style={{
          padding: "16px",
          margin: 0,
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
          fontSize: "12px",
          lineHeight: 1.5,
          color: "#b00020",
          background: "#fff",
          height: "100vh",
          boxSizing: "border-box",
          overflow: "auto",
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
        }}
      >
        <div style={{ fontWeight: 700, marginBottom: 8, fontSize: "13px" }}>
          {this.props.label ?? "App"} failed to render
        </div>
        {detail}
      </div>
    );
  }
}
