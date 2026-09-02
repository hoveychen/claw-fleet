// 渲染兜底。一个子组件在 render 里抛异常时,React 会把**整棵树**卸掉 —— 手机上
// 的表现是页面一片纯白、控制台什么都没有(手机浏览器通常也打不开控制台)。
//
// 这已经咬到过两次,两次都是「主机回来的数据形状不对」而不是我们自己的逻辑错:
//   1. 一张决策卡少了 `riskTags` 数组 → 渲染卡片时抛 → 整个 app 变白;
//   2. 主机把 `sources_config` 回成对象而不是数组 → `.filter` 不是函数 → 同上。
// 两次的共性是致命的:**坏的是一个局部,代价是全部**。所以这里的目标不是「把错误
// 打印出来」,而是把爆炸半径收进出问题的那一块 —— 一张坏卡只坏那张卡,其余卡照答;
// 一个 tab 崩了,底部导航还在,切走再切回来自动重试。
//
// 桌面端有一个同源物(claw-fleet-desktop/app/components/ErrorBoundary.tsx),但它
// 是给开发者看的整屏红字堆栈、且不可恢复;手机上需要的是面向用户的一句话 + 可恢复
// + 可展开的技术细节。两边刻意不共用:它们是两个独立的前端包,恢复语义也不同。
//
// 样式全部内联 —— CSS / 字体 / 设计系统本身可能正是崩掉的那部分。

import { Component, type ErrorInfo, type ReactNode } from "react";
import { t } from "./i18n";

/** 崩溃详情的可读文本。抽成纯函数是为了能被单测钉住 —— 这段文本是老板/我事后
 *  唯一能拿到的线索,格式退化(比如丢了 componentStack)不该悄悄发生。 */
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

interface Props {
  /** 哪一块崩了。进标题也进 console —— 「决策卡 g1」比「出错了」有用得多。 */
  label: string;
  /** 它变化时自动清掉错误状态并重试渲染。
   *
   *  这是「可恢复」的全部机制:决策卡传卡 id(翻到下一张自动好),tab 视图传 tab
   *  名(切走再回来自动重试)。不做成让调用方写 `key=` 是刻意的 —— 那样一旦忘写,
   *  错误状态就会粘在那儿,而「忘了」不该由用户承担。 */
  resetKey?: string;
  /** `screen` = 占满可用高度(整页/整 tab);`inline` = 一小块(单张卡片位置)。 */
  variant?: "screen" | "inline";
  children: ReactNode;
}

interface State {
  error: Error | null;
  componentStack: string | null;
  /** 上一次渲染时的 resetKey。用它比对来实现自动恢复。 */
  seenKey?: string;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    if (state.seenKey !== props.resetKey) {
      // resetKey 变了:换了另一张卡 / 另一个 tab,前一次的失败与这次无关。
      return { seenKey: props.resetKey, error: null, componentStack: null };
    }
    return null;
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    this.setState({ componentStack: info.componentStack ?? null });
    // 有 DevTools 的人仍然拿得到原始对象。
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
        <button
          onClick={this.retry}
          style={{
            padding: "6px 12px",
            borderRadius: "8px",
            border: "1px solid rgba(176, 0, 32, 0.4)",
            background: "transparent",
            color: "inherit",
            fontSize: "13px",
          }}
        >
          {t("重试")}
        </button>
        {/* 细节默认折叠:手机屏幕不该被堆栈占满,但它必须能被复制出来给我看。 */}
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
