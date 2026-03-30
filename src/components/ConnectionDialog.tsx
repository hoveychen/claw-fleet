import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore, type Connection } from "../store";
import styles from "./ConnectionDialog.module.css";

// ── Types mirroring Rust structs ─────────────────────────────────────────────

export interface RemoteConnection {
  id: string;
  label: string;
  host: string;
  port: number;
  username: string;
  identityFile: string | null;
  jumpHost: string | null;
  sshProfile: string | null;
  probePort: number;
}

interface ConnectProgress {
  step: string;
  done: boolean;
  error: string | null;
  updateLast?: boolean;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function newId(): string {
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

function emptyConn(): RemoteConnection {
  return {
    id: newId(),
    label: "",
    host: "",
    port: 22,
    username: "",
    identityFile: null,
    jumpHost: null,
    sshProfile: null,
    probePort: 7007,
  };
}

// ── Component ────────────────────────────────────────────────────────────────

interface Props {
  onConnected: (conn: Connection) => void;
}

type View = "welcome" | "remote";
type ConnType = "manual" | "profile";

export function ConnectionDialog({ onConnected }: Props) {
  const { t } = useTranslation();
  const [view, setView] = useState<View>("welcome");
  const [savedConns, setSavedConns] = useState<RemoteConnection[]>([]);
  const [selectedSavedId, setSelectedSavedId] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);

  // Form state for a new connection
  const [form, setForm] = useState<RemoteConnection>(emptyConn);
  const [connType, setConnType] = useState<ConnType>("manual");

  // SSH config profiles from ~/.ssh/config
  const [sshProfiles, setSshProfiles] = useState<string[]>([]);

  // Connection in progress
  const [connecting, setConnecting] = useState(false);
  const [progressSteps, setProgressSteps] = useState<string[]>([]);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [connectDone, setConnectDone] = useState(false);

  const unlistenRef = useRef<(() => void) | null>(null);

  // Load saved connections and SSH profiles on mount.
  useEffect(() => {
    invoke<RemoteConnection[]>("list_saved_connections").then((conns) => {
      setSavedConns(conns);
    });
    invoke<string[]>("list_ssh_profiles").then(setSshProfiles).catch(() => {});
  }, []);

  // Listen for progress events
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<ConnectProgress>("remote-connect-progress", (event) => {
      const p = event.payload;
      setProgressSteps((prev) =>
        p.updateLast && prev.length > 0
          ? [...prev.slice(0, -1), p.step]
          : [...prev, p.step]
      );
      if (p.done) {
        setConnectDone(true);
        setConnecting(false);
      }
      if (p.error) {
        setConnectError(p.error);
        setConnecting(false);
      }
    }).then((fn) => {
      unlisten = fn;
      unlistenRef.current = fn;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  const handleDeleteSaved = useCallback(
    async (e: React.MouseEvent, id: string) => {
      e.stopPropagation();
      await invoke("delete_connection", { id });
      setSavedConns((prev) => prev.filter((c) => c.id !== id));
      if (selectedSavedId === id) {
        setSelectedSavedId(null);
      }
    },
    [selectedSavedId]
  );

  const handleBrowseKey = useCallback(async () => {
    const selected = await invoke<string | null>("pick_file", {
      title: t("connect_dialog.select_identity_file"),
    });
    if (selected) {
      setForm((f) => ({ ...f, identityFile: selected }));
    }
  }, [t]);

  const handleConnectLocal = useCallback(() => {
    onConnected({ type: "local" });
  }, [onConnected]);

  const handleConnectRemote = useCallback(async () => {
    setProgressSteps([]);
    setConnectError(null);
    setConnectDone(false);
    setConnecting(true);

    // Determine which connection to use
    let conn: RemoteConnection;
    const formVisible = showForm || savedConns.length === 0;
    if (selectedSavedId && !formVisible) {
      const found = savedConns.find((c) => c.id === selectedSavedId);
      if (!found) return;
      conn = found;
    } else {
      if (connType === "profile") {
        if (!form.sshProfile?.trim()) {
          setConnectError(t("connect_dialog.error_profile_required"));
          setConnecting(false);
          return;
        }
        conn = { ...form, host: "", username: "", port: 22 };
      } else {
        if (!form.host || !form.username) {
          setConnectError(t("connect_dialog.error_host_user_required"));
          setConnecting(false);
          return;
        }
        conn = { ...form, sshProfile: null };
      }
    }

    try {
      useSessionsStore.getState().setScanReady(false);
      await invoke("connect_remote", { conn });
      // Success is signaled by the progress event with done:true
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setConnectError(msg);
      setConnecting(false);
    }
  }, [selectedSavedId, showForm, savedConns, form, connType, onConnected, t]);

  // When connectDone fires, close after a short delay
  useEffect(() => {
    if (connectDone) {
      let remoteConn: RemoteConnection | null = null;
      const formVisible = showForm || savedConns.length === 0;
      if (selectedSavedId && !formVisible) {
        remoteConn = savedConns.find((c) => c.id === selectedSavedId) ?? null;
      } else {
        remoteConn = form;
      }
      if (remoteConn) {
        const conn = remoteConn;
        const t = setTimeout(() => onConnected({ type: "remote", connection: conn }), 600);
        return () => clearTimeout(t);
      }
    }
  }, [connectDone, onConnected, selectedSavedId, showForm, savedConns, form]);

  const formVisible = showForm || savedConns.length === 0;
  const formValid =
    connType === "profile"
      ? !!form.sshProfile?.trim()
      : !!form.host.trim() && !!form.username.trim();

  const isConnectDisabled =
    connecting ||
    (!selectedSavedId && !formVisible) ||
    (formVisible && !selectedSavedId && !formValid);

  // ── Welcome view (local hero + remote link) ──────────────────────────────

  if (view === "welcome") {
    return (
      <div className={styles.overlay}>
        <div className={styles.dialog}>
          <div className={styles.welcomeHeader}>
            <div className={styles.logoMark}>
              <svg width="40" height="40" viewBox="0 0 40 40" fill="none">
                <rect width="40" height="40" rx="10" fill="var(--color-accent)" opacity="0.12" />
                <path d="M12 28V16l8-6 8 6v12l-8 4-8-4z" stroke="var(--color-accent)" strokeWidth="2" fill="none" />
                <circle cx="20" cy="20" r="3" fill="var(--color-accent)" />
              </svg>
            </div>
            <h1 className={styles.welcomeTitle}>{t("connect_dialog.title")}</h1>
            <p className={styles.welcomeSubtitle}>{t("connect_dialog.subtitle")}</p>
          </div>

          <button className={styles.heroCard} onClick={handleConnectLocal}>
            <div className={styles.heroIconWrap}>
              <svg width="32" height="32" viewBox="0 0 32 32" fill="none">
                <rect x="4" y="6" width="24" height="16" rx="2" stroke="currentColor" strokeWidth="1.8" />
                <path d="M12 26h8" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
                <path d="M16 22v4" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
                <circle cx="16" cy="14" r="2.5" stroke="currentColor" strokeWidth="1.5" fill="none" />
                <path d="M11 18.5a5.5 5.5 0 0 1 10 0" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" fill="none" />
              </svg>
            </div>
            <div className={styles.heroText}>
              <span className={styles.heroTitle}>{t("connect_dialog.local_title")}</span>
              <span className={styles.heroDesc}>{t("connect_dialog.local_desc")}</span>
            </div>
            <span className={styles.heroAction}>{t("connect_dialog.connect_local")}</span>
          </button>

          <div className={styles.dividerRow}>
            <span className={styles.dividerLine} />
          </div>

          <button
            className={styles.remoteLink}
            onClick={() => {
              setView("remote");
              if (savedConns.length > 0) {
                setSelectedSavedId(savedConns[0].id);
              }
            }}
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" className={styles.remoteLinkIcon}>
              <circle cx="4" cy="8" r="2" stroke="currentColor" strokeWidth="1.2" />
              <circle cx="12" cy="8" r="2" stroke="currentColor" strokeWidth="1.2" />
              <path d="M6 8h4" stroke="currentColor" strokeWidth="1.2" />
            </svg>
            {t("connect_dialog.or_remote")}
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" className={styles.remoteLinkArrow}>
              <path d="M5 3l4 4-4 4" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
        </div>
      </div>
    );
  }

  // ── Remote view ──────────────────────────────────────────────────────────

  return (
    <div className={styles.overlay}>
      <div className={styles.dialog}>
        {/* Header with back button */}
        <div className={styles.remoteHeader}>
          <button
            className={styles.backBtn}
            onClick={() => {
              setView("welcome");
              setConnectError(null);
              setProgressSteps([]);
              setConnecting(false);
            }}
          >
            {t("connect_dialog.back")}
          </button>
          <span className={styles.remoteTitle}>{t("connect_dialog.remote_title")}</span>
        </div>

        {/* Saved connections */}
        {savedConns.length > 0 && !showForm && (
          <div className={styles.savedSection}>
            <span className={styles.sectionLabel}>{t("connect_dialog.saved_connections")}</span>
            <div className={styles.savedList}>
              {savedConns.map((c) => (
                <div
                  key={c.id}
                  className={`${styles.savedItem} ${
                    selectedSavedId === c.id ? styles.selectedSaved : ""
                  }`}
                  onClick={() => setSelectedSavedId(c.id)}
                >
                  <div className={styles.savedItemInfo}>
                    <div className={styles.savedItemLabel}>
                      {c.label || (c.sshProfile ? `profile: ${c.sshProfile}` : c.host)}
                    </div>
                    <div className={styles.savedItemHost}>
                      {c.sshProfile
                        ? `ssh ${c.sshProfile}`
                        : `${c.username}@${c.host}:${c.port}`}
                    </div>
                  </div>
                  <button
                    className={styles.deleteBtn}
                    onClick={(e) => handleDeleteSaved(e, c.id)}
                    title={t("connect_dialog.remove_saved")}
                  >
                    ✕
                  </button>
                </div>
              ))}
            </div>
            <button
              className={styles.addNewBtn}
              onClick={() => {
                setShowForm(true);
                setSelectedSavedId(null);
                setForm(emptyConn());
                setConnType("manual");
              }}
            >
              {t("connect_dialog.add_new_remote")}
            </button>
          </div>
        )}

        {/* New connection form */}
        {formVisible && (
          <div className={styles.form}>
            <span className={styles.sectionLabel}>
              {savedConns.length > 0 ? t("connect_dialog.new_connection") : t("connect_dialog.remote_connection")}
            </span>

            {/* Connection type toggle */}
            <div className={styles.connTypeTabs}>
              <button
                className={`${styles.connTypeTab} ${connType === "manual" ? styles.connTypeActive : ""}`}
                onClick={() => setConnType("manual")}
              >
                {t("connect_dialog.manual")}
              </button>
              <button
                className={`${styles.connTypeTab} ${connType === "profile" ? styles.connTypeActive : ""}`}
                onClick={() => setConnType("profile")}
              >
                {t("connect_dialog.ssh_config_profile")}
              </button>
            </div>

            <div className={styles.fieldGroup}>
              <label className={styles.fieldLabel}>{t("connect_dialog.label_optional")}</label>
              <input
                className={styles.fieldInput}
                placeholder={t("connect_dialog.label_placeholder")}
                value={form.label}
                onChange={(e) => setForm((f) => ({ ...f, label: e.target.value }))}
              />
            </div>

            {connType === "profile" ? (
              /* ── SSH Config Profile mode ── */
              <div className={styles.fieldGroup}>
                <label className={styles.fieldLabel}>{t("connect_dialog.profile_name")}</label>
                <input
                  className={styles.fieldInput}
                  list="ssh-profiles-list"
                  placeholder="my-server"
                  value={form.sshProfile ?? ""}
                  onChange={(e) =>
                    setForm((f) => ({ ...f, sshProfile: e.target.value || null }))
                  }
                />
                {sshProfiles.length > 0 && (
                  <datalist id="ssh-profiles-list">
                    {sshProfiles.map((p) => (
                      <option key={p} value={p} />
                    ))}
                  </datalist>
                )}
              </div>
            ) : (
              /* ── Manual mode ── */
              <>
                <div className={styles.formRow}>
                  <div className={styles.fieldGroup} style={{ flex: 3 }}>
                    <label className={styles.fieldLabel}>{t("connect_dialog.host_ip")}</label>
                    <input
                      className={styles.fieldInput}
                      placeholder="192.168.1.100"
                      value={form.host}
                      onChange={(e) => setForm((f) => ({ ...f, host: e.target.value }))}
                    />
                  </div>
                  <div className={styles.fieldGroup} style={{ flex: 1 }}>
                    <label className={styles.fieldLabel}>{t("connect_dialog.ssh_port")}</label>
                    <input
                      className={styles.fieldInput}
                      type="number"
                      placeholder="22"
                      value={form.port}
                      onChange={(e) =>
                        setForm((f) => ({ ...f, port: parseInt(e.target.value) || 22 }))
                      }
                    />
                  </div>
                </div>

                <div className={styles.fieldGroup}>
                  <label className={styles.fieldLabel}>{t("connect_dialog.username")}</label>
                  <input
                    className={styles.fieldInput}
                    placeholder="ubuntu"
                    value={form.username}
                    onChange={(e) => setForm((f) => ({ ...f, username: e.target.value }))}
                  />
                </div>

                <div className={styles.fieldGroup}>
                  <label className={styles.fieldLabel}>
                    {t("connect_dialog.identity_file")}
                  </label>
                  <div className={styles.fileRow}>
                    <input
                      className={styles.fieldInput}
                      placeholder="~/.ssh/id_rsa"
                      value={form.identityFile ?? ""}
                      onChange={(e) =>
                        setForm((f) => ({
                          ...f,
                          identityFile: e.target.value || null,
                        }))
                      }
                    />
                    <button className={styles.browseBtn} onClick={handleBrowseKey}>
                      {t("connect_dialog.browse")}
                    </button>
                  </div>
                </div>

                <div className={styles.fieldGroup}>
                  <label className={styles.fieldLabel}>
                    {t("connect_dialog.jump_host")}
                  </label>
                  <input
                    className={styles.fieldInput}
                    placeholder="user@bastion.example.com"
                    value={form.jumpHost ?? ""}
                    onChange={(e) =>
                      setForm((f) => ({
                        ...f,
                        jumpHost: e.target.value || null,
                      }))
                    }
                  />
                </div>
              </>
            )}

            <div className={styles.formRow}>
              <div className={styles.fieldGroup}>
                <label className={styles.fieldLabel}>{t("connect_dialog.probe_port")}</label>
                <input
                  className={styles.fieldInput}
                  type="number"
                  placeholder="7007"
                  value={form.probePort}
                  onChange={(e) =>
                    setForm((f) => ({
                      ...f,
                      probePort: parseInt(e.target.value) || 7007,
                    }))
                  }
                />
              </div>
            </div>

            {savedConns.length > 0 && (
              <button
                className={styles.btnSecondary}
                style={{ alignSelf: "flex-start" }}
                onClick={() => {
                  setShowForm(false);
                  setSelectedSavedId(savedConns[0].id);
                }}
              >
                {t("connect_dialog.back_to_saved")}
              </button>
            )}
          </div>
        )}

        {/* Progress area */}
        {(connecting || progressSteps.length > 0) && (
          <div className={styles.progress}>
            {connecting && <div className={styles.progressSpinner} />}
            <div>
              {progressSteps.map((step, i) => (
                <div
                  key={i}
                  className={
                    i === progressSteps.length - 1 && connectDone
                      ? styles.progressDone
                      : i === progressSteps.length - 1 && connectError
                      ? styles.progressError
                      : styles.progressText
                  }
                >
                  {step}
                </div>
              ))}
              {connectError && (
                <div className={styles.progressError}>Error: {connectError}</div>
              )}
            </div>
          </div>
        )}

        {/* Actions */}
        <div className={styles.actions}>
          {connectError && (
            <button
              className={styles.btnSecondary}
              onClick={() => {
                setConnectError(null);
                setProgressSteps([]);
                setConnecting(false);
              }}
            >
              {t("connect_dialog.retry")}
            </button>
          )}
          <button
            className={styles.btnPrimary}
            disabled={isConnectDisabled}
            onClick={handleConnectRemote}
          >
            {connecting ? t("connect_dialog.connecting") : t("connect_dialog.connect")}
          </button>
        </div>
      </div>
    </div>
  );
}
