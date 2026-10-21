//! Text helpers shared by Fleet's FTS5 indexes (sessions in
//! [`crate::search_index`], the library in [`crate::library_index`]).
//!
//! `unicode61` tokenizes a run of CJK as one token, so both indexes store text
//! with every CJK character space-separated ([`segment_cjk`]) and cut display
//! snippets from the untouched original ([`build_snippet`]).

/// True for scripts that `unicode61` does not segment: it treats a whole run of
/// them as a single token, so only a query equal to that entire run can match.
///
/// Covers Han (incl. ext-A and compatibility), kana, and Hangul syllables.
///
/// CJK punctuation and fullwidth forms are included deliberately. They are not
/// "characters to search for", but they sit *inside* Chinese runs, and leaving
/// them out strands a separator next to them that `desegment_cjk` then cannot
/// remove — snippets came back reading `已合并 ，main` instead of `已合并，main`.
pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303F}'   // CJK symbols + punctuation (。、「」…)
        | '\u{3040}'..='\u{30FF}' // hiragana + katakana
        | '\u{3400}'..='\u{4DBF}' // CJK ext A
        | '\u{4E00}'..='\u{9FFF}' // CJK unified
        | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
        | '\u{F900}'..='\u{FAFF}' // CJK compatibility
        | '\u{FF00}'..='\u{FFEF}' // fullwidth forms (，！？：；)
    )
}

/// Surround every CJK character with spaces so `unicode61` emits one token per
/// character instead of one token per run. Applied to both the indexed text and
/// the query, which turns a Chinese query into a phrase of single-character
/// tokens — i.e. a substring match against the original run.
///
/// Chosen over `tokenize='trigram'` after measuring both on a 5,000-line sample
/// of the real corpus: trigram inflated the index 2.9x (8.9 MB vs 3.1 MB) *and*
/// cannot match two-character words at all, which are the most common shape in
/// Chinese. This approach measured 3.1 MB — identical to the status quo.
pub(crate) fn segment_cjk(text: &str) -> String {
    // Fast path: pure-ASCII input (the overwhelming majority of lines) is
    // returned untouched, so English-only corpora pay nothing.
    if !text.chars().any(is_cjk) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + text.len() / 2);
    let mut prev_cjk = false;
    for c in text.chars() {
        let cur_cjk = is_cjk(c);
        // Insert a separator only at a CJK boundary where one is not already
        // present. Skipping it when `c` is itself whitespace keeps a space the
        // original text had from being doubled.
        if (cur_cjk || prev_cjk) && !c.is_whitespace() && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push(c);
        prev_cjk = cur_cjk;
    }
    out
}

/// Number of characters of context shown around a match in a snippet.
pub(crate) const SNIPPET_RADIUS: usize = 40;

/// Split a user query into the terms to highlight, in original (unsegmented)
/// form. Mirrors [`sanitize_fts_query`]'s whitespace split so the highlighted
/// terms are exactly the ones that were searched for.
pub(crate) fn query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Find the first case-insensitive occurrence of `needle` in `hay`, returning a
/// byte offset. ASCII-lowercases both sides; CJK is unaffected by case folding.
pub(crate) fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    hay.to_lowercase()
        .find(&needle.to_lowercase())
        .and_then(|i| {
            // `to_lowercase` can change byte lengths (e.g. 'İ'), which would make
            // the offset meaningless. Fall back to an exact search in that case.
            if hay.to_lowercase().len() == hay.len() {
                Some(i)
            } else {
                hay.find(needle)
            }
        })
}

/// Build a display snippet from the **original** text, so nothing the indexer
/// did to make CJK searchable can leak into the UI.
///
/// This replaces FTS5's `snippet()`, which can only cut from the segmented
/// `content` column and therefore returned text with a space between every CJK
/// character. Cutting from `raw` sidesteps the whole reconstruction problem:
/// there is no separator to guess about, because none was ever inserted.
pub(crate) fn build_snippet(raw: &str, terms: &[String]) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();

    // Byte offset -> char index, so we can work in characters (CJK-safe).
    let byte_to_char: Vec<(usize, usize)> = raw
        .char_indices()
        .enumerate()
        .map(|(ci, (bi, _))| (bi, ci))
        .collect();
    let char_index_of = |byte: usize| -> usize {
        byte_to_char
            .binary_search_by_key(&byte, |(b, _)| *b)
            .map(|i| byte_to_char[i].1)
            .unwrap_or_else(|i| byte_to_char.get(i).map(|(_, c)| *c).unwrap_or(chars.len()))
    };

    // Locate every term; anchor the window on the earliest one.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        if let Some(byte) = find_ci(raw, term) {
            let start = char_index_of(byte);
            spans.push((start, start + term.chars().count()));
        }
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_unstable();

    let anchor = spans[0].0;
    let win_start = anchor.saturating_sub(SNIPPET_RADIUS);
    let win_end = (spans[0].1 + SNIPPET_RADIUS).min(chars.len());

    // Emit the window, wrapping any span that falls inside it.
    let mut out = String::new();
    if win_start > 0 {
        out.push('…');
    }
    let mut i = win_start;
    while i < win_end {
        if let Some(&(s, e)) = spans.iter().find(|&&(s, _)| s == i) {
            let e = e.min(win_end);
            out.push_str("<mark>");
            out.extend(&chars[s..e]);
            out.push_str("</mark>");
            i = e;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    if win_end < chars.len() {
        out.push('…');
    }
    Some(out)
}

/// Undo [`segment_cjk`] for display: drop the single space FTS5 hands back
/// between two CJK characters. Spaces adjacent to non-CJK are left alone, so
/// `"决 策 卡 walks fleet"` restores to `"决策卡 walks fleet"`.
///
/// Only used as a fallback now that [`build_snippet`] cuts from `raw`: it still
/// covers the case where a match lives in a different indexed row than the one
/// whose `raw` we are holding.
pub(crate) fn desegment_cjk(text: &str) -> String {
    if !text.chars().any(is_cjk) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' '
            && i > 0
            && i + 1 < chars.len()
            && is_cjk(chars[i - 1])
            && is_cjk(chars[i + 1])
        {
            i += 1; // drop this separator
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Sanitize user input for FTS5 MATCH syntax.
/// Splits on whitespace, quotes each token, joins with spaces (implicit AND).
///
/// A token containing CJK is segmented first, so the quoted result is a phrase
/// query over consecutive single-character tokens — `"决 策"` matches `决策`
/// anywhere inside an indexed run, but still requires the two characters to be
/// adjacent and in order.
pub(crate) fn sanitize_fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            // Remove any embedded double quotes to prevent FTS5 syntax errors.
            let clean = token.replace('"', "");
            let clean = segment_cjk(&clean);
            let clean = clean.trim();
            if clean.is_empty() {
                String::new()
            } else {
                format!("\"{}\"", clean)
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`sanitize_fts_query`] for short-field search (titles, tags, file names):
/// a token ending in an ASCII letter or digit also matches as a prefix, so
/// `deploy` finds `deployment` the way the substring scan it replaced did.
/// CJK needs no such help — a segmented phrase already matches anywhere
/// inside a run.
pub(crate) fn sanitize_fts_query_prefix(input: &str) -> String {
    sanitize_fts_query(input)
        .split("\" \"")
        .map(|t| t.trim_matches('"'))
        .filter(|t| !t.is_empty())
        .map(|t| {
            if t.chars().last().is_some_and(|c| c.is_ascii_alphanumeric()) {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_query_stars_ascii_tokens_only() {
        assert_eq!(sanitize_fts_query_prefix("deploy 决策"), "\"deploy\"* \"决 策\"");
        assert_eq!(sanitize_fts_query_prefix("v2 \"x\""), "\"v2\"* \"x\"*");
        assert_eq!(sanitize_fts_query_prefix("决策卡 fleet"), "\"决 策 卡\" \"fleet\"*");
        assert_eq!(sanitize_fts_query_prefix("  "), "");
    }
}
