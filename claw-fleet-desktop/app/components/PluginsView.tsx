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
  sourceKind: string; // "internal" | "external" | "catalog"
  pluginId: string;
  enabled: boolean;
  installCount: number | null;
  rootPath: string;
  manifestPath: string;
  contributes: PluginContributions;
  isDownloaded: boolean;
}

interface MarketplaceItem {
  name: string;
  source: string | null;
  repo: string | null;
  url: string | null;
  path: string | null;
  installLocation: string | null;
}

type SectionKey = "enabled" | "downloaded" | "catalog";

interface SectionExpanded {
  enabled: boolean;
  downloaded: boolean;
  catalog: boolean;
}

const DEFAULT_EXPANDED: SectionExpanded = {
  enabled: true,
  downloaded: true,
  catalog: false,
};

function formatInstalls(n: number | null): string | null {
  if (n == null) return null;
  if (n < 1000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

function bucketOf(plugin: PluginItem): SectionKey {
  if (plugin.enabled) return "enabled";
  if (plugin.isDownloaded) return "downloaded";
  return "catalog";
}

export function PluginsView() {
  const { t } = useTranslation();
  const [plugins, setPlugins] = useState<PluginItem[]>([]);
  const [marketplaces, setMarketplaces] = useState<MarketplaceItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<PluginItem | null>(null);
  const [expanded, setExpanded] = useState<SectionExpanded>(DEFAULT_EXPANDED);

  const load = useCallback(async () => {
    try {
      const [pluginData, mkData] = await Promise.all([
        invoke<PluginItem[]>("list_plugins"),
        invoke<MarketplaceItem[]>("list_marketplaces").catch(() => []),
      ]);
      setPlugins(pluginData);
      setMarketplaces(mkData);
    } catch {
      setPlugins([]);
      setMarketplaces([]);
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

  const sections = useMemo(() => {
    const buckets: Record<SectionKey, PluginItem[]> = {
      enabled: [],
      downloaded: [],
      catalog: [],
    };
    for (const p of filtered) buckets[bucketOf(p)].push(p);
    return buckets;
  }, [filtered]);

  const filterActive = query.trim().length > 0;

  useEffect(() => {
    if (!selected) return;
    if (!filtered.find((p) => p.pluginId === selected.pluginId)) {
      setSelected(null);
    }
  }, [filtered, selected]);

  const toggleSection = useCallback((key: SectionKey) => {
    setExpanded((s) => ({ ...s, [key]: !s[key] }));
  }, []);

  const renderSection = (
    key: SectionKey,
    titleKey: string,
    emptyKey: string,
  ) => {
    const items = sections[key];
    // When the user is filtering, force-expand any non-empty section so
    // hidden matches don't disappear behind a collapsed header.
    const open = filterActive ? items.length > 0 : expanded[key];
    return (
      <div key={key}>
        <button
          className={pluginStyles.section_header}
          onClick={() => toggleSection(key)}
          type="button"
        >
          <span
            className={`${pluginStyles.section_chevron} ${open ? pluginStyles.section_chevron_open : ""}`}
          >
            ▶
          </span>
          {t(titleKey)}
          <span className={pluginStyles.section_count}>{items.length}</span>
        </button>
        {open && items.length === 0 && (
          <div className={pluginStyles.section_empty}>{t(emptyKey)}</div>
        )}
        {open &&
          items.map((plugin) => {
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
                  {!plugin.isDownloaded && (
                    <span
                      className={`${pluginStyles.chip} ${pluginStyles.chip_catalog}`}
                    >
                      {t("plugins.catalog_only")}
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
      </div>
    );
  };

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
        <MarketplaceBar
          marketplaces={marketplaces}
          onChanged={() => setLoaded(false)}
        />
      </header>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {!loaded && <p className={styles.empty}>{t("plugins.loading")}</p>}
          {loaded && plugins.length === 0 && (
            <p className={styles.empty}>{t("plugins.no_plugins")}</p>
          )}
          {loaded && plugins.length > 0 && filtered.length === 0 && (
            <p className={styles.empty}>{t("plugins.no_match")}</p>
          )}
          {loaded && filtered.length > 0 && (
            <>
              {renderSection(
                "enabled",
                "plugins.section_enabled",
                "plugins.section_empty_enabled",
              )}
              {renderSection(
                "downloaded",
                "plugins.section_downloaded",
                "plugins.section_empty_downloaded",
              )}
              {renderSection(
                "catalog",
                "plugins.section_catalog",
                "plugins.section_empty_catalog",
              )}
            </>
          )}
        </aside>

        <main className={styles.detail_pane}>
          {selected ? (
            <PluginDetail
              plugin={selected}
              onChanged={() => {
                setLoaded(false);
              }}
            />
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

function PluginDetail({
  plugin,
  onChanged,
}: {
  plugin: PluginItem;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const isLocal = useConnectionStore((s) => s.connection?.type === "local");
  const [pending, setPending] = useState(false);
  const [toggleError, setToggleError] = useState<string | null>(null);

  // Clear stale error when navigating to a different plugin.
  useEffect(() => {
    setToggleError(null);
  }, [plugin.pluginId]);

  const reveal = useCallback(async () => {
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(plugin.manifestPath);
    } catch (e) {
      console.error("revealItemInDir failed:", e);
    }
  }, [plugin.manifestPath]);

  const runMutation = useCallback(
    async (cmd: string, args: Record<string, unknown>) => {
      setPending(true);
      setToggleError(null);
      try {
        await invoke<void>(cmd, args);
        onChanged();
      } catch (e) {
        setToggleError(typeof e === "string" ? e : String(e));
      } finally {
        setPending(false);
      }
    },
    [onChanged],
  );

  const onToggle = useCallback(
    () =>
      runMutation("set_plugin_enabled", {
        pluginId: plugin.pluginId,
        enabled: !plugin.enabled,
      }),
    [runMutation, plugin.pluginId, plugin.enabled],
  );

  const onInstall = useCallback(
    () => runMutation("install_plugin", { pluginId: plugin.pluginId }),
    [runMutation, plugin.pluginId],
  );

  const onUninstall = useCallback(
    () => runMutation("uninstall_plugin", { pluginId: plugin.pluginId }),
    [runMutation, plugin.pluginId],
  );

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

  const statusLabel = plugin.enabled
    ? t("plugins.enabled")
    : plugin.isDownloaded
      ? t("plugins.downloaded_disabled")
      : t("plugins.catalog_only");
  const statusChipClass = plugin.enabled
    ? pluginStyles.chip_enabled
    : plugin.isDownloaded
      ? pluginStyles.chip_disabled
      : pluginStyles.chip_catalog;

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
        <div className={styles.detail_actions}>
          {!plugin.isDownloaded && (
            <button
              className={pluginStyles.toggle_btn}
              onClick={onInstall}
              disabled={pending}
              type="button"
            >
              {pending ? t("plugins.toggle_pending") : t("plugins.install_btn")}
            </button>
          )}
          {plugin.isDownloaded && (
            <button
              className={pluginStyles.toggle_btn}
              onClick={onToggle}
              disabled={pending}
              type="button"
            >
              {pending
                ? t("plugins.toggle_pending")
                : plugin.enabled
                  ? t("plugins.toggle_disable")
                  : t("plugins.toggle_enable")}
            </button>
          )}
          {plugin.isDownloaded && (
            <button
              className={pluginStyles.toggle_btn}
              onClick={onUninstall}
              disabled={pending}
              type="button"
            >
              {pending ? t("plugins.toggle_pending") : t("plugins.uninstall_btn")}
            </button>
          )}
          {isLocal && plugin.isDownloaded && (
            <button
              className={styles.promote_btn}
              onClick={reveal}
              title={t("plugins.reveal_manifest")}
            >
              {t("plugins.reveal_manifest")}
            </button>
          )}
        </div>
      </div>
      {toggleError && (
        <div className={pluginStyles.toggle_error}>
          {t("plugins.toggle_error")}: {toggleError}
        </div>
      )}

      <div className={styles.detail_body}>
        {plugin.description && (
          <p className={pluginStyles.detail_description}>
            {plugin.description}
          </p>
        )}

        <dl className={pluginStyles.detail_meta_grid}>
          <dt className={pluginStyles.detail_meta_label}>
            {t("plugins.meta_status")}
          </dt>
          <dd className={pluginStyles.detail_meta_value}>
            <span className={`${pluginStyles.chip} ${statusChipClass}`}>
              {statusLabel}
            </span>
          </dd>

          <dt className={pluginStyles.detail_meta_label}>
            {t("plugins.meta_id")}
          </dt>
          <dd className={pluginStyles.detail_meta_value}>{plugin.pluginId}</dd>

          <dt className={pluginStyles.detail_meta_label}>
            {t("plugins.meta_marketplace")}
          </dt>
          <dd className={pluginStyles.detail_meta_value}>
            {plugin.marketplace}
          </dd>

          {plugin.author && (
            <>
              <dt className={pluginStyles.detail_meta_label}>
                {t("plugins.meta_author")}
              </dt>
              <dd className={pluginStyles.detail_meta_value}>
                {plugin.author}
              </dd>
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

        {plugin.isDownloaded && contribChips.length > 0 && (
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

        {plugin.isDownloaded && plugin.rootPath && (
          <>
            <div className={pluginStyles.detail_section_title}>
              {t("plugins.path_label")}
            </div>
            <div
              className={pluginStyles.detail_meta_value}
              style={{ fontSize: 11, fontFamily: "var(--font-mono, monospace)" }}
            >
              {plugin.rootPath}
            </div>
          </>
        )}
      </div>
    </>
  );
}

function MarketplaceBar({
  marketplaces,
  onChanged,
}: {
  marketplaces: MarketplaceItem[];
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const [source, setSource] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = useCallback(async () => {
    const trimmed = source.trim();
    if (!trimmed) return;
    setPending(true);
    setError(null);
    try {
      await invoke<void>("add_marketplace", { source: trimmed });
      setSource("");
      setAdding(false);
      onChanged();
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setPending(false);
    }
  }, [source, onChanged]);

  const remove = useCallback(
    async (name: string) => {
      const confirmed = window.confirm(
        t("plugins.remove_marketplace_confirm", { name }),
      );
      if (!confirmed) return;
      setPending(true);
      setError(null);
      try {
        await invoke<void>("remove_marketplace", { name });
        onChanged();
      } catch (e) {
        setError(typeof e === "string" ? e : String(e));
      } finally {
        setPending(false);
      }
    },
    [onChanged, t],
  );

  return (
    <div className={pluginStyles.marketplace_bar}>
      <span className={pluginStyles.marketplace_label}>
        {t("plugins.marketplaces_label")}
      </span>
      {marketplaces.length === 0 && (
        <span style={{ fontStyle: "italic" }}>
          {t("plugins.no_marketplaces")}
        </span>
      )}
      {marketplaces.map((mk) => (
        <span key={mk.name} className={pluginStyles.marketplace_chip}>
          {mk.name}
          <button
            className={pluginStyles.marketplace_chip_remove}
            type="button"
            onClick={() => remove(mk.name)}
            disabled={pending}
            title="×"
          >
            ×
          </button>
        </span>
      ))}
      {!adding && (
        <button
          className={pluginStyles.marketplace_add_btn}
          type="button"
          onClick={() => setAdding(true)}
          disabled={pending}
        >
          {t("plugins.add_marketplace_btn")}
        </button>
      )}
      {adding && (
        <div className={pluginStyles.marketplace_add_form}>
          <input
            className={pluginStyles.marketplace_input}
            type="text"
            autoFocus
            value={source}
            onChange={(e) => setSource(e.target.value)}
            placeholder={t("plugins.add_marketplace_placeholder")}
            disabled={pending}
            onKeyDown={(e) => {
              if (e.key === "Enter") submit();
              if (e.key === "Escape") {
                setAdding(false);
                setSource("");
                setError(null);
              }
            }}
          />
          <button
            className={pluginStyles.toggle_btn}
            type="button"
            onClick={submit}
            disabled={pending || source.trim().length === 0}
          >
            {pending
              ? t("plugins.toggle_pending")
              : t("plugins.add_marketplace_submit")}
          </button>
          <button
            className={pluginStyles.toggle_btn}
            type="button"
            onClick={() => {
              setAdding(false);
              setSource("");
              setError(null);
            }}
            disabled={pending}
          >
            {t("plugins.add_marketplace_cancel")}
          </button>
        </div>
      )}
      {error && (
        <div className={pluginStyles.toggle_error} style={{ width: "100%" }}>
          {t("plugins.toggle_error")}: {error}
        </div>
      )}
    </div>
  );
}
