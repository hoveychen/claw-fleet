import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { useI18n } from "./i18n";
import styles from "./ConfirmDialog.module.css";

export interface ConfirmPrompt {
  message: string;
}

interface PendingConfirm extends ConfirmPrompt {
  resolve: (answer: boolean) => void;
}

export interface ConfirmController {
  request: (message: string) => Promise<boolean>;
  settle: (answer: boolean) => void;
  current: () => ConfirmPrompt | null;
  subscribe: (listener: () => void) => () => void;
  cancelAll: () => void;
}

/** FIFO keeps two destructive actions from overwriting each other's resolver. */
export function createConfirmController(): ConfirmController {
  const queue: PendingConfirm[] = [];
  const listeners = new Set<() => void>();
  let snapshot: ConfirmPrompt | null = null;

  const publish = () => {
    snapshot = queue[0] ? { message: queue[0].message } : null;
    for (const listener of listeners) listener();
  };

  return {
    request(message) {
      return new Promise<boolean>((resolve) => {
        queue.push({ message, resolve });
        if (queue.length === 1) publish();
      });
    },
    settle(answer) {
      const pending = queue.shift();
      if (!pending) return;
      pending.resolve(answer);
      publish();
    },
    current: () => snapshot,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    cancelAll() {
      const pending = queue.splice(0);
      for (const item of pending) item.resolve(false);
      if (pending.length > 0) publish();
    },
  };
}

type Confirm = (message: string) => Promise<boolean>;

const ConfirmContext = createContext<Confirm | null>(null);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [controller] = useState(createConfirmController);
  const prompt = useSyncExternalStore(
    controller.subscribe,
    controller.current,
    controller.current,
  );
  const { t } = useI18n();
  const settle = useCallback((answer: boolean) => controller.settle(answer), [controller]);

  useEffect(() => () => controller.cancelAll(), [controller]);

  useEffect(() => {
    if (!prompt) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") settle(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [prompt, settle]);

  return (
    <ConfirmContext.Provider value={controller.request}>
      {children}
      {prompt && (
        <div className={styles.overlay} onClick={() => settle(false)}>
          <div
            className={styles.dialog}
            role="alertdialog"
            aria-modal="true"
            aria-describedby="fleet-confirm-message"
            onClick={(event) => event.stopPropagation()}
          >
            <p id="fleet-confirm-message" className={styles.message}>
              {prompt.message}
            </p>
            <div className={styles.actions}>
              <button className={styles.cancel} onClick={() => settle(false)}>
                {t("取消")}
              </button>
              <button className={styles.confirm} onClick={() => settle(true)} autoFocus>
                {t("确认")}
              </button>
            </div>
          </div>
        </div>
      )}
    </ConfirmContext.Provider>
  );
}

export function useConfirm(): Confirm {
  const confirm = useContext(ConfirmContext);
  if (!confirm) throw new Error("useConfirm must be used inside ConfirmProvider");
  return confirm;
}
