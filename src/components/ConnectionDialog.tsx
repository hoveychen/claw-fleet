import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import styles from "./ConnectionDialog.module.css";

// ── Types mirroring Rust structs ─────────────────────────────────────────────

export interface RemoteConnection {
  id: string;
  label: string;
  host: string;
  port: number;
  username: string;
  identityFile: string | null;
  probePort: number;
}

interface ConnectProgress {
  step: string;
  done: boolean;
  error: string | null;
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
    probePort: 7007,
  };
}

// ── Component ────────────────────────────────────────────────────────────────

interface Props {
  /** Called when connection is established. `null` = local mode. */
  onConnected: (remote: RemoteConnection | null) => void;
}

type Mode = "local" | "remote";

export function ConnectionDialog({ onConnected }: Props) {
  const [mode, setMode] = useState<Mode>("local");
  const [savedConns, setSavedConns] = useState<RemoteConnection[]>([]);
  const [selectedSavedId, setSelectedSavedId] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);

  // Form state for a new connection
  const [form, setForm] = useState<RemoteConnection>(emptyConn);

  // Connection in progress
  const [connecting, setConnecting] = useState(false);
  const [progressSteps, setProgressSteps] = useState<string[]>([]);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [connectDone, setConnectDone] = useState(false);

  const unlistenRef = useRef<(() => void) | null>(null);

  // Load saved connections on mount
  useEffect(() => {
    invoke<RemoteConnection[]>("list_saved_connections").then((conns) => {
      setSavedConns(conns);
      if (conns.length > 0) {
        // Pre-select the first saved connection and switch to remote mode
        setMode("remote");
        setSelectedSavedId(conns[0].id);
      }
    });
  }, []);

  // Listen for progress events
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<ConnectProgress>("remote-connect-progress", (event) => {
      const p = event.payload;
      setProgressSteps((prev) => [...prev, p.step]);
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
      onConnected(null);
      return;
    }

    // Determine which connection to use
    let conn: RemoteConnection;
    if (selectedSavedId && !showForm) {
      const found = savedConns.find((c) => c.id === selectedSavedId);
      if (!found) return;
      conn = found;
    } else {
      if (!form.host || !form.username) {
        setConnectError("Host and Username are required.");
        setConnecting(false);
        return;
      }
      conn = { ...form };
    }

    try {
      await invoke("connect_remote", { conn });
      // Success is signaled by the progress event with done:true
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setConnectError(msg);
      setConnecting(false);
    }
  }, [mode, selectedSavedId, showForm, savedConns, form, onConnected]);

  // When connectDone fires, close after a short delay
  useEffect(() => {
    if (connectDone) {
      let conn: RemoteConnection | null = null;
      if (selectedSavedId && !showForm) {
        conn = savedConns.find((c) => c.id === selectedSavedId) ?? null;
      } else if (showForm || savedConns.length === 0) {
        conn = form;
      }
      const t = setTimeout(() => onConnected(conn), 600);
      return () => clearTimeout(t);
    }
  }, [connectDone, onConnected, selectedSavedId, showForm, savedConns, form]);

  const isConnectDisabled =
    connecting ||
    (mode === "remote" && !selectedSavedId && !showForm) ||
    (mode === "remote" &&
      showForm &&
      (!form.host.trim() || !form.username.trim()));

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
                          {c.label || c.host}
                        </div>
                        <div className={styles.savedItemHost}>
                          {c.username}@{c.host}:{c.port}
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
                  }}
                >
                  + Add New Remote
                </button>
              </div>
            )}

            {/* New connection form */}
            {(showForm || savedConns.length === 0) && (
              <div className={styles.form}>
                <span className={styles.sectionLabel}>
                  {savedConns.length > 0 ? "New Connection" : "Remote Connection"}
                </span>

                <div className={styles.fieldGroup}>
                  <label className={styles.fieldLabel}>Label (optional)</label>
                  <input
                    className={styles.fieldInput}
                    placeholder="e.g. My Dev Server"
                    value={form.label}
                    onChange={(e) => setForm((f) => ({ ...f, label: e.target.value }))}
                  />
                </div>

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
