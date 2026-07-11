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

    claw_fleet_desktop::run()
}
