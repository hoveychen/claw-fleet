// 添加一台「HTTP 直连主机」之前先探一下。
//
// 不探的后果不是「少一点确认感」:填错的那台会安静地躺在设备列表里,永远停在
// 「连接中…」——而失败的原因(地址不对 / token 不对 / 那台没开放跨源 / 混合内容
// 被浏览器拦)在界面上完全看不出来,只能猜。这几种原因的修法各不相同,所以值得
// 在**入册之前**就把它们区分开告诉用户。
//
// 浏览器有一个躲不开的限制:**跨源被拒与网络不通在 JS 里长得一模一样**。fetch 对
// 一个缺 CORS 头的响应抛的就是普通 TypeError,拿不到状态码也拿不到原因(那是刻意
// 的隐私设计)。所以那一档只能如实说「连不上,或那台没开放跨源」,不能假装分得清。

/** 探测结果。`ok` 之外的每一种都对应一句能照着做的话。 */
export type HostProbeResult =
  | { ok: true }
  /** 地址不是绝对 http(s) URL。 */
  | { ok: false; reason: "not-a-url" }
  /** https 页面去连明文 http —— 浏览器直接封杀(localhost 除外)。 */
  | { ok: false; reason: "mixed-content" }
  /** 连不上,或那台主机没有对这个 origin 开放跨源。两者在浏览器里无法区分。 */
  | { ok: false; reason: "unreachable"; detail: string }
  /** 连上了,但 token 不对(或那台要 token 而这里没给)。 */
  | { ok: false; reason: "unauthorized"; status: number }
  /** 连上了,但那不是一台 Fleet 主机(没有 /mobile_rpc)。 */
  | { ok: false; reason: "not-fleet"; status: number };

export interface ProbeOptions {
  fetchImpl?: typeof fetch;
  /** 页面自身的协议(`window.location.protocol`),用于混合内容判断。 */
  pageProtocol?: string;
  timeoutMs?: number;
}

/** 探测用的方法。选 `wiki_list` 是因为它只读、不写、在任何主机上都答得出来
 *  (没发布过就是空数组),而且答对了就证明**整条链**通了:CORS 头、token 门、
 *  以及 `/mobile_rpc` 背后那个方法表。 */
const PROBE_METHOD = "wiki_list";

const PROBE_TIMEOUT_MS = 8_000;

export async function probeHost(
  rawUrl: string,
  token: string | null,
  opts: ProbeOptions = {},
): Promise<HostProbeResult> {
  const pageProtocol = opts.pageProtocol ?? globalThis.location?.protocol ?? "https:";
  const trimmed = rawUrl.trim().replace(/\/+$/, "");
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return { ok: false, reason: "not-a-url" };
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    return { ok: false, reason: "not-a-url" };
  }
  const isLocal = url.hostname === "localhost" || url.hostname === "127.0.0.1";
  if (pageProtocol === "https:" && url.protocol === "http:" && !isLocal) {
    return { ok: false, reason: "mixed-content" };
  }

  const fetchImpl = opts.fetchImpl ?? globalThis.fetch;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), opts.timeoutMs ?? PROBE_TIMEOUT_MS);
  let res: Response;
  try {
    const headers: Record<string, string> = { "Content-Type": "application/json" };
    if (token) headers.Authorization = `Bearer ${token}`;
    res = await fetchImpl(`${trimmed}/mobile_rpc`, {
      method: "POST",
      headers,
      body: JSON.stringify({ method: PROBE_METHOD, params: {} }),
      signal: controller.signal,
    });
  } catch (e) {
    // 跨源被拒也走这里 —— 浏览器不把原因交给 JS。
    return {
      ok: false,
      reason: "unreachable",
      detail: e instanceof Error ? e.message : String(e),
    };
  } finally {
    clearTimeout(timer);
  }

  if (res.status === 401 || res.status === 403) {
    return { ok: false, reason: "unauthorized", status: res.status };
  }
  if (!res.ok) {
    return { ok: false, reason: "not-fleet", status: res.status };
  }
  // 200 但答的不是 `{ok:…}` 信封 —— 那是别的什么服务占着这个地址。
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    return { ok: false, reason: "not-fleet", status: res.status };
  }
  if (typeof body !== "object" || body === null || !("ok" in body)) {
    return { ok: false, reason: "not-fleet", status: res.status };
  }
  return { ok: true };
}

/** 探测结果对应的中文说明。放在这里而不是组件里,好让文案与判定并排看得见 ——
 *  两者错配过一次就再没人信这几句话了。调用方负责过 i18n。 */
export function probeMessage(result: HostProbeResult): string {
  if (result.ok) return "";
  switch (result.reason) {
    case "not-a-url":
      return "地址要写成完整的 https://… 形式";
    case "mixed-content":
      return "浏览器不允许这个页面连明文 http 地址,请用 https(或先架一层 TLS)";
    case "unreachable":
      return "连不上这台主机,或它没有对本页开放跨源访问(要指向一台带 admin token 的 Fleet 主机)";
    case "unauthorized":
      return "这台主机拒绝了这个 token";
    case "not-fleet":
      return "这个地址答的不是 Fleet 主机";
  }
}
