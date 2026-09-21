//! Live generation speed for dsh sessions, differenced from the roster.
//!
//! # Why a differencer and not a transcript fold
//!
//! The other two sources compute speed while they are already walking a
//! transcript: [`crate::session::stats`] collects every assistant message's
//! `(timestamp, output_tokens)` as it parses, and finishes with a 5-minute
//! window over those pairs. dsh has no transcript on disk that Fleet reads —
//! its roster hands over folded projections instead, and the only per-session
//! token figure in there is a **cumulative** counter.
//!
//! So the pairs have to be manufactured: sample the counter each time the scan
//! sees it, keep the samples for five minutes, and difference across the
//! window. Measured against a live 0.1.5 server, `projections.values.tokenUsage`
//! advances once per assembled `assistant/message` — the same granularity the
//! Claude fold gets from a transcript, so the two sources' numbers are
//! comparable rather than merely similarly named.
//!
//! # Why not the live event socket
//!
//! The obvious alternative was to count tokens off the mux downlink
//! ([`crate::dsh_events`]) as they stream. It buys nothing: dsh's durable log
//! carries no per-chunk usage — verified by decompressing a real
//! `session.v3.jsonl.zstd`, where the only `usage` block sits on
//! `assistant/message` — so the socket would update at exactly the same moments
//! the roster already does, at the cost of a second state machine.
//!
//! # The window, and why it matches Claude's
//!
//! `tokens generated in the window / (now − oldest sample)`, the same shape as
//! [`crate::session::stats::SessionStats`]. Dividing by *now* rather than by the
//! newest sample's timestamp is what makes an idle session decay towards zero
//! instead of holding its last burst's rate until the burst slides out of the
//! window.
//!
//! Cost rides the same window off [`crate::dsh_cost`]'s recorded spend. That
//! figure is refreshed for one session per poll, so a session's dollars land a
//! poll or two after its tokens do — the $/min is honest about what was
//! actually charged, just slightly behind the tok/s.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::OnceLock;

/// How far back a sample stays relevant. Same five minutes the Claude fold uses.
const WINDOW_MS: u64 = 5 * 60 * 1000;

/// One observation of a session's cumulative counters.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    at_ms: u64,
    output_tokens: u64,
    cost_usd: f64,
}

/// Per-session sample history. Bounded by [`WINDOW_MS`] rather than by count: a
/// session polled every few seconds keeps well under a hundred entries, and one
/// that stops being polled is dropped entirely by [`sweep`].
type History = HashMap<String, VecDeque<Sample>>;

static HISTORY: OnceLock<Mutex<History>> = OnceLock::new();

fn history() -> &'static Mutex<History> {
    HISTORY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record where a session's counters stand now, and answer its current speed as
/// `(tokens per second, USD per minute)`.
///
/// Both are `0.0` until the session has been seen twice — one sample is a
/// position, not a rate — and both return to `0.0` as an idle session's window
/// empties out.
pub fn observe(session_id: &str, output_tokens: u64, cost_usd: f64, now_ms: u64) -> (f64, f64) {
    let mut guard = history()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let samples = guard.entry(session_id.to_string()).or_default();
    push(
        samples,
        Sample {
            at_ms: now_ms,
            output_tokens,
            cost_usd,
        },
    );
    speed_of(samples, now_ms)
}

/// Forget every session whose newest sample has fallen out of the window.
///
/// Called once per scan with the sessions the roster still names, so a deleted
/// session — or one whose whole dsh server went away — cannot pin memory. Kept
/// separate from [`observe`] because a scan that failed outright must not be
/// read as "every session is gone".
pub fn sweep(now_ms: u64) {
    let mut guard = history()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|_, samples| {
        samples
            .back()
            .is_some_and(|s| now_ms.saturating_sub(s.at_ms) <= WINDOW_MS)
    });
}

/// Append a sample and drop everything older than the window.
///
/// A counter that went *backwards* resets the history instead of extending it.
/// That is not paranoia about dsh: a session id is only unique per server, and
/// Fleet adopts whatever server is running, so a restarted server replaying an
/// id with lower counters would otherwise underflow into a permanent zero.
fn push(samples: &mut VecDeque<Sample>, sample: Sample) {
    if samples
        .back()
        .is_some_and(|prev| prev.output_tokens > sample.output_tokens)
    {
        samples.clear();
    }
    samples.push_back(sample);
    while samples
        .front()
        .is_some_and(|s| sample.at_ms.saturating_sub(s.at_ms) > WINDOW_MS)
    {
        samples.pop_front();
    }
}

/// The window's rates. Pure, so the decay and the two-sample floor are testable
/// without a clock or a server.
fn speed_of(samples: &VecDeque<Sample>, now_ms: u64) -> (f64, f64) {
    if samples.len() < 2 {
        return (0.0, 0.0);
    }
    let (Some(first), Some(last)) = (samples.front(), samples.back()) else {
        return (0.0, 0.0);
    };
    let duration_s = now_ms.saturating_sub(first.at_ms) as f64 / 1000.0;
    if duration_s <= 0.0 {
        return (0.0, 0.0);
    }
    let tokens = last.output_tokens.saturating_sub(first.output_tokens) as f64;
    // Spend is re-read from a cache that another process may also write, and a
    // re-price can revise a figure down; a negative delta means "no spend to
    // report", not a refund.
    let usd = (last.cost_usd - first.cost_usd).max(0.0);
    (tokens / duration_s, usd * 60.0 / duration_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(samples: &[(u64, u64, f64)]) -> VecDeque<Sample> {
        let mut q = VecDeque::new();
        for &(at_ms, output_tokens, cost_usd) in samples {
            push(
                &mut q,
                Sample {
                    at_ms,
                    output_tokens,
                    cost_usd,
                },
            );
        }
        q
    }

    /// One sample is a position on a cumulative counter, not a rate — the very
    /// first poll of a session must not invent a speed out of its lifetime
    /// total.
    #[test]
    fn a_single_sample_is_not_a_rate() {
        assert_eq!(speed_of(&window(&[(0, 5_000, 1.0)]), 1_000), (0.0, 0.0));
    }

    /// 600 tokens and $0.30 over the 60s since the oldest sample.
    #[test]
    fn differences_the_cumulative_counters_across_the_window() {
        let w = window(&[(0, 1_000, 1.00), (60_000, 1_600, 1.30)]);
        let (tok_s, usd_min) = speed_of(&w, 60_000);
        assert!((tok_s - 10.0).abs() < 1e-9, "tok/s was {tok_s}");
        assert!((usd_min - 0.30).abs() < 1e-9, "usd/min was {usd_min}");
    }

    /// The divisor is `now`, not the newest sample: a session that stopped
    /// generating has to decay rather than hold its last burst's rate.
    #[test]
    fn an_idle_session_decays_towards_zero() {
        let w = window(&[(0, 1_000, 0.0), (10_000, 2_000, 0.0)]);
        let (busy, _) = speed_of(&w, 10_000);
        let (idle, _) = speed_of(&w, 100_000);
        assert!((busy - 100.0).abs() < 1e-9, "tok/s was {busy}");
        assert!(idle < busy / 9.0, "{idle} did not decay from {busy}");
    }

    /// Samples older than the window are dropped, so the rate reflects recent
    /// generation rather than the session's whole life.
    #[test]
    fn samples_older_than_the_window_leave_it() {
        let now = WINDOW_MS + 1_000;
        let w = window(&[
            (0, 0, 0.0),
            (WINDOW_MS - 5_000, 100_000, 0.0),
            (now, 100_500, 0.0),
        ]);
        assert_eq!(w.len(), 2, "only the first sample should have aged out");
        let (tok_s, _) = speed_of(&w, now);
        // 500 tokens over the surviving 6s span. Had the aged-out sample stayed,
        // the same call would have reported the session's whole 100_500-token
        // life spread over five minutes — four times this rate.
        assert!((tok_s - 500.0 / 6.0).abs() < 1e-9, "tok/s was {tok_s}");
    }

    /// A counter that moved backwards is a different session behind the same id
    /// (a restarted dsh server). Restart the history rather than underflow.
    #[test]
    fn a_counter_going_backwards_restarts_the_history() {
        let w = window(&[(0, 9_000, 5.0), (1_000, 12_000, 6.0), (2_000, 40, 0.0)]);
        assert_eq!(w.len(), 1, "history should have been cleared");
        assert_eq!(speed_of(&w, 3_000), (0.0, 0.0));
    }

    /// A re-price that revises a session's recorded spend down is not income.
    #[test]
    fn a_downward_spend_revision_never_reports_negative_cost() {
        let w = window(&[(0, 1_000, 2.00), (60_000, 1_060, 1.50)]);
        let (_, usd_min) = speed_of(&w, 60_000);
        assert_eq!(usd_min, 0.0);
    }

    /// `observe` is the whole public surface: it has to fold a sample in and
    /// answer off the same history, and `sweep` has to forget a session that
    /// stopped being polled.
    #[test]
    fn observe_accumulates_and_sweep_forgets() {
        let id = "session-observe-and-sweep";
        assert_eq!(observe(id, 1_000, 1.0, 0), (0.0, 0.0));
        let (tok_s, usd_min) = observe(id, 1_600, 1.3, 60_000);
        assert!((tok_s - 10.0).abs() < 1e-9, "tok/s was {tok_s}");
        assert!((usd_min - 0.30).abs() < 1e-9, "usd/min was {usd_min}");

        sweep(60_000);
        assert!(history().lock().unwrap().contains_key(id));
        sweep(60_000 + WINDOW_MS + 1);
        assert!(!history().lock().unwrap().contains_key(id));
    }
}
