import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
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

type Mode = "local" | "remote";
type ConnType = "manual" | "profile";

export function ConnectionDialog({ onConnected }: Props) {
  const [mode, setMode] = useState<Mode>("local");
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
  // If local AI tools are detected, prefer local mode even when saved remotes exist.
  useEffect(() => {
    Promise.all([
      invoke<RemoteConnection[]>("list_saved_connections"),
      invoke<{ cli_installed: boolean; claude_dir_exists: boolean; has_sessions: boolean }>("check_setup_status").catch(() => null),
    ]).then(([conns, status]) => {
      setSavedConns(conns);
      const hasLocalAI = status && (status.cli_installed || status.claude_dir_exists || status.has_sessions);
      if (conns.length > 0 && !hasLocalAI) {
        setMode("remote");
        setSelectedSavedId(conns[0].id);
      }
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
      title: "Select SSH Identity File",
    });
    if (selected) {
      setForm((f) => ({ ...f, identityFile: selected }));
    }
  }, []);

  const handleConnect = useCallback(async () => {
    setProgressSteps([]);
    setConnectError(null);
    setConnectDone(false);
    setConnecting(true);

    if (mode === "local") {
      onConnected({ type: "local" });
      return;
    }

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
          setConnectError("SSH profile name is required.");
          setConnecting(false);
          return;
        }
        conn = { ...form, host: "", username: "", port: 22 };
      } else {
        if (!form.host || !form.username) {
          setConnectError("Host and Username are required.");
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
  }, [mode, selectedSavedId, showForm, savedConns, form, connType, onConnected]);

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
    (mode === "remote" && !selectedSavedId && !formVisible) ||
    (mode === "remote" && formVisible && !selectedSavedId && !formValid);

  return (
    <div className={styles.overlay}>
      <div className={styles.dialog}>
        {/* Header */}
        <div className={styles.header}>
          <span className={styles.icon}>⚓</span>
          <span className={styles.title}>Connect to Claude Fleet</span>
        </div>

        {/* Mode selector */}
        <div className={styles.modeTabs}>
          <button
            className={`${styles.modeTab} ${mode === "local" ? styles.active : ""}`}
            onClick={() => {
              setMode("local");
              setConnectError(null);
              setProgressSteps([]);
            }}
          >
            <span className={styles.modeIcon}>💻</span>
            Local
          </button>
          <button
            className={`${styles.modeTab} ${mode === "remote" ? styles.active : ""}`}
            onClick={() => {
              setMode("remote");
              setConnectError(null);
              setProgressSteps([]);
            }}
          >
            <span className={styles.modeIcon}>🌐</span>
            Remote (SSH)
          </button>
        </div>

        {/* Remote section */}
        {mode === "remote" && (
          <>
            {/* Saved connections */}
            {savedConns.length > 0 && !showForm && (
              <div className={styles.savedSection}>
                <span className={styles.sectionLabel}>Saved Connections</span>
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
                        title="Remove saved connection"
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
                  + Add New Remote
                </button>
              </div>
            )}

            {/* New connection form */}
            {formVisible && (
              <div className={styles.form}>
                <span className={styles.sectionLabel}>
                  {savedConns.length > 0 ? "New Connection" : "Remote Connection"}
                </span>

                {/* Connection type toggle */}
                <div className={styles.connTypeTabs}>
                  <button
                    className={`${styles.connTypeTab} ${connType === "manual" ? styles.connTypeActive : ""}`}
                    onClick={() => setConnType("manual")}
                  >
                    Manual
                  </button>
                  <button
                    className={`${styles.connTypeTab} ${connType === "profile" ? styles.connTypeActive : ""}`}
                    onClick={() => setConnType("profile")}
                  >
                    SSH Config Profile
                  </button>
                </div>

                <div className={styles.fieldGroup}>
                  <label className={styles.fieldLabel}>Label (optional)</label>
                  <input
                    className={styles.fieldInput}
                    placeholder="e.g. My Dev Server"
                    value={form.label}
                    onChange={(e) => setForm((f) => ({ ...f, label: e.target.value }))}
                  />
                </div>

                {connType === "profile" ? (
                  /* ── SSH Config Profile mode ── */
                  <div className={styles.fieldGroup}>
                    <label className={styles.fieldLabel}>Profile Name</label>
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
                        <label className={styles.fieldLabel}>Host / IP</label>
                        <input
                          className={styles.fieldInput}
                          placeholder="192.168.1.100"
                          value={form.host}
                          onChange={(e) => setForm((f) => ({ ...f, host: e.target.value }))}
                        />
                      </div>
                      <div className={styles.fieldGroup} style={{ flex: 1 }}>
                        <label className={styles.fieldLabel}>SSH Port</label>
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
                      <label className={styles.fieldLabel}>Username</label>
                      <input
                        className={styles.fieldInput}
                        placeholder="ubuntu"
                        value={form.username}
                        onChange={(e) => setForm((f) => ({ ...f, username: e.target.value }))}
                      />
                    </div>

                    <div className={styles.fieldGroup}>
                      <label className={styles.fieldLabel}>
                        Identity File (optional)
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
                          Browse…
                        </button>
                      </div>
                    </div>

                    <div className={styles.fieldGroup}>
                      <label className={styles.fieldLabel}>
                        Jump Host (optional)
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
                    <label className={styles.fieldLabel}>Probe Port</label>
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
                    ← Back to saved
                  </button>
                )}
              </div>
            )}
          </>
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
          {mode === "local" ? (
            <button className={styles.btnPrimary} onClick={handleConnect}>
              Connect Locally
            </button>
          ) : (
            <>
              {connectError && (
                <button
                  className={styles.btnSecondary}
                  onClick={() => {
                    setConnectError(null);
                    setProgressSteps([]);
                    setConnecting(false);
                  }}
                >
                  Retry
                </button>
              )}
              <button
                className={styles.btnPrimary}
                disabled={isConnectDisabled}
                onClick={handleConnect}
              >
                {connecting ? "Connecting…" : "Connect"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
