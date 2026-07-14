//! `fleet account` — account info and rate-limit usage.

use crate::fmt::*;
use claw_fleet_core::account::{
    fetch_account_info_blocking as fetch_account_info, AccountInfo, UsageStats,
};

pub(crate) fn cmd_account(as_json: bool) {
    match fetch_account_info() {
        Ok(info) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
                return;
            }
            print_account(&info);
        }
        Err(e) => {
            eprintln!("Error fetching account info: {e}");
            std::process::exit(1);
        }
    }
}

fn print_account(info: &AccountInfo) {
    let b = c_bold();
    let r = c_reset();

    println!("{b}Account{r}");
    println!("  {b}{:<16}{r}  {} <{}>", "Name:", info.full_name, info.email);
    if !info.organization_name.is_empty() {
        println!("  {b}{:<16}{r}  {}", "Organization:", info.organization_name);
    }
    println!("  {b}{:<16}{r}  {}", "Plan:", info.plan);
    let source = match info.usage_source.as_str() {
        "foxy-switcher" => "Foxy Switcher",
        "anthropic" => "Anthropic API",
        other if !other.is_empty() => other,
        _ => "Anthropic API",
    };
    println!("  {b}{:<16}{r}  {}", "Usage source:", source);

    let has_usage = info.five_hour.is_some()
        || info.seven_day.is_some()
        || info.seven_day_sonnet.is_some();

    if has_usage {
        println!();
        println!("{b}Rate Limits{r}");

        let print_stat = |label: &str, stat: &UsageStats| {
            let bar = print_usage_bar(stat);
            let resets = format_resets_at(&stat.resets_at);
            let prev = stat.prev_utilization.map(|p| {
                let arrow = if p < stat.utilization { "↑" } else { "↓" };
                format!("  {d}(prev {:.1}% {arrow}){r}", p * 100.0, d = c_dim(), r = c_reset())
            }).unwrap_or_default();
            println!(
                "  {b}{:<16}{r}  {}  {d}resets {}{r}{}",
                label, bar, resets, prev,
                d = c_dim(), r = c_reset()
            );
        };

        if let Some(ref s) = info.five_hour {
            print_stat("5h window:", s);
        }
        if let Some(ref s) = info.seven_day {
            print_stat("7d window:", s);
        }
        if let Some(ref s) = info.seven_day_sonnet {
            print_stat("7d Sonnet:", s);
        }
    } else {
        println!();
        println!("  {d}No usage data available.{r}", d = c_dim(), r = c_reset());
    }
}
