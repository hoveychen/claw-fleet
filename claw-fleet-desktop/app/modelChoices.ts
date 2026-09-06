import type { DshModelCatalog } from "./generated/types";

// Shared catalog of selectable Claude models. Consumers prepend their own
// "default" entry, since the default differs per surface (the new-session
// launcher, for one, follows the CLI's own configured model).
export const CLAUDE_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "claude-fable-5-1", label: "Fable 5.1" },
  { value: "claude-fable-5", label: "Fable 5" },
  { value: "claude-opus-5", label: "Opus 5" },
  { value: "claude-opus-4-8", label: "Opus 4.8" },
  { value: "claude-sonnet-5", label: "Sonnet 5" },
  { value: "claude-sonnet-4-6", label: "Sonnet 4.6" },
  { value: "claude-haiku-4-5-20251001", label: "Haiku 4.5" },
];

// `claude --effort <level>` accepted values (verified against `claude --help`).
export const CLAUDE_EFFORT_CHOICES: string[] = ["low", "medium", "high", "xhigh", "max"];

// Selectable Codex models (`codex exec -m <model>`), curated from the ids
// Codex ships (see `~/.codex/models_cache.json`). The "" default follows
// Codex's own configured model. Kept small on purpose — add ids as needed.
//
// Third-party models are NOT listed here: they come from the host's Codex
// profile files at runtime (see `codexProfileChoices`). Hardcoding them would
// offer models whose provider block may not exist on the machine running Codex.
export const CODEX_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "gpt-6-astra", label: "GPT-6 Astra" },
  { value: "gpt-5.6-sol", label: "GPT-5.6 Sol" },
  { value: "gpt-5.6-terra", label: "GPT-5.6 Terra" },
  { value: "gpt-5.6-luna", label: "GPT-5.6 Luna" },
  { value: "gpt-5.5", label: "GPT-5.5" },
];

/** A Codex profile-v2 file as returned by the `list_codex_profiles` command —
 *  one `<CODEX_HOME>/<name>.config.toml` on whichever host runs Codex. */
export interface CodexProfile {
  name: string;
  model: string | null;
  model_provider: string | null;
  reasoning_effort: string | null;
}

/** Turn discovered profiles into model-picker entries.
 *
 *  A `[model_providers.<id>]` block declares only how to reach a provider, not
 *  which models it serves, so a profile file is the only thing in Codex's
 *  config that names a usable third-party model. One profile = one entry.
 *
 *  The value carries the `profile:` marker the backend splits into
 *  `codex exec -p <name>` (see `push_model_args` in `codex_launch.rs`); `-p`
 *  brings the profile's model *and* provider, which is why the raw model id is
 *  never sent for these. The label prefers the profile's own model id — that is
 *  what the user recognises — and falls back to the profile name for a profile
 *  that sets no model of its own. Effort stays selectable regardless: Fleet's
 *  `-c model_reasoning_effort=` flag overrides whatever the profile sets. */
export function codexProfileChoices(
  profiles: CodexProfile[],
): { value: string; label: string }[] {
  return profiles.map((p) => {
    const model = p.model?.trim();
    const provider = p.model_provider?.trim();
    const label = model
      ? provider
        ? `${model} (${provider})`
        : model
      : p.name;
    return { value: `profile:${p.name}`, label };
  });
}

// Codex reasoning effort (`-c model_reasoning_effort=<level>`). Distinct from
// Claude's --effort scale (no "xhigh"/"max"); Codex adds "minimal".
export const CODEX_EFFORT_CHOICES: string[] = ["minimal", "low", "medium", "high"];

// ── dsh model catalogue → menu ───────────────────────────────────────────────
//
// dsh is the one agent whose model list is not curated by Fleet: it publishes
// the host's own configured providers through `llm.models`, which on this
// machine means 2 DeepSeek models and 276 openrouter ones across 43 vendors.
// A flat 278-item popover is unusable, so the menu is two levels — the shaping
// rules below are what turn the catalogue into that.

/** openrouter vendors pinned to the model menu's top level. Chosen by the boss,
 *  not derived: the wire carries no popularity or recency signal to rank by
 *  (`description` is null for every openrouter row, and whether a model has a
 *  `reasoning` block says nothing about how much anyone wants it). */
export const DSH_FEATURED_VENDORS: string[] = [
  "anthropic",
  "deepseek",
  "openai",
  "google",
  "moonshotai",
];

/** A provider group with at most this many models is listed inline, whole:
 *  folding two DeepSeek models behind a submenu would cost a click and save
 *  nothing. Above it the group folds by vendor. */
export const DSH_INLINE_GROUP_CAP = 20;

/** One selectable dsh model, flattened for the picker. */
export interface DshModelPick {
  /** The `provider/model` string that goes into the spawn spec. */
  value: string;
  label: string;
  /** This model's own effort ladder — empty when it has no reasoning control. */
  efforts: string[];
  /** dsh's own default, or `""` when it publishes none. Shown on the effort
   *  menu's "default" item so the un-chosen state names what will happen. */
  defaultEffort: string;
}

/** A second-level group of models. */
export interface DshModelFolder {
  /** Stable React key. */
  id: string;
  /** The vendor prefix this folder collects, or `""` for the catch-all of
   *  unfeatured vendors (the label for that one is localized by the caller). */
  vendor: string;
  models: DshModelPick[];
}

export interface DshModelMenu {
  /** Offered at the top level, in catalogue order. */
  inline: DshModelPick[];
  /** Featured vendors in `DSH_FEATURED_VENDORS` order, then the catch-all. */
  folders: DshModelFolder[];
}

/** The vendor prefix of a provider-scoped model id (`anthropic/claude-opus-5`
 *  → `anthropic`), or `""` for an id that carries none. */
function vendorOf(modelId: string): string {
  const i = modelId.indexOf("/");
  return i > 0 ? modelId.slice(0, i) : "";
}

/** Shape a `llm.models` catalogue into the launcher's two-level model menu.
 *
 *  Total by construction: every model in the catalogue comes out either inline
 *  or in exactly one folder. A model that were dropped here would be
 *  unreachable from the UI while dsh happily accepts it.
 *
 *  A missing / empty catalogue yields an empty menu rather than an error — the
 *  picker then offers only its own "default" item, which is honest: the session
 *  runs on whatever `~/.dsh/settings.yaml` selects. Fields are read
 *  defensively because RemoteBackend fetches this from *another* host's
 *  `fleet serve`, whose build may predate any of them.
 */
export function dshModelMenu(catalog: DshModelCatalog | null | undefined): DshModelMenu {
  const inline: DshModelPick[] = [];
  const folders: DshModelFolder[] = [];

  const pick = (m: DshModelCatalog["groups"][number]["models"][number]): DshModelPick => ({
    value: m.spec,
    label: m.name || m.id,
    efforts: (m.efforts ?? []).map((e) => e.id),
    defaultEffort: m.defaultEffort ?? "",
  });

  for (const group of catalog?.groups ?? []) {
    const models = group.models ?? [];
    if (models.length <= DSH_INLINE_GROUP_CAP) {
      inline.push(...models.map(pick));
      continue;
    }
    // Bucket by vendor once, then emit featured vendors in the boss's order so
    // the menu does not reshuffle as the catalogue grows.
    const byVendor = new Map<string, DshModelPick[]>();
    for (const m of models) {
      const vendor = DSH_FEATURED_VENDORS.includes(vendorOf(m.id)) ? vendorOf(m.id) : "";
      const bucket = byVendor.get(vendor) ?? [];
      bucket.push(pick(m));
      byVendor.set(vendor, bucket);
    }
    for (const vendor of DSH_FEATURED_VENDORS) {
      const models = byVendor.get(vendor);
      if (models?.length) folders.push({ id: `${group.id}:${vendor}`, vendor, models });
    }
    const rest = byVendor.get("");
    if (rest?.length) folders.push({ id: `${group.id}:*`, vendor: "", models: rest });
  }

  return { inline, folders };
}

/** Look a spec up across both menu levels — the selected model may live in a
 *  folder, and the pill label plus the effort ladder both need it. */
export function dshFindPick(menu: DshModelMenu, spec: string): DshModelPick | undefined {
  if (!spec) return undefined;
  return (
    menu.inline.find((p) => p.value === spec) ??
    menu.folders.flatMap((f) => f.models).find((p) => p.value === spec)
  );
}

// The agent tools Fleet can launch a new session with. `agentToolsForSources`
// filters this down to the sources actually enabled + available, so listing a
// tool here does not by itself put it in the launcher.
export const AGENT_TOOL_CHOICES: { value: string; label: string }[] = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
  // dsh registers under its own bare name, so no sourceNameToTool mapping is
  // needed. Its source is additionally gated on the binary existing (see
  // `agent_source::build_sources`), which is what keeps it out of the launcher
  // on a machine with no dsh installed.
  { value: "dsh", label: "dsh" },
];

/** A source entry as returned by the `get_sources_config` backend command. */
export interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

/** Map an agent-source name (as used by the backend registry) to the launcher's
 *  tool value. The Claude source is registered under "claude-code" but the
 *  launcher tool value is the bare "claude". */
function sourceNameToTool(name: string): string {
  return name === "claude-code" ? "claude" : name;
}

/** Map a session's `agentSource` onto the launcher tool value its composer
 *  should offer. Anything Fleet cannot launch falls back to Claude, which is
 *  what the resume/schedule editors did with a hardcoded ternary before dsh
 *  existed — and which quietly offered Claude's models for a dsh session. */
export function toolForAgentSource(agentSource: string | undefined | null): string {
  const tool = sourceNameToTool((agentSource ?? "").trim());
  return AGENT_TOOL_CHOICES.some((c) => c.value === tool) ? tool : "claude";
}

/** Which Token-tab panel a session's `agentSource` should render.
 *
 *  Each source records token usage in its own vocabulary and reaches it over its
 *  own transport, so the panels are not interchangeable:
 *  - `claude` — the attribution panel, parsed out of the session's JSONL.
 *  - `codex`  — the rollout's cumulative `total_token_usage`, read from the file.
 *  - `dsh`    — dsh's `session.list` projections, fetched over RPC. dsh has no
 *    transcript file at all, so the file-reading panels return nothing for it.
 */
export function tokenPanelForAgentSource(
  agentSource: string | undefined | null,
): "claude" | "codex" | "dsh" {
  const source = (agentSource ?? "").trim();
  if (source === "codex") return "codex";
  if (source === "dsh") return "dsh";
  return "claude";
}

/** Restrict the launchable agent tools to the sources that are actually being
 *  monitored — a source must be both enabled (the settings toggle) and available
 *  (installed). This is what keeps Codex out of the launcher when its source is
 *  turned off or the CLI isn't installed. Falls back to Claude-only when the
 *  config hasn't loaded yet or nothing matched, so the launcher is never empty
 *  and never flashes an unmonitored tool before hiding it. */
export function agentToolsForSources(
  sources: SourceInfo[],
): { value: string; label: string }[] {
  const active = new Set(
    sources.filter((s) => s.enabled && s.available).map((s) => sourceNameToTool(s.name)),
  );
  const filtered = AGENT_TOOL_CHOICES.filter((c) => active.has(c.value));
  return filtered.length ? filtered : [AGENT_TOOL_CHOICES[0]];
}

// `claude --permission-mode <mode>` values exposed in the new-session
// launcher (a curated subset of the CLI's choices — "auto"/"dontAsk" are
// omitted as they mostly matter for interactive runs). Labels come from the
// `new_session.permission_*` locale keys.
export const CLAUDE_PERMISSION_MODE_CHOICES: string[] = [
  "acceptEdits",
  "plan",
  "bypassPermissions",
];
