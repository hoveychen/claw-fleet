//! `fleet report` — view or generate daily reports (metrics, AI summary, lessons).

use crate::fmt::*;

pub(crate) fn cmd_report(date: Option<String>, backfill: bool, regenerate: bool, gen_lessons: bool, gen_summary: bool, as_json: bool, lang: &str) {
    use claw_fleet_core::daily_report::{
        ReportStore, generate_report_from_sessions, scan_sessions_for_date,
        generate_lessons_routed, generate_ai_summary_routed,
    };
    use claw_fleet_core::llm_provider::LlmConfig;
    let llm_cfg = LlmConfig::default();

    let store = ReportStore::open().expect("cannot open report store");

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
            let tz = chrono::Local::now().format("%Z").to_string();
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
        let tz = chrono::Local::now().format("%Z").to_string();
        let report = generate_report_from_sessions(&target_date, &tz, &session_refs);
        store.save_report(&report).ok();
        println!("Regenerated report for {}", target_date);
    }

    if gen_summary {
        match store.get_report(&target_date) {
            Ok(Some(report)) => {
                eprint!("Generating AI summary (may take up to 2 minutes)...");
                match generate_ai_summary_routed(&llm_cfg, &report, lang) {
                    Some(summary) => {
                        eprintln!(" done");
                        store.update_ai_summary(&target_date, &summary).ok();
                        if as_json {
                            println!("{}", serde_json::to_string_pretty(&summary).unwrap());
                        } else {
                            println!("{summary}");
                        }
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
            Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); }
        }
        if !gen_lessons { return; }
    }

    if gen_lessons {
        match store.get_report(&target_date) {
            Ok(Some(report)) => {
                eprint!("Generating lessons (may take up to 3 minutes)...");
                match generate_lessons_routed(&llm_cfg, &report, lang) {
                    Some(lessons) => {
                        eprintln!(" done ({} lessons found)", lessons.len());
                        store.update_lessons(&target_date, &lessons).ok();
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
            Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); }
        }
        return;
    }

    match store.get_report(&target_date) {
        Ok(Some(report)) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
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
        println!("   {d}From:{r} {} / {}", lesson.workspace_name, lesson.session_id);
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

    if let Some(ref summary) = report.ai_summary {
        println!("{b}AI Summary{r}");
        println!("{}", summary);
    }
}
