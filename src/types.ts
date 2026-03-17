// ── Session types (mirroring Rust structs) ───────────────────────────────────

export type SessionStatus =
  | "streaming"
  | "processing"
  | "waitingInput"
  | "active"
  | "delegating"
  | "idle";

export interface SessionInfo {
  id: string;
  workspacePath: string;
  workspaceName: string;
  ideName: string | null;
  isSubagent: boolean;
  parentSessionId: string | null;
  agentType: string | null;
  agentDescription: string | null;
  slug: string | null;
  status: SessionStatus;
  tokenSpeed: number;
  totalOutputTokens: number;
  lastMessagePreview: string | null;
  lastActivityMs: number;
  jsonlPath: string;
}

// ── Message / content block types ───────────────────────────────────────────

export type ContentBlockType =
  | "text"
  | "tool_use"
  | "tool_result"
  | "thinking"
  | "redacted_thinking"
  | "image"
  | "document"
  | "server_tool_use"
  | "web_search_tool_result"
  | "search_result";

export interface TextBlock {
  type: "text";
  text: string;
}

export interface ToolUseBlock {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export interface ToolResultBlock {
  type: "tool_result";
  tool_use_id: string;
  content: string | ContentBlock[];
  is_error?: boolean;
}

export interface ThinkingBlock {
  type: "thinking";
  thinking: string;
}

export interface RedactedThinkingBlock {
  type: "redacted_thinking";
}

export interface ImageBlock {
  type: "image";
  source: { type: string; media_type: string; data: string };
}

export type ContentBlock =
  | TextBlock
  | ToolUseBlock
  | ToolResultBlock
  | ThinkingBlock
  | RedactedThinkingBlock
  | ImageBlock
  | { type: string; [key: string]: unknown };

export interface MessageUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

export interface RawMessage {
  type: "user" | "assistant" | "progress" | "queue-operation" | "last-prompt" | "file-history-snapshot";
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  agentId?: string;
  sessionId?: string;
  slug?: string;
  message?: {
    role: "user" | "assistant";
    model?: string;
    id?: string;
    content: ContentBlock[] | string;
    stop_reason?: string | null;
    usage?: MessageUsage;
  };
}
