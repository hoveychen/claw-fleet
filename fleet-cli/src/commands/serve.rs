//! `fleet serve` — the HTTP probe server used by the Fleet app for remote
//! monitoring. A thin wrapper over `claw_fleet_core::hooks_server::serve` so
//! fleet-cli and fleet-hooks-server share one code path.

pub(crate) fn cmd_serve(port: u16, token: String, port_file: Option<std::path::PathBuf>) {
    claw_fleet_core::hooks_server::serve(port, token, port_file);
}
