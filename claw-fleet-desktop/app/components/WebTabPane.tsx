import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, Globe, ShieldAlert } from "lucide-react";

import type { AuxDoc } from "../detailAux";
import { AuxDocBar, AuxPane } from "./AuxDocBar";
import { buildWebMenu, type AuxCardTail } from "./auxDocMenu";
import styles from "./TabPanes.module.css";

/** What the host learned about framing this URL — see `gui/url_embed.rs`. */
interface UrlEmbedProbe {
  embeddable: boolean;
  reason: string | null;
  status: number | null;
}

/**
 * One web page as an auxiliary-rail reader, so a doc or dashboard the agent
 * linked can sit beside the prose instead of pulling focus into a browser.
 *
 * A plain `<iframe>` is not enough on its own: sites that send `X-Frame-Options`
 * or a CSP `frame-ancestors` refuse to render in one, and they do it silently —
 * a blocked frame still fires `load`, and its document is cross-origin, so no
 * amount of frontend probing can tell "refused" from "blank page". The host can
 * read those headers, so we ask it first (`probe_url_embeddable`) and show a
 * plain refusal card, with the system browser one click away, instead of an
 * unexplained white rectangle.
 *
 * The probe fails *open*: an offline / DNS / TLS failure renders the frame
 * anyway and lets the page state its own error. What the probe learned is also
 * the card's density line — the status code and whether framing was allowed are
 * exactly what you want to know when a frame looks wrong.
 */
export function WebTabPane({ doc, tail }: { doc: AuxDoc; tail: AuxCardTail }) {
  const { t } = useTranslation();
  const [probe, setProbe] = useState<UrlEmbedProbe | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Bumped by 重新加载: re-probes and, because it keys the iframe, forces a fresh
  // load even when the src string is unchanged.
  const [nonce, setNonce] = useState(0);
  const url = doc.ref;

  useEffect(() => {
    let stale = false;
    setProbe(null);
    invoke<UrlEmbedProbe>("probe_url_embeddable", { url })
      .then((r) => {
        // A null (a host that doesn't know this command) is not a refusal.
        if (!stale) setProbe(r ?? { embeddable: true, reason: null, status: null });
      })
      .catch(() => {
        // The command itself failed (never mind the site) — same fail-open rule.
        if (!stale) setProbe({ embeddable: true, reason: null, status: null });
      });
    return () => {
      stale = true;
    };
  }, [url, nonce]);

  const open = () => {
    openUrl(url).catch((e) => console.error("openUrl failed:", url, e));
  };

  const build = buildWebMenu({
    doc,
    tail,
    t,
    fail: setError,
    onReload: () => setNonce((n) => n + 1),
  });

  // The host is the name; the path is the fact that tells two pages of the same
  // site apart, which is what a rail full of chips actually needs.
  let host = url;
  let path = "";
  try {
    const u = new URL(url);
    host = u.host;
    path = `${u.pathname}${u.search}`;
  } catch {
    // A hand-typed or malformed url still deserves a readable header.
  }

  return (
    <AuxPane menuItems={build.menu} className={styles.pane}>
      <AuxDocBar
        kind="web"
        icon={<Globe size={14} strokeWidth={1.8} />}
        title={host}
        titleHint={url}
        facts={[
          { text: path, strong: true },
          { text: probe?.status != null ? `HTTP ${probe.status}` : "" },
          {
            text:
              probe == null
                ? t("tabs.web_probing_short", "检查中…")
                : probe.embeddable
                  ? t("tabs.web_embeddable", "可嵌入")
                  : t("tabs.web_not_embeddable", "禁止嵌入"),
          },
        ]}
        actions={build.actions}
        menuItems={build.menu}
        onCollapse={tail.onToggle}
        onClose={tail.onClose}
      />
      {error && <p className={styles.error_line}>{error}</p>}
      <div className={styles.body}>
        {probe === null ? (
          <p className={styles.status}>{t("tabs.web_probing", "正在检查该站是否允许嵌入…")}</p>
        ) : probe.embeddable ? (
          // allow-same-origin is safe here in a way it would not be for local
          // HTML: the frame's document IS the remote site, a different origin
          // from the app's, so it keeps its own cookies/storage (which most real
          // sites need to render at all) without any reach into ours. No
          // allow-top-navigation, so the page cannot navigate the app away.
          <iframe
            key={`${url}#${nonce}`}
            className={styles.frame}
            sandbox="allow-scripts allow-same-origin allow-forms"
            src={url}
            title={url}
          />
        ) : (
          <div className={styles.blocked}>
            <ShieldAlert size={26} strokeWidth={1.3} />
            <span className={styles.blocked_title}>
              {t("tabs.web_blocked", "该站禁止被嵌入")}
            </span>
            {probe.reason && <code className={styles.blocked_reason}>{probe.reason}</code>}
            <button type="button" className={styles.blocked_btn} onClick={open}>
              <ExternalLink size={13} strokeWidth={1.7} />
              {t("tabs.web_open_browser", "在浏览器中打开")}
            </button>
          </div>
        )}
      </div>
    </AuxPane>
  );
}
