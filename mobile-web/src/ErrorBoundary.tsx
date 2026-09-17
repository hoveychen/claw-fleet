// Fallback for render errors. When a child component throws during render, React
// unmounts the **entire tree** — on mobile, the result is a blank white page with
// no console (and the mobile browser usually can't open the console anyway).
//
// This has bitten us twice, both times due to malformed data from the server,
// not a logic error:
//   1. A decision card missing a `riskTags` array → throws during render → entire
//      app turns white;
//   2. Server returns `sources_config` as an object instead of an array →
//      `.filter` is not a function → same result.
// The pattern in both cases is fatal: **damage is localized, but cost is global**.
// So the goal here is not just to "log the error", but to contain the blast radius
// to the broken piece — a bad card only breaks that card, others still work; a
// broken tab still leaves the bottom nav working, and switching away and back
// retries automatically.
//
// The desktop has an equivalent component (claw-fleet-desktop/app/components/
// ErrorBoundary.tsx), but it's a full-screen red stack dump for developers and
// non-recoverable. Mobile needs user-facing messaging + recovery + expandable
// technical details. They intentionally don't share code: they're separate
// frontend packages with different recovery semantics.
//
// All styles are inlined — CSS/fonts/design system itself might be what crashed.

import { Component, type ErrorInfo, type ReactNode } from "react";
import { t } from "./i18n";

/** Readable text describing the crash. Extracted to a pure function so it can
 *  be pinned by unit tests — this text is the only clue available afterward to
 *  diagnose the issue. Format degradation (e.g., losing componentStack) should
 *  never happen silently. */
export function formatBoundaryDetail(
  error: Error,
  componentStack: string | null,
): string {
  const parts = [
    `${error.name}: ${error.message}`,
    error.stack ?? "(no stack)",
  ];
  if (componentStack) parts.push(`Component stack:${componentStack}`);
  return parts.join("\n\n");
}

const BUTTON_STYLE = {
  padding: "6px 12px",
  borderRadius: "8px",
  border: "1px solid rgba(176, 0, 32, 0.4)",
  background: "transparent",
  color: "inherit",
  fontSize: "13px",
} as const;

interface Props {
  /** Which piece crashed. Goes into the title and console — "decision card g1"
   *  is way more useful than "error occurred". */
  label: string;
  /** When this changes, automatically clear error state and retry rendering.
   *
   *  This is the entire recovery mechanism: decision cards pass card id
   *  (advancing to the next card auto-recovers), tab views pass tab name
   *  (switching away and back auto-retries). We intentionally don't make
   *  callers write `key=` themselves — if forgotten, the error state would
   *  persist, and we shouldn't burden the caller with that forgetfulness. */
  resetKey?: string;
  /** `screen` = fill available height (full page/full tab); `inline` = small
   *  piece (single card location). */
  variant?: "screen" | "inline";
  /** An escape route — "leave here" — adding an extra button to the fallback.
   *
   *  Required when wrapping a modal layer: the modal's `HistoryLayer` (which
   *  handles system back) and its content are the same JSX block, replaced
   *  together by the fallback, so **the back button can't dismiss the modal**.
   *  We've verified this: closing the new session form step, the modal state
   *  doesn't change, the fallback stays. On iOS PWA there's no system back at all,
   *  so if only "retry" is available, one crash traps the user. */
  onDismiss?: { label: string; run: () => void };
  children: ReactNode;
}

interface State {
  error: Error | null;
  componentStack: string | null;
  /** The resetKey from the previous render. Used to detect changes and
   *  auto-recover. */
  seenKey?: string;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    if (state.seenKey !== props.resetKey) {
      // resetKey changed: switched to another card/tab, prior failure unrelated.
      return { seenKey: props.resetKey, error: null, componentStack: null };
    }
    return null;
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    this.setState({ componentStack: info.componentStack ?? null });
    // People with DevTools can still access the raw objects.
    console.error(`[${this.props.label}] render error:`, error, info);
  }

  private retry = () => this.setState({ error: null, componentStack: null });

  render(): ReactNode {
    const { error, componentStack } = this.state;
    if (!error) return this.props.children;

    const inline = this.props.variant === "inline";
    return (
      <div
        style={{
          margin: inline ? "8px 0" : 0,
          padding: inline ? "12px" : "16px",
          minHeight: inline ? undefined : "40vh",
          boxSizing: "border-box",
          borderRadius: inline ? "12px" : 0,
          border: "1px solid rgba(176, 0, 32, 0.35)",
          background: "rgba(176, 0, 32, 0.06)",
          color: "#b00020",
          fontSize: "13px",
          lineHeight: 1.6,
          overflow: "auto",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 6 }}>
          {t("这一块没能显示出来")}
        </div>
        <div style={{ opacity: 0.85, marginBottom: 10 }}>
          {t("其余部分仍然可用。{0}", this.props.label)}
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button onClick={this.retry} style={BUTTON_STYLE}>
            {t("重试")}
          </button>
          {this.props.onDismiss && (
            <button onClick={this.props.onDismiss.run} style={BUTTON_STYLE}>
              {this.props.onDismiss.label}
            </button>
          )}
        </div>
        {/* Details collapsed by default: mobile screen shouldn't be consumed by
            a stack trace, but it must be copyable for diagnosis. */}
        <details style={{ marginTop: 10 }}>
          <summary style={{ cursor: "pointer", fontSize: "12px", opacity: 0.8 }}>
            {t("技术细节")}
          </summary>
          <pre
            style={{
              margin: "8px 0 0",
              fontSize: "11px",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
              fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
            }}
          >
            {formatBoundaryDetail(error, componentStack)}
          </pre>
        </details>
      </div>
    );
  }
}
