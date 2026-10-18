import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../store";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { mergeProjects, pathTail, type ProjectRow, type RegisteredProject } from "../projectList";
import { Spinner } from "./loading";
import styles from "./Onboarding.module.css";

/** How many recent directories the first-run card offers as projects. */
export const MAX_PROJECT_CANDIDATES = 6;

/**
 * Project candidates for the first-run card: the same merge the Projects page
 * shows (registered projects plus the repo root of every scanned session),
 * most recently active first, capped.
 */
export function projectCandidates(projects: readonly ProjectRow[]): ProjectRow[] {
  return projects.slice(0, MAX_PROJECT_CANDIDATES);
}

/**
 * First-run "What should get done?" card. Shown once the environment check
 * passes, in place of the old "all set, Fleet is monitoring" card: the first
 * thing a new user does is hand Fleet a task, not read about a dashboard.
 * Submitting closes onboarding and opens the new-task composer seeded with the
 * words and the picked directory (no pick → the composer asks for one).
 */
export function OnboardingLaunch({
  onLaunch,
  launching,
}: {
  onLaunch: (prompt: string, workspace: string | null) => void;
  launching: boolean;
}) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const chatWorkspace = useChatWorkspace();
  const [registered, setRegistered] = useState<RegisteredProject[] | null>(null);
  useEffect(() => {
    invoke<RegisteredProject[]>("list_projects")
      .then((rows) => setRegistered(rows ?? []))
      .catch(() => setRegistered([]));
  }, []);

  const candidates = useMemo(
    () => (registered ? projectCandidates(mergeProjects(registered, sessions, chatWorkspace)) : null),
    [registered, sessions, chatWorkspace],
  );

  const [text, setText] = useState("");
  // `undefined` = the user hasn't picked yet, so the most recent project is
  // preselected; `null` = explicitly "choose later".
  const [picked, setPicked] = useState<string | null | undefined>(undefined);
  const workspace = picked === undefined ? (candidates?.[0]?.path ?? null) : picked;

  const submit = () => {
    if (launching) return;
    onLaunch(text.trim(), workspace);
  };

  return (
    <div className={`${styles.card} ${styles.card_launch}`}>
      <div className={styles.card_header}>
        <span className={styles.card_title}>{t("onboarding.launch.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.launch.description")}</p>
      <div className={styles.launch}>
        <input
          type="text"
          className={styles.launch_input}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // An IME's Enter commits the composition, not the task.
            if (e.key === "Enter" && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder={t("home.launch_placeholder")}
          aria-label={t("home.launch_placeholder")}
          spellCheck={false}
          autoFocus
        />
        <button
          type="button"
          className={styles.btn_primary}
          onClick={submit}
          disabled={launching}
          data-testid="onboarding-launch"
        >
          {launching && <><Spinner size={12} />{" "}</>}
          {t("onboarding.launch.start")}
        </button>
      </div>
      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("onboarding.launch.projects_label")}</span>
        {candidates === null ? (
          <span className={styles.hint}>{t("onboarding.launch.projects_loading")}</span>
        ) : candidates.length === 0 ? (
          <span className={styles.hint}>{t("onboarding.launch.projects_empty")}</span>
        ) : (
          <div className={styles.project_chips} role="radiogroup">
            {candidates.map((p) => (
              <button
                key={p.path}
                type="button"
                role="radio"
                aria-checked={workspace === p.path}
                className={`${styles.project_chip} ${workspace === p.path ? styles.project_chip_active : ""}`}
                onClick={() => setPicked(p.path)}
                title={p.path}
              >
                <span className={styles.project_chip_name}>{p.name}</span>
                <span className={styles.project_chip_path}>{pathTail(p.path)}</span>
              </button>
            ))}
            <button
              type="button"
              role="radio"
              aria-checked={workspace === null}
              className={`${styles.project_chip} ${workspace === null ? styles.project_chip_active : ""}`}
              onClick={() => setPicked(null)}
            >
              <span className={styles.project_chip_name}>{t("onboarding.launch.choose_later")}</span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
