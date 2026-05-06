import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore } from "../store";
import styles from "./MemoryView.module.css";
import pluginStyles from "./PluginsView.module.css";

interface PluginContributions {
  commands: number;
  agents: number;
  skills: number;
  hooks: boolean;
  mcp: boolean;
}

interface PluginItem {
  name: string;
  description: string;
  author: string | null;
  version: string | null;
  homepage: string | null;
  marketplace: string;
  sourceKind: string; // "internal" | "external"
  pluginId: string;
  enabled: boolean;
  installCount: number | null;
  rootPath: string;
  manifestPath: string;
  contributes: PluginContributions;
}

function formatInstalls(n: number | null): string | null {
  if (n == null) return null;
  if (n < 1000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

export function PluginsView() {
  const { t } = useTranslation();
  const [plugins, setPlugins] = useState<PluginItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<PluginItem | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await invoke<PluginItem[]>("list_plugins");
      setPlugins(data);
    } catch {
      setPlugins([]);
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    if (!loaded) load();
  }, [loaded, load]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return plugins;
    return plugins.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.description.toLowerCase().includes(q) ||
        p.marketplace.toLowerCase().includes(q),
    );
  }, [plugins, query]);

  useEffect(() => {
    if (!selected) return;
    if (!filtered.find((p) => p.pluginId === selected.pluginId)) {
      setSelected(null);
    }
  }, [filtered, selected]);

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("plugins.panel_title")}</h1>
          {loaded && plugins.length > 0 && (
            <span className={styles.count}>{plugins.length}</span>
          )}
        </div>
        <input
          className={styles.search}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("plugins.filter_placeholder")}
        />
      </header>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("plugins.loading")}</p>}
          {loaded && filtered.length === 0 && (
            <p className={styles.empty}>{t("plugins.no_plugins")}</p>
          )}
          {filtered.map((plugin) => {
            const active = selected?.pluginId === plugin.pluginId;
            const installs = formatInstalls(plugin.installCount);
            return (
              <button
                key={plugin.pluginId}
                className={`${styles.file_item} ${active ? styles.file_item_active : ""}`}
                onClick={() => setSelected(plugin)}
              >
                <div className={pluginStyles.row_main}>
                  <span className={pluginStyles.row_name}>{plugin.name}</span>
                  {plugin.enabled && (
                    <span
                      className={`${pluginStyles.chip} ${pluginStyles.chip_enabled}`}
                    >
                      {t("plugins.enabled")}
                    </span>
                  )}
                </div>
                <div className={pluginStyles.row_meta}>
                  <span className={pluginStyles.marketplace_tag}>
                    {plugin.marketplace}
                    {plugin.sourceKind === "external" ? " · ext" : ""}
                  </span>
                  {installs && (
                    <span className={pluginStyles.row_install}>
                      ↓ {installs}
                    </span>
                  )}
                </div>
              </button>
            );
          })}
        </aside>

        <main className={styles.detail_pane}>
          {selected ? (
            <PluginDetail plugin={selected} />
          ) : (
            <div className={styles.placeholder}>
              {loaded && plugins.length > 0
                ? t("plugins.pick_plugin")
                : t("plugins.no_plugins")}
            </div>
          )}
        </main>
      </div>
    </div>
  );
}

function PluginDetail({ plugin }: { plugin: PluginItem }) {
  const { t } = useTranslation();
  const isLocal = useConnectionStore(
    (s) => s.connection?.type === "local",
  );

  const reveal = useCallback(async () => {
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(plugin.manifestPath);
    } catch (e) {
      console.error("revealItemInDir failed:", e);
    }
  }, [plugin.manifestPath]);

  const openHomepage = useCallback(async () => {
    if (!plugin.homepage) return;
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(plugin.homepage);
    } catch (e) {
      console.error("openUrl failed:", e);
    }
  }, [plugin.homepage]);

  const contribChips: { label: string; key: string }[] = [];
  if (plugin.contributes.commands > 0) {
    contribChips.push({
      key: "commands",
      label: t("plugins.contributes_commands", {
        count: plugin.contributes.commands,
      }),
    });
  }
  if (plugin.contributes.agents > 0) {
    contribChips.push({
      key: "agents",
      label: t("plugins.contributes_agents", {
        count: plugin.contributes.agents,
      }),
    });
  }
  if (plugin.contributes.skills > 0) {
    contribChips.push({
      key: "skills",
      label: t("plugins.contributes_skills", {
        count: plugin.contributes.skills,
      }),
    });
  }
  if (plugin.contributes.hooks) {
    contribChips.push({ key: "hooks", label: t("plugins.contributes_hooks") });
  }
  if (plugin.contributes.mcp) {
    contribChips.push({ key: "mcp", label: t("plugins.contributes_mcp") });
  }

  const installs = formatInstalls(plugin.installCount);

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>{plugin.name}</span>
          <span className={styles.detail_sep}>·</span>
          {plugin.marketplace}
          {plugin.sourceKind === "external" && (
            <span
              className={`${pluginStyles.chip} ${pluginStyles.chip_external}`}
              style={{ marginLeft: 8 }}
            >
              {t("plugins.external")}
            </span>
          )}
        </div>
        {isLocal && (
          <div className={styles.detail_actions}>
            <button
              className={styles.promote_btn}
              onClick={reveal}
              title={t("plugins.reveal_manifest")}
            >
              {t("plugins.reveal_manifest")}
            </button>
          </div>
        )}
      </div>

      <div className={styles.detail_body}>
        {plugin.description && (
          <p className={pluginStyles.detail_description}>{plugin.description}</p>
        )}

        <dl className={pluginStyles.detail_meta_grid}>
          <dt className={pluginStyles.detail_meta_label}>
            {t("plugins.meta_status")}
          </dt>
          <dd className={pluginStyles.detail_meta_value}>
            <span
              className={`${pluginStyles.chip} ${plugin.enabled ? pluginStyles.chip_enabled : pluginStyles.chip_disabled}`}
            >
              {plugin.enabled ? t("plugins.enabled") : t("plugins.disabled")}
            </span>
          </dd>

          <dt className={pluginStyles.detail_meta_label}>
            {t("plugins.meta_id")}
          </dt>
          <dd className={pluginStyles.detail_meta_value}>{plugin.pluginId}</dd>

          {plugin.author && (
            <>
              <dt className={pluginStyles.detail_meta_label}>
                {t("plugins.meta_author")}
              </dt>
              <dd className={pluginStyles.detail_meta_value}>{plugin.author}</dd>
            </>
          )}

          {plugin.version && (
            <>
              <dt className={pluginStyles.detail_meta_label}>
                {t("plugins.meta_version")}
              </dt>
              <dd className={pluginStyles.detail_meta_value}>
                {plugin.version}
              </dd>
            </>
          )}

          {installs && (
            <>
              <dt className={pluginStyles.detail_meta_label}>
                {t("plugins.meta_installs")}
              </dt>
              <dd className={pluginStyles.detail_meta_value}>{installs}</dd>
            </>
          )}

          {plugin.homepage && (
            <>
              <dt className={pluginStyles.detail_meta_label}>
                {t("plugins.meta_homepage")}
              </dt>
              <dd className={pluginStyles.detail_meta_value}>
                <a
                  className={pluginStyles.homepage_link}
                  href={plugin.homepage}
                  onClick={(e) => {
                    e.preventDefault();
                    openHomepage();
                  }}
                >
                  {plugin.homepage}
                </a>
              </dd>
            </>
          )}
        </dl>

        {contribChips.length > 0 && (
          <>
            <div className={pluginStyles.detail_section_title}>
              {t("plugins.contributes_title")}
            </div>
            <div className={pluginStyles.contributes_row}>
              {contribChips.map((chip) => (
                <span key={chip.key} className={pluginStyles.chip}>
                  {chip.label}
                </span>
              ))}
            </div>
          </>
        )}

        <div className={pluginStyles.detail_section_title}>
          {t("plugins.path_label")}
        </div>
        <div
          className={pluginStyles.detail_meta_value}
          style={{ fontSize: 11, fontFamily: "var(--font-mono, monospace)" }}
        >
          {plugin.rootPath}
        </div>
      </div>
    </>
  );
}
