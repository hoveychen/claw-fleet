export type EmbedView = "task_detail" | "decision_inbox" | "decision_card" | "usage";

export interface EmbedClaims {
  project_id: string;
  task_id?: string;
  allowed_origins: string[];
  views: EmbedView[];
  exp: number;
  iat?: number;
  jti?: string;
}

export class EmbedAuthError extends Error {
  constructor(readonly code: string) {
    super(code);
    this.name = "EmbedAuthError";
  }
}

export class MemoryEmbedToken {
  private value: string | null = null;

  get(): string | null {
    return this.value;
  }

  set(token: string): void {
    this.value = token;
  }

  clear(): void {
    this.value = null;
  }
}

export interface ValidateEmbedOptions {
  parentOrigin: string;
  nowSeconds?: number;
  taskId?: string;
  view: EmbedView;
}

export function validateEmbedToken(
  token: string,
  options: ValidateEmbedOptions,
): EmbedClaims {
  const claims = decodeClaims(token);
  const now = options.nowSeconds ?? Math.floor(Date.now() / 1_000);
  if (!Number.isFinite(claims.exp) || claims.exp <= now) {
    throw new EmbedAuthError("embed_token_expired");
  }
  if (!claims.allowed_origins.includes(options.parentOrigin)) {
    throw new EmbedAuthError("embed_origin_denied");
  }
  if (claims.task_id && options.taskId !== claims.task_id) {
    throw new EmbedAuthError("embed_task_denied");
  }
  if (!claims.views.includes(options.view)) {
    throw new EmbedAuthError("embed_view_denied");
  }
  return claims;
}

export function decodeClaims(token: string): EmbedClaims {
  const parts = token.split(".");
  if (parts.length !== 3) throw new EmbedAuthError("embed_token_invalid");
  try {
    const payload = parts[1].replaceAll("-", "+").replaceAll("_", "/");
    const padded = payload.padEnd(Math.ceil(payload.length / 4) * 4, "=");
    const parsed = JSON.parse(atob(padded)) as Partial<EmbedClaims>;
    if (
      typeof parsed.project_id !== "string" ||
      !Array.isArray(parsed.allowed_origins) ||
      !parsed.allowed_origins.every((origin) => typeof origin === "string") ||
      !Array.isArray(parsed.views) ||
      typeof parsed.exp !== "number"
    ) {
      throw new Error("claims");
    }
    return parsed as EmbedClaims;
  } catch (error) {
    if (error instanceof EmbedAuthError) throw error;
    throw new EmbedAuthError("embed_token_invalid");
  }
}

export interface MessageTarget {
  postMessage(message: unknown, targetOrigin: string): void;
}

export function postToParent(
  parent: MessageTarget,
  message: unknown,
  targetOrigin: string,
): void {
  if (!isExactHttpOrigin(targetOrigin)) {
    throw new EmbedAuthError("embed_origin_denied");
  }
  parent.postMessage(message, targetOrigin);
}

export function parentOriginFromReferrer(referrer: string): string {
  try {
    const origin = new URL(referrer).origin;
    if (!isExactHttpOrigin(origin)) throw new Error("origin");
    return origin;
  } catch {
    throw new EmbedAuthError("embed_origin_denied");
  }
}

function isExactHttpOrigin(origin: string): boolean {
  if (!origin || origin === "*") return false;
  try {
    const url = new URL(origin);
    return (
      (url.protocol === "https:" || url.protocol === "http:") &&
      url.origin === origin &&
      url.username === "" &&
      url.password === ""
    );
  } catch {
    return false;
  }
}
