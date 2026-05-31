import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuditStore, useDetailStore, useSessionsStore, useUIStore } from "../store";
import type {
  AuditEvent,
  AuditRiskLevel,
  AuditRuleInfo,
  AuditSummary,
  GuardAllowRule,
  SuggestedRule,
} from "../types";
import { ShieldCheck } from "lucide-react";
import { EmptyState } from "./EmptyState";
import styles from "./AuditView.module.css";

// ── Risk level helpers ──────────────────────────────────────────────────────

const RISK_LABELS: Record<AuditRiskLevel, string> = {
  critical: "CRIT",
  high: "HIGH",
  medium: "MED",
};

const RISK_CLASS: Record<AuditRiskLevel, string> = {
  critical: "risk_critical",
  high: "risk_high",
  medium: "risk_medium",
};

const CATEGORY_ORDER = [
  "privilege_escalation",
  "data_exfiltration",
  "network",
  "git",
  "filesystem",
  "container",
  "package",
  "process",
  "cloud",
  "scheduled_task",
  "python",
  "custom",
];

// ── Tab type ────────────────────────────────────────────────────────────────

type AuditTab = "events" | "rules";

// ── Component ───────────────────────────────────────────────────────────────

export function AuditView() {
  const { t, i18n } = useTranslation();
  const [tab, setTab] = useState<AuditTab>("events");

  return (
    <div className={styles.page}>
      <header className={styles.header} data-tauri-drag-region>
        <div className={styles.title_row}>
          <h1 className={styles.title}>{t("audit.panel_title")}</h1>
        </div>
        <div className={styles.tab_bar}>
          <button
            className={`${styles.tab_btn} ${tab === "events" ? styles.tab_active : ""}`}
            onClick={() => setTab("events")}
          >
            {t("audit.tab_events")}
          </button>
          <button
            className={`${styles.tab_btn} ${tab === "rules" ? styles.tab_active : ""}`}
            onClick={() => setTab("rules")}
          >
            {t("audit.tab_rules")}
          </button>
        </div>
      </header>
      {tab === "events" ? <EventsTab /> : <RulesTab lang={i18n.language} />}
    </div>
  );
}

// ── Events Tab ──────────────────────────────────────────────────────────────

function EventsTab() {
  const { t } = useTranslation();
  const [summary, setSummary] = useState<AuditSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<AuditRiskLevel | "all">("all");
  const [selectedEvent, setSelectedEvent] = useState<AuditEvent | null>(null);
  const { sessions } = useSessionsStore();
  const { open } = useDetailStore();
  const { setViewMode } = useUIStore();
  const { isRead, markAsRead, getEventKey, setCriticalEvents, markAllCriticalAsRead, unreadCriticalCount } =
    useAuditStore();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<AuditSummary>("get_audit_events");
      setSummary(data);
      setCriticalEvents(data.events.filter((e) => e.riskLevel === "critical"));
    } catch {
      setSummary({ events: [], totalSessionsScanned: 0 });
    } finally {
      setLoading(false);
    }
  }, [setCriticalEvents]);

  useEffect(() => { load(); }, [load]);

  const filtered = summary?.events.filter(
    (e) => filter === "all" || e.riskLevel === filter,
  ) ?? [];

  const grouped = useMemo(() => {
    const map = new Map<string, AuditEvent[]>();
    for (const e of filtered) {
      const key = e.sessionId;
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(e);
    }
    return map;
  }, [filtered]);

  const counts: Record<AuditRiskLevel, number> = { critical: 0, high: 0, medium: 0 };
  for (const e of summary?.events ?? []) {
    counts[e.riskLevel]++;
  }

  const navigateToSession = (jsonlPath: string) => {
    const session = sessions.find((s) => s.jsonlPath === jsonlPath);
    if (session) {
      setViewMode("list");
      open(session);
    }
  };

  return (
    <>
      {/* Filter bar */}
      <div className={styles.filter_bar}>
        <FilterChip
          label={t("audit.all")}
          count={summary?.events.length ?? 0}
          active={filter === "all"}
          onClick={() => setFilter("all")}
        />
        {(["critical", "high", "medium"] as const).map((level) => (
          <FilterChip
            key={level}
            label={RISK_LABELS[level]}
            count={counts[level]}
            active={filter === level}
            tone={RISK_CLASS[level]}
            onClick={() => setFilter(level)}
          />
        ))}
        <div className={styles.filter_spacer} />
        {unreadCriticalCount > 0 && (
          <button
            className={styles.action_btn}
            onClick={markAllCriticalAsRead}
            title={t("audit.mark_all_read")}
          >
            ✓ {t("audit.mark_all_read")}
          </button>
        )}
        <button className={styles.icon_btn} onClick={load} title={t("audit.refresh")}>
          ↻
        </button>
        {summary && (
          <span className={styles.scan_info}>
            {t("audit.scanned", { count: summary.totalSessionsScanned })}
          </span>
        )}
      </div>

      {/* Body: list + detail */}
      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {loading && <p className={styles.empty}>{t("audit.scanning")}</p>}
          {!loading && filtered.length === 0 && (
            <EmptyState
              icon={<ShieldCheck size={28} strokeWidth={1.5} />}
              title={t("empty_state.audit_title")}
              subtitle={t("empty_state.audit_subtitle")}
            />
          )}
          {!loading && Array.from(grouped.entries()).map(([sessionId, events]) => (
            <div key={sessionId} className={styles.workspace_group}>
              <button
                className={styles.workspace_header}
                onClick={() => navigateToSession(events[0].jsonlPath)}
                title={t("audit.go_to_session")}
              >
                <span className={styles.workspace_name}>{events[0].workspaceName}</span>
                <span className={styles.workspace_source}>{events[0].agentSource}</span>
                <span className={styles.workspace_count}>{events.length}</span>
              </button>
              <div className={styles.card_list}>
                {events.map((event, i) => {
                  const read = event.riskLevel === "critical" && isRead(event);
                  const active = selectedEvent === event;
                  return (
                    <button
                      key={`${sessionId}-${i}`}
                      className={`${styles.card} ${active ? styles.card_active : ""} ${read ? styles.card_read : ""}`}
                      onClick={() => setSelectedEvent(event)}
                    >
                      <span className={`${styles.risk_badge} ${styles[RISK_CLASS[event.riskLevel]]}`}>
                        {RISK_LABELS[event.riskLevel]}
                      </span>
                      <div className={styles.card_body}>
                        <div className={styles.card_title}>{event.commandSummary}</div>
                        <div className={styles.card_meta}>
                          {event.timestamp && <span>{formatTime(event.timestamp)}</span>}
                          {event.riskTags.length > 0 && (
                            <>
                              <span className={styles.meta_dot}>·</span>
                              <span className={styles.card_tags_inline}>{event.riskTags.join(" · ")}</span>
                            </>
                          )}
                        </div>
                      </div>
                      {event.riskLevel === "critical" && !read && (
                        <span
                          role="button"
                          tabIndex={0}
                          className={styles.read_btn}
                          title={t("audit.mark_read")}
                          onClick={(e) => { e.stopPropagation(); markAsRead(getEventKey(event)); }}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                              e.preventDefault();
                              e.stopPropagation();
                              markAsRead(getEventKey(event));
                            }
                          }}
                        >
                          ✓
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </aside>

        {selectedEvent && (
          <main className={styles.detail_pane}>
            <div className={styles.detail_header}>
              <div className={styles.detail_title}>
                <span className={`${styles.risk_badge} ${styles[RISK_CLASS[selectedEvent.riskLevel]]}`}>
                  {RISK_LABELS[selectedEvent.riskLevel]}
                </span>
                <span className={styles.detail_workspace}>{selectedEvent.workspaceName}</span>
                <span className={styles.detail_sep}>/</span>
                <span className={styles.detail_name}>{selectedEvent.toolName}</span>
              </div>
              <button className={styles.detail_close} onClick={() => setSelectedEvent(null)}>✕</button>
            </div>
            <div className={styles.detail_body}>
              <DetailRow label={t("audit.workspace")} value={selectedEvent.workspaceName} />
              <DetailRow label={t("audit.source")} value={selectedEvent.agentSource} />
              {selectedEvent.timestamp && <DetailRow label={t("audit.time")} value={new Date(selectedEvent.timestamp).toLocaleString()} />}
              {selectedEvent.riskTags.length > 0 && (
                <div className={styles.detail_row}>
                  <span className={styles.detail_label}>{t("audit.tags")}</span>
                  <span className={styles.detail_value}>
                    {selectedEvent.riskTags.map((tag) => <span key={tag} className={styles.tag}>{tag}</span>)}
                  </span>
                </div>
              )}
              <div className={styles.command_section}>
                <div className={styles.command_label}>{t("audit.full_command")}</div>
                <pre className={styles.command_pre}>{selectedEvent.fullCommand}</pre>
              </div>
              <div className={styles.detail_actions}>
                {selectedEvent.riskLevel === "critical" && !isRead(selectedEvent) && (
                  <button className={styles.read_detail_btn} onClick={() => markAsRead(getEventKey(selectedEvent))}>
                    ✓ {t("audit.mark_read")}
                  </button>
                )}
                <button className={styles.goto_btn} onClick={() => navigateToSession(selectedEvent.jsonlPath)}>
                  {t("audit.go_to_session")}
                </button>
              </div>
            </div>
          </main>
        )}
      </div>
    </>
  );
}

// ── Rules Tab ───────────────────────────────────────────────────────────────

function RulesTab({ lang }: { lang: string }) {
  const { t } = useTranslation();
  const [rules, setRules] = useState<AuditRuleInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedRule, setSelectedRule] = useState<AuditRuleInfo | null>(null);
  const [showSuggest, setShowSuggest] = useState(false);
  const [query, setQuery] = useState("");

  const loadRules = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<AuditRuleInfo[]>("get_audit_rules");
      setRules(data);
    } catch {
      setRules([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { loadRules(); }, [loadRules]);

  const filteredRules = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return rules;
    return rules.filter((r) => {
      if (r.tag.toLowerCase().includes(q)) return true;
      if (r.descriptionZh && r.descriptionZh.toLowerCase().includes(q)) return true;
      if (r.descriptionEn && r.descriptionEn.toLowerCase().includes(q)) return true;
      if (r.patterns.some((p) => p.toLowerCase().includes(q))) return true;
      return false;
    });
  }, [rules, query]);

  const grouped = useMemo(() => {
    const map = new Map<string, AuditRuleInfo[]>();
    for (const r of filteredRules) {
      const cat = r.category || "custom";
      if (!map.has(cat)) map.set(cat, []);
      map.get(cat)!.push(r);
    }
    const sorted = new Map<string, AuditRuleInfo[]>();
    for (const cat of CATEGORY_ORDER) {
      if (map.has(cat)) sorted.set(cat, map.get(cat)!);
    }
    for (const [cat, rules] of map) {
      if (!sorted.has(cat)) sorted.set(cat, rules);
    }
    return sorted;
  }, [filteredRules]);

  const handleToggle = async (rule: AuditRuleInfo) => {
    try {
      await invoke("set_audit_rule_enabled", { id: rule.id, enabled: !rule.enabled });
      setRules((prev) => prev.map((r) => r.id === rule.id ? { ...r, enabled: !r.enabled } : r));
      if (selectedRule?.id === rule.id) {
        setSelectedRule({ ...rule, enabled: !rule.enabled });
      }
    } catch (e) {
      console.error("Failed to toggle rule:", e);
    }
  };

  const handleDelete = async (id: string) => {
    if (!confirm(t("audit.rule_delete_confirm"))) return;
    try {
      await invoke("delete_custom_audit_rule", { id });
      setRules((prev) => prev.filter((r) => r.id !== id));
      if (selectedRule?.id === id) setSelectedRule(null);
    } catch (e) {
      console.error("Failed to delete rule:", e);
    }
  };

  const catLabel = (cat: string) => {
    const key = `audit.cat_${cat}`;
    const val = t(key);
    return val === key ? cat : val;
  };

  const desc = (rule: AuditRuleInfo) =>
    lang.startsWith("zh") ? rule.descriptionZh : rule.descriptionEn;

  if (showSuggest) {
    return (
      <SuggestView
        lang={lang}
        onClose={() => { setShowSuggest(false); loadRules(); }}
      />
    );
  }

  return (
    <>
      {/* Action bar */}
      <div className={styles.filter_bar}>
        <button className={styles.primary_btn} onClick={() => setShowSuggest(true)}>
          + {t("audit.new_rule")}
        </button>
        <input
          type="text"
          className={styles.search_input}
          placeholder={t("audit.rule_search_placeholder")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className={styles.filter_spacer} />
        <span className={styles.scan_info}>
          {query.trim() ? `${filteredRules.length} / ${rules.length}` : rules.length} {t("audit.tab_rules").toLowerCase()}
        </span>
      </div>

      <div className={styles.body}>
        <aside className={styles.list_pane}>
          {loading && <p className={styles.empty}>{t("audit.scanning")}</p>}
          {!loading && rules.length === 0 && <p className={styles.empty}>{t("audit.no_rules")}</p>}
          {!loading && rules.length > 0 && filteredRules.length === 0 && (
            <p className={styles.empty}>{t("audit.no_matching_rules")}</p>
          )}
          {!loading && Array.from(grouped.entries()).map(([cat, catRules]) => (
            <div key={cat} className={styles.workspace_group}>
              <div className={styles.workspace_header_static}>
                <span className={styles.workspace_name}>{catLabel(cat)}</span>
                <span className={styles.workspace_count}>{catRules.length}</span>
              </div>
              <div className={styles.card_list}>
                {catRules.map((rule) => {
                  const active = selectedRule?.id === rule.id;
                  return (
                    <button
                      key={rule.id}
                      className={`${styles.card} ${active ? styles.card_active : ""} ${!rule.enabled ? styles.card_read : ""}`}
                      onClick={() => setSelectedRule(rule)}
                    >
                      <span className={`${styles.risk_badge} ${styles[RISK_CLASS[rule.level]]}`}>
                        {RISK_LABELS[rule.level]}
                      </span>
                      <div className={styles.card_body}>
                        <div className={styles.card_title_row}>
                          <span className={styles.card_title}>{rule.tag}</span>
                          {!rule.builtin && <span className={styles.custom_badge}>{t("audit.rule_custom")}</span>}
                        </div>
                        <div className={styles.card_hook}>{desc(rule)}</div>
                        {rule.patterns[0] && (
                          <code className={styles.rule_pattern_preview} title={rule.patterns.join("\n")}>
                            {rule.patterns[0]}
                            {rule.patterns.length > 1 ? ` +${rule.patterns.length - 1}` : ""}
                          </code>
                        )}
                      </div>
                      <label className={styles.toggle} onClick={(e) => e.stopPropagation()}>
                        <input type="checkbox" checked={rule.enabled} onChange={() => handleToggle(rule)} />
                        <span className={styles.toggle_slider} />
                      </label>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </aside>

        {selectedRule && (
          <main className={styles.detail_pane}>
            <div className={styles.detail_header}>
              <div className={styles.detail_title}>
                <span className={`${styles.risk_badge} ${styles[RISK_CLASS[selectedRule.level]]}`}>
                  {RISK_LABELS[selectedRule.level]}
                </span>
                <span className={styles.detail_name}>{selectedRule.tag}</span>
              </div>
              <button className={styles.detail_close} onClick={() => setSelectedRule(null)}>✕</button>
            </div>
            <div className={styles.detail_body}>
              <DetailRow label={t("audit.rule_id")} value={selectedRule.id} />
              <DetailRow label={t("audit.rule_level")} value={t(`audit.level_${selectedRule.level}`)} />
              <DetailRow label={t("audit.rule_category")} value={catLabel(selectedRule.category)} />
              <DetailRow label={t("audit.rule_match_mode")} value={t(`audit.match_${selectedRule.matchMode}`)} />
              <div className={styles.detail_row}>
                <span className={styles.detail_label}>{t("audit.rule_description")}</span>
              </div>
              <p className={styles.rule_desc}>{desc(selectedRule)}</p>
              <div className={styles.detail_row}>
                <span className={styles.detail_label}>{t("audit.rule_patterns")}</span>
              </div>
              <pre className={styles.command_pre}>
                {selectedRule.patterns.join("\n")}
              </pre>
              <div className={styles.detail_actions}>
                <label className={styles.toggle}>
                  <input type="checkbox" checked={selectedRule.enabled} onChange={() => handleToggle(selectedRule)} />
                  <span className={styles.toggle_slider} />
                </label>
                <span className={styles.toggle_label}>
                  {selectedRule.enabled ? t("audit.rule_builtin") : t("audit.rule_custom")}
                </span>
                {!selectedRule.builtin && (
                  <button className={styles.delete_btn} onClick={() => handleDelete(selectedRule.id)}>
                    {t("audit.rule_delete")}
                  </button>
                )}
              </div>
            </div>
          </main>
        )}
      </div>

      <GuardAllowRulesSection />
    </>
  );
}

// ── Guard Allow Rules Section ───────────────────────────────────────────────

function GuardAllowRulesSection() {
  const { t, i18n } = useTranslation();
  const [rules, setRules] = useState<GuardAllowRule[]>([]);
  const [loading, setLoading] = useState(true);
  const [expanded, setExpanded] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<GuardAllowRule[]>("list_guard_allow_rules");
      setRules(data);
    } catch (e) {
      console.error("list_guard_allow_rules failed:", e);
      setRules([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleRemove = async (id: string) => {
    if (!confirm(t("guard.allow_rules_remove_confirm"))) return;
    try {
      await invoke("remove_guard_allow_rule", { id });
      setRules((prev) => prev.filter((r) => r.id !== id));
    } catch (e) {
      console.error("remove_guard_allow_rule failed:", e);
    }
  };

  const formatCreated = (iso: string): string => {
    try {
      const d = new Date(iso);
      return d.toLocaleString(i18n.language);
    } catch {
      return iso;
    }
  };

  return (
    <section className={styles.allow_rules_section}>
      <button
        type="button"
        className={styles.allow_rules_header}
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        <span className={styles.allow_rules_chevron}>{expanded ? "▾" : "▸"}</span>
        <span className={styles.allow_rules_title}>{t("guard.allow_rules_title")}</span>
        <span className={styles.workspace_count}>{rules.length}</span>
      </button>

      {expanded && (
        <div className={styles.allow_rules_body}>
          {loading ? (
            <p className={styles.empty}>{t("audit.scanning")}</p>
          ) : rules.length === 0 ? (
            <p className={styles.empty}>{t("guard.allow_rules_empty")}</p>
          ) : (
            <table className={styles.allow_rules_table}>
              <thead>
                <tr>
                  <th>{t("guard.allow_rules_col_prefix")}</th>
                  <th>{t("guard.allow_rules_col_tag")}</th>
                  <th>{t("guard.allow_rules_col_created")}</th>
                  <th aria-label="actions" />
                </tr>
              </thead>
              <tbody>
                {rules.map((r) => (
                  <tr key={r.id}>
                    <td>
                      <code className={styles.allow_rules_prefix}>{r.prefix}</code>
                    </td>
                    <td>
                      {r.sourceTag ? (
                        <span className={styles.custom_badge}>{r.sourceTag}</span>
                      ) : (
                        <span className={styles.empty_inline}>—</span>
                      )}
                    </td>
                    <td className={styles.allow_rules_created}>{formatCreated(r.createdAt)}</td>
                    <td>
                      <button
                        className={styles.delete_btn}
                        onClick={() => handleRemove(r.id)}
                      >
                        {t("guard.allow_rules_remove")}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </section>
  );
}

// ── AI Suggestion View ──────────────────────────────────────────────────────

function SuggestView({ lang, onClose }: { lang: string; onClose: () => void }) {
  const { t } = useTranslation();
  const [concern, setConcern] = useState("");
  const [suggestions, setSuggestions] = useState<SuggestedRule[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [added, setAdded] = useState(false);

  const handleGenerate = async () => {
    if (!concern.trim()) return;
    setLoading(true);
    setError(null);
    setSuggestions([]);
    setSelected(new Set());
    try {
      const data = await invoke<SuggestedRule[]>("suggest_audit_rules", {
        concern: concern.trim(),
        lang,
      });
      setSuggestions(data);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const toggleSelect = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const handleAddSelected = async () => {
    for (const s of suggestions.filter((s) => selected.has(s.id))) {
      try {
        await invoke("save_custom_audit_rule", {
          rule: {
            id: s.id,
            level: s.level,
            tag: s.tag,
            matchMode: s.matchMode,
            patterns: s.patterns,
            descriptionEn: s.descriptionEn,
            descriptionZh: s.descriptionZh,
            enabled: true,
            builtin: false,
            category: s.category,
          },
        });
      } catch (e) {
        console.error("Failed to add rule:", e);
      }
    }
    setAdded(true);
    setTimeout(onClose, 500);
  };

  const desc = (s: SuggestedRule) =>
    lang.startsWith("zh") ? s.descriptionZh : s.descriptionEn;

  return (
    <div className={styles.suggest_container}>
      <div className={styles.suggest_header}>
        <button className={styles.suggest_back} onClick={onClose}>← {t("audit.tab_rules")}</button>
        <h3 className={styles.suggest_title}>{t("audit.new_rule")}</h3>
      </div>

      <div className={styles.suggest_input_row}>
        <textarea
          className={styles.suggest_textarea}
          placeholder={t("audit.suggest_placeholder")}
          value={concern}
          onChange={(e) => setConcern(e.target.value)}
          rows={3}
        />
        <button
          className={styles.primary_btn}
          onClick={handleGenerate}
          disabled={loading || !concern.trim()}
        >
          {loading ? t("audit.suggest_loading") : t("audit.suggest_btn")}
        </button>
      </div>

      {error && <p className={styles.suggest_error}>{t("audit.suggest_error")}: {error}</p>}

      {!loading && suggestions.length === 0 && concern.trim() && !error && (
        <p className={styles.empty}>{t("audit.suggest_empty")}</p>
      )}

      {suggestions.length > 0 && (
        <div className={styles.suggest_results}>
          {suggestions.map((s) => (
            <div key={s.id} className={`${styles.suggest_card} ${selected.has(s.id) ? styles.suggest_card_selected : ""}`} onClick={() => toggleSelect(s.id)}>
              <div className={styles.suggest_card_header}>
                <input type="checkbox" checked={selected.has(s.id)} onChange={() => toggleSelect(s.id)} onClick={(e) => e.stopPropagation()} />
                <span className={`${styles.risk_badge} ${styles[RISK_CLASS[s.level]]}`}>
                  {RISK_LABELS[s.level]}
                </span>
                <span className={styles.suggest_tag}>{s.tag}</span>
              </div>
              <p className={styles.suggest_desc}>{desc(s)}</p>
              <div className={styles.suggest_patterns}>
                {s.patterns.map((p, i) => <code key={i} className={styles.tag}>{p}</code>)}
              </div>
              <div className={styles.suggest_reasoning}>
                <span className={styles.detail_label}>{t("audit.suggest_reasoning")}</span>
                <span className={styles.suggest_reasoning_text}>{s.reasoning}</span>
              </div>
            </div>
          ))}

          <button
            className={styles.primary_btn}
            disabled={selected.size === 0 || added}
            onClick={handleAddSelected}
          >
            {t("audit.suggest_add_selected")} ({selected.size})
          </button>
        </div>
      )}
    </div>
  );
}

// ── Filter chip ──────────────────────────────────────────────────────────────

function FilterChip({
  label,
  count,
  active,
  tone,
  onClick,
}: {
  label: string;
  count: number;
  active: boolean;
  tone?: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`${styles.chip} ${active ? styles.chip_active : ""} ${tone ? styles[tone] ?? "" : ""}`}
      onClick={onClick}
    >
      <span className={styles.chip_label}>{label}</span>
      <span className={styles.chip_count}>{count}</span>
    </button>
  );
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.detail_row}>
      <span className={styles.detail_label}>{label}</span>
      <span className={styles.detail_value}>{value}</span>
    </div>
  );
}

function formatTime(ts: string): string {
  try {
    const d = new Date(ts);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return "";
  }
}
