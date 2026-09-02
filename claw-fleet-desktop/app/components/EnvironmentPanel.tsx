import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore } from "../store";
import { isWebBuild } from "../hostEnv";
import type { RemoteWorkspacesConfig } from "../types";
import { AgentSourceIcon } from "./SessionCard";
import styles from "./EnvironmentPanel.module.css";

// ── Wire types (serde camelCase from claw-fleet-core) ─────────────────────────

interface HarnessStatus {
  source: string;
  installed: boolean;
  path: string | null;
  version: string | null;
  channel: string | null;
  loggedIn: boolean | null;
  authDetail: string | null;
}

interface FoxyCustody {
  alive: boolean;
  managesClaude: boolean;
  managesCodex: boolean;
}

interface InstallError {
  code: string;
  message: string;
}

interface UpdateReport {
  source: string;
  before: string | null;
  after: string | null;
  status: HarnessStatus;
}

interface ClaudeLoginPoll {
  parse: { authUrl: string | null; awaitingCode: boolean; token: string | null };
  running: boolean;
  tokenSaved: boolean;
}

interface CodexLoginPoll {
  parse: { authUrl: string | null; success: boolean; portBusy: boolean };
  running: boolean;
  loggedIn: boolean;
}

interface RemoteCodexLoginPoll {
  parse: { verifyUrl: string | null; userCode: string | null; success: boolean };
  running: boolean;
  loggedIn: boolean;
}

/** One rca workspace host's probe state in the 远端主机 section. */
interface RemoteHostState {
  statuses: HarnessStatus[] | null;
  probing: boolean;
  error: string | null;
}

const SOURCES = ["claude-code", "codex", "dsh"] as const;
const SOURCE_NAMES: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  dsh: "dsh",
};

function isInstallError(e: unknown): e is InstallError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

async function openExternal(url: string) {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url).catch(() => {});
}

// ── Login flow hooks (claude / codex poll loops) ─────────────────────────────

interface ClaudeFlow {
  procId: string;
  url: string | null;
  awaitingCode: boolean;
  done: boolean;
  error: string | null;
}

interface CodexFlow {
  procId: string;
  url: string | null;
  done: boolean;
  portBusy: boolean;
  error: string | null;
}

// ── Panel ─────────────────────────────────────────────────────────────────────

export function EnvironmentPanel() {
  const { t } = useTranslation();
  const connection = useConnectionStore((s) => s.connection);
  const isRemote = connection?.type === "remote";
  // A browser tab can neither run installers nor drive a login pty on the
  // machine the user sits at; statuses still show (routed to the serving host).
  const actionsDisabled = isRemote || isWebBuild();

  const [statuses, setStatuses] = useState<HarnessStatus[] | null>(null);
  const [custody, setCustody] = useState<FoxyCustody | null>(null);
  const [probing, setProbing] = useState(false);

  // Which source has an install/update running, and its streamed log tail.
  const [busy, setBusy] = useState<Record<string, "install" | "update" | "node" | null>>({});
  const [logs, setLogs] = useState<Record<string, string[]>>({});
  const [errors, setErrors] = useState<Record<string, InstallError | null>>({});
  const [updateNote, setUpdateNote] = useState<Record<string, string | null>>({});
  /** dsh install hit NodeMissing → offer the node bootstrap. */
  const [needNode, setNeedNode] = useState(false);

  const [claudeFlow, setClaudeFlow] = useState<ClaudeFlow | null>(null);
  const [claudeCode, setClaudeCode] = useState("");
  const [codexFlow, setCodexFlow] = useState<CodexFlow | null>(null);

  // dsh credential editor
  const [dshRefs, setDshRefs] = useState<string[] | null>(null);
  const [dshConfigured, setDshConfigured] = useState<Record<string, boolean>>({});
  const [dshOpen, setDshOpen] = useState(false);
  const [dshRef, setDshRef] = useState("");
  const [dshKey, setDshKey] = useState("");
  const [dshMsg, setDshMsg] = useState<string | null>(null);

  // rca remote-workspace hosts (phase 2): probed lazily per host — each probe
  // is an ssh round trip, so nothing fires until the user asks.
  const [remoteHosts, setRemoteHosts] = useState<
    Array<{ path: string; label: string; ssh: boolean }>
  >([]);
  const [hostState, setHostState] = useState<Record<string, RemoteHostState>>({});
  const [remoteBusy, setRemoteBusy] = useState<Record<string, string | null>>({});
  const [remoteCodexFlow, setRemoteCodexFlow] = useState<{
    path: string;
    procId: string;
    url: string | null;
    code: string | null;
    done: boolean;
    error: string | null;
  } | null>(null);

  const probe = useCallback(async () => {
    setProbing(true);
    try {
      const [s, c] = await Promise.all([
        invoke<HarnessStatus[]>("harness_statuses"),
        invoke<FoxyCustody>("harness_login_context").catch(() => null),
      ]);
      setStatuses(s);
      if (c) setCustody(c);
    } catch (e) {
      setStatuses([]);
      console.error("harness_statuses failed", e);
    } finally {
      setProbing(false);
    }
  }, []);

  useEffect(() => {
    void probe();
  }, [probe]);

  // Streamed installer output → per-source log tail.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      unlisten = await listen<{ source: string; line: string }>(
        "harness-install-progress",
        (e) => {
          setLogs((prev) => {
            const cur = prev[e.payload.source] ?? [];
            return { ...prev, [e.payload.source]: [...cur.slice(-7), e.payload.line] };
          });
        },
      );
    })();
    return () => unlisten?.();
  }, []);

  // ── install / update actions ────────────────────────────────────────────────

  const runInstall = useCallback(
    async (source: string) => {
      setBusy((p) => ({ ...p, [source]: "install" }));
      setErrors((p) => ({ ...p, [source]: null }));
      setLogs((p) => ({ ...p, [source]: [] }));
      try {
        await invoke<HarnessStatus>("install_harness", { source });
        if (source === "dsh") setNeedNode(false);
      } catch (e) {
        if (isInstallError(e)) {
          setErrors((p) => ({ ...p, [source]: e }));
          if (e.code === "node-missing") setNeedNode(true);
        } else {
          setErrors((p) => ({ ...p, [source]: { code: "install-failed", message: String(e) } }));
        }
      } finally {
        setBusy((p) => ({ ...p, [source]: null }));
        void probe();
      }
    },
    [probe],
  );

  const runUpdate = useCallback(
    async (source: string) => {
      setBusy((p) => ({ ...p, [source]: "update" }));
      setErrors((p) => ({ ...p, [source]: null }));
      setLogs((p) => ({ ...p, [source]: [] }));
      setUpdateNote((p) => ({ ...p, [source]: null }));
      try {
        const r = await invoke<UpdateReport>("update_harness", { source });
        const note =
          r.before && r.after
            ? r.before === r.after
              ? t("env.up_to_date", { version: r.after })
              : t("env.updated", { before: r.before, after: r.after })
            : t("env.updated_unknown");
        setUpdateNote((p) => ({ ...p, [source]: note }));
      } catch (e) {
        setErrors((p) => ({
          ...p,
          [source]: isInstallError(e) ? e : { code: "install-failed", message: String(e) },
        }));
      } finally {
        setBusy((p) => ({ ...p, [source]: null }));
        void probe();
      }
    },
    [probe, t],
  );

  const runNodeInstall = useCallback(async () => {
    setBusy((p) => ({ ...p, dsh: "node" }));
    setErrors((p) => ({ ...p, dsh: null }));
    setLogs((p) => ({ ...p, node: [] }));
    try {
      await invoke<string>("install_node_runtime");
      setNeedNode(false);
    } catch (e) {
      setErrors((p) => ({
        ...p,
        dsh: isInstallError(e) ? e : { code: "install-failed", message: String(e) },
      }));
    } finally {
      setBusy((p) => ({ ...p, dsh: null }));
    }
  }, []);

  // ── claude login flow ───────────────────────────────────────────────────────

  const claudePollRef = useRef<number | null>(null);
  const stopClaudePoll = useCallback(() => {
    if (claudePollRef.current !== null) {
      window.clearInterval(claudePollRef.current);
      claudePollRef.current = null;
    }
  }, []);

  const startClaudeLogin = useCallback(async () => {
    try {
      const procId = await invoke<string>("claude_login_start");
      setClaudeFlow({ procId, url: null, awaitingCode: false, done: false, error: null });
      stopClaudePoll();
      claudePollRef.current = window.setInterval(async () => {
        try {
          const p = await invoke<ClaudeLoginPoll>("claude_login_poll", { id: procId });
          setClaudeFlow((prev) =>
            prev && prev.procId === procId
              ? {
                  ...prev,
                  url: p.parse.authUrl ?? prev.url,
                  awaitingCode: p.parse.awaitingCode && !p.tokenSaved,
                  done: p.tokenSaved,
                  error:
                    !p.running && !p.tokenSaved && !p.parse.awaitingCode && !p.parse.authUrl
                      ? t("env.login_proc_exited")
                      : prev.error,
                }
              : prev,
          );
          if (p.tokenSaved || !p.running) {
            stopClaudePoll();
            if (p.tokenSaved) void probe();
          }
        } catch {
          /* transient poll failure — next tick retries */
        }
      }, 1200);
    } catch (e) {
      setClaudeFlow({ procId: "", url: null, awaitingCode: false, done: false, error: String(e) });
    }
  }, [probe, stopClaudePoll, t]);

  const cancelClaudeLogin = useCallback(async () => {
    stopClaudePoll();
    const id = claudeFlow?.procId;
    setClaudeFlow(null);
    setClaudeCode("");
    if (id) await invoke("claude_login_cancel", { id }).catch(() => {});
  }, [claudeFlow, stopClaudePoll]);

  const submitClaudeCode = useCallback(async () => {
    if (!claudeFlow || !claudeCode.trim()) return;
    await invoke("claude_login_submit_code", { id: claudeFlow.procId, code: claudeCode }).catch(
      (e) => setClaudeFlow((p) => (p ? { ...p, error: String(e) } : p)),
    );
    setClaudeCode("");
  }, [claudeFlow, claudeCode]);

  // ── codex login flow ────────────────────────────────────────────────────────

  const codexPollRef = useRef<number | null>(null);
  const stopCodexPoll = useCallback(() => {
    if (codexPollRef.current !== null) {
      window.clearInterval(codexPollRef.current);
      codexPollRef.current = null;
    }
  }, []);

  const startCodexLogin = useCallback(async () => {
    try {
      const procId = await invoke<string>("codex_login_start", { deviceAuth: false });
      setCodexFlow({ procId, url: null, done: false, portBusy: false, error: null });
      stopCodexPoll();
      codexPollRef.current = window.setInterval(async () => {
        try {
          const p = await invoke<CodexLoginPoll>("codex_login_poll", { id: procId });
          setCodexFlow((prev) =>
            prev && prev.procId === procId
              ? {
                  ...prev,
                  url: p.parse.authUrl ?? prev.url,
                  done: p.loggedIn,
                  portBusy: p.parse.portBusy,
                  error:
                    !p.running && !p.loggedIn
                      ? p.parse.portBusy
                        ? t("env.codex_port_busy")
                        : t("env.login_proc_exited")
                      : prev.error,
                }
              : prev,
          );
          if (p.loggedIn || !p.running) {
            stopCodexPoll();
            if (p.loggedIn) void probe();
          }
        } catch {
          /* transient poll failure — next tick retries */
        }
      }, 1200);
    } catch (e) {
      setCodexFlow({ procId: "", url: null, done: false, portBusy: false, error: String(e) });
    }
  }, [probe, stopCodexPoll, t]);

  const cancelCodexLogin = useCallback(async () => {
    stopCodexPoll();
    const id = codexFlow?.procId;
    setCodexFlow(null);
    if (id) await invoke("codex_login_cancel", { id }).catch(() => {});
  }, [codexFlow, stopCodexPoll]);

  useEffect(() => () => {
    stopClaudePoll();
    stopCodexPoll();
  }, [stopClaudePoll, stopCodexPoll]);

  // ── dsh credentials ─────────────────────────────────────────────────────────

  const loadDshCreds = useCallback(async () => {
    setDshMsg(null);
    try {
      const refs = await invoke<string[]>("dsh_credential_refs");
      setDshRefs(refs);
      if (refs.length > 0) {
        const desc = await invoke<{ credentials: Record<string, { configured: boolean }> }>(
          "dsh_credentials_describe",
          { refs },
        );
        const map: Record<string, boolean> = {};
        for (const r of refs) map[r] = desc.credentials?.[r]?.configured ?? false;
        setDshConfigured(map);
        setDshRef((cur) => cur || refs[0]);
      }
    } catch (e) {
      setDshRefs([]);
      setDshMsg(String(e));
    }
  }, []);

  const saveDshKey = useCallback(async () => {
    if (!dshRef.trim() || !dshKey.trim()) return;
    setDshMsg(null);
    try {
      await invoke("dsh_credentials_set", { reference: dshRef.trim(), value: dshKey });
      setDshKey("");
      setDshMsg(t("env.dsh_key_saved", { ref: dshRef.trim() }));
      await loadDshCreds();
    } catch (e) {
      setDshMsg(String(e));
    }
  }, [dshRef, dshKey, loadDshCreds, t]);

  // ── rca remote-workspace hosts (phase 2) ────────────────────────────────────

  useEffect(() => {
    // The rca registry is this machine's; on a RemoteBackend connection the
    // panel already shows that host's own environment instead.
    if (isRemote) return;
    invoke<RemoteWorkspacesConfig>("list_remote_workspaces")
      .then((cfg) =>
        setRemoteHosts(
          (cfg.workspaces ?? []).map((w) => ({
            path: w.path,
            label: w.label || w.sshTarget || w.path,
            ssh: !!w.sshTarget,
          })),
        ),
      )
      .catch(() => {});
  }, [isRemote]);

  const probeHost = useCallback(async (path: string) => {
    setHostState((p) => ({ ...p, [path]: { statuses: null, probing: true, error: null } }));
    try {
      const statuses = await invoke<HarnessStatus[]>("remote_workspace_harness_statuses", { path });
      setHostState((p) => ({ ...p, [path]: { statuses, probing: false, error: null } }));
    } catch (e) {
      setHostState((p) => ({ ...p, [path]: { statuses: null, probing: false, error: String(e) } }));
    }
  }, []);

  const installRemote = useCallback(
    async (path: string, source: string) => {
      setRemoteBusy((p) => ({ ...p, [path]: source }));
      const logKey = `remote:${path}:${source}`;
      setLogs((p) => ({ ...p, [logKey]: [] }));
      try {
        const statuses = await invoke<HarnessStatus[]>("install_harness_remote", { path, source });
        setHostState((p) => ({ ...p, [path]: { statuses, probing: false, error: null } }));
      } catch (e) {
        const msg = isInstallError(e) ? `${t(`env.err.${e.code}`, { defaultValue: e.code })}\n${e.message}` : String(e);
        setHostState((p) => ({
          ...p,
          [path]: { statuses: p[path]?.statuses ?? null, probing: false, error: msg },
        }));
      } finally {
        setRemoteBusy((p) => ({ ...p, [path]: null }));
      }
    },
    [t],
  );

  const remoteCodexPollRef = useRef<number | null>(null);
  const stopRemoteCodexPoll = useCallback(() => {
    if (remoteCodexPollRef.current !== null) {
      window.clearInterval(remoteCodexPollRef.current);
      remoteCodexPollRef.current = null;
    }
  }, []);

  const startRemoteCodexLogin = useCallback(
    async (path: string) => {
      try {
        const procId = await invoke<string>("remote_codex_login_start", { path });
        setRemoteCodexFlow({ path, procId, url: null, code: null, done: false, error: null });
        stopRemoteCodexPoll();
        remoteCodexPollRef.current = window.setInterval(async () => {
          try {
            const p = await invoke<RemoteCodexLoginPoll>("remote_codex_login_poll", {
              id: procId,
              path,
            });
            setRemoteCodexFlow((prev) =>
              prev && prev.procId === procId
                ? {
                    ...prev,
                    url: p.parse.verifyUrl ?? prev.url,
                    code: p.parse.userCode ?? prev.code,
                    done: p.loggedIn,
                    error: !p.running && !p.loggedIn ? t("env.login_proc_exited") : prev.error,
                  }
                : prev,
            );
            if (p.loggedIn || !p.running) {
              stopRemoteCodexPoll();
              if (p.loggedIn) void probeHost(path);
            }
          } catch {
            /* transient poll failure — next tick retries */
          }
        }, 1500);
      } catch (e) {
        setRemoteCodexFlow({ path, procId: "", url: null, code: null, done: false, error: String(e) });
      }
    },
    [probeHost, stopRemoteCodexPoll, t],
  );

  const cancelRemoteCodexLogin = useCallback(async () => {
    stopRemoteCodexPoll();
    const id = remoteCodexFlow?.procId;
    setRemoteCodexFlow(null);
    if (id) await invoke("codex_login_cancel", { id }).catch(() => {});
  }, [remoteCodexFlow, stopRemoteCodexPoll]);

  useEffect(() => () => stopRemoteCodexPoll(), [stopRemoteCodexPoll]);

  // ── render helpers ──────────────────────────────────────────────────────────

  const channelLabel = (channel: string | null) =>
    channel ? t(`env.channel.${channel}`, { defaultValue: channel }) : "";

  const renderLoginState = (s: HarnessStatus) => {
    if (s.source === "dsh") {
      return (
        <span className={styles.muted}>
          {t("env.dsh_byok")}
        </span>
      );
    }
    const managed =
      custody?.alive &&
      ((s.source === "claude-code" && custody.managesClaude) ||
        (s.source === "codex" && custody.managesCodex));
    if (managed) return <span className={styles.badge_ok}>{t("env.managed_by_foxy")}</span>;
    if (s.loggedIn) {
      return (
        <span className={styles.badge_ok}>
          {t("env.logged_in")}
          {s.authDetail ? ` · ${s.authDetail}` : ""}
        </span>
      );
    }
    return <span className={styles.badge_warn}>{t("env.not_logged_in")}</span>;
  };

  const renderClaudeLogin = (s: HarnessStatus) => {
    const managed = custody?.alive && custody.managesClaude;
    if (!s.installed || managed || s.loggedIn) return null;
    if (!claudeFlow) {
      return (
        <button className={styles.action_btn} onClick={() => void startClaudeLogin()} disabled={actionsDisabled}>
          {t("env.login_btn")}
        </button>
      );
    }
    return (
      <div className={styles.flow}>
        {claudeFlow.done ? (
          <div className={styles.flow_done}>{t("env.claude_login_done")}</div>
        ) : (
          <>
            <div className={styles.flow_hint}>{t("env.claude_login_hint")}</div>
            {claudeFlow.url && (
              <button className={styles.link_btn} onClick={() => void openExternal(claudeFlow.url!)}>
                {t("env.open_browser")}
              </button>
            )}
            {claudeFlow.awaitingCode && (
              <div className={styles.code_row}>
                <input
                  className={styles.code_input}
                  value={claudeCode}
                  placeholder={t("env.paste_code")}
                  onChange={(e) => setClaudeCode(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && void submitClaudeCode()}
                />
                <button className={styles.action_btn} onClick={() => void submitClaudeCode()}>
                  {t("env.submit_code")}
                </button>
              </div>
            )}
            {claudeFlow.error && <div className={styles.error}>{claudeFlow.error}</div>}
            <button className={styles.link_btn} onClick={() => void cancelClaudeLogin()}>
              {t("env.cancel")}
            </button>
          </>
        )}
      </div>
    );
  };

  const renderCodexLogin = (s: HarnessStatus) => {
    const managed = custody?.alive && custody.managesCodex;
    if (!s.installed || managed || s.loggedIn) return null;
    if (!codexFlow) {
      return (
        <button className={styles.action_btn} onClick={() => void startCodexLogin()} disabled={actionsDisabled}>
          {t("env.login_btn")}
        </button>
      );
    }
    return (
      <div className={styles.flow}>
        {codexFlow.done ? (
          <div className={styles.flow_done}>{t("env.codex_login_done")}</div>
        ) : (
          <>
            <div className={styles.flow_hint}>{t("env.codex_login_hint")}</div>
            {codexFlow.url && (
              <button className={styles.link_btn} onClick={() => void openExternal(codexFlow.url!)}>
                {t("env.open_browser")}
              </button>
            )}
            {codexFlow.error && <div className={styles.error}>{codexFlow.error}</div>}
            <button className={styles.link_btn} onClick={() => void cancelCodexLogin()}>
              {t("env.cancel")}
            </button>
          </>
        )}
      </div>
    );
  };

  const renderDshCreds = (s: HarnessStatus) => {
    if (!s.installed) return null;
    if (!dshOpen) {
      return (
        <button
          className={styles.action_btn}
          disabled={actionsDisabled}
          onClick={() => {
            setDshOpen(true);
            void loadDshCreds();
          }}
        >
          {t("env.dsh_creds_btn")}
        </button>
      );
    }
    return (
      <div className={styles.flow}>
        {dshRefs === null ? (
          <div className={styles.muted}>{t("env.loading")}</div>
        ) : dshRefs.length === 0 ? (
          <div className={styles.muted}>{t("env.dsh_no_refs")}</div>
        ) : (
          <>
            <div className={styles.flow_hint}>{t("env.dsh_creds_hint")}</div>
            <div className={styles.code_row}>
              <select
                className={styles.code_input}
                value={dshRef}
                onChange={(e) => setDshRef(e.target.value)}
              >
                {dshRefs.map((r) => (
                  <option key={r} value={r}>
                    {r}
                    {dshConfigured[r] ? ` · ${t("env.configured")}` : ""}
                  </option>
                ))}
              </select>
            </div>
            <div className={styles.code_row}>
              <input
                className={styles.code_input}
                type="password"
                value={dshKey}
                placeholder={t("env.dsh_key_placeholder")}
                onChange={(e) => setDshKey(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && void saveDshKey()}
              />
              <button className={styles.action_btn} onClick={() => void saveDshKey()}>
                {t("env.save")}
              </button>
            </div>
          </>
        )}
        {dshMsg && <div className={styles.flow_hint}>{dshMsg}</div>}
        <button className={styles.link_btn} onClick={() => setDshOpen(false)}>
          {t("env.close")}
        </button>
      </div>
    );
  };

  const renderCard = (s: HarnessStatus) => {
    const b = busy[s.source];
    const err = errors[s.source];
    const log = logs[s.source === "dsh" && b === "node" ? "node" : s.source] ?? [];
    const extensionChannel = (s.channel ?? "").endsWith("extension");
    return (
      <div key={s.source} className={styles.card}>
        <div className={styles.card_head}>
          <AgentSourceIcon source={s.source} />
          <span className={styles.card_name}>{SOURCE_NAMES[s.source] ?? s.source}</span>
          {s.installed ? (
            <span className={styles.badge_ok}>
              {t("env.installed")}
              {s.version ? ` v${s.version}` : ""}
            </span>
          ) : (
            <span className={styles.badge_warn}>{t("env.not_installed")}</span>
          )}
          {renderLoginState(s)}
        </div>
        {s.installed && (
          <div className={styles.card_meta}>
            {s.channel && <span>{t("env.channel_label")}: {channelLabel(s.channel)}</span>}
            {s.path && <span className={styles.path}>{s.path}</span>}
          </div>
        )}
        <div className={styles.actions}>
          {!s.installed && (
            <button
              className={styles.action_btn_primary}
              disabled={!!b || actionsDisabled}
              onClick={() => void runInstall(s.source)}
            >
              {b === "install" ? t("env.installing") : t("env.install_btn")}
            </button>
          )}
          {s.installed && !extensionChannel && (
            <button
              className={styles.action_btn}
              disabled={!!b || actionsDisabled}
              onClick={() => void runUpdate(s.source)}
            >
              {b === "update" ? t("env.updating") : t("env.update_btn")}
            </button>
          )}
          {s.source === "dsh" && needNode && (
            <button
              className={styles.action_btn_primary}
              disabled={!!b || actionsDisabled}
              onClick={() => void runNodeInstall()}
            >
              {b === "node" ? t("env.installing") : t("env.install_node_btn")}
            </button>
          )}
          {s.source === "claude-code" && renderClaudeLogin(s)}
          {s.source === "codex" && renderCodexLogin(s)}
          {s.source === "dsh" && renderDshCreds(s)}
        </div>
        {updateNote[s.source] && <div className={styles.flow_done}>{updateNote[s.source]}</div>}
        {b && log.length > 0 && (
          <pre className={styles.log}>{log.join("\n")}</pre>
        )}
        {err && (
          <div className={styles.error}>
            {t(`env.err.${err.code}`, { defaultValue: err.code })}
            <details>
              <summary>{t("env.err_details")}</summary>
              <pre className={styles.log}>{err.message}</pre>
            </details>
          </div>
        )}
      </div>
    );
  };

  const renderRemoteHost = (host: { path: string; label: string; ssh: boolean }) => {
    const st = hostState[host.path];
    const busySource = remoteBusy[host.path];
    const flow = remoteCodexFlow?.path === host.path ? remoteCodexFlow : null;
    return (
      <div key={host.path} className={styles.card}>
        <div className={styles.card_head}>
          <span className={styles.card_name}>{host.label}</span>
          <span className={styles.path}>{host.path}</span>
          {host.ssh ? (
            <button
              className={styles.action_btn}
              disabled={st?.probing || actionsDisabled}
              onClick={() => void probeHost(host.path)}
            >
              {st?.probing ? t("env.probing") : t("env.probe_host")}
            </button>
          ) : (
            <span className={styles.muted}>{t("env.pairing_no_actions")}</span>
          )}
        </div>
        {st?.error && <div className={styles.error}>{st.error}</div>}
        {st?.statuses?.map((s) => {
          const logKey = `remote:${host.path}:${s.source}`;
          const log = logs[logKey] ?? [];
          return (
            <div key={s.source} className={styles.card_head}>
              <AgentSourceIcon source={s.source} />
              <span className={styles.card_name}>{SOURCE_NAMES[s.source] ?? s.source}</span>
              {s.installed ? (
                <span className={styles.badge_ok}>
                  {t("env.installed")}
                  {s.version ? ` v${s.version}` : ""}
                </span>
              ) : (
                <span className={styles.badge_warn}>{t("env.not_installed")}</span>
              )}
              {s.source !== "dsh" &&
                (s.loggedIn === true ? (
                  <span className={styles.badge_ok}>{t("env.logged_in")}</span>
                ) : s.loggedIn === false ? (
                  <span className={styles.badge_warn}>{t("env.not_logged_in")}</span>
                ) : (
                  <span className={styles.muted}>{t("env.login_unknown")}</span>
                ))}
              {!s.installed && (
                <button
                  className={styles.action_btn}
                  disabled={!!busySource || actionsDisabled}
                  onClick={() => void installRemote(host.path, s.source)}
                >
                  {busySource === s.source ? t("env.installing") : t("env.install_btn")}
                </button>
              )}
              {s.source === "codex" && s.installed && s.loggedIn === false && !flow && (
                <button
                  className={styles.action_btn}
                  disabled={actionsDisabled}
                  onClick={() => void startRemoteCodexLogin(host.path)}
                >
                  {t("env.login_btn")}
                </button>
              )}
              {busySource === s.source && log.length > 0 && (
                <pre className={styles.log}>{log.join("\n")}</pre>
              )}
            </div>
          );
        })}
        {flow && (
          <div className={styles.flow}>
            {flow.done ? (
              <div className={styles.flow_done}>{t("env.codex_login_done")}</div>
            ) : (
              <>
                <div className={styles.flow_hint}>{t("env.remote_codex_device_hint")}</div>
                {flow.code && <div className={styles.card_name}>{flow.code}</div>}
                {flow.url && (
                  <button className={styles.link_btn} onClick={() => void openExternal(flow.url!)}>
                    {t("env.open_browser")}
                  </button>
                )}
                {flow.error && <div className={styles.error}>{flow.error}</div>}
                <button className={styles.link_btn} onClick={() => void cancelRemoteCodexLogin()}>
                  {t("env.cancel")}
                </button>
              </>
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className={styles.root}>
      <div className={styles.head_row}>
        <p className={styles.subtitle}>{t("env.subtitle")}</p>
        <button className={styles.action_btn} onClick={() => void probe()} disabled={probing}>
          {probing ? t("env.probing") : t("env.refresh")}
        </button>
      </div>
      {isRemote && <div className={styles.remote_note}>{t("env.remote_note")}</div>}
      {statuses === null ? (
        <div className={styles.muted}>{t("env.loading")}</div>
      ) : (
        SOURCES.map((src) => {
          const s = statuses.find((x) => x.source === src);
          return s ? renderCard(s) : null;
        })
      )}
      {!isRemote && remoteHosts.length > 0 && (
        <>
          <div className={styles.head_row}>
            <p className={styles.subtitle}>{t("env.remote_hosts")}</p>
          </div>
          {remoteHosts.map(renderRemoteHost)}
        </>
      )}
    </div>
  );
}
