/// Hardware/gesture back dispatcher — the native mirror of mobile-web's NavStack
/// + useNavStack. The browser client leans on `window.history` + popstate; the
/// atomic service instead routes the system back event through the @Entry page's
/// `onBackPress()` into `handleBack()` here.
///
/// Model: every full-screen overlay (会话详情 / 知识库文档 / 仓库 / 用量 / 新会话
/// sheet …) registers ONE layer on mount and drops it on unmount. A layer's
/// `onBack` closure does the right thing for that overlay — and because a layer
/// can hold internal sub-state (WikiDocView's `[[slug]]` doc chain, Composer's
/// DirPicker), its `onBack` may resolve one internal level at a time while the
/// component (and thus its layer) stays mounted; the layer is only removed when
/// the component actually unmounts. So `handleBack()` deliberately does NOT pop
/// the layer itself — it just fires the top layer's `onBack`.
///
/// When no overlays remain, the root handler (owned by MainShell) decides: not on
/// the 决策 tab → return to it; on the 决策 tab → arm "再按一次返回退出", then let
/// the system exit on the second press. Returning `false` from `handleBack()`
/// yields to the platform default (background/exit).

interface BackLayer {
  id: number;
  onBack: () => void;
}

let layers: BackLayer[] = [];
let nextId: number = 1;
/// Root handler: returns true when it consumed the back (tab switch / exit arm),
/// false to let the system exit. Defaults to "leave" (e.g. the pairing gate).
let rootHandler: () => boolean = () => false;

/// Register an overlay layer; returns the id used to drop it on unmount.
export function pushBackLayer(onBack: () => void): number {
  const id = nextId;
  nextId += 1;
  layers.push({ id, onBack });
  return id;
}

/// Remove a layer on component unmount. No-op if already gone.
export function dropBackLayer(id: number): void {
  const i = layers.findIndex((l: BackLayer) => l.id === id);
  if (i !== -1) {
    layers.splice(i, 1);
  }
}

export function setRootBackHandler(fn: () => boolean): void {
  rootHandler = fn;
}

/// Called from Index.onBackPress. Returns true when the back was consumed (do
/// not exit), false to let the platform handle it (exit/background the app).
export function handleBack(): boolean {
  if (layers.length > 0) {
    const top = layers[layers.length - 1];
    top.onBack();
    return true;
  }
  return rootHandler();
}
