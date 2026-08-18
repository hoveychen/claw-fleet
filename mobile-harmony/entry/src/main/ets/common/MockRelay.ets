import { util } from '@kit.ArkTS';
import { RelayClient, RelayHandlers } from './Relay';
import {
  mockSessions, mockSnapshot, mockTodayUsage, mockAccountUsage, mockUsageHistory,
  mockTaskPlans, mockWiki, mockRepoList, mockRepoDetail, mockTail, mockLiveThinking,
  mockWorkflowTrees, mockHandoffChain, mockTokenBreakdown,
} from './MockData';

/// The no-scan trial client. It IS-A RelayClient (ArkTS nominal subtyping), so it
/// drops into every `client: RelayClient` slot — all 12 views consume it
/// unchanged. It never opens a socket: `connect()` synthesises an authed, online
/// desktop by firing the handlers with simulated fixtures, and `request<T>`
/// answers each relay method from MockData. Write methods (spawn / enqueue /
/// interrupt / stop / mark …) return a fake success so the demo feels fully
/// usable — the header's 「试用模式」 badge makes clear it's simulated, so faking
/// the writes is not deceptive.
///
/// This exists so the atomic service is usable and reviewable standalone, before
/// the user scans a desktop pairing QR.

// Simulate a brief connect handshake so the UI shows the same connecting→online
// transition a real pairing does (rather than snapping to online instantly).
const CONNECT_DELAY_MS = 350;

// ── View-private reply shapes (mirrored so typed literals compile) ──────────────

interface OkResp { ok: boolean; }
interface PathResp { path: string; }
interface AnalysisResp { analysis: string; }
interface AssetResp { mime: string; base64: string; }
interface TailDeltaResp { lines: Object[]; newOffset: number; }
interface WikiFileResp { mime: string; base64: string; }
interface WikiExportResp { filename: string; mime: string; base64: string; }
interface BrowseDirResp { path: string; parent: string | null; entries: Object[]; truncated: boolean; }

function b64(text: string): string {
  const bytes = new util.TextEncoder().encodeInto(text);
  return new util.Base64Helper().encodeToStringSync(bytes);
}

export class MockRelayClient extends RelayClient {
  private mockHandlers: RelayHandlers;
  private started = false;

  constructor(handlers: RelayHandlers) {
    // Dummy origin/secret — no socket is ever opened, so these are inert. The
    // secret still clears the relay's >=16-char guard shape for consistency.
    super('https://trial.local', 'trial0000000000000000', handlers);
    this.mockHandlers = handlers;
  }

  // Always authed: gates that block on `isAuthed` (usage poll, requests) pass.
  get isAuthed(): boolean {
    return true;
  }

  connect(): void {
    if (this.started) {
      return;
    }
    this.started = true;
    setTimeout(() => {
      // Same order a real authed→online transition drives in MainShell: link up,
      // sessions in, then agent online (which pulls pending_snapshot + usage).
      this.mockHandlers.onStatus?.(true);
      this.mockHandlers.onSessions?.(mockSessions());
      this.mockHandlers.onAgentOnline?.(true);
    }, CONNECT_DELAY_MS);
  }

  // No socket / no timers / no farewell to send.
  close(): void {}
  kickNow(_reason: string): void {}
  sayGoodbye(): void {}

  // Answering a decision card is a write. Report true (socket "open") so the
  // view removes the card optimistically, keeping the demo responsive.
  answer(_payload: Object): boolean {
    return true;
  }

  request<T>(method: string, _params?: Object, _timeoutMs?: number): Promise<T> {
    return Promise.resolve(this.replyFor(method) as T);
  }

  /// Map a relay method name to its simulated reply. Read methods return MockData
  /// fixtures; write methods return a fake `{ok:true}`; unknown methods fall back
  /// to an empty ok so no view ever hangs on a missing branch.
  private replyFor(method: string): Object {
    switch (method) {
      // ── read: shell / sessions / decisions ──
      case 'pending_snapshot': return mockSnapshot();
      case 'today_usage': return mockTodayUsage();
      case 'task_plans': return mockTaskPlans();
      case 'session_search': return [] as Object[];
      // ── read: session detail ──
      case 'tail': return mockTail();
      case 'tail_delta': { const d: TailDeltaResp = { lines: [], newOffset: 999_999 }; return d; }
      case 'live_thinking': return mockLiveThinking();
      case 'workflow_trees': return mockWorkflowTrees();
      case 'handoff_chain': return mockHandoffChain();
      case 'token_breakdown': return mockTokenBreakdown();
      case 'session_decisions': return [] as Object[];
      case 'guard_analyze': { const a: AnalysisResp = { analysis: '这是一条删除 dist 目录的命令,产物可由构建脚本重新生成,风险中等。' }; return a; }
      case 'decision_asset': { const r: AssetResp = { mime: 'image/png', base64: '' }; return r; }
      // ── read: wiki ──
      case 'wiki_list': return mockWiki();
      case 'wiki_search': return [] as Object[];
      case 'wiki_file': { const f: WikiFileResp = { mime: 'text/markdown', base64: b64('# Fleet 架构总览\n\n这是试用模式下的示例知识库文档。\n\n- 决策卡\n- 任务与会话\n- 知识库与仓库') }; return f; }
      case 'wiki_export': { const e: WikiExportResp = { filename: 'overview.md', mime: 'text/markdown', base64: b64('# Fleet 架构总览\n') }; return e; }
      // ── read: repos ──
      case 'repo_list': return mockRepoList();
      case 'repo_detail': return mockRepoDetail();
      // ── read: usage / account ──
      case 'account_usage': return mockAccountUsage();
      case 'usage_history': return mockUsageHistory();
      // ── read: dir picker / workspace ──
      case 'chat_workspace': { const p: PathResp = { path: '/Users/demo/workspace' }; return p; }
      case 'browse_dir': { const b: BrowseDirResp = { path: '/Users/demo/workspace', parent: null, entries: [], truncated: false }; return b; }
      case 'upload_attachment': { const u: PathResp = { path: '/tmp/mock-upload' }; return u; }
      // ── write: faked success (trial can't mutate the desktop) ──
      case 'spawn_session':
      case 'enqueue_message':
      case 'resume_session':
      case 'interrupt':
      case 'stop':
      case 'stop_workspace':
      case 'session_mark':
      case 'session_read': { const ok: OkResp = { ok: true }; return ok; }
      default: { const ok: OkResp = { ok: true }; return ok; }
    }
  }
}
