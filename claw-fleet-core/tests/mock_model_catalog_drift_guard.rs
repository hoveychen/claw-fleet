//! Drift-guard for the desktop demo mode's copy of Fleet's model catalog.
//!
//! `claw-fleet-desktop/app/mock/data.ts` exports `MOCK_MODEL_CATALOG`, the
//! stand-in the mocked Tauri layer answers `model_catalog` with. It is a
//! hand-written transcription of what [`model_catalog::picker_catalog`] really
//! returns, and until this test nothing tied the two together: the mock is
//! plain TypeScript data that always type-checks, so the desktop's 1267
//! frontend tests stay green no matter how far it drifts from `models.toml`.
//!
//! That is not hypothetical. On 2026-09-10 DeepSeek retired the two older flash
//! ids into aliases of V4.1 Flash and announced V4 Pro's retirement into it as
//! well; `models.toml` was updated so the picker offers exactly one dsh row,
//! and the mock went on advertising three. Demo mode and every screenshot taken
//! from it showed a menu the real app no longer has, and nothing failed.
//!
//! The check is deliberately field-by-field and order-sensitive. A menu is an
//! ordered thing — "the same models in a different order" is still a mock that
//! misrepresents the product — and the per-model effort ladders are exactly
//! where a lazily-transcribed mock goes wrong (they differ *within* a harness:
//! `gpt-5.5` stops at `xhigh`, the deepseek route offers `off` and has no
//! `medium`).
//!
//! Availability is **not** compared. `picker_catalog()`'s `available` flag
//! reports which harnesses are installed on the machine running the test, so
//! asserting it would pass locally and fail on a CI box with a different set of
//! agents installed. The test uses [`picker_catalog_with`] with an
//! always-available probe, which yields the machine-independent half — the
//! model rows, which is the half the mock transcribes.

use claw_fleet_core::model_catalog::picker_catalog_with;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MOCK_DATA_TS: &str = "../claw-fleet-desktop/app/mock/data.ts";

/// One row as the mock spells it: the arguments of an `m(...)` call.
#[derive(Debug, PartialEq)]
struct MockRow {
    id: String,
    label: String,
    harness: String,
    tier: String,
    efforts: Vec<String>,
    default_effort: Option<String>,
}

fn read_mock() -> String {
    let p: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join(MOCK_DATA_TS);
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("drift-guard: cannot read {}: {e}", p.display()))
}

/// The `const NAME = ["a", "b"];` ladder constants the `m(...)` calls refer to.
///
/// Resolved rather than special-cased: the mock is free to introduce another
/// shared ladder, and a guard that only knew today's four names would quietly
/// start skipping rows instead of failing.
fn ladder_consts(src: &str) -> HashMap<String, Vec<String>> {
    // `[\s\S]` rather than `.` because several of these span lines.
    let re = Regex::new(r"const\s+([A-Z][A-Z0-9_]*)\s*=\s*\[([\s\S]*?)\]\s*;").unwrap();
    re.captures_iter(src)
        .map(|c| (c[1].to_string(), string_list(&c[2])))
        .collect()
}

/// The string literals in a `["a", "b"]` body, in order.
fn string_list(body: &str) -> Vec<String> {
    Regex::new(r#""([^"]*)""#)
        .unwrap()
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .collect()
}

/// The `MOCK_MODEL_CATALOG = [...]` array body.
fn catalog_block(src: &str) -> &str {
    let start = src
        .find("export const MOCK_MODEL_CATALOG")
        .expect("drift-guard: MOCK_MODEL_CATALOG not found — did the mock get renamed?");
    let open = start
        + src[start..]
            .find('[')
            .expect("drift-guard: MOCK_MODEL_CATALOG has no array literal");
    let close = matching(src, open, '[', ']');
    &src[open..=close]
}

/// Index of the delimiter closing the one at `open`, skipping string literals.
fn matching(src: &str, open: usize, o: char, c: char) -> usize {
    let bytes: Vec<char> = src.chars().collect();
    // Byte index → char index is not needed: the mock is ASCII in these
    // regions, but comments elsewhere are not, so walk chars and map back.
    let char_start = src[..open].chars().count();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, ch) in bytes.iter().enumerate().skip(char_start) {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if *ch == '\\' {
                escaped = true;
            } else if *ch == q {
                quote = None;
            }
            continue;
        }
        match *ch {
            '"' | '\'' | '`' => quote = Some(*ch),
            x if x == o => depth += 1,
            x if x == c => {
                depth -= 1;
                if depth == 0 {
                    return src
                        .char_indices()
                        .nth(i)
                        .expect("index in range")
                        .0;
                }
            }
            _ => {}
        }
    }
    panic!("drift-guard: unbalanced {o}{c} starting at byte {open}");
}

/// Split a call's argument list at top-level commas.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in args.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '[' | '(' | '{' => {
                depth += 1;
                cur.push(ch);
            }
            ']' | ')' | '}' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(['"', '\'']).to_string()
}

/// Parse the mock into `harness name → ordered rows`.
///
/// Buckets `m(...)` calls by the nearest preceding `name: "..."` key, which is
/// how the mock groups them; a hand-rolled scan rather than a regex because the
/// calls wrap across lines whenever their arguments get long.
fn parse_mock(src: &str) -> Vec<(String, Vec<MockRow>)> {
    let ladders = ladder_consts(src);
    let block = catalog_block(src);
    let name_re = Regex::new(r#"name:\s*"([a-z0-9-]+)""#).unwrap();

    let mut groups: Vec<(String, Vec<MockRow>)> = Vec::new();
    let mut cursor = 0usize;
    while let Some(idx) = block[cursor..].find("m(") {
        let at = cursor + idx;
        // `m(` must be a call, not the tail of another identifier.
        let prev = block[..at].chars().last().unwrap_or(' ');
        if prev.is_alphanumeric() || prev == '_' || prev == '.' {
            cursor = at + 2;
            continue;
        }
        let close = matching(block, at + 1, '(', ')');
        let args = split_args(&block[at + 2..close]);
        cursor = close + 1;
        if args.len() < 5 {
            continue;
        }

        let efforts = {
            let a = &args[4];
            if a.starts_with('[') {
                string_list(a)
            } else {
                ladders.get(a.as_str()).cloned().unwrap_or_else(|| {
                    panic!("drift-guard: unknown effort ladder constant `{a}` in the mock")
                })
            }
        };
        let row = MockRow {
            id: unquote(&args[0]),
            label: unquote(&args[1]),
            harness: unquote(&args[2]),
            tier: unquote(&args[3]),
            efforts,
            default_effort: args.get(5).map(|s| unquote(s)).filter(|s| s != "null"),
        };

        // Which harness group is this call inside? The last `name: "..."` before it.
        let owner = name_re
            .captures_iter(&block[..at])
            .last()
            .map(|c| c[1].to_string())
            .expect("drift-guard: an m(...) row appears before any harness `name:` key");
        match groups.last_mut() {
            Some((n, rows)) if *n == owner => rows.push(row),
            _ => groups.push((owner, vec![row])),
        }
    }
    groups
}

/// The mock's `MOCK_MODEL_CATALOG` must equal the real `picker_catalog()`
/// payload, harness for harness and row for row.
#[test]
fn mock_model_catalog_matches_the_real_picker_catalog() {
    let mock = parse_mock(&read_mock());
    assert!(
        !mock.is_empty(),
        "drift-guard: parsed zero rows out of the mock — the parser, not the mock, \
         is what broke"
    );

    let real = picker_catalog_with(|_| true);
    let real_names: Vec<&str> = real.iter().map(|h| h.name.as_str()).collect();
    let mock_names: Vec<&str> = mock.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        mock_names, real_names,
        "drift-guard: MOCK_MODEL_CATALOG lists different harnesses than \
         picker_catalog(), or lists them in a different order"
    );

    for ((name, rows), h) in mock.iter().zip(real.iter()) {
        let mock_ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let real_ids: Vec<&str> = h.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            mock_ids, real_ids,
            "drift-guard: the `{name}` rows in claw-fleet-desktop/app/mock/data.ts no \
             longer match models.toml. Demo mode and every screenshot taken from it \
             would show a menu the real app does not have. Update MOCK_MODEL_CATALOG."
        );

        for (r, m) in rows.iter().zip(h.models.iter()) {
            assert_eq!(r.label, m.label, "drift-guard: {} label", r.id);
            assert_eq!(r.harness, m.harness, "drift-guard: {} harness", r.id);
            assert_eq!(
                Some(r.tier.clone()),
                m.tier,
                "drift-guard: {} tier",
                r.id
            );
            assert_eq!(
                r.efforts, m.efforts,
                "drift-guard: {} effort ladder — these differ within a harness, so a \
                 copied-from-a-sibling ladder is the likely cause",
                r.id
            );
            assert_eq!(
                r.default_effort, m.default_effort,
                "drift-guard: {} default effort",
                r.id
            );
        }
    }
}

/// The parser earns its keep only if it would actually notice a change, so
/// prove it against a doctored copy rather than trusting the green run above.
///
/// A guard whose parser silently matches nothing is worse than no guard: it
/// reports success forever. The row to delete is taken from the parse itself
/// rather than named as a literal, so this stays honest as the catalog changes
/// — hard-coding today's id would turn this into a second thing to remember to
/// update, and it would fail for the wrong reason when it was not.
#[test]
fn the_parser_sees_a_row_that_was_removed() {
    let src = read_mock();
    let before = parse_mock(&src);
    let dsh_before = &before
        .iter()
        .find(|(n, _)| n == "dsh")
        .expect("drift-guard: no dsh group in the mock")
        .1;
    let victim = format!("m(\"{}\"", dsh_before[0].id);
    // Rename the call rather than deleting text: the scanner tracks string
    // literals to find the matching bracket, so cutting through a quote pair
    // would desync it and this test would fail on a mangled fixture instead of
    // on the thing it means to check.
    let doctored = src.replacen(&victim, &format!("notM(\"{}\"", dsh_before[0].id), 1);
    assert!(
        doctored.len() > src.len(),
        "drift-guard: the row `{victim}` the parser reported is not in the source \
         text — the parser is inventing rows"
    );

    let dsh_after = parse_mock(&doctored)
        .iter()
        .find(|(n, _)| n == "dsh")
        .map(|(_, rows)| rows.len())
        .unwrap_or(0);
    assert_eq!(
        dsh_after,
        dsh_before.len() - 1,
        "drift-guard: removing a row did not change what the parser sees — the guard \
         would pass through any drift"
    );
}
