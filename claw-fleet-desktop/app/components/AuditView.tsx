import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuditStore, useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { AuditEvent, AuditRiskLevel, AuditRuleInfo, AuditSummary, SuggestedRule } from "../types";
import styles from "./AuditView.module.css";

// ── Risk level helpers ──────────────────────────────────────────────────────

const RISK_COLORS: Record<AuditRiskLevel, string> = {
  critical: "#ef4444",
  high: "#f97316",
  medium: "#eab308",
};

const RISK_LABELS: Record<AuditRiskLevel, string> = {
  critical: "CRIT",
  high: "HIGH",
  medium: "MED",
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
    <div className={styles.container}>
      {/* Header with tabs */}
      <div className={styles.header} data-tauri-drag-region>
        <h2 className={styles.title}>{t("audit.panel_title")}</h2>
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
      </div>
      {tab === "events" ? <EventsTab /> : <RulesTab lang={i18n.language} />}
    </div>
  );
}

// ── Events Tab (original audit view) ────────────────────────────────────────

function EventsTab() {
  const { t } = useTranslation();
  const [summary, setSummary] = useState<AuditSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<AuditRiskLevel | "all">("all");
  const [selectedEvent, setSelectedEvent] = useState<AuditEvent | null>(null);
  const { sessions } = useSessionsStore();
  const { open } = useDetailStore();
  const { setViewMode } = useUIStore();
  const { isRead, markAsRead, getEventKey, setCriticalEvents } = useAuditStore();

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
    (e) => filter === "all" || e.riskLevel === filter
  ) ?? [];

  const grouped = new Map<string, AuditEvent[]>();
  for (const e of filtered) {
    const key = e.sessionId;
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key)!.push(e);
  }

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
      <div className={styles.filter_row}>
        <div className={styles.filter_bar}>
          {(["all", "critical", "high", "medium"] as const).map((level) => {
            const count = level === "all"
              ? (summary?.events.length ?? 0)
              : counts[level];
            return (
              <button
                key={level}
                className={`${styles.filter_btn} ${filter === level ? styles.filter_active : ""}`}
                onClick={() => setFilter(level)}
                style={level !== "all" ? { color: RISK_COLORS[level] } : undefined}
              >
                {level === "all" ? t("audit.all") : RISK_LABELS[level]}
                {count > 0 && ` (${count})`}
              </button>
            );
          })}
        </div>
        <button className={styles.refresh_btn} onClick={load} title={t("audit.refresh")}>
          ↻
        </button>
        {summary && (
          <span className={styles.scan_info}>
            {t("audit.scanned", { count: summary.totalSessionsScanned })}
          </span>
        )}
      </div>

      {/* Body: event list + detail panel */}
      <div className={styles.body}>
        <div className={styles.event_list}>
          {loading && <p className={styles.empty}>{t("audit.scanning")}</p>}
          {!loading && filtered.length === 0 && <p className={styles.empty}>{t("audit.no_events")}</p>}
          {!loading && Array.from(grouped.entries()).map(([sessionId, events]) => (
            <div key={sessionId} className={styles.session_group}>
              <div className={styles.session_header} onClick={() => navigateToSession(events[0].jsonlPath)}>
                <span className={styles.session_name}>{events[0].workspaceName}</span>
                <span className={styles.session_source}>{events[0].agentSource}</span>
                <span className={styles.session_count}>{events.length}</span>
              </div>
              {events.map((event, i) => {
                const read = event.riskLevel === "critical" && isRead(event);
                return (
                  <div
                    key={`${sessionId}-${i}`}
                    className={`${styles.event_row} ${selectedEvent === event ? styles.event_row_selected : ""} ${read ? styles.event_row_read : ""}`}
                    onClick={() => setSelectedEvent(event)}
                  >
                    <span className={styles.risk_badge} style={{ background: `${RISK_COLORS[event.riskLevel]}20`, color: RISK_COLORS[event.riskLevel] }}>
                      {RISK_LABELS[event.riskLevel]}
                    </span>
                    <span className={styles.event_command}>{event.commandSummary}</span>
                    {event.riskLevel === "critical" && !read && (
                      <button className={styles.read_btn} title={t("audit.mark_read")} onClick={(e) => { e.stopPropagation(); markAsRead(getEventKey(event)); }}>✓</button>
                    )}
                    {event.timestamp && <span className={styles.event_time}>{formatTime(event.timestamp)}</span>}
                  </div>
                );
              })}
            </div>
          ))}
        </div>

        {selectedEvent && (
          <div className={styles.detail_panel}>
            <div className={styles.detail_header}>
              <span className={styles.risk_badge} style={{ background: `${RISK_COLORS[selectedEvent.riskLevel]}20`, color: RISK_COLORS[selectedEvent.riskLevel] }}>
                {RISK_LABELS[selectedEvent.riskLevel]}
              </span>
              <span className={styles.detail_title}>{selectedEvent.toolName}</span>
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
          </div>
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

  const grouped = useMemo(() => {
    const map = new Map<string, AuditRuleInfo[]>();
    for (const r of rules) {
      const cat = r.category || "custom";
      if (!map.has(cat)) map.set(cat, []);
      map.get(cat)!.push(r);
    }
    // Sort by predefined order
    const sorted = new Map<string, AuditRuleInfo[]>();
    for (const cat of CATEGORY_ORDER) {
      if (map.has(cat)) sorted.set(cat, map.get(cat)!);
    }
    // Append any unknown categories
    for (const [cat, rules] of map) {
      if (!sorted.has(cat)) sorted.set(cat, rules);
    }
    return sorted;
  }, [rules]);

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
      <div className={styles.filter_row}>
        <button className={styles.new_rule_btn} onClick={() => setShowSuggest(true)}>
          + {t("audit.new_rule")}
        </button>
        <span className={styles.scan_info}>
          {rules.length} {t("audit.tab_rules").toLowerCase()}
        </span>
      </div>

      <div className={styles.body}>
        {/* Left: rule list grouped by category */}
        <div className={styles.event_list}>
          {loading && <p className={styles.empty}>{t("audit.scanning")}</p>}
          {!loading && rules.length === 0 && <p className={styles.empty}>{t("audit.no_rules")}</p>}
          {!loading && Array.from(grouped.entries()).map(([cat, catRules]) => (
            <div key={cat} className={styles.session_group}>
              <div className={styles.category_header}>
                <span className={styles.session_name}>{catLabel(cat)}</span>
                <span className={styles.session_count}>{catRules.length}</span>
              </div>
              {catRules.map((rule) => (
                <div
                  key={rule.id}
                  className={`${styles.event_row} ${selectedRule?.id === rule.id ? styles.event_row_selected : ""} ${!rule.enabled ? styles.event_row_read : ""}`}
                  onClick={() => setSelectedRule(rule)}
                >
                  <span className={styles.risk_badge} style={{ background: `${RISK_COLORS[rule.level]}20`, color: RISK_COLORS[rule.level] }}>
                    {RISK_LABELS[rule.level]}
                  </span>
                  <span className={styles.event_command}>{rule.tag}</span>
                  {!rule.builtin && <span className={styles.custom_badge}>{t("audit.rule_custom")}</span>}
                  <label className={styles.toggle} onClick={(e) => e.stopPropagation()}>
                    <input type="checkbox" checked={rule.enabled} onChange={() => handleToggle(rule)} />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
              ))}
            </div>
          ))}
        </div>

        {/* Right: rule detail */}
        {selectedRule && (
          <div className={styles.detail_panel}>
            <div className={styles.detail_header}>
              <span className={styles.risk_badge} style={{ background: `${RISK_COLORS[selectedRule.level]}20`, color: RISK_COLORS[selectedRule.level] }}>
                {RISK_LABELS[selectedRule.level]}
              </span>
              <span className={styles.detail_title}>{selectedRule.tag}</span>
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
          </div>
        )}
      </div>
    </>
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
          className={styles.suggest_generate_btn}
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
                <span className={styles.risk_badge} style={{ background: `${RISK_COLORS[s.level]}20`, color: RISK_COLORS[s.level] }}>
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
            className={styles.suggest_add_btn}
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
