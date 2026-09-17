// The "Paste a pairing link" entry in the pairing gate.
//
// Why this path is essential: native shells rely on App Link to intercept the
// scanned URL, but App Link requires the host to be **hardcoded at compile time**
// in AndroidManifest. A custom relay's host isn't known at compile time, so that
// path is structurally unavailable—scanning only opens the browser, never the app.
// Paste doesn't depend on any host declaration, so it's how custom-relay users
// can get in.
//
// iOS home-screen web apps also use it: there's no address bar, so there's no way
// to open a link with #k= again (see App.tsx pairing-gate comments).
//
// It's also the fallback when the camera is denied or unavailable.

import { useState } from "react";
import { useI18n } from "../i18n";
import { type PairedLink, parsePairingLink } from "../pairingLink";
import styles from "./PairPasteForm.module.css";

export function PairPasteForm({ onPaired }: { onPaired: (paired: PairedLink) => void }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [raw, setRaw] = useState("");
  const [bad, setBad] = useState(false);

  if (!open) {
    return (
      <button className={styles.link} onClick={() => setOpen(true)}>
        {t("改为粘贴配对链接")}
      </button>
    );
  }

  const submit = () => {
    const paired = parsePairingLink(raw);
    if (!paired) {
      setBad(true);
      return;
    }
    onPaired(paired);
  };

  return (
    <div className={styles.form}>
      <p className={styles.hint}>
        {t("在桌面端「移动端」板块点「复制配对链接」，把它贴到这里。摄像头用不了、或链接是从别的设备发过来的，都走这条。")}
      </p>
      <textarea
        className={styles.input}
        value={raw}
        onChange={(e) => {
          setRaw(e.target.value);
          setBad(false);
        }}
        placeholder="https://relay.example.com/#k=…"
        rows={3}
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
      />
      {bad && <p className={styles.error}>{t("这不像一条配对链接。它应该形如 https://<你的 relay>/#k=<密钥>。")}</p>}
      <div className={styles.actions}>
        <button className={styles.cancel} onClick={() => setOpen(false)}>
          {t("取消")}
        </button>
        <button className={styles.submit} disabled={!raw.trim()} onClick={submit}>
          {t("配对")}
        </button>
      </div>
    </div>
  );
}
