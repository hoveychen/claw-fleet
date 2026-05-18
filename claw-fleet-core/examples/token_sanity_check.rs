// Standalone sanity check: runs token_analysis on a real JSONL and prints totals.
// Run with: cargo run -p claw-fleet-core --example token_sanity_check -- <jsonl>
use claw_fleet_core::token_analysis::aggregate_task;
use std::env;
use std::path::Path;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(jsonl) = args.first() else {
        eprintln!("usage: token_sanity_check <main-jsonl-path>");
        std::process::exit(1);
    };
    let project_root = Path::new("/Users/hoveychen/workspace/claude-fleet");
    let task = aggregate_task(Path::new(jsonl), Some(project_root)).expect("ok");
    println!("=== Task breakdown ===");
    println!("main session: {} ({} msgs, model={:?})", task.main.session_id, task.main.messages, task.main.model);
    println!("subagents: {}", task.subagents.len());
    println!("baseline_loaded: {}  bundle_size_tokens: {}", task.baseline_loaded, task.bundle_size_tokens);
    println!();
    println!("Totals:");
    println!("  usage.input         {}", task.totals_usage.input_tokens);
    println!("  usage.cache_creation {}", task.totals_usage.cache_creation_tokens);
    println!("  usage.cache_read    {}", task.totals_usage.cache_read_tokens);
    println!("  usage.output        {}", task.totals_usage.output_tokens);
    println!("  est cost USD        ${:.2}", task.totals_estimated_cost_usd.unwrap_or(0.0));
    println!();
    println!("Sources (input attribution):");
    let s = &task.totals_sources;
    let total = s.total();
    let print = |label: &str, v: u64| println!("  {:30}  {:>10}  ({:5.1}%)", label, v, 100.0 * v as f64 / total.max(1) as f64);
    print("cc_base_system_prompt", s.cc_base_system_prompt);
    print("tool_defs", s.tool_defs);
    print("user_claudemd", s.user_claudemd);
    print("project_claudemd", s.project_claudemd);
    print("fleet_reminders", s.fleet_reminders);
    print("memory_files", s.memory_files);
    print("skills_manifest", s.skills_manifest);
    print("visible_user_text", s.visible_user_text);
    print("visible_tool_result", s.visible_tool_result);
    print("visible_system_reminder", s.visible_system_reminder);
    print("visible_prev_assistant", s.visible_prev_assistant);
    print("visible_compact_summary", s.visible_compact_summary);
    print("ttl_refresh_overhead", s.ttl_refresh_overhead);
    print("residual_unexplained", s.residual_unexplained);
    println!();
    println!("Output:");
    let o = &task.totals_output;
    println!("  text                {}", o.output_text);
    println!("  thinking_visible    {}", o.output_thinking_visible);
    println!("  tool_use            {}", o.output_tool_use);
    println!("  reasoning_invisible {}", o.output_reasoning_invisible);
    println!();
    println!("fit_confidence (main) = {:.2}", task.main.fit_confidence);
}
