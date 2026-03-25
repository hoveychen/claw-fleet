import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./AccountInfo.module.css";

interface AccountInfoData {
  email: string;
  full_name: string;
  organization_name: string;
  plan: string;
  auth_method: string;
  five_hour: unknown;
  seven_day: unknown;
  seven_day_sonnet: unknown;
}

interface OpenClawProviderInfo {
  provider: string;
  authType: string;
  status: string;
  label: string;
  expiresAt: number | null;
  remainingMs: number | null;
}

interface OpenClawAccountInfoData {
  version: string;
  defaultModel: string;
  providers: OpenClawProviderInfo[];
}

interface CursorAccountInfoData {
  email: string;
  signUpType: string;
  membershipType: string;
  subscriptionStatus: string;
  totalPrompts: number;
}

interface CodexUsageData {
  planType?: string;
  limitName?: string;
}

// ── Main AccountInfo panel ───────────────────────────────────────────────────

export function AccountInfo({ embedded }: { embedded?: boolean } = {}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const [isMacOS, setIsMacOS] = useState(false);
  const [showAiModal, setShowAiModal] = useState(false);
  const [cliInstallState, setCliInstallState] = useState<"idle" | "installing" | "done" | "error">("idle");
  const [cliInstallMsg, setCliInstallMsg] = useState<string | null>(null);

  // Account info state
  const [info, setInfo] = useState<AccountInfoData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // OpenClaw account state
  const [openclawInfo, setOpenclawInfo] = useState<OpenClawAccountInfoData | null>(null);
  const [openclawError, setOpenclawError] = useState<string | null>(null);
  const [openclawLoading, setOpenclawLoading] = useState(false);
  const [hasOpenclaw, setHasOpenclaw] = useState(false);

  // Cursor account state
  const [cursorInfo, setCursorInfo] = useState<CursorAccountInfoData | null>(null);
  const [cursorError, setCursorError] = useState<string | null>(null);
  const [cursorLoading, setCursorLoading] = useState(false);
  const [hasCursor, setHasCursor] = useState(false);

  // Codex account state
  const [codexInfo, setCodexInfo] = useState<CodexUsageData | null>(null);
  const [codexError, setCodexError] = useState<string | null>(null);
  const [codexLoading, setCodexLoading] = useState(false);
  const [hasCodex, setHasCodex] = useState(false);

  useEffect(() => {
    invoke<string>("get_platform").then((p) => setIsMacOS(p === "macos"));
    loadAccount();
    invoke<{ detected_tools: { openclaw: boolean; cursor: boolean; codex: boolean } }>("check_setup_status")
      .then((s) => {
        if (s.detected_tools.openclaw) {
          setHasOpenclaw(true);
          loadOpenclawAccount();
        }
        if (s.detected_tools.cursor) {
          setHasCursor(true);
          loadCursorAccount();
        }
        if (s.detected_tools.codex) {
          setHasCodex(true);
          loadCodexAccount();
        }
      })
      .catch(() => {});
  }, []);

  async function loadAccount() {
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<AccountInfoData>("get_account_info");
      setInfo(data);
    } catch (e) {
      setError(String(e));
      if (!logPath) {
        invoke<string>("get_log_path").then(setLogPath).catch(() => {});
      }
    } finally {
      setLoading(false);
    }
  }

  async function loadOpenclawAccount() {
    setOpenclawLoading(true);
    setOpenclawError(null);
    try {
      const data = await invoke<OpenClawAccountInfoData>("get_source_account", { source: "openclaw" });
      setOpenclawInfo(data);
    } catch (e) {
      setOpenclawError(String(e));
    } finally {
      setOpenclawLoading(false);
    }
  }

  async function loadCursorAccount() {
    setCursorLoading(true);
    setCursorError(null);
    try {
      const data = await invoke<CursorAccountInfoData>("get_source_account", { source: "cursor" });
      setCursorInfo(data);
    } catch (e) {
      setCursorError(String(e));
    } finally {
      setCursorLoading(false);
    }
  }

  async function loadCodexAccount() {
    setCodexLoading(true);
    setCodexError(null);
    try {
      // Codex has no separate account endpoint; plan info is in usage data
      const data = await invoke<CodexUsageData>("get_source_usage", { source: "codex" });
      setCodexInfo(data);
    } catch (e) {
      setCodexError(String(e));
    } finally {
      setCodexLoading(false);
    }
  }

  async function installCLI() {
    setCliInstallState("installing");
    setCliInstallMsg(null);
    try {
      const path = await invoke<string>("install_fleet_cli");
      setCliInstallState("done");
      setCliInstallMsg(t("account.cli_installed", { path }));
    } catch (e) {
      setCliInstallState("error");
      setCliInstallMsg(String(e));
    }
  }

  const panelContent = (
    <>
      {loading && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          {logPath && <p className={styles.log_hint}>{t("account.debug_log", { path: logPath })}</p>}
          <button className={styles.retry} onClick={loadAccount}>{t("account.retry")}</button>
        </div>
      )}
      {info && (
        <section className={styles.section}>
          <Row label={t("account.auth")} value="Claude AI" />
          <Row label={t("account.email")} value={info.email} />
          <Row label={t("account.org")} value={info.organization_name} />
          <Row label={t("account.plan")} value={info.plan} />
        </section>
      )}

      {hasOpenclaw && (
        <>
          <div className={styles.section_divider} />
          {openclawLoading && <p className={styles.dim}>{t("account.loading")}</p>}
          {openclawError && (
            <div className={styles.error}>
              <p>{openclawError}</p>
              <button className={styles.retry} onClick={loadOpenclawAccount}>{t("account.retry")}</button>
            </div>
          )}
          {openclawInfo && (
            <section className={styles.section}>
              <Row label={t("account.auth")} value="OpenClaw" />
              <Row label={t("account.openclaw_version")} value={openclawInfo.version} />
              <Row label={t("account.openclaw_model")} value={openclawInfo.defaultModel} />
              {openclawInfo.providers.map((p) => (
                <Row
                  key={p.label}
                  label={p.provider}
                  value={`${p.authType} (${p.status})${p.remainingMs != null ? ` — ${Math.round(p.remainingMs / 3600000)}h left` : ""}`}
                />
              ))}
            </section>
          )}
        </>
      )}

      {hasCursor && (
        <>
          <div className={styles.section_divider} />
          {cursorLoading && <p className={styles.dim}>{t("account.loading")}</p>}
          {cursorError && (
            <div className={styles.error}>
              <p>{cursorError}</p>
              <button className={styles.retry} onClick={loadCursorAccount}>{t("account.retry")}</button>
            </div>
          )}
          {cursorInfo && (
            <section className={styles.section}>
              <Row label={t("account.auth")} value="Cursor" />
              <Row label={t("account.email")} value={cursorInfo.email} />
              <Row label={t("account.cursor_plan")} value={cursorInfo.membershipType} />
              <Row label={t("account.cursor_status")} value={cursorInfo.subscriptionStatus} />
              <Row label={t("account.cursor_sign_up")} value={cursorInfo.signUpType} />
              <Row label={t("account.cursor_prompts")} value={String(cursorInfo.totalPrompts)} />
            </section>
          )}
        </>
      )}

      {hasCodex && (
        <>
          <div className={styles.section_divider} />
          {codexLoading && <p className={styles.dim}>{t("account.loading")}</p>}
          {codexError && (
            <div className={styles.error}>
              <p>{codexError}</p>
              <button className={styles.retry} onClick={loadCodexAccount}>{t("account.retry")}</button>
            </div>
          )}
          {codexInfo && (
            <section className={styles.section}>
              <Row label={t("account.auth")} value="Codex" />
              {codexInfo.planType && <Row label={t("account.plan")} value={codexInfo.planType} />}
              {codexInfo.limitName && <Row label={t("account.codex_limit")} value={codexInfo.limitName} />}
            </section>
          )}
        </>
      )}

      <div className={styles.cli_section}>
        <button
          className={styles.cli_install_btn}
          onClick={() => setShowAiModal(true)}
          title={t("account.ai_btn_hint")}
        >
          {t("account.ai_btn")}
        </button>
      </div>
    </>
  );

  // Embedded mode: no collapsible wrapper, render content directly
  if (embedded) {
    return (
      <>
        <div className={styles.panel}>{panelContent}</div>
        {showAiModal && (
          <AiSetupModal
            onClose={() => setShowAiModal(false)}
            isMacOS={isMacOS}
            cliInstallState={cliInstallState}
            cliInstallMsg={cliInstallMsg}
            onInstallCLI={installCLI}
          />
        )}
      </>
    );
  }

  return (
    <div className={styles.container}>
      <button
        className={styles.toggle}
        onClick={() => setExpanded((v) => !v)}
      >
        <span className={styles.toggle_label}>{t("account.panel_title")}</span>
        <span className={styles.toggle_icon}>{expanded ? "\u25B2" : "\u25BC"}</span>
      </button>

      {expanded && <div className={styles.panel}>{panelContent}</div>}
      {showAiModal && (
        <AiSetupModal
          onClose={() => setShowAiModal(false)}
          isMacOS={isMacOS}
          cliInstallState={cliInstallState}
          cliInstallMsg={cliInstallMsg}
          onInstallCLI={installCLI}
        />
      )}
    </div>
  );
}

// ── AI Setup Modal ────────────────────────────────────────────────────────────

interface DetectedTool {
  name: string;
  skill_path: string;
}

interface SkillInstallResult {
  installed: DetectedTool[];
  errors: string[];
}

interface AiSetupModalProps {
  onClose: () => void;
  isMacOS: boolean;
  cliInstallState: "idle" | "installing" | "done" | "error";
  cliInstallMsg: string | null;
  onInstallCLI: () => void;
}

function AiSetupModal({ onClose, isMacOS, cliInstallState, cliInstallMsg, onInstallCLI }: AiSetupModalProps) {
  const { t } = useTranslation();
  const [detectedTools, setDetectedTools] = useState<DetectedTool[] | null>(null);
  const [skillState, setSkillState] = useState<"idle" | "installing" | "done" | "error">("idle");
  const [installResult, setInstallResult] = useState<SkillInstallResult | null>(null);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);

  // Detect tools on mount
  useEffect(() => {
    invoke<DetectedTool[]>("detect_ai_tools")
      .then(setDetectedTools)
      .catch(() => setDetectedTools([]));
  }, []);

  async function saveSkillFile() {
    setSaveMsg(null);
    try {
      const path = await invoke<string>("save_skill_file");
      setSaveMsg(path);
    } catch (e) {
      if (String(e) !== "cancelled") setSaveMsg("✗ " + String(e));
    }
  }

  async function installSkill() {
    setSkillState("installing");
    try {
      const result = await invoke<SkillInstallResult>("install_fleet_skill");
      setInstallResult(result);
      setSkillState(result.installed.length > 0 ? "done" : "error");
    } catch (e) {
      setInstallResult({ installed: [], errors: [String(e)] });
      setSkillState("error");
    }
  }

  const noToolsDetected = detectedTools !== null && detectedTools.length === 0;

  return (
    <div className={styles.modal_overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.modal_header}>
          <span className={styles.modal_icon}>🤖</span>
          <h3 className={styles.modal_title}>{t("account.ai_modal_title")}</h3>
        </div>
        <p className={styles.modal_desc}>{t("account.ai_modal_desc")}</p>

        {/* Step 1: CLI in PATH */}
        <div className={styles.modal_step}>
          <div className={styles.step_label}>
            <span className={styles.step_num}>1</span>
            <span className={styles.step_title}>{t("account.ai_step1_title")}</span>
          </div>
          <p className={styles.step_desc}>{t("account.ai_step1_desc")}</p>
          {isMacOS ? (
            <div className={styles.step_action}>
              <button
                className={styles.step_btn}
                onClick={onInstallCLI}
                disabled={cliInstallState === "installing" || cliInstallState === "done"}
              >
                {cliInstallState === "installing"
                  ? t("account.cli_installing")
                  : cliInstallState === "done"
                  ? "✓ " + t("account.cli_installed_btn")
                  : t("account.cli_install_btn")}
              </button>
              {cliInstallMsg && (
                <span className={cliInstallState === "done" ? styles.step_ok : styles.step_err}>
                  {cliInstallMsg}
                </span>
              )}
            </div>
          ) : (
            <p className={styles.step_hint}>{t("account.ai_step1_other")}</p>
          )}
        </div>

        {/* Step 2: Install skill */}
        <div className={styles.modal_step}>
          <div className={styles.step_label}>
            <span className={styles.step_num}>2</span>
            <span className={styles.step_title}>{t("account.ai_step2_title")}</span>
          </div>
          <p className={styles.step_desc}>{t("account.ai_step2_desc")}</p>

          {/* Results after install */}
          {installResult && (
            <div className={styles.tool_list}>
              {installResult.installed.map((tool) => (
                <div key={tool.name} className={styles.tool_row}>
                  <span className={styles.step_ok}>✓ {tool.name}</span>
                  <span className={styles.tool_path}>{tool.skill_path}</span>
                </div>
              ))}
              {installResult.errors.map((err, i) => (
                <p key={i} className={styles.step_err}>{err}</p>
              ))}
            </div>
          )}

          {!installResult && noToolsDetected && (
            <p className={styles.step_hint}>{t("account.ai_no_tools")}</p>
          )}

          <div className={styles.step_action}>
            <button
              className={styles.step_btn}
              onClick={installSkill}
              disabled={noToolsDetected || skillState === "installing" || skillState === "done"}
            >
              {skillState === "installing"
                ? t("account.ai_skill_installing")
                : skillState === "done"
                ? "✓ " + t("account.ai_skill_installed_btn")
                : t("account.ai_skill_install_btn")}
            </button>
            <button className={styles.step_btn_secondary} onClick={saveSkillFile}>
              {t("account.ai_skill_save_btn")}
            </button>
          </div>
          {saveMsg && (
            <span className={saveMsg.startsWith("✗") ? styles.step_err : styles.step_ok}>
              {saveMsg.startsWith("✗") ? saveMsg : "✓ " + saveMsg}
            </span>
          )}
        </div>

        <div className={styles.modal_footer}>
          <button className={styles.modal_close_btn} onClick={onClose}>
            {t("account.ai_modal_close")}
          </button>
        </div>
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.row}>
      <span className={styles.row_label}>{label}</span>
      <span className={styles.row_value}>{value}</span>
    </div>
  );
}
