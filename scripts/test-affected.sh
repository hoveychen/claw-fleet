#!/usr/bin/env bash
# Run only the tests a change can reach, plus a fixed set of cross-cutting
# guard tests. The full regression is CI's job (.github/workflows/ci.yml runs
# it on every branch push); local verification should not re-run it.
#
# Usage:
#   scripts/test-affected.sh             # diff = merge-base(main) .. working tree
#   scripts/test-affected.sh --base REF  # diff against REF instead of main
#   scripts/test-affected.sh --dry-run   # print the commands, run nothing
#
# How the selection works:
#   Rust  — a changed file `<crate>/src/a/b.rs` becomes the libtest filter `a::b::`
#           on that crate's unit-test target (`--lib`, or `--bins` for bin-only
#           crates). `lib.rs` / `main.rs` mean "the whole target". A changed
#           `<crate>/tests/x.rs` runs `--test x`. Integration tests are never
#           compiled unless named, which is where most of the time went.
#           When claw-fleet-core changes, the crates CI tests on top of it are
#           compile-checked (`cargo check --tests`), not run.
#   TS    — `vitest related --run <changed files>` per package (vitest walks the
#           import graph), plus `tsc --noEmit`, plus the desktop CSS token lint
#           when a stylesheet changed. shared-ts/ changes fan out to both packages.
#
# Guard tests: tests that go red in module B when a registry in module A is
# edited — exactly the kind a module filter misses (2026-09-05: the
# INJECT_RULES guard in permissions_injector only failed on a full run). They
# run whenever their side of the repo changed. When you write a new test of
# that shape, add it to the lists below.
set -uo pipefail

CORE_GUARD_FILTERS=(
  "permissions_injector::"
  "mcp_control::"
  "control_plane::"
  "wakeup_guard::"
  "tests::every_skip_target_names_a_real_target"
  "tests::every_bundled_body_is_a_skill"
)
CORE_GUARD_TESTS=(
  mobile_relay_drift_guard
  mock_model_catalog_drift_guard
  source_registry_guard
  zip_mime_drift_guard
  home_env_lock_guard
  claude_md_writer_contract
)
DESKTOP_GUARD_TESTS=(
  app/localeKeys.test.ts
  app/capabilityCoverage.test.ts
  app/tauriCoreProbe.wiring.test.ts
)
MOBILE_GUARD_TESTS=(
  src/i18nKeys.test.ts
)
# Crates whose tests CI runs and that build on claw-fleet-core.
CORE_DEPENDENTS=(fleet-cli fleet-hooks-server)

base="main"
dry_run=0
while [ $# -gt 0 ]; do
  case "$1" in
    --base) base="$2"; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

root="$(git rev-parse --show-toplevel)" || exit 2
cd "$root" || exit 2
merge_base="$(git merge-base "$base" HEAD)" || { echo "cannot find merge-base with $base" >&2; exit 2; }

changed="$( { git diff --name-only "$merge_base"; git ls-files --others --exclude-standard; } | sort -u)"
if [ -z "$changed" ]; then
  echo "no changes against $base"
  exit 0
fi

failures=()
run() {
  echo
  echo "+ $*"
  [ "$dry_run" = 1 ] && return 0
  if ! "$@"; then
    failures+=("$*")
  fi
}
in_dir() {
  local dir="$1"; shift
  echo
  echo "+ (cd $dir && $*)"
  [ "$dry_run" = 1 ] && return 0
  if ! (cd "$dir" && "$@"); then
    failures+=("(cd $dir && $*)")
  fi
}
contains() {
  local needle="$1"; shift
  local x
  for x in "$@"; do [ "$x" = "$needle" ] && return 0; done
  return 1
}

# ── Rust ────────────────────────────────────────────────────────────────────
rust_crates=()
for f in $changed; do
  case "$f" in *.rs|*/Cargo.toml) ;; *) continue ;; esac
  crate="${f%%/*}"
  [ -f "$crate/Cargo.toml" ] || continue
  contains "$crate" "${rust_crates[@]+"${rust_crates[@]}"}" || rust_crates+=("$crate")
done

for crate in "${rust_crates[@]+"${rust_crates[@]}"}"; do
  pkg="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$crate/Cargo.toml" | head -1)"
  if [ -f "$crate/src/lib.rs" ]; then target="--lib"; else target="--bins"; fi
  whole=0
  filters=()
  tests=()
  for f in $changed; do
    case "$f" in
      "$crate"/Cargo.toml|"$crate"/build.rs|"$crate"/src/lib.rs|"$crate"/src/main.rs) whole=1 ;;
      "$crate"/src/*.rs)
        [ -f "$f" ] || { whole=1; continue; }   # deleted module: run everything
        m="${f#"$crate"/src/}"; m="${m%.rs}"; m="${m%/mod}"
        filters+=("${m//\//::}::") ;;
      "$crate"/tests/*.rs)
        t="${f#"$crate"/tests/}"; t="${t%%/*}"; t="${t%.rs}"
        [ -f "$crate/tests/$t.rs" ] && tests+=("$t") ;;
    esac
  done
  if [ "$crate" = "claw-fleet-core" ]; then
    filters+=("${CORE_GUARD_FILTERS[@]}")
    tests+=("${CORE_GUARD_TESTS[@]}")
  fi

  if [ "$whole" = 1 ]; then
    run cargo test -p "$pkg" "$target"
  elif [ ${#filters[@]} -gt 0 ]; then
    run cargo test -p "$pkg" "$target" -- "${filters[@]}"
  fi
  if [ ${#tests[@]} -gt 0 ]; then
    args=()
    for t in $(printf '%s\n' "${tests[@]}" | sort -u); do args+=(--test "$t"); done
    run cargo test -p "$pkg" "${args[@]}"
  fi
done

if contains claw-fleet-core "${rust_crates[@]+"${rust_crates[@]}"}"; then
  args=()
  for d in "${CORE_DEPENDENTS[@]}"; do args+=(-p "$d"); done
  run cargo check --tests "${args[@]}"
fi

# ── TypeScript ──────────────────────────────────────────────────────────────
# ts_package <dir> <guard test>...
ts_package() {
  local dir="$1" prefix="$1/"
  shift
  local guards=("$@")
  local files=() f shared=0
  for f in $changed; do
    case "$f" in
      "$prefix"*.ts|"$prefix"*.tsx) [ -f "$f" ] && files+=("${f#"$prefix"}") ;;
      shared-ts/*) shared=1; [ -f "$f" ] && files+=("../$f") ;;
    esac
  done
  local touched=0
  if [ ${#files[@]} -gt 0 ] || [ "$shared" = 1 ]; then touched=1; fi
  # A Rust wire-type change regenerates app/generated/types.ts, which is TS too.
  printf '%s\n' "$changed" | grep -q "^$prefix" && touched=1
  [ "$touched" = 1 ] || return 0

  in_dir "$dir" pnpm exec tsc --noEmit
  in_dir "$dir" pnpm exec vitest related --run "${files[@]+"${files[@]}"}" "${guards[@]}"
}

ts_package claw-fleet-desktop "${DESKTOP_GUARD_TESTS[@]}"
ts_package mobile-web "${MOBILE_GUARD_TESTS[@]}"
if printf '%s\n' "$changed" | grep -q '^claw-fleet-desktop/.*\.css$'; then
  in_dir claw-fleet-desktop node scripts/check-css-tokens.mjs
fi

# ── Summary ─────────────────────────────────────────────────────────────────
echo
if [ "$dry_run" = 1 ]; then
  echo "dry run: nothing executed"
  exit 0
fi
if [ ${#failures[@]} -gt 0 ]; then
  echo "FAILED (${#failures[@]}):"
  printf '  %s\n' "${failures[@]}"
  echo "A test that is red here but unrelated to your diff: rerun it alone before blaming the change."
  exit 1
fi
echo "affected tests green — the full regression runs in CI on push"
