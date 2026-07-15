//! Single source of truth for the HTTP route paths shared between the
//! `fleet serve` router (`hooks_server::serve`) and the RemoteBackend HTTP
//! client (`claw-fleet-desktop/src/remote.rs`).
//!
//! Both sides reference the SAME constant, so renaming a path is a single
//! edit that either updates both ends or fails to compile — turning the
//! former runtime-404 drift (see tests/backend_drift_guard.rs Check B) into
//! a compile error. Paths carrying query params (`?path=…`) or path
//! segments (`/sources/<name>/account`) store only the constant prefix;
//! callers append the query/segment as before.

pub const A2UI_RENDER_PENDING: &str = "/a2ui-render/pending";
pub const A2UI_RENDER_RESPOND: &str = "/a2ui-render/respond";
pub const ANALYZE: &str = "/analyze";
pub const APPLY_ELICITATION_HOOK: &str = "/apply_elicitation_hook";
pub const APPLY_GUARD_HOOK: &str = "/apply_guard_hook";
pub const APPLY_HOOKS: &str = "/apply_hooks";
pub const APPLY_INTERACTION_MODE: &str = "/apply_interaction_mode";
pub const APPLY_MODEL_GUIDANCE: &str = "/apply_model_guidance";
pub const APPLY_PLAN_APPROVAL_HOOK: &str = "/apply_plan_approval_hook";
pub const APPLY_PRD_MODE: &str = "/apply_prd_mode";
pub const APPLY_WIKI_GUIDANCE: &str = "/apply_wiki_guidance";
pub const AUDIT: &str = "/audit";
pub const AUDIT_CHECK_UPDATE: &str = "/audit/check-update";
pub const AUDIT_PATTERN_INFO: &str = "/audit/pattern-info";
pub const AUDIT_RULES: &str = "/audit/rules";
pub const AUDIT_RULES_DELETE: &str = "/audit/rules/delete";
pub const AUDIT_RULES_SAVE: &str = "/audit/rules/save";
pub const AUDIT_RULES_SUGGEST: &str = "/audit/rules/suggest";
pub const AUDIT_RULES_TOGGLE: &str = "/audit/rules/toggle";
pub const BROWSE_DIR: &str = "/browse_dir";
pub const CHAT_WORKSPACE: &str = "/chat_workspace";
pub const CLAUDE_BINARY_OVERRIDE: &str = "/claude_binary_override";
pub const DAILY_REPORT: &str = "/daily_report";
pub const DAILY_REPORT_AI_SUMMARY: &str = "/daily_report/ai_summary";
pub const DAILY_REPORT_APPEND_LESSON: &str = "/daily_report/append_lesson";
pub const DAILY_REPORT_GENERATE: &str = "/daily_report/generate";
pub const DAILY_REPORT_LESSONS: &str = "/daily_report/lessons";
pub const DAILY_REPORT_STATS: &str = "/daily_report_stats";
pub const DECISION_ASSET: &str = "/decision_asset";
pub const ENQUEUE_MESSAGE: &str = "/enqueue_message";
pub const ELICITATION_PENDING: &str = "/elicitation/pending";
pub const ELICITATION_RESPOND: &str = "/elicitation/respond";
pub const ELICITATION_UPLOAD: &str = "/elicitation/upload";
pub const EXPLORER_DIR: &str = "/explorer_dir";
pub const EXPLORER_FILE: &str = "/explorer_file";
pub const EXPLORER_ROOTS: &str = "/explorer_roots";
pub const FILE_SIZE: &str = "/file_size";
pub const FLEET_ASK_PENDING: &str = "/fleet-ask/pending";
pub const FLEET_ASK_RESPOND: &str = "/fleet-ask/respond";
pub const FLEET_LLM_USAGE_DAILY: &str = "/fleet_llm_usage/daily";
pub const GIT_PULL: &str = "/git_pull";
pub const GIT_PUSH: &str = "/git_push";
pub const GIT_STATUS: &str = "/git_status";
pub const GUARD_ALLOW_RULES: &str = "/guard/allow-rules";
pub const GUARD_ALLOW_RULES_REMOVE: &str = "/guard/allow-rules/remove";
pub const GUARD_ANALYZE: &str = "/guard/analyze";
pub const GUARD_PENDING: &str = "/guard/pending";
pub const GUARD_RESPOND: &str = "/guard/respond";
pub const HANDOFF_CHAIN: &str = "/handoff_chain";
pub const HEALTH: &str = "/health";
pub const HOOKS_PLAN: &str = "/hooks_plan";
pub const INTERACTION_DIAGNOSTICS: &str = "/interaction_diagnostics";
pub const INTERRUPT: &str = "/interrupt";
pub const LIST_CLAUDE_BINARIES: &str = "/list_claude_binaries";
pub const LIVE_THINKING: &str = "/live_thinking";
pub const LLM_CONFIG: &str = "/llm/config";
pub const LLM_PROVIDERS: &str = "/llm/providers";
pub const MEMORIES: &str = "/memories";
pub const MEMORY_CONTENT: &str = "/memory_content";
pub const MEMORY_HISTORY: &str = "/memory_history";
pub const MESSAGES: &str = "/messages";
pub const MOBILE_RELAY_CONFIG: &str = "/mobile-relay/config";
pub const MOBILE_RELAY_QR: &str = "/mobile-relay/qr";
pub const MOBILE_RELAY_ROTATE: &str = "/mobile-relay/rotate";
pub const MOBILE_RELAY_STATUS: &str = "/mobile-relay/status";
pub const PERMISSION_PROMPT_PENDING: &str = "/permission-prompt/pending";
pub const PERMISSION_PROMPT_RESPOND: &str = "/permission-prompt/respond";
pub const PLAN_APPROVAL_PENDING: &str = "/plan-approval/pending";
pub const PLAN_APPROVAL_RESPOND: &str = "/plan-approval/respond";
pub const PLUGINS: &str = "/plugins";
pub const PLUGINS_INSTALL: &str = "/plugins/install";
pub const PLUGINS_MARKETPLACES: &str = "/plugins/marketplaces";
pub const PLUGINS_MARKETPLACES_ADD: &str = "/plugins/marketplaces/add";
pub const PLUGINS_MARKETPLACES_REMOVE: &str = "/plugins/marketplaces/remove";
pub const PLUGINS_SET_ENABLED: &str = "/plugins/set_enabled";
pub const PLUGINS_UNINSTALL: &str = "/plugins/uninstall";
pub const PROC_CLEAR: &str = "/proc_clear";
pub const PROC_INPUT: &str = "/proc_input";
pub const PROC_KILL: &str = "/proc_kill";
pub const PROC_OUTPUT: &str = "/proc_output";
pub const PROC_RESIZE: &str = "/proc_resize";
pub const PROC_RUN: &str = "/proc_run";
pub const PROCS: &str = "/procs";
pub const REMOVE_ELICITATION_HOOK: &str = "/remove_elicitation_hook";
pub const REMOVE_GUARD_HOOK: &str = "/remove_guard_hook";
pub const REMOVE_HOOKS: &str = "/remove_hooks";
pub const REMOVE_INTERACTION_MODE: &str = "/remove_interaction_mode";
pub const REMOVE_MODEL_GUIDANCE: &str = "/remove_model_guidance";
pub const REMOVE_PLAN_APPROVAL_HOOK: &str = "/remove_plan_approval_hook";
pub const REMOVE_PRD_MODE: &str = "/remove_prd_mode";
pub const REMOVE_WIKI_GUIDANCE: &str = "/remove_wiki_guidance";
pub const RESUME_SESSION: &str = "/resume_session";
pub const SCRATCHPAD_DIR: &str = "/scratchpad_dir";
pub const SCRATCHPAD_FILE: &str = "/scratchpad_file";
pub const SEARCH: &str = "/search";
pub const SESSION_DECISIONS: &str = "/session_decisions";
pub const SESSION_MARK: &str = "/session_mark";
pub const SESSION_READ: &str = "/session_read";
pub const SESSIONS: &str = "/sessions";
pub const SET_SOURCE_ENABLED: &str = "/set_source_enabled";
pub const SETUP_STATUS: &str = "/setup-status";
pub const SKILL_CONTENT: &str = "/skill_content";
pub const SKILL_DELETE: &str = "/skill_delete";
pub const SKILL_FILES: &str = "/skill_files";
pub const SKILL_HISTORY: &str = "/skill_history";
pub const SKILLS: &str = "/skills";
pub const SOURCES_CLAUDE_ACCOUNT: &str = "/sources/claude/account";
pub const SOURCES_CONFIG: &str = "/sources_config";
pub const SPAWN_SESSION: &str = "/spawn_session";
pub const STOP: &str = "/stop";
pub const STOP_WORKSPACE: &str = "/stop_workspace";
pub const TAIL: &str = "/tail";
pub const TASK_PLANS: &str = "/task_plans";
pub const TEST_DECISION_END_TO_END: &str = "/test_decision_end_to_end";
pub const TEST_DECISION_VIA_CLAUDE_CLI: &str = "/test_decision_via_claude_cli";
pub const TODAY_USAGE: &str = "/today_usage";
pub const TOKEN_BREAKDOWN: &str = "/token_breakdown";
pub const USAGE_HISTORY: &str = "/usage_history";
pub const USAGE_SUMMARIES: &str = "/usage_summaries";
pub const USER_ATTACHMENT: &str = "/user_attachment";
pub const WIKI_DELETE: &str = "/wiki_delete";
pub const WIKI_DELETE_FOLDER: &str = "/wiki_delete_folder";
pub const WIKI_DOC: &str = "/wiki_doc";
pub const WIKI_DOCS: &str = "/wiki_docs";
pub const WIKI_EXPORT: &str = "/wiki_export";
pub const WIKI_FILE: &str = "/wiki_file";
pub const WIKI_MOVE: &str = "/wiki_move";
pub const WIKI_MOVE_FOLDER: &str = "/wiki_move_folder";
pub const WIKI_SEARCH: &str = "/wiki_search";
pub const WORKFLOW_TREES: &str = "/workflow_trees";

/// Prefix arm: `/sources/<name>/account|usage` is matched by prefix on the
/// server and built with `format!` on the client.
pub const SOURCES_PREFIX: &str = "/sources/";

