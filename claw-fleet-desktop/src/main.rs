// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `<app-binary> fleet-proc-host <id>` — re-exec target for detached
    // workspace command hosts (see `claw_fleet_core::proc_runner`). Must run
    // before Tauri boots.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() >= 3 && args[1] == claw_fleet_core::proc_runner::HOST_ARGV_MARKER {
            claw_fleet_core::proc_runner::host_main(&args[2]);
        }
    }

    // Thread panics in a GUI app are otherwise invisible: stderr goes nowhere
    // when Finder launches us, and a panic inside a tauri `(async)` command is
    // swallowed by the task harness — the invoke promise just never settles
    // (the dsh 「永久加载中」 bug hid behind exactly this for three debugging
    // sessions). Mirror every panic into the debug log, then let the default
    // hook print to stderr as before.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
            claw_fleet_core::log_debug(&format!("PANIC on thread '{thread}': {info}"));
            default_hook(info);
        }));
    }

    claw_fleet_desktop::run()
}
