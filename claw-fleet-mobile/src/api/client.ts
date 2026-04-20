import type {
  SessionInfo,
  RawMessage,
  AuditSummary,
  DailyReport,
  DailyReportStats,
  WaitingAlert,
  GuardRequest,
  ElicitationRequest,
} from "../types";

/** Mirror of claw_fleet_core::backend::MAX_ATTACHMENT_BYTES (50 MiB). */
export const MAX_ATTACHMENT_BYTES = 50 * 1024 * 1024;

export class FleetApiClient {
  constructor(
    private baseUrl: string,
    private token: string,
  ) {}

  private async fetch<T>(
    path: string,
    params?: Record<string, string>,
  ): Promise<T> {
    const url = new URL(path, this.baseUrl);
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        url.searchParams.set(k, v);
      }
    }
    const res = await fetch(url.toString(), {
      headers: { Authorization: `Bearer ${this.token}` },
    });
    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new Error(`API ${res.status}: ${body}`);
    }
    return res.json();
  }

  async health(): Promise<{ version: string; status: string }> {
    return this.fetch("/health");
  }

  async listSessions(): Promise<SessionInfo[]> {
    return this.fetch("/sessions");
  }

  async getMessages(jsonlPath: string): Promise<RawMessage[]> {
    return this.fetch("/messages", { path: jsonlPath });
  }

  async killSession(pid: number): Promise<void> {
    await this.fetch("/stop", { pid: String(pid) });
  }

  async getAuditEvents(): Promise<AuditSummary> {
    return this.fetch("/audit");
  }

  async getWaitingAlerts(): Promise<WaitingAlert[]> {
    return this.fetch("/waiting_alerts");
  }

  async getDailyReport(date: string): Promise<DailyReport | null> {
    return this.fetch("/daily_report", { date });
  }

  async getDailyReportStats(
    from: string,
    to: string,
  ): Promise<DailyReportStats[]> {
    return this.fetch("/daily_report_stats", { from, to });
  }

  async generateDailyReport(date: string): Promise<DailyReport> {
    return this.fetch("/daily_report/generate", { date });
  }

  async searchSessions(
    query: string,
    limit = 20,
  ): Promise<
    Array<{
      sessionId: string;
      snippet: string;
      rank: number;
    }>
  > {
    return this.fetch("/search", { q: query, limit: String(limit) });
  }

  // ── Guard / Elicitation ─────────────────────────────────────────────

  async getGuardPending(): Promise<GuardRequest[]> {
    return this.fetch("/guard/pending");
  }

  async respondGuard(id: string, allow: boolean): Promise<void> {
    await this.postJson("/guard/respond", {
      id,
      decision: allow ? "Allow" : "Block",
    });
  }

  async analyzeGuard(
    command: string,
    context: string,
    lang: string,
  ): Promise<string | null> {
    try {
      const resp: { analysis?: string } = await this.postJson(
        "/guard/analyze",
        { command, context, lang },
      );
      return resp.analysis ?? null;
    } catch {
      return null;
    }
  }

  async getElicitationPending(): Promise<ElicitationRequest[]> {
    return this.fetch("/elicitation/pending");
  }

  async respondElicitation(
    id: string,
    declined: boolean,
    answers: Record<string, string>,
  ): Promise<void> {
    await this.postJson("/elicitation/respond", { id, declined, answers });
  }

  /** Upload a file/image attachment, returns the absolute path visible to the agent. */
  async uploadElicitationAttachment(
    bytes: ArrayBuffer | Uint8Array,
    name: string,
  ): Promise<string> {
    const url = new URL("/elicitation/upload", this.baseUrl);
    url.searchParams.set("name", name);
    const byteLength =
      bytes instanceof Uint8Array ? bytes.byteLength : bytes.byteLength;
    if (byteLength > MAX_ATTACHMENT_BYTES) {
      throw new Error(
        `attachment too large: ${byteLength} bytes (max ${MAX_ATTACHMENT_BYTES})`,
      );
    }
    let body: ArrayBuffer;
    if (bytes instanceof Uint8Array) {
      body = bytes.buffer.slice(
        bytes.byteOffset,
        bytes.byteOffset + bytes.byteLength,
      ) as ArrayBuffer;
    } else {
      body = bytes;
    }
    const res = await fetch(url.toString(), {
      method: "POST",
      headers: {
        Authorization: `Bearer ${this.token}`,
        "Content-Type": "application/octet-stream",
      },
      body,
    });
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      throw new Error(`API ${res.status}: ${text}`);
    }
    const json: { path: string } = await res.json();
    return json.path;
  }

  /** SSE event source URL (use with EventSource or custom SSE) */
  sseUrl(): string {
    return `${this.baseUrl}/events?token=${this.token}`;
  }

  // ── POST helper ───────────────────────────────────────────────────────

  private async postJson<T>(path: string, body: unknown): Promise<T> {
    const url = new URL(path, this.baseUrl);
    const res = await fetch(url.toString(), {
      method: "POST",
      headers: {
        Authorization: `Bearer ${this.token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      throw new Error(`API ${res.status}: ${text}`);
    }
    return res.json();
  }
}
