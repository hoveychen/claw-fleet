#!/usr/bin/env node
/*
 * check-css-tokens — guard against the class of theming bug where a stylesheet
 * references a CSS custom property (`var(--foo)`) that is never defined anywhere.
 *
 * Why: FilesView.module.css shipped an entire file wired to a set of
 * Tokyo-Night token names (`--text-secondary`, `--accent`, `--border`, …) that
 * were never defined in this app. Because every reference had a hardcoded
 * fallback, the fallback silently rendered — cold grays/blues that clashed with
 * the warm paper light theme and looked fine to the type checker. Nothing
 * caught it. This lint does.
 *
 * Checks:
 *   [error] var(--x) where --x is never defined in any scanned CSS → exit 1.
 *   [error] var(--x) where --x is DEPRECATED → exit 1. These names still resolve
 *           (App.css keeps them as aliases until every consumer is migrated), so
 *           nothing else would ever catch a fresh reference — it would just quietly
 *           re-open the split the alias was created to close.
 *   [warn]  bare hardcoded color literals in a .module.css outside a var()
 *           fallback → printed, does NOT fail the build (semantic colors and
 *           deliberate fixed canvases are legitimate; this is advisory only).
 *   [warn]  border-radius with a raw px value off the --radius-* ladder.
 *
 * Run from claw-fleet-desktop/: `node scripts/check-css-tokens.mjs`
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const APP_DIR = "app";

/** Recursively collect every .css file under dir. */
function collectCss(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) collectCss(p, out);
    else if (name.endsWith(".css")) out.push(p);
  }
  return out;
}

// Custom properties injected at runtime via inline style (element.style), so
// they are intentionally never defined in CSS. Not a bug — exempt from the check.
//   --drift: Onboarding.tsx sets it per confetti particle for the fall animation.
const RUNTIME_INJECTED = new Set(["--drift"]);

// Retired token names — tombstones. App.css no longer defines them, so the
// undefined-var check below would already catch a reference; this map is kept so
// the failure says WHICH token to use instead of just "undefined". (During the
// migration they were live aliases, and this list was the only thing that could
// see a regression, because the name still resolved.)
const DEPRECATED = new Map([
  ["--color-bg-input", "--color-bg-field (form controls are a RAISED surface)"],
  ["--color-bg-tertiary", "--color-bg-sunken (the one recessed surface role)"],
  ["--color-bg-sidebar", "--color-bg (it was a no-op alias of the page canvas)"],
]);

// Every value on the --radius-* ladder now reads as var(--radius-x), so a raw px
// border-radius means a new step got invented. 1px/1.5px hairlines sit below the
// smallest token and stay raw.
const RAW_RADIUS_OK = new Set(["1px", "1.5px"]);
const RADIUS_RE = /border-radius:\s*(\d+(?:\.\d+)?px)\s*;/;

const DEF_RE = /(--[A-Za-z0-9-]+)\s*:/g; // `--x:` — only ever a definition
const USE_RE = /var\(\s*(--[A-Za-z0-9-]+)/g; // `var(--x` — a reference

// Bare color literals for the advisory warning. Deliberately excludes anything
// inside var(...) (handled by stripping var() first) and pure-black overlays
// rgba(0,0,0,…) which are hue-neutral scrims/shadows.
const HEX_RE = /#[0-9a-fA-F]{3,8}\b/g;
const RGB_RE = /rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/g;

const files = collectCss(APP_DIR);

const defined = new Set();
for (const f of files) {
  const text = readFileSync(f, "utf8");
  let m;
  while ((m = DEF_RE.exec(text))) defined.add(m[1]);
}

const undefinedRefs = []; // {file, line, token}
const deprecatedRefs = []; // {file, line, token, replacement}
const bareColors = []; // {file, line, snippet}
const offLadderRadii = []; // {file, line, value}

for (const f of files) {
  const rel = relative(".", f);
  const isTokenSource = rel.endsWith("App.css"); // where the aliases are declared
  const lines = readFileSync(f, "utf8").split("\n");
  lines.forEach((line, i) => {
    // undefined / deprecated var references
    let m;
    USE_RE.lastIndex = 0;
    while ((m = USE_RE.exec(line))) {
      if (!defined.has(m[1]) && !RUNTIME_INJECTED.has(m[1])) {
        undefinedRefs.push({ file: rel, line: i + 1, token: m[1] });
      }
      if (DEPRECATED.has(m[1]) && !isTokenSource) {
        deprecatedRefs.push({ file: rel, line: i + 1, token: m[1], replacement: DEPRECATED.get(m[1]) });
      }
    }
    // advisory: a border-radius that isn't on the ladder
    const rr = RADIUS_RE.exec(line);
    if (rr && !RAW_RADIUS_OK.has(rr[1])) {
      offLadderRadii.push({ file: rel, line: i + 1, value: rr[1] });
    }
    // advisory: bare color literals in .module.css, outside var() fallbacks
    if (rel.endsWith(".module.css")) {
      const stripped = line.replace(/var\([^)]*\)/g, ""); // drop var(...) incl. fallbacks
      const noComment = stripped.replace(/\/\*.*?\*\//g, "");
      let hit = false;
      if (HEX_RE.test(noComment)) hit = true;
      RGB_RE.lastIndex = 0;
      let rm;
      while ((rm = RGB_RE.exec(noComment))) {
        const [r, g, b] = [Number(rm[1]), Number(rm[2]), Number(rm[3])];
        if (!(r === 0 && g === 0 && b === 0)) hit = true; // skip pure-black scrims
      }
      if (hit) bareColors.push({ file: rel, line: i + 1, snippet: line.trim() });
    }
    HEX_RE.lastIndex = 0;
  });
}

if (offLadderRadii.length) {
  console.warn(`\n⚠  ${offLadderRadii.length} border-radius value(s) off the --radius-* ladder (advisory):`);
  for (const r of offLadderRadii) console.warn(`   ${r.file}:${r.line}  border-radius: ${r.value}`);
  console.warn("   → ladder: --radius-hair 2 / -xs 4 / -sm 6 / -md 8 / -lg 12 / -pill 999.");
}

if (bareColors.length) {
  console.warn(`\n⚠  ${bareColors.length} bare color literal(s) in module.css (advisory — not failing build):`);
  for (const b of bareColors) console.warn(`   ${b.file}:${b.line}  ${b.snippet}`);
  console.warn("   → prefer var(--color-*) tokens unless this is a fixed canvas / chart / semantic color.");
}

if (deprecatedRefs.length) {
  console.error(`\n✖  ${deprecatedRefs.length} reference(s) to RETIRED CSS variables (removed from App.css — they resolve to nothing):`);
  for (const d of deprecatedRefs) console.error(`   ${d.file}:${d.line}  var(${d.token}) → use ${d.replacement}`);
  console.error("");
  process.exit(1);
}

if (undefinedRefs.length) {
  console.error(`\n✖  ${undefinedRefs.length} reference(s) to UNDEFINED CSS variables (fallback renders silently — likely a theming bug):`);
  for (const u of undefinedRefs) console.error(`   ${u.file}:${u.line}  var(${u.token}) — never defined`);
  console.error("\n   Fix: point these at a defined --color-* token, or define the token in App.css.\n");
  process.exit(1);
}

console.log(`✓ css-tokens: ${files.length} files, ${defined.size} tokens defined, no undefined var() references.`);
