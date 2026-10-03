//! `fleet report` — view or generate daily reports (metrics, lessons, drift
//! checks, and the day's "needs your judgment" items).

use crate::fmt::*;

pub(crate) fn cmd_report(
    date: Option<String>,
    backfill: bool,
    regenerate: bool,
    gen_lessons: bool,
    drift: bool,
    drift_chain: Option<String>,
    as_json: bool,
    lang: &str,
) {
    use claw_fleet_core::daily_report::{
        generate_lessons_routed, generate_report_from_sessions,
        local_tz_tag, scan_sessions_for_date, ReportStore,
    };
    use claw_fleet_core::llm_provider::LlmConfig;
    let llm_cfg = LlmConfig::default();

    let store = ReportStore::open().expect("cannot open report store");

    if let Some(chain_id) = drift_chain {
        eprint!("Checking chain {chain_id} (may take a minute)...");
        match claw_fleet_core::drift_check::check_chain_now(&llm_cfg, &chain_id, lang) {
            Ok(check) => {
                eprintln!(" done");
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&check).unwrap());
                } else {
                    print_drift(std::slice::from_ref(&check));
                }
            }
            Err(e) => {
                eprintln!(" failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if drift {
        eprint!("Checking relay chains that are due...");
        let flagged = claw_fleet_core::drift_check::run_due_checks(&llm_cfg, lang);
        eprintln!(" done ({} need attention)", flagged.len());
        if as_json {
            println!("{}", serde_json::to_string_pretty(&flagged).unwrap());
        } else {
            print_drift(&flagged);
        }
        return;
    }

    if backfill {
        let today = chrono::Local::now();
        for days_ago in 1..=90 {
            let date = (today - chrono::Duration::days(days_ago))
                .format("%Y-%m-%d")
                .to_string();
            if store.get_report(&date).ok().flatten().is_some() {
                continue;
            }
            let sessions = scan_sessions_for_date(&date);
            if sessions.is_empty() {
                continue;
            }
            let session_refs: Vec<_> = sessions.iter().collect();
            let tz = local_tz_tag(&date);
            let report = generate_report_from_sessions(&date, &tz, &session_refs);
            store.save_report(&report).ok();
            println!(
                "Generated report for {}: {} sessions, {} tokens",
                date,
                report.metrics.total_sessions,
                report.metrics.total_input_tokens + report.metrics.total_output_tokens
            );
        }
        println!("Backfill complete.");
        return;
    }

    let target_date = date.unwrap_or_else(|| {
        (chrono::Local::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string()
    });

    if regenerate {
        let sessions = scan_sessions_for_date(&target_date);
        if sessions.is_empty() {
            eprintln!("No sessions found for {}", target_date);
            std::process::exit(1);
        }
        let session_refs: Vec<_> = sessions.iter().collect();
        let tz = local_tz_tag(&target_date);
        let report = generate_report_from_sessions(&target_date, &tz, &session_refs);
        store.save_report(&report).ok();
        println!("Regenerated report for {}", target_date);
    }

    if gen_lessons {
        match store.get_report(&target_date) {
            Ok(Some(report)) => {
                eprint!("Generating lessons (may take up to 3 minutes)...");
                match generate_lessons_routed(&llm_cfg, &report, lang) {
                    Some(outcome) => {
                        eprintln!(
                            " done ({} recurring, {} single-session candidates, {} adopted-lesson violations)",
                            outcome.lessons.len(),
                            outcome.candidates.len(),
                            outcome.violations.len()
                        );
                        store.save_lessons_outcome(&target_date, &outcome).ok();
                        let lessons = outcome.lessons;
                        if as_json {
                            println!("{}", serde_json::to_string_pretty(&lessons).unwrap());
                            return;
                        }
                        print_lessons(&lessons);
                    }
                    None => {
                        eprintln!(" failed (claude CLI unavailable or timed out)");
                        std::process::exit(1);
                    }
                }
            }
            Ok(None) => {
                eprintln!("No report for {}. Use --regenerate first.", target_date);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    match store.get_report(&target_date) {
        Ok(Some(report)) => {
            let attention = claw_fleet_core::daily_report::attention_for_date(&target_date);
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "report": report,
                        "attention": attention,
                    }))
                    .unwrap()
                );
            } else {
                print_attention(&attention);
                print_report(&report);
            }
        }
        Ok(None) => {
            eprintln!(
                "No report for {}. Use --regenerate to generate.",
                target_date
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn print_lessons(lessons: &[claw_fleet_core::daily_report::Lesson]) {
    let b = c_bold();
    let d = c_dim();
    let r = c_reset();

    if lessons.is_empty() {
        println!("No AI mistakes found in this day's sessions.");
        return;
    }
    println!("{b}Lessons Learned{r}\n");
    for (i, lesson) in lessons.iter().enumerate() {
        println!("{}. {b}{}{r}", i + 1, lesson.content);
        println!("   {d}Why:{r} {}", lesson.reason);
        println!(
            "   {d}From:{r} {} / {}",
            lesson.workspace_name, lesson.session_id
        );
        println!();
    }
}

fn print_drift(checks: &[claw_fleet_core::drift_check::DriftCheck]) {
    let b = c_bold();
    let d = c_dim();
    let r = c_reset();
    for c in checks {
        println!(
            "{b}{}{r} {d}[{:?}] {} hops, chain {}{r}",
            c.workspace_name, c.verdict, c.session_count, c.chain_id
        );
        println!("   {d}Goal:{r} {}", c.goal);
        if !c.question.is_empty() {
            println!("   {d}Question:{r} {}", c.question);
        }
        println!("   {d}Evidence:{r} {}", c.evidence);
        println!("   {d}Latest session:{r} {}", c.latest_session_id);
        println!();
    }
}

/// The day's "needs your judgment" items, printed ahead of the metrics.
fn print_attention(a: &claw_fleet_core::daily_report::DailyAttention) {
    let b = c_bold();
    let d = c_dim();
    let r = c_reset();
    if a.drift.is_empty() && a.lessons.is_empty() && a.violations.is_empty() {
        println!("{d}Nothing needs your judgment on {}.{r}\n", a.date);
        return;
    }
    println!("{b}Needs your judgment \u{2014} {}{r}\n", a.date);
    if !a.drift.is_empty() {
        println!("{b}Relay chains that may have drifted{r}");
        print_drift(&a.drift);
    }
    if !a.lessons.is_empty() {
        println!("{b}Recurring lessons{r}");
        for l in &a.lessons {
            println!("  {d}\u{2022}{r} {}", l.content);
            println!(
                "    {d}{} \u{00b7} seen in {} sessions{r}",
                l.workspace_name,
                l.evidence_session_ids.len()
            );
        }
        println!();
    }
    if !a.violations.is_empty() {
        println!("{b}Adopted lessons broken again{r}");
        for v in &a.violations {
            println!("  {d}\u{2022}{r} {}", v.lesson_content);
            if !v.note.is_empty() {
                println!("    {d}{}{r}", v.note);
            }
            println!("    {d}sessions: {}{r}", v.session_ids.join(", "));
        }
        println!();
    }
}

fn print_report(report: &claw_fleet_core::daily_report::DailyReport) {
    let b = c_bold();
    let d = c_dim();
    let r = c_reset();

    println!("{b}Daily Report \u{2014} {}{r}", report.date);
    println!();
    println!("  Sessions:    {}", report.metrics.total_sessions);
    println!("  Subagents:   {}", report.metrics.total_subagents);
    println!(
        "  Tokens:      {} in / {} out",
        format_tokens(report.metrics.total_input_tokens),
        format_tokens(report.metrics.total_output_tokens)
    );
    println!("  Tool calls:  {}", report.metrics.total_tool_calls);
    println!();

    if !report.metrics.tool_call_breakdown.is_empty() {
        println!("{b}Tool Calls{r}");
        let mut tools: Vec<_> = report.metrics.tool_call_breakdown.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1));
        for (tool, count) in tools {
            println!("  {tool:<20} {count}");
        }
        println!();
    }

    for proj in &report.metrics.projects {
        println!(
            "{b}{}{r} {d}({}){r}",
            proj.workspace_name, proj.workspace_path
        );
        println!(
            "  {} sessions, {} tool calls, {} tokens",
            proj.session_count,
            proj.tool_calls,
            format_tokens(proj.total_input_tokens + proj.total_output_tokens)
        );
        for s in &proj.sessions {
            let title = s.title.as_deref().unwrap_or("(untitled)");
            let sub = if s.is_subagent { " [sub]" } else { "" };
            println!(
                "  {d}\u{2022}{r} {title}{sub} {d}({}){r}",
                format_tokens(s.output_tokens)
            );
        }
        println!();
    }
}
