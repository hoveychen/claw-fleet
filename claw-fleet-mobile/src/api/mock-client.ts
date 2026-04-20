/**
 * Mock API client for development/preview without a real desktop server.
 * Activate by connecting to url "mock" with any token.
 */
import { MOCK_SESSIONS, MESSAGES_MAP } from "../mock";
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

export class MockApiClient {
  async health(): Promise<{ version: string; status: string }> {
    return { version: "mock-1.0.0", status: "ok" };
  }

  async listSessions(): Promise<SessionInfo[]> {
    // Simulate activity: jitter speeds and update timestamps
    return MOCK_SESSIONS.map((s) => ({
      ...s,
      tokenSpeed:
        s.status === "idle"
          ? 0
          : Math.max(0, s.tokenSpeed + (Math.random() - 0.3) * 10),
      lastActivityMs:
        s.status === "idle" ? s.lastActivityMs : Date.now() - Math.random() * 5000,
    }));
  }

  async getMessages(jsonlPath: string): Promise<RawMessage[]> {
    return MESSAGES_MAP[jsonlPath] ?? [];
  }

  async killSession(_pid: number): Promise<void> {}

  async getAuditEvents(): Promise<AuditSummary> {
    return { events: [], totalSessionsScanned: 0 };
  }

  async getWaitingAlerts(): Promise<WaitingAlert[]> {
    return MOCK_SESSIONS.filter((s) => s.status === "waitingInput").map((s) => ({
      sessionId: s.id,
      workspaceName: s.workspaceName,
      summary: s.lastMessagePreview || "Waiting for input",
      detectedAtMs: Date.now() - 60000,
      jsonlPath: s.jsonlPath,
      source: s.agentSource,
    }));
  }

  async getDailyReport(_date: string): Promise<DailyReport | null> {
    return null;
  }

  async getDailyReportStats(
    _from: string,
    _to: string,
  ): Promise<DailyReportStats[]> {
    return [];
  }

  async generateDailyReport(_date: string): Promise<DailyReport> {
    throw new Error("Not available in mock mode");
  }

  async searchSessions(
    _query: string,
    _limit?: number,
  ): Promise<Array<{ sessionId: string; snippet: string; rank: number }>> {
    return [];
  }

  async getGuardPending(): Promise<GuardRequest[]> {
    return [];
  }

  async respondGuard(_id: string, _allow: boolean): Promise<void> {}

  async analyzeGuard(
    _command: string,
    _context: string,
    _lang: string,
  ): Promise<string | null> {
    return null;
  }

  async getElicitationPending(): Promise<ElicitationRequest[]> {
    return [];
  }

  async respondElicitation(
    _id: string,
    _declined: boolean,
    _answers: Record<string, string>,
  ): Promise<void> {}

  async uploadElicitationAttachment(
    _bytes: ArrayBuffer | Uint8Array,
    name: string,
  ): Promise<string> {
    return `/tmp/mock-attachments/${name}`;
  }

  sseUrl(): string {
    return "";
  }
}
