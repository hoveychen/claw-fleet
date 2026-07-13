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
 *   [warn]  bare hardcoded color literals in a .module.css outside a var()
 *           fallback → printed, does NOT fail the build (semantic colors and
 *           deliberate fixed canvases are legitimate; this is advisory only).
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
const bareColors = []; // {file, line, snippet}

for (const f of files) {
  const rel = relative(".", f);
  const lines = readFileSync(f, "utf8").split("\n");
  lines.forEach((line, i) => {
    // undefined var references
    let m;
    USE_RE.lastIndex = 0;
    while ((m = USE_RE.exec(line))) {
      if (!defined.has(m[1]) && !RUNTIME_INJECTED.has(m[1])) {
        undefinedRefs.push({ file: rel, line: i + 1, token: m[1] });
      }
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

if (bareColors.length) {
  console.warn(`\n⚠  ${bareColors.length} bare color literal(s) in module.css (advisory — not failing build):`);
  for (const b of bareColors) console.warn(`   ${b.file}:${b.line}  ${b.snippet}`);
  console.warn("   → prefer var(--color-*) tokens unless this is a fixed canvas / chart / semantic color.");
}

if (undefinedRefs.length) {
  console.error(`\n✖  ${undefinedRefs.length} reference(s) to UNDEFINED CSS variables (fallback renders silently — likely a theming bug):`);
  for (const u of undefinedRefs) console.error(`   ${u.file}:${u.line}  var(${u.token}) — never defined`);
  console.error("\n   Fix: point these at a defined --color-* token, or define the token in App.css.\n");
  process.exit(1);
}

console.log(`✓ css-tokens: ${files.length} files, ${defined.size} tokens defined, no undefined var() references.`);
