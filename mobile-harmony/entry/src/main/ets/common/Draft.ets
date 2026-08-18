/// 未提交表单草稿的持久化底座 —— native 镜像 mobile-web/src/draft.ts 的 localStorage 用法。
/// 新会话 sheet 输入到一半的内容,在意外关闭 sheet / 切标签 / 系统杀进程后不该丢:
/// 落到 PersistentStorage(与 themeMode 见 Theme.ets、lang 见 I18n.ets 同机制),重开自动
/// 恢复,提交成功清空。
///
/// 本模块只负责「按 key 存取一个字符串」——序列化/浅合并交给调用方(Composer 自己把它的
/// 具体草稿结构 JSON 化)。这样底座与草稿形状解耦,后续别的表单也能复用。
///
/// 存储形态:每个 key 对应 PersistentStorage 里一个字符串槽。`persistProp` 幂等地把
/// `key→磁盘` 绑定起来(默认值只在首次生效,之后取磁盘值),之后 `AppStorage.setOrCreate`
/// 即落盘。空串视作「无草稿」。

const PREFIX = 'fleet-draft:';

/// 幂等地确保某 key 已绑定持久化。重复调用无副作用——`persistProp` 见到 key 已在磁盘时
/// 会沿用磁盘值、忽略这里给的默认值,所以先前写入的草稿不会被 '' 覆盖。
function ensure(key: string): void {
  PersistentStorage.persistProp(PREFIX + key, '');
}

/// 读回原始字符串;从未写过 / 已清空(空串)→ null。解析交给调用方。
export function loadDraftRaw(key: string): string | null {
  ensure(key);
  const raw = AppStorage.get<string>(PREFIX + key);
  return (raw === undefined || raw === null || raw.length === 0) ? null : raw;
}

/// 写入原始字符串(通常是 JSON.stringify 的结果)。落盘 best-effort。
export function saveDraftRaw(key: string, raw: string): void {
  ensure(key);
  AppStorage.setOrCreate(PREFIX + key, raw);
}

/// 清空草稿(写回空串,等同「无草稿」)。提交成功后调用。
export function clearDraft(key: string): void {
  ensure(key);
  AppStorage.setOrCreate(PREFIX + key, '');
}
