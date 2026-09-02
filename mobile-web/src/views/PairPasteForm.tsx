// 配对门里的「粘贴配对链接」入口。
//
// 为什么必须有这条路:原生壳靠 App Link 接住扫码的 URL,而 App Link 要求在
// AndroidManifest 里**编译期**写死 host。自建 relay 的 host 编译期不可知,所以
// 那条路对它结构上不可用——扫码只会打开浏览器,永远进不了 app。粘贴不依赖任何
// host 声明,是自建 relay 用户进得来的那一条。
//
// 它同时是相机被拒/不可用时扫码的兜底。

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
        {t("在桌面端「移动端」板块点「复制配对链接」，把它贴到这里。自建 relay 只能走这条路——二维码扫出来的链接系统交不到 app 手上。")}
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
