//! Shell command AST extraction.
//!
//! Wraps `conch-parser` to expose a single helper —
//! [`extract_simple_commands`] — that walks a Bash command string and returns
//! every `SimpleCommand` it finds, recursing through subshells (`( ... )`),
//! brace groups (`{ ... }`), command substitutions (`$(...)` / backticks),
//! pipes, and the string argument of `bash -c`, `sh -c`, `zsh -c`, `eval`.
//!
//! Environment-variable prefixes (`GIT_TRACE=1 git push`) do NOT pollute the
//! returned argv — the env-var assignments are stripped and only the command
//! word survives.
//!
//! If parsing fails (truncated quoting, unsupported syntax, etc.) the input is
//! tokenised with `shell-words` and returned as a single `SimpleCommand`
//! fallback, so callers always get *something* to match against.

use conch_parser::ast::{
    AndOr, AndOrList, Arithmetic, Command, ComplexWord, CompoundCommandKind,
    DefaultCompoundCommand, DefaultListableCommand, DefaultPipeableCommand, DefaultSimpleCommand,
    GuardBodyPair, ListableCommand, Parameter, ParameterSubstitution, PipeableCommand,
    RedirectOrCmdWord, RedirectOrEnvVar, SimpleWord, TopLevelCommand, TopLevelWord, Word,
};
use conch_parser::lexer::Lexer;
use conch_parser::parse::DefaultParser;
use serde::{Deserialize, Serialize};

/// A flattened shell SimpleCommand: just its argv, with env-var prefixes
/// removed.  Empty `argv` is never returned (callers can rely on
/// `argv[0]` existing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub argv: Vec<String>,
}

type SubstT = ParameterSubstitution<
    Parameter<String>,
    TopLevelWord<String>,
    TopLevelCommand<String>,
    Arithmetic<String>,
>;
type SimpleT = SimpleWord<String, Parameter<String>, Box<SubstT>>;
type WordT = Word<String, SimpleT>;
type ComplexT = ComplexWord<WordT>;

/// Parse `cmd` and return every SimpleCommand reachable from the AST.
///
/// On parse failure, falls back to a single tokenised argv via `shell-words`.
pub fn extract_simple_commands(cmd: &str) -> Vec<SimpleCommand> {
    if let Some(simples) = try_parse(cmd) {
        return simples;
    }
    // Parse failed.  A single non-shell segment (e.g. a JS arrow function
    // `patchwright-cli eval () => ...`) makes conch-parser reject the *entire*
    // compound command, even though the other segments are perfectly ordinary.
    // Collapsing the whole string into one flat argv via `shell_words::split`
    // would headline it with the first word (`cd`) and silently destroy every
    // other leaf's argv head — so prefix-based guard allow-rules like
    // `patchwright-cli eval` could never match a leaf buried mid-chain.
    //
    // Instead, split on top-level connectors and salvage each segment on its
    // own: a segment that parses cleanly contributes its real SimpleCommands,
    // and only the genuinely-unparseable segment falls back to a tokenised
    // argv (which still keeps its own argv head intact).
    let segments = split_top_level_connectors(cmd);
    if segments.len() > 1 {
        let mut out = Vec::new();
        for seg in &segments {
            out.extend(salvage_segment(seg));
        }
        if !out.is_empty() {
            return out;
        }
    }
    salvage_segment(cmd)
}

/// Best-effort recovery of a single connector-free segment: try the real
/// parser first, then fall back to a dumb `shell_words` tokenise.  Better to
/// over-match (and risk a false positive) than to miss the command entirely.
fn salvage_segment(seg: &str) -> Vec<SimpleCommand> {
    if let Some(simples) = try_parse(seg) {
        if !simples.is_empty() {
            return simples;
        }
    }
    match shell_words::split(seg) {
        Ok(argv) if !argv.is_empty() => vec![SimpleCommand { argv }],
        _ => Vec::new(),
    }
}

/// Split `cmd` on top-level shell connectors, returning each non-empty segment
/// paired with the connector that *precedes* it (`None` for the first).  Used
/// only on the parse-failure fallback path; see [`split_top_level_connectors`].
fn split_top_level_segments(cmd: &str) -> Vec<(Option<Connector>, String)> {
    let mut segments: Vec<(Option<Connector>, String)> = Vec::new();
    let mut cur = String::new();
    // Connector that precedes the segment currently being accumulated.
    let mut pending: Option<Connector> = None;
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = cmd.chars().peekable();
    let flush = |pending: &mut Option<Connector>,
                     cur: &mut String,
                     segments: &mut Vec<(Option<Connector>, String)>,
                     next: Connector| {
        segments.push((*pending, std::mem::take(cur)));
        *pending = Some(next);
    };
    while let Some(c) = chars.next() {
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            cur.push(c);
            if c == '\\' {
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            } else if c == '"' {
                in_double = false;
            }
            continue;
        }
        match c {
            '\\' => {
                cur.push(c);
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            }
            '\'' => {
                in_single = true;
                cur.push(c);
            }
            '"' => {
                in_double = true;
                cur.push(c);
            }
            ';' | '\n' => flush(&mut pending, &mut cur, &mut segments, Connector::Semi),
            '&' => {
                let conn = if chars.peek() == Some(&'&') {
                    chars.next();
                    Connector::And
                } else {
                    // Background `&` — closest structural match is a sequence.
                    Connector::Semi
                };
                flush(&mut pending, &mut cur, &mut segments, conn);
            }
            '|' => {
                let conn = if chars.peek() == Some(&'|') {
                    chars.next();
                    Connector::Or
                } else {
                    Connector::Pipe
                };
                flush(&mut pending, &mut cur, &mut segments, conn);
            }
            _ => cur.push(c),
        }
    }
    segments.push((pending, cur));
    segments
        .into_iter()
        .map(|(conn, s)| (conn, s.trim().to_string()))
        .filter(|(_, s)| !s.is_empty())
        .collect()
}

/// Split `cmd` on top-level shell connectors (`;`, newline, `&&`, `||`, `|`,
/// background `&`), honouring single/double quotes and backslash escapes so we
/// never split inside a quoted string.  Used only on the parse-failure
/// fallback path, so subshell/`$()` boundaries are treated leniently — the
/// goal is to recover argv heads, not to be a faithful shell tokenizer.
fn split_top_level_connectors(cmd: &str) -> Vec<String> {
    split_top_level_segments(cmd)
        .into_iter()
        .map(|(_, s)| s)
        .collect()
}

fn try_parse(cmd: &str) -> Option<Vec<SimpleCommand>> {
    let mut out = Vec::new();
    let lexer = Lexer::new(cmd.chars());
    let parser = DefaultParser::new(lexer);
    for top in parser {
        match top {
            Ok(t) => visit_top_level(&t, &mut out),
            Err(_) => return None,
        }
    }
    Some(out)
}

fn parse_into(cmd: &str, out: &mut Vec<SimpleCommand>) {
    let lexer = Lexer::new(cmd.chars());
    let parser = DefaultParser::new(lexer);
    for top in parser {
        match top {
            Ok(t) => visit_top_level(&t, out),
            Err(_) => return,
        }
    }
}

fn visit_top_level(cmd: &TopLevelCommand<String>, out: &mut Vec<SimpleCommand>) {
    visit_command(&cmd.0, out);
}

fn visit_command(
    cmd: &Command<AndOrList<DefaultListableCommand>>,
    out: &mut Vec<SimpleCommand>,
) {
    match cmd {
        Command::Job(list) | Command::List(list) => {
            visit_listable(&list.first, out);
            for ao in &list.rest {
                match ao {
                    AndOr::And(c) | AndOr::Or(c) => visit_listable(c, out),
                }
            }
        }
    }
}

fn visit_listable(l: &DefaultListableCommand, out: &mut Vec<SimpleCommand>) {
    match l {
        ListableCommand::Single(p) => visit_pipeable(p, out),
        ListableCommand::Pipe(_, items) => {
            for p in items {
                visit_pipeable(p, out);
            }
        }
    }
}

fn visit_pipeable(p: &DefaultPipeableCommand, out: &mut Vec<SimpleCommand>) {
    match p {
        PipeableCommand::Simple(simple) => visit_simple(simple, out),
        PipeableCommand::Compound(c) => visit_compound(c, out),
        PipeableCommand::FunctionDef(_, body) => visit_compound(body, out),
    }
}

fn visit_compound(c: &DefaultCompoundCommand, out: &mut Vec<SimpleCommand>) {
    visit_compound_kind(&c.kind, out);
}

fn visit_compound_kind(
    k: &CompoundCommandKind<String, TopLevelWord<String>, TopLevelCommand<String>>,
    out: &mut Vec<SimpleCommand>,
) {
    match k {
        CompoundCommandKind::Brace(cmds) | CompoundCommandKind::Subshell(cmds) => {
            for c in cmds {
                visit_top_level(c, out);
            }
        }
        CompoundCommandKind::While(GuardBodyPair { guard, body })
        | CompoundCommandKind::Until(GuardBodyPair { guard, body }) => {
            for c in guard {
                visit_top_level(c, out);
            }
            for c in body {
                visit_top_level(c, out);
            }
        }
        CompoundCommandKind::If {
            conditionals,
            else_branch,
        } => {
            for gb in conditionals {
                for c in &gb.guard {
                    visit_top_level(c, out);
                }
                for c in &gb.body {
                    visit_top_level(c, out);
                }
            }
            if let Some(eb) = else_branch {
                for c in eb {
                    visit_top_level(c, out);
                }
            }
        }
        CompoundCommandKind::For { body, .. } => {
            for c in body {
                visit_top_level(c, out);
            }
        }
        CompoundCommandKind::Case { arms, .. } => {
            for arm in arms {
                for c in &arm.body {
                    visit_top_level(c, out);
                }
            }
        }
    }
}

fn visit_simple(s: &DefaultSimpleCommand, out: &mut Vec<SimpleCommand>) {
    let mut argv: Vec<String> = Vec::new();
    for r in &s.redirects_or_cmd_words {
        if let RedirectOrCmdWord::CmdWord(w) = r {
            argv.push(flatten_top_word(w).unwrap_or_default());
        }
    }
    // Visit command substitutions hidden inside the argv words.
    for r in &s.redirects_or_cmd_words {
        if let RedirectOrCmdWord::CmdWord(w) = r {
            visit_substs_in_complex(&w.0, out);
        }
    }
    // Env-var assignments like `GIT_TRACE=1 ...` show up in
    // `redirects_or_env_vars`; we don't emit them as argv (matching shell
    // semantics) but we still descend into their values to catch command
    // substitutions on the right-hand side.
    for r in &s.redirects_or_env_vars {
        if let RedirectOrEnvVar::EnvVar(_, Some(w)) = r {
            visit_substs_in_complex(&w.0, out);
        }
    }

    if argv.is_empty() {
        return;
    }

    // Recurse into the *string argument* of bash/sh/zsh -c, and eval's joined
    // args.  This is where a lot of evasion attempts hide.
    if argv.len() >= 2 {
        let head = argv[0].as_str();
        if matches!(head, "bash" | "sh" | "zsh") {
            if let Some(pos) = argv.iter().position(|a| a == "-c") {
                if let Some(script) = argv.get(pos + 1) {
                    parse_into(script, out);
                }
            }
        } else if head == "eval" {
            let joined = argv[1..].join(" ");
            parse_into(&joined, out);
        }
    }

    out.push(SimpleCommand { argv });
}

fn visit_substs_in_complex(c: &ComplexT, out: &mut Vec<SimpleCommand>) {
    match c {
        ComplexWord::Single(w) => visit_substs_in_word(w, out),
        ComplexWord::Concat(ws) => {
            for w in ws {
                visit_substs_in_word(w, out);
            }
        }
    }
}

fn visit_substs_in_word(w: &WordT, out: &mut Vec<SimpleCommand>) {
    match w {
        Word::Simple(s) => visit_substs_in_simple(s, out),
        Word::SingleQuoted(_) => {}
        Word::DoubleQuoted(parts) => {
            for s in parts {
                visit_substs_in_simple(s, out);
            }
        }
    }
}

fn visit_substs_in_simple(s: &SimpleT, out: &mut Vec<SimpleCommand>) {
    if let SimpleWord::Subst(b) = s {
        if let ParameterSubstitution::Command(cmds) = b.as_ref() {
            for c in cmds {
                visit_top_level(c, out);
            }
        }
    }
}

fn flatten_top_word(w: &TopLevelWord<String>) -> Option<String> {
    flatten_complex(&w.0)
}

fn flatten_complex(c: &ComplexT) -> Option<String> {
    match c {
        ComplexWord::Single(w) => flatten_word(w),
        ComplexWord::Concat(words) => {
            let mut out = String::new();
            for w in words {
                out.push_str(&flatten_word(w)?);
            }
            Some(out)
        }
    }
}

fn flatten_word(w: &WordT) -> Option<String> {
    match w {
        Word::Simple(s) => flatten_simple(s),
        Word::SingleQuoted(s) => Some(s.clone()),
        Word::DoubleQuoted(parts) => {
            let mut out = String::new();
            for p in parts {
                out.push_str(&flatten_simple(p)?);
            }
            Some(out)
        }
    }
}

fn flatten_simple(s: &SimpleT) -> Option<String> {
    match s {
        SimpleWord::Literal(s) | SimpleWord::Escaped(s) => Some(s.clone()),
        // Param / Subst can't be resolved statically; flatten to empty so the
        // surrounding token survives.  The recursion in `visit_substs_in_*`
        // handles the actual command substitutions separately.
        SimpleWord::Param(_) | SimpleWord::Subst(_) => Some(String::new()),
        SimpleWord::Star => Some("*".into()),
        SimpleWord::Question => Some("?".into()),
        SimpleWord::SquareOpen => Some("[".into()),
        SimpleWord::SquareClose => Some("]".into()),
        SimpleWord::Tilde => Some("~".into()),
        SimpleWord::Colon => Some(":".into()),
    }
}

/// Tokenise a rule string into argv-style tokens.  Mostly delegates to
/// `shell-words::split` so quoted multi-word tokens (`"git rebase -i"`) are
/// honoured.  Whitespace-only rules return an empty `Vec`, which never
/// matches anything (so a blank rule won't accidentally allow every command).
pub fn tokenize_rule(rule: &str) -> Vec<String> {
    shell_words::split(rule).unwrap_or_else(|_| {
        rule.split_whitespace().map(|s| s.to_string()).collect()
    })
}

/// Common command wrappers that *delegate* to the real command in their
/// remaining args.  When matching a rule against an argv, we also consider
/// the argv after stripping any of these wrappers (and their flags), so a
/// rule like `curl` will fire on `sudo -u root curl ...` and `env FOO=1
/// curl ...` just like it does on `curl ...` itself.
const SHELL_WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nohup", "time", "command", "builtin", "exec", "xargs",
];

/// Skip past any wrapper prefix in `argv` and return the slice starting at the
/// real command.  Idempotent: calling on an already-stripped argv is a no-op.
fn strip_wrappers(argv: &[String]) -> &[String] {
    let mut i = 0;
    while i < argv.len() {
        let tok = argv[i].as_str();
        if !SHELL_WRAPPERS.contains(&tok) {
            break;
        }
        i += 1;
        match tok {
            "sudo" | "doas" => {
                while i < argv.len() && argv[i].starts_with('-') {
                    let needs_value = matches!(argv[i].as_str(), "-u" | "-g" | "-p" | "-C");
                    i += 1;
                    if needs_value && i < argv.len() {
                        i += 1;
                    }
                }
            }
            "env" => {
                while i < argv.len() && argv[i].contains('=') {
                    i += 1;
                }
            }
            "xargs" => {
                while i < argv.len() && argv[i].starts_with('-') {
                    let needs_value = matches!(argv[i].as_str(), "-I" | "-n" | "-P" | "-J");
                    i += 1;
                    if needs_value && i < argv.len() {
                        i += 1;
                    }
                }
            }
            // nohup / time / command / builtin / exec: single wrapper token.
            _ => {}
        }
    }
    &argv[i..]
}

/// Returns `true` if any `SimpleCommand` extracted from `cmd_str` has an argv
/// that starts with `rule_tokens`.  Matching is whole-token: rule `["git",
/// "push"]` matches argv `["git", "push", "origin"]` but NOT `["git",
/// "pushdaemon"]` and NOT `["git", "status"]`.
///
/// The argv is also re-checked after stripping common wrappers (`sudo`,
/// `env`, `nohup`, `time`, ...) so a `curl` rule fires on `sudo curl ...`.
///
/// Additionally, flag tokens (`-x`, `--verbose`, `-s=clw`) are skipped before
/// matching so a rule `patchwright-cli eval` fires on argv
/// `["patchwright-cli", "-s=clw", "eval", ...]`.  This pairs with the
/// frontend's `computeLeafAllowPrefix`, which builds rule prefixes from
/// `argv[0]` + the first non-flag token.  A lone `-` is treated as an
/// argument (stdin/stdout), not a flag.
///
/// A `rule_tokens` of length 0 never matches.
pub fn argv_has_rule_prefix(argv: &[String], rule_tokens: &[String]) -> bool {
    if rule_tokens.is_empty() {
        return false;
    }
    if starts_with_tokens(argv, rule_tokens) {
        return true;
    }
    let stripped = strip_wrappers(argv);
    if stripped.len() != argv.len() && starts_with_tokens(stripped, rule_tokens) {
        return true;
    }
    let no_flags = strip_flag_tokens(argv);
    if no_flags.len() != argv.len() && starts_with_tokens(&no_flags, rule_tokens) {
        return true;
    }
    let stripped_no_flags = strip_flag_tokens(stripped);
    if stripped_no_flags.len() != stripped.len()
        && starts_with_tokens(&stripped_no_flags, rule_tokens)
    {
        return true;
    }
    false
}

/// A flag token is one starting with `-` and at least 2 chars long. A bare
/// `-` is a stdin/stdout sentinel for many tools (`cat -`), not a flag.
fn is_flag_token(tok: &str) -> bool {
    tok.len() > 1 && tok.starts_with('-')
}

/// Filter out flag tokens; preserve relative order of the remaining tokens.
/// Note: this is intentionally naive — for `-x value` (flag value as a
/// separate token) we leave `value` in place. Most modern CLIs accept the
/// `-x=value` form that this helper handles cleanly.
fn strip_flag_tokens(argv: &[String]) -> Vec<&String> {
    argv.iter().filter(|t| !is_flag_token(t)).collect()
}

fn starts_with_tokens<S: AsRef<str>>(argv: &[S], rule_tokens: &[String]) -> bool {
    if argv.len() < rule_tokens.len() {
        return false;
    }
    argv.iter()
        .zip(rule_tokens.iter())
        .all(|(a, b)| a.as_ref() == b.as_str())
}

/// Top-level convenience: parse `cmd_str` into SimpleCommands and check
/// whether any of them is prefixed by `rule_str`'s tokens.
pub fn cmd_matches_rule(cmd_str: &str, rule_str: &str) -> bool {
    let rule_tokens = tokenize_rule(rule_str);
    if rule_tokens.is_empty() {
        return false;
    }
    extract_simple_commands(cmd_str)
        .iter()
        .any(|sc| argv_has_rule_prefix(&sc.argv, &rule_tokens))
}

// ── Structured view (P8): for UI rendering ─────────────────────────────────

/// A linear, UI-oriented view of a shell command: every executable leaf and
/// the connector that joins it to the next one.  Designed to be sent over
/// the GuardRequest wire so the front-end can render a structured command
/// block (rather than one giant un-readable string).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandView {
    pub leaves: Vec<CommandLeaf>,
    /// `connectors.len() == leaves.len().saturating_sub(1)`.  The connector
    /// at index `i` joins `leaves[i]` to `leaves[i + 1]`.
    pub connectors: Vec<Connector>,
}

/// One executable leaf in the structured view.  `argv` is the same as the
/// matching engine sees; `nested` is set when the argv represents an
/// interpreter invocation (e.g. `bash -c "..."`, `python3 -c "..."`,
/// `eval "..."`) and the embedded script could itself be parsed.
///
/// `triggering` and `already_allowed` are populated by
/// [`crate::audit::annotate_view_with_flags`] before the view ships in a
/// `GuardRequest`; default-false leaves stay invisible in JSON so older
/// desktop clients keep round-tripping the wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandLeaf {
    pub argv: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nested: Option<NestedScript>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub triggering: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub already_allowed: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NestedScript {
    pub kind: NestedKind,
    /// The raw script string before re-parsing.
    pub raw: String,
    /// Parsed view of the script.  Boxed to keep the enclosing type small
    /// and to break the recursive size cycle.
    pub view: Box<CommandView>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NestedKind {
    /// `bash -c "..."`
    BashC,
    /// `sh -c "..."`
    ShC,
    /// `zsh -c "..."`
    ZshC,
    /// `python -c "..."` / `python3 -c "..."`
    PythonC,
    /// `node -e "..."`
    NodeE,
    /// `eval "..."`
    Eval,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Connector {
    /// `&&`
    And,
    /// `||`
    Or,
    /// `|`
    Pipe,
    /// `;` or newline between top-level commands
    Semi,
}

struct ViewBuilder {
    leaves: Vec<CommandLeaf>,
    connectors: Vec<Connector>,
}

impl ViewBuilder {
    fn new() -> Self {
        Self {
            leaves: Vec::new(),
            connectors: Vec::new(),
        }
    }

    fn push(&mut self, leaf: CommandLeaf, connector_before: Option<Connector>) {
        if !self.leaves.is_empty() {
            self.connectors
                .push(connector_before.unwrap_or(Connector::Semi));
        }
        self.leaves.push(leaf);
    }

    fn finish(self) -> CommandView {
        CommandView {
            leaves: self.leaves,
            connectors: self.connectors,
        }
    }
}

/// Build a structured view of `cmd`.  On parse failure, falls back to a
/// single leaf containing the tokenised argv.
pub fn extract_structured_view(cmd: &str) -> CommandView {
    let mut builder = ViewBuilder::new();
    let mut next_connector: Option<Connector> = None;
    let lexer = Lexer::new(cmd.chars());
    let parser = DefaultParser::new(lexer);
    let mut parse_ok = true;
    for top in parser {
        match top {
            Ok(t) => {
                visit_top_for_view(&t, &mut builder, &mut next_connector);
                // Between two top-level commands separated by newline,
                // treat the next leaf as Semi-joined.
                next_connector = Some(Connector::Semi);
            }
            Err(_) => {
                parse_ok = false;
                break;
            }
        }
    }
    if !parse_ok || builder.leaves.is_empty() {
        // Parse failed (or yielded nothing).  Don't keep the partially-parsed
        // prefix AND append the whole command flattened on top — that
        // duplicates leaves and produces a `cd`-headed blob whose argv carries
        // raw connectors (`;`, `|`) as tokens, which then mis-renders in the
        // guard card.  Instead, rebuild from connector-split segments so every
        // segment keeps its own argv head (mirrors `extract_simple_commands`).
        let mut fb = ViewBuilder::new();
        for (conn, seg) in split_top_level_segments(cmd) {
            for (i, sc) in salvage_segment(&seg).into_iter().enumerate() {
                let connector_before = if i == 0 { conn } else { Some(Connector::Semi) };
                fb.push(
                    CommandLeaf {
                        argv: sc.argv,
                        nested: None,
                        triggering: false,
                        already_allowed: false,
                    },
                    connector_before,
                );
            }
        }
        if !fb.leaves.is_empty() {
            return fb.finish();
        }
        // Last resort (e.g. unterminated quote → no recoverable segments):
        // keep the raw command as a single leaf so the UI still shows
        // something readable.
        let argv = shell_words::split(cmd).unwrap_or_else(|_| {
            cmd.split_whitespace().map(|s| s.to_string()).collect()
        });
        let mut single = ViewBuilder::new();
        if !argv.is_empty() {
            single.leaves.push(CommandLeaf {
                argv,
                nested: None,
                triggering: false,
                already_allowed: false,
            });
        }
        return single.finish();
    }
    builder.finish()
}

fn visit_top_for_view(
    cmd: &TopLevelCommand<String>,
    b: &mut ViewBuilder,
    next: &mut Option<Connector>,
) {
    visit_command_for_view(&cmd.0, b, next);
}

fn visit_command_for_view(
    cmd: &Command<AndOrList<DefaultListableCommand>>,
    b: &mut ViewBuilder,
    next: &mut Option<Connector>,
) {
    match cmd {
        Command::Job(list) | Command::List(list) => {
            visit_listable_for_view(&list.first, b, next);
            for ao in &list.rest {
                let (conn, body) = match ao {
                    AndOr::And(c) => (Connector::And, c),
                    AndOr::Or(c) => (Connector::Or, c),
                };
                *next = Some(conn);
                visit_listable_for_view(body, b, next);
            }
        }
    }
}

fn visit_listable_for_view(
    l: &DefaultListableCommand,
    b: &mut ViewBuilder,
    next: &mut Option<Connector>,
) {
    match l {
        ListableCommand::Single(p) => visit_pipeable_for_view(p, b, next),
        ListableCommand::Pipe(_, items) => {
            for (i, p) in items.iter().enumerate() {
                if i > 0 {
                    *next = Some(Connector::Pipe);
                }
                visit_pipeable_for_view(p, b, next);
            }
        }
    }
}

fn visit_pipeable_for_view(
    p: &DefaultPipeableCommand,
    b: &mut ViewBuilder,
    next: &mut Option<Connector>,
) {
    match p {
        PipeableCommand::Simple(simple) => visit_simple_for_view(simple, b, next),
        PipeableCommand::Compound(c) => visit_compound_for_view(c, b, next),
        PipeableCommand::FunctionDef(_, body) => visit_compound_for_view(body, b, next),
    }
}

fn visit_compound_for_view(
    c: &DefaultCompoundCommand,
    b: &mut ViewBuilder,
    next: &mut Option<Connector>,
) {
    let cmds: Vec<&TopLevelCommand<String>> = match &c.kind {
        CompoundCommandKind::Brace(cmds) | CompoundCommandKind::Subshell(cmds) => {
            cmds.iter().collect()
        }
        CompoundCommandKind::While(GuardBodyPair { guard, body })
        | CompoundCommandKind::Until(GuardBodyPair { guard, body }) => {
            guard.iter().chain(body.iter()).collect()
        }
        CompoundCommandKind::If {
            conditionals,
            else_branch,
        } => {
            let mut all = Vec::new();
            for gb in conditionals {
                all.extend(gb.guard.iter());
                all.extend(gb.body.iter());
            }
            if let Some(eb) = else_branch {
                all.extend(eb.iter());
            }
            all
        }
        CompoundCommandKind::For { body, .. } => body.iter().collect(),
        CompoundCommandKind::Case { arms, .. } => {
            arms.iter().flat_map(|a| a.body.iter()).collect()
        }
    };
    for (i, child) in cmds.iter().enumerate() {
        if i > 0 {
            *next = Some(Connector::Semi);
        }
        visit_top_for_view(child, b, next);
    }
}

fn visit_simple_for_view(
    s: &DefaultSimpleCommand,
    b: &mut ViewBuilder,
    next: &mut Option<Connector>,
) {
    let mut argv: Vec<String> = Vec::new();
    for r in &s.redirects_or_cmd_words {
        if let RedirectOrCmdWord::CmdWord(w) = r {
            argv.push(display_top_word(w));
        }
    }
    if argv.is_empty() {
        // Pure variable assignment (e.g. `task_id=$(curl ... | jq ...)`).
        // conch-parser parks the LHS in `redirects_or_env_vars` with no
        // cmd word, so there's nothing to display as a leaf — but the RHS
        // command substitution is real work the user needs to see.
        // Splice its commands into the parent builder so they appear as
        // first-class leaves with their natural connectors.
        for r in &s.redirects_or_env_vars {
            if let RedirectOrEnvVar::EnvVar(_, Some(w)) = r {
                splice_substs_from_complex(&w.0, b, next);
            }
        }
        return;
    }
    let nested = detect_nested(&argv);
    let conn = next.take();
    b.push(
        CommandLeaf {
            argv,
            nested,
            triggering: false,
            already_allowed: false,
        },
        conn,
    );
}

fn splice_substs_from_complex(c: &ComplexT, b: &mut ViewBuilder, next: &mut Option<Connector>) {
    match c {
        ComplexWord::Single(w) => splice_substs_from_word(w, b, next),
        ComplexWord::Concat(words) => {
            for w in words {
                splice_substs_from_word(w, b, next);
            }
        }
    }
}

fn splice_substs_from_word(w: &WordT, b: &mut ViewBuilder, next: &mut Option<Connector>) {
    match w {
        Word::Simple(s) => splice_substs_from_simple(s, b, next),
        Word::SingleQuoted(_) => {}
        Word::DoubleQuoted(parts) => {
            for p in parts {
                splice_substs_from_simple(p, b, next);
            }
        }
    }
}

fn splice_substs_from_simple(s: &SimpleT, b: &mut ViewBuilder, next: &mut Option<Connector>) {
    if let SimpleWord::Subst(boxed) = s {
        if let ParameterSubstitution::Command(cmds) = boxed.as_ref() {
            for c in cmds {
                visit_top_for_view(c, b, next);
                *next = Some(Connector::Semi);
            }
        }
    }
}

// ── Display-aware flattening: preserve `$VAR` / `$(...)` literally ─────────
//
// The matching path (`flatten_top_word`) collapses Param/Subst to "" because
// it can't know the runtime value.  But for the UI we want the user to *see*
// the variable interpolation and command substitution that's about to run —
// otherwise `echo "task=$id"` renders as just `echo task=`, which hides the
// actual data flow.  These helpers reconstruct the original surface syntax
// from the AST so the structured view is readable.

fn display_top_word(w: &TopLevelWord<String>) -> String {
    display_complex(&w.0)
}

fn display_complex(c: &ComplexT) -> String {
    match c {
        ComplexWord::Single(w) => display_word(w),
        ComplexWord::Concat(words) => {
            let mut out = String::new();
            for w in words {
                out.push_str(&display_word(w));
            }
            out
        }
    }
}

fn display_word(w: &WordT) -> String {
    match w {
        Word::Simple(s) => display_simple(s),
        Word::SingleQuoted(s) => s.clone(),
        Word::DoubleQuoted(parts) => {
            let mut out = String::new();
            for p in parts {
                out.push_str(&display_simple(p));
            }
            out
        }
    }
}

fn display_simple(s: &SimpleT) -> String {
    match s {
        SimpleWord::Literal(s) | SimpleWord::Escaped(s) => s.clone(),
        SimpleWord::Param(p) => display_param(p),
        SimpleWord::Subst(b) => display_subst(b.as_ref()),
        SimpleWord::Star => "*".into(),
        SimpleWord::Question => "?".into(),
        SimpleWord::SquareOpen => "[".into(),
        SimpleWord::SquareClose => "]".into(),
        SimpleWord::Tilde => "~".into(),
        SimpleWord::Colon => ":".into(),
    }
}

fn display_param(p: &Parameter<String>) -> String {
    match p {
        Parameter::At => "$@".into(),
        Parameter::Star => "$*".into(),
        Parameter::Pound => "$#".into(),
        Parameter::Question => "$?".into(),
        Parameter::Dash => "$-".into(),
        Parameter::Dollar => "$$".into(),
        Parameter::Bang => "$!".into(),
        Parameter::Positional(n) => format!("${}", n),
        Parameter::Var(name) => format!("${}", name),
    }
}

fn display_subst(s: &SubstT) -> String {
    fn name_of(p: &Parameter<String>) -> String {
        let raw = display_param(p);
        raw.trim_start_matches('$').to_string()
    }
    match s {
        ParameterSubstitution::Command(cmds) => {
            let mut inner = String::new();
            for (i, c) in cmds.iter().enumerate() {
                if i > 0 {
                    inner.push_str("; ");
                }
                inner.push_str(&reconstruct_top_level(c));
            }
            format!("$({})", inner)
        }
        ParameterSubstitution::Arith(_) => "$((..))".into(),
        ParameterSubstitution::Len(p) => format!("${{#{}}}", name_of(p)),
        ParameterSubstitution::Default(colon, p, _) => {
            format!("${{{}{}-..}}", name_of(p), if *colon { ":" } else { "" })
        }
        ParameterSubstitution::Assign(colon, p, _) => {
            format!("${{{}{}=..}}", name_of(p), if *colon { ":" } else { "" })
        }
        ParameterSubstitution::Error(colon, p, _) => {
            format!("${{{}{}?..}}", name_of(p), if *colon { ":" } else { "" })
        }
        ParameterSubstitution::Alternative(colon, p, _) => {
            format!("${{{}{}+..}}", name_of(p), if *colon { ":" } else { "" })
        }
        ParameterSubstitution::RemoveSmallestSuffix(p, _) => format!("${{{}%..}}", name_of(p)),
        ParameterSubstitution::RemoveLargestSuffix(p, _) => format!("${{{}%%..}}", name_of(p)),
        ParameterSubstitution::RemoveSmallestPrefix(p, _) => format!("${{{}#..}}", name_of(p)),
        ParameterSubstitution::RemoveLargestPrefix(p, _) => format!("${{{}##..}}", name_of(p)),
    }
}

fn reconstruct_top_level(cmd: &TopLevelCommand<String>) -> String {
    reconstruct_command(&cmd.0)
}

fn reconstruct_command(cmd: &Command<AndOrList<DefaultListableCommand>>) -> String {
    match cmd {
        Command::Job(list) | Command::List(list) => {
            let mut out = reconstruct_listable(&list.first);
            for ao in &list.rest {
                let (sep, body) = match ao {
                    AndOr::And(c) => (" && ", c),
                    AndOr::Or(c) => (" || ", c),
                };
                out.push_str(sep);
                out.push_str(&reconstruct_listable(body));
            }
            out
        }
    }
}

fn reconstruct_listable(l: &DefaultListableCommand) -> String {
    match l {
        ListableCommand::Single(p) => reconstruct_pipeable(p),
        ListableCommand::Pipe(_, items) => items
            .iter()
            .map(reconstruct_pipeable)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn reconstruct_pipeable(p: &DefaultPipeableCommand) -> String {
    match p {
        PipeableCommand::Simple(s) => reconstruct_simple(s),
        PipeableCommand::Compound(_) | PipeableCommand::FunctionDef(_, _) => "(..)".into(),
    }
}

fn reconstruct_simple(s: &DefaultSimpleCommand) -> String {
    let mut parts: Vec<String> = Vec::new();
    for r in &s.redirects_or_env_vars {
        if let RedirectOrEnvVar::EnvVar(name, value) = r {
            match value {
                Some(v) => parts.push(format!("{}={}", name, display_top_word(v))),
                None => parts.push(format!("{}=", name)),
            }
        }
    }
    for r in &s.redirects_or_cmd_words {
        if let RedirectOrCmdWord::CmdWord(w) = r {
            parts.push(display_top_word(w));
        }
    }
    parts.join(" ")
}

fn detect_nested(argv: &[String]) -> Option<NestedScript> {
    if argv.len() < 2 {
        return None;
    }
    let head = argv[0].as_str();
    // The "interpreter -c <script>" / "node -e <script>" / "eval <script>" pattern.
    let (kind, script_str) = match head {
        "bash" | "sh" | "zsh" => {
            let pos = argv.iter().position(|a| a == "-c")?;
            let script = argv.get(pos + 1)?.clone();
            let kind = match head {
                "bash" => NestedKind::BashC,
                "sh" => NestedKind::ShC,
                _ => NestedKind::ZshC,
            };
            (kind, script)
        }
        "python" | "python3" | "python2" => {
            let pos = argv.iter().position(|a| a == "-c")?;
            let script = argv.get(pos + 1)?.clone();
            (NestedKind::PythonC, script)
        }
        "node" => {
            let pos = argv.iter().position(|a| a == "-e")?;
            let script = argv.get(pos + 1)?.clone();
            (NestedKind::NodeE, script)
        }
        "eval" => {
            let script = argv[1..].join(" ");
            (NestedKind::Eval, script)
        }
        _ => return None,
    };
    // Python scripts aren't shell — don't re-parse them with conch-parser.
    let inner_view = if matches!(kind, NestedKind::PythonC | NestedKind::NodeE) {
        // Treat the script body as a single opaque "leaf" so the UI can
        // still show the lines indented but we don't pretend it's bash.
        CommandView {
            leaves: vec![CommandLeaf {
                argv: vec![script_str.clone()],
                nested: None,
                triggering: false,
                already_allowed: false,
            }],
            connectors: Vec::new(),
        }
    } else {
        extract_structured_view(&script_str)
    };
    Some(NestedScript {
        kind,
        raw: script_str,
        view: Box::new(inner_view),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argvs(cmd: &str) -> Vec<Vec<String>> {
        extract_simple_commands(cmd)
            .into_iter()
            .map(|s| s.argv)
            .collect()
    }

    fn contains_argv_prefix(cmd: &str, prefix: &[&str]) -> bool {
        let prefix: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
        extract_simple_commands(cmd)
            .into_iter()
            .any(|s| s.argv.starts_with(&prefix))
    }

    #[test]
    fn plain_command() {
        assert_eq!(argvs("git push"), vec![vec!["git", "push"]]);
    }

    #[test]
    fn and_separator() {
        assert!(contains_argv_prefix(
            "sleep 2 && git push origin main",
            &["git", "push"]
        ));
    }

    #[test]
    fn or_separator() {
        assert!(contains_argv_prefix("false || git push", &["git", "push"]));
    }

    #[test]
    fn semicolon_separator() {
        assert!(contains_argv_prefix("echo hi; git push", &["git", "push"]));
    }

    #[test]
    fn pipe_separator() {
        let v = argvs("echo abc | grep b");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], vec!["echo", "abc"]);
        assert_eq!(v[1], vec!["grep", "b"]);
    }

    #[test]
    fn subshell() {
        assert!(contains_argv_prefix("(git push)", &["git", "push"]));
    }

    #[test]
    fn brace_group() {
        assert!(contains_argv_prefix("{ git push; }", &["git", "push"]));
    }

    #[test]
    fn command_substitution_dollar_paren() {
        assert!(contains_argv_prefix("echo $(git push)", &["git", "push"]));
    }

    #[test]
    fn command_substitution_backtick() {
        assert!(contains_argv_prefix("echo `git push`", &["git", "push"]));
    }

    #[test]
    fn bash_dash_c() {
        assert!(contains_argv_prefix(
            r#"bash -c "git push origin main""#,
            &["git", "push", "origin", "main"]
        ));
    }

    #[test]
    fn sh_dash_c() {
        assert!(contains_argv_prefix(
            r#"sh -c "git push""#,
            &["git", "push"]
        ));
    }

    #[test]
    fn eval_string() {
        assert!(contains_argv_prefix(r#"eval "git push""#, &["git", "push"]));
    }

    #[test]
    fn nested_compounds() {
        assert!(contains_argv_prefix(
            "(sleep 1 && (echo x | git push))",
            &["git", "push"]
        ));
    }

    #[test]
    fn env_prefix_does_not_pollute_argv() {
        let v = argvs("GIT_TRACE=1 git push origin main");
        assert!(v.iter().any(|argv| argv
            == &vec!["git", "push", "origin", "main"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()));
        // No SimpleCommand starts with GIT_TRACE=1.
        assert!(!v.iter().any(|argv| argv[0].contains('=')));
    }

    #[test]
    fn if_then() {
        assert!(contains_argv_prefix(
            "if true; then git push; fi",
            &["git", "push"]
        ));
    }

    #[test]
    fn heredoc_followed_by_command() {
        assert!(contains_argv_prefix(
            "cat <<EOF\nhello\nEOF\ngit push",
            &["git", "push"]
        ));
    }

    #[test]
    fn fallback_on_parse_error() {
        // Unterminated double quote — parser should fail and we fall back to
        // shell-words tokenise.  shell-words will *also* fail on the
        // unterminated quote; for malformed input we accept returning empty
        // rather than crashing.
        let v = argvs("git push \"unterminated");
        // Either parser recovered something, or fallback returned empty —
        // neither should panic.
        assert!(v.iter().all(|argv| !argv.is_empty()) || v.is_empty());
    }

    #[test]
    fn compound_with_unparseable_segment_keeps_other_leaves() {
        // Regression: a long compound command where ONE segment uses
        // non-shell syntax (a JS arrow function `() =>`) must not collapse the
        // whole command into a single flat argv headed by the first word
        // (`cd`).  If it did, prefix-based guard allow-rules like
        // `patchwright-cli eval` would never match and the guard would prompt
        // even though the user already signed an allow rule for it.
        let cmd = "cd /tmp/web ; ls dist/*.js | xargs basename ; cd /tmp \
                   && patchwright-cli goto http://localhost:8899/x.html ; sleep 4 ; \
                   patchwright-cli eval () => document.getElementById('result').textContent \
                   | grep -oE '\\{.*\\}' | head -1";
        assert!(
            cmd_matches_rule(cmd, "patchwright-cli eval"),
            "the `patchwright-cli eval` leaf should still be recoverable from a \
             compound command containing an unparseable JS-arrow segment"
        );
        // The unrelated leaves should also survive as their own commands.
        assert!(contains_argv_prefix(cmd, &["patchwright-cli", "goto"]));
        assert!(contains_argv_prefix(cmd, &["sleep"]));
    }

    #[test]
    fn empty_command_yields_no_simples() {
        assert!(argvs("").is_empty());
        assert!(argvs("   ").is_empty());
    }

    #[test]
    fn no_match_when_binary_differs() {
        assert!(!contains_argv_prefix("git status", &["git", "push"]));
    }

    // ── cmd_matches_rule (P3) ──────────────────────────────────────────────

    #[test]
    fn rule_matches_plain() {
        assert!(cmd_matches_rule("git push", "git push"));
        assert!(cmd_matches_rule("git push origin main", "git push"));
    }

    #[test]
    fn rule_blocks_different_subcommand() {
        assert!(!cmd_matches_rule("git status", "git push"));
    }

    #[test]
    fn rule_respects_token_boundary() {
        // `git pushdaemon` should NOT be matched by `git push` — tokens are
        // compared as whole words, not character prefix.
        assert!(!cmd_matches_rule("git pushdaemon now", "git push"));
    }

    #[test]
    fn rule_matches_through_and() {
        assert!(cmd_matches_rule(
            "sleep 2 && git push origin main",
            "git push"
        ));
    }

    #[test]
    fn rule_matches_through_bash_dash_c() {
        assert!(cmd_matches_rule(
            r#"bash -c "git push origin main""#,
            "git push"
        ));
    }

    #[test]
    fn rule_matches_through_subshell() {
        assert!(cmd_matches_rule("(git push)", "git push"));
    }

    #[test]
    fn rule_matches_through_cmdsubst() {
        assert!(cmd_matches_rule("echo $(git push)", "git push"));
    }

    #[test]
    fn rule_matches_through_eval() {
        assert!(cmd_matches_rule(r#"eval "git push""#, "git push"));
    }

    #[test]
    fn rule_empty_never_matches() {
        assert!(!cmd_matches_rule("git push", ""));
        assert!(!cmd_matches_rule("git push", "   "));
    }

    #[test]
    fn rule_quoted_multi_word_token() {
        // shell-words tokenisation of a quoted rule.
        let toks = tokenize_rule(r#""git push" origin"#);
        assert_eq!(toks, vec!["git push", "origin"]);
    }

    #[test]
    fn tight_pipe_without_spaces() {
        // `echo hi |nc foo` — no space between `|` and `nc`.
        assert!(contains_argv_prefix("echo hi |nc foo", &["nc"]));
        assert!(contains_argv_prefix("echo hi|nc foo", &["nc"]));
    }

    #[test]
    fn rule_matches_through_sudo_wrapper() {
        assert!(cmd_matches_rule(
            "sudo curl https://example.com",
            "curl"
        ));
        assert!(cmd_matches_rule(
            "sudo -u root curl https://example.com",
            "curl"
        ));
    }

    #[test]
    fn rule_matches_through_env_wrapper() {
        assert!(cmd_matches_rule(
            "env FOO=1 BAR=2 curl https://example.com",
            "curl"
        ));
    }

    #[test]
    fn rule_matches_through_nohup_time() {
        assert!(cmd_matches_rule("nohup curl https://e.com &", "curl"));
        assert!(cmd_matches_rule("time curl https://e.com", "curl"));
    }

    #[test]
    fn sudo_rule_still_matches_sudo_invocation() {
        // The sudo audit rule itself must still fire on `sudo X`.
        assert!(cmd_matches_rule("sudo curl https://e.com", "sudo"));
    }

    // ── Flag-skipping prefix match (guard-always-allow-prefix) ──────────────
    //
    // Frontend `computeLeafAllowPrefix` builds the rule from `argv[0]` plus
    // the first non-flag argv token (so `patchwright-cli -s=clw eval` →
    // `patchwright-cli eval`). For that rule to actually short-circuit the
    // next invocation, matching must skip flag tokens in the live argv too.

    #[test]
    fn rule_matches_when_flag_sits_between_command_and_subcommand() {
        // The exact case from Boss's screenshot.
        assert!(cmd_matches_rule(
            r#"patchwright-cli -s=clw eval "() => ({})""#,
            "patchwright-cli eval"
        ));
    }

    #[test]
    fn rule_matches_when_multiple_single_token_flags_precede_subcommand() {
        assert!(cmd_matches_rule(
            "patchwright-cli --verbose -s=clw eval '({})'",
            "patchwright-cli eval"
        ));
    }

    #[test]
    fn rule_still_blocks_wrong_subcommand_after_flag() {
        // Skipping flags must not whitelist a *different* subcommand.
        assert!(!cmd_matches_rule(
            "patchwright-cli -s=clw screenshot foo.png",
            "patchwright-cli eval"
        ));
    }

    #[test]
    fn lone_dash_is_not_a_flag() {
        // `cat -` reads stdin; the `-` is an argument, not a flag.
        assert!(cmd_matches_rule("cat -", "cat -"));
    }

    #[test]
    fn rule_matches_when_env_var_prefixes_command_with_flag() {
        // Boss's actual screenshot: env var + flag + subcommand in one leaf.
        // conch_parser already strips `TASK_ID=...` from argv, then
        // strip_flag_tokens drops `-s=clw`, leaving ["patchwright-cli","eval",...]
        // which `patchwright-cli eval` matches.
        assert!(cmd_matches_rule(
            r#"TASK_ID=abc123 patchwright-cli -s=clw eval "() => ({})""#,
            "patchwright-cli eval"
        ));
    }

    #[test]
    fn rule_matches_when_env_vars_and_flags_combine_in_pipeline() {
        // Multi-leaf pipeline where the env-var-prefixed leaf is in the middle.
        assert!(cmd_matches_rule(
            r#"echo go | TASK_ID=abc patchwright-cli -s=clw eval "(...)" | tail -10"#,
            "patchwright-cli eval"
        ));
    }

    // ── Structured view tests (P8) ─────────────────────────────────────────

    fn view_argvs(view: &CommandView) -> Vec<Vec<String>> {
        view.leaves.iter().map(|l| l.argv.clone()).collect()
    }

    #[test]
    fn view_plain_command() {
        let v = extract_structured_view("git push");
        assert_eq!(view_argvs(&v), vec![vec!["git".to_string(), "push".into()]]);
        assert!(v.connectors.is_empty());
    }

    #[test]
    fn view_and_or_pipe_semi_connectors() {
        let v = extract_structured_view("a && b || c | d; e");
        assert_eq!(v.leaves.len(), 5);
        assert_eq!(
            v.connectors,
            vec![
                Connector::And,
                Connector::Or,
                Connector::Pipe,
                Connector::Semi
            ]
        );
    }

    #[test]
    fn view_bash_c_nests() {
        let v = extract_structured_view(r#"bash -c "git push origin main""#);
        assert_eq!(v.leaves.len(), 1);
        let leaf = &v.leaves[0];
        assert_eq!(leaf.argv[0], "bash");
        let nested = leaf.nested.as_ref().expect("expected nested");
        assert_eq!(nested.kind, NestedKind::BashC);
        assert_eq!(nested.raw, "git push origin main");
        assert_eq!(
            view_argvs(&nested.view),
            vec![vec!["git", "push", "origin", "main"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()]
        );
    }

    #[test]
    fn view_python_c_treats_script_as_opaque() {
        let v = extract_structured_view(
            r#"python3 -c "import os; os.system('rm -rf /')""#,
        );
        let nested = v.leaves[0].nested.as_ref().expect("python nested");
        assert_eq!(nested.kind, NestedKind::PythonC);
        // Python script is a single opaque leaf — no shell-parsing of `;`.
        assert_eq!(nested.view.leaves.len(), 1);
        assert!(nested.view.connectors.is_empty());
    }

    #[test]
    fn view_eval_nests_and_reparses() {
        let v = extract_structured_view(r#"eval "git push && echo done""#);
        let nested = v.leaves[0].nested.as_ref().expect("eval nested");
        assert_eq!(nested.kind, NestedKind::Eval);
        assert_eq!(nested.view.leaves.len(), 2);
        assert_eq!(nested.view.connectors, vec![Connector::And]);
    }

    #[test]
    fn view_fallback_on_parse_error() {
        // Malformed quoting — we should still return *something* readable.
        let v = extract_structured_view("git push \"unterminated");
        // Either parser recovered, or fallback gave us a single leaf.
        // Never empty, never a panic.
        // (shell-words may itself reject the unterminated quote; if so the
        // view ends up empty. Both outcomes are acceptable provided no panic.)
        let _ = v;
    }

    #[test]
    fn view_compound_with_unparseable_segment_no_flat_blob() {
        // Regression: a compound command where ONE segment uses non-shell
        // syntax (a JS arrow function `() =>`) used to make the view append a
        // duplicate giant leaf — the *whole* command flattened into one argv
        // headed by `cd`, with shell connectors (`;`, `|`) surviving AS argv
        // tokens.  That blob then mis-rendered in the guard card and got a
        // stray `triggering=true`.  After the fix the view must expose each
        // segment as its own leaf with its own head, and no leaf may contain a
        // raw connector token.
        let cmd = "cd /tmp/web ; ls dist/*.js | xargs basename ; cd /tmp \
                   && patchwright-cli goto http://localhost:8899/x.html ; sleep 4 ; \
                   patchwright-cli eval () => document.getElementById('result').textContent \
                   | grep -oE '\\{.*\\}' | head -1";
        let v = extract_structured_view(cmd);
        for leaf in &v.leaves {
            assert!(
                !leaf.argv.iter().any(|t| matches!(t.as_str(), ";" | "|" | "&&" | "||" | "&")),
                "no leaf may carry a raw shell connector as an argv token; got {:?}",
                leaf.argv
            );
        }
        // The eval segment must surface as a leaf headed by `patchwright-cli eval`.
        assert!(
            v.leaves.iter().any(|l| l.argv.starts_with(&[
                "patchwright-cli".to_string(),
                "eval".to_string()
            ])),
            "expected a `patchwright-cli eval` leaf; got {:?}",
            v.leaves.iter().map(|l| &l.argv).collect::<Vec<_>>()
        );
        // And the `cd` leaf must be just `cd <dir>`, not the whole flattened chain.
        let cd_leaves: Vec<_> = v.leaves.iter().filter(|l| l.argv.first().map(|s| s == "cd").unwrap_or(false)).collect();
        assert!(
            cd_leaves.iter().all(|l| l.argv.len() <= 2),
            "a `cd` leaf ballooned into a flattened chain: {:?}",
            cd_leaves.iter().map(|l| &l.argv).collect::<Vec<_>>()
        );
    }

    #[test]
    fn view_subshell_flattens_to_sequential_leaves() {
        let v = extract_structured_view("(echo a && echo b)");
        assert_eq!(v.leaves.len(), 2);
        assert_eq!(v.connectors, vec![Connector::And]);
    }

    #[test]
    fn view_nested_bash_then_python() {
        let v = extract_structured_view(
            r#"bash -c "python3 -c \"print(1)\"""#,
        );
        // outer bash leaf
        let outer = &v.leaves[0];
        let bash_nested = outer.nested.as_ref().expect("bash nested");
        assert_eq!(bash_nested.kind, NestedKind::BashC);
        // inside bash -c there's a python invocation
        let inner_python_leaf = &bash_nested.view.leaves[0];
        let py_nested = inner_python_leaf
            .nested
            .as_ref()
            .expect("python nested inside bash");
        assert_eq!(py_nested.kind, NestedKind::PythonC);
    }

    #[test]
    fn view_serializes_to_json_round_trip() {
        let v = extract_structured_view("sleep 2 && git push");
        let json = serde_json::to_string(&v).unwrap();
        let back: CommandView = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn view_preserves_variable_interpolation() {
        // Bug A: `echo "task=$id"` used to render as `["echo", "task="]`
        // because Param flattened to "" in the display path.
        let v = extract_structured_view(r#"echo "task=$id""#);
        assert_eq!(view_argvs(&v), vec![vec!["echo".to_string(), "task=$id".into()]]);
    }

    #[test]
    fn view_preserves_param_in_unquoted_token() {
        let v = extract_structured_view(r#"curl -H "Authorization: Bearer $TOKEN" url"#);
        assert_eq!(
            view_argvs(&v),
            vec![vec![
                "curl".to_string(),
                "-H".into(),
                "Authorization: Bearer $TOKEN".into(),
                "url".into(),
            ]]
        );
    }

    #[test]
    fn view_preserves_command_substitution_placeholder() {
        let v = extract_structured_view(r#"echo "today=$(date)""#);
        let argv = &v.leaves[0].argv;
        assert_eq!(argv[0], "echo");
        // The token should preserve the substitution so the user sees that
        // `date` is part of what runs, not a bare `today=`.
        assert!(
            argv[1].contains("$(date)"),
            "expected $(date) inside token, got {:?}",
            argv[1]
        );
    }

    #[test]
    fn view_expands_pure_assignment_subst_into_leaves() {
        // Bug B: `task_id=$(curl ... | jq ...)` is a SimpleCommand with no
        // cmd word — used to be skipped entirely, hiding curl + jq from the
        // user.  Now the inner pipeline is spliced in as proper leaves.
        let v = extract_structured_view(
            r#"task_id=$(curl -s localhost:9090/x | jq -r .id) ; echo done"#,
        );
        let argvs = view_argvs(&v);
        assert_eq!(argvs.len(), 3, "got leaves: {:?}", argvs);
        assert_eq!(argvs[0], vec!["curl", "-s", "localhost:9090/x"]);
        assert_eq!(argvs[1], vec!["jq", "-r", ".id"]);
        assert_eq!(argvs[2], vec!["echo", "done"]);
        assert_eq!(v.connectors, vec![Connector::Pipe, Connector::Semi]);
    }

    #[test]
    fn view_assignment_before_other_command() {
        // Make sure a leading assignment followed by another top-level cmd
        // gets the connectors right: curl, jq joined by Pipe, then Semi to
        // the echo.
        let v = extract_structured_view(
            r#"x=$(curl a | jq .) ; echo done"#,
        );
        assert_eq!(v.leaves.len(), 3);
        assert_eq!(v.connectors, vec![Connector::Pipe, Connector::Semi]);
    }

    #[test]
    fn rule_binary_only_matches_any_subcommand() {
        // Coarse rule `git` matches every `git ...` invocation — by design,
        // matches old semantics for users who want the broad rule.
        assert!(cmd_matches_rule("git status", "git"));
        assert!(cmd_matches_rule("git push", "git"));
    }

    // ── Serde compatibility for triggering / already_allowed flags ──────────

    #[test]
    fn leaf_default_flags_are_omitted_from_json() {
        let leaf = CommandLeaf {
            argv: vec!["ls".into(), "-la".into()],
            nested: None,
            triggering: false,
            already_allowed: false,
        };
        let json = serde_json::to_string(&leaf).unwrap();
        assert!(!json.contains("triggering"), "default-false must skip serializing: {json}");
        assert!(!json.contains("alreadyAllowed"), "default-false must skip serializing: {json}");
        assert!(!json.contains("already_allowed"), "default-false must skip serializing: {json}");
    }

    #[test]
    fn leaf_truthy_flags_appear_in_json_and_round_trip() {
        let leaf = CommandLeaf {
            argv: vec!["eval".into(), "() => 1".into()],
            nested: None,
            triggering: true,
            already_allowed: true,
        };
        let json = serde_json::to_string(&leaf).unwrap();
        assert!(json.contains("\"triggering\":true"), "got {json}");
        // serde inherits container rename if any; cmd_ast::CommandLeaf has no
        // rename_all, so the field stays snake_case on the wire.
        assert!(json.contains("\"already_allowed\":true"), "got {json}");

        let decoded: CommandLeaf = serde_json::from_str(&json).unwrap();
        assert!(decoded.triggering);
        assert!(decoded.already_allowed);
    }

    #[test]
    fn leaf_legacy_json_without_flags_still_parses() {
        // Pre-flag CLI versions ship CommandLeaf JSON without these fields.
        // serde(default) keeps them deserializing as false.
        let legacy = r#"{"argv":["ls","-la"]}"#;
        let decoded: CommandLeaf = serde_json::from_str(legacy).unwrap();
        assert!(!decoded.triggering);
        assert!(!decoded.already_allowed);
    }
}
