//! Project-level architecture overview, injected into the Layer 1 context of
//! every worker / master session.
//!
//! V1 reads `~/.fleet/projects/<project_id>/architecture.md`. V2 will auto-
//! generate this with a dedicated indexer agent (out of scope for V1 — file
//! is hand-authored or copied in by the user / team).
//!
//! Master session bootstrapping calls `load_for_project` and concatenates the
//! returned string into the Layer 1 prompt **after** the project's
//! `CLAUDE.md` so the agent-behavior constraints come first.

use std::path::PathBuf;

use crate::paths::get_fleet_dir;
use crate::task::ProjectId;

/// `~/.fleet/projects/<project_id>/architecture.md` if it exists, else None.
///
/// Canonical storage location for the project-level Layer 1
/// constant. V1 is hand-authored; V2+ will auto-index it.
pub fn architecture_path(project_id: &ProjectId) -> Option<PathBuf> {
    let dir = get_fleet_dir()?;
    Some(dir.join("projects").join(project_id).join("architecture.md"))
}

/// Load the architecture overview for a project. Returns:
/// - `Ok(Some(content))` when the file exists and is non-empty.
/// - `Ok(None)` when the file is missing or empty — caller should fall back
///   to "no overview, suggest user create one".
/// - `Err` on read failure (permissions, IO).
pub fn load_for_project(project_id: &ProjectId) -> Result<Option<String>, String> {
    let Some(path) = architecture_path(project_id) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("read architecture.md for {project_id}: {e}"))?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(content))
}

/// Compose the worker/master session's Layer 1 context block. Returns a
/// markdown string the caller can paste into the SYSTEM prompt. When the
/// architecture overview is absent, includes a one-line note pointing the
/// user at where to create it; the agent shouldn't pretend it has context
/// it doesn't.
///
/// Renders the loaded `architecture.md` as the Layer 1 project
/// constant block consumed by [`crate::worker::compose_layer1`].
pub fn render_layer1_block(project_id: &ProjectId) -> String {
    match load_for_project(project_id) {
        Ok(Some(arch)) => format!(
            "## Project Architecture Overview\n\n{}\n",
            arch.trim_end()
        ),
        Ok(None) => format!(
            "## Project Architecture Overview\n\n_(no overview yet — \
             create `~/.fleet/projects/{project_id}/architecture.md` so future \
             sessions get a faster on-ramp)_\n"
        ),
        Err(e) => format!(
            "## Project Architecture Overview\n\n_(failed to load: {e})_\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }

    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    #[test]
    fn load_returns_none_when_file_absent() {
        let _g = crate::paths::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let result = load_for_project(&"unknown".to_string()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_reads_existing_file() {
        let _g = crate::paths::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let proj = "proj-123".to_string();
        let path = architecture_path(&proj).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# arch\n\nfoo\n").unwrap();
        let content = load_for_project(&proj).unwrap();
        assert_eq!(content.as_deref(), Some("# arch\n\nfoo\n"));
    }

    #[test]
    fn load_treats_empty_file_as_none() {
        let _g = crate::paths::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let proj = "empty".to_string();
        let path = architecture_path(&proj).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "   \n\n").unwrap();
        assert!(load_for_project(&proj).unwrap().is_none());
    }

    #[test]
    fn render_layer1_block_includes_arch_when_present() {
        let _g = crate::paths::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let proj = "demo".to_string();
        let path = architecture_path(&proj).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "core: claw-fleet-core\n").unwrap();
        let rendered = render_layer1_block(&proj);
        assert!(rendered.contains("Project Architecture Overview"));
        assert!(rendered.contains("core: claw-fleet-core"));
    }

    #[test]
    fn render_layer1_block_falls_back_when_missing() {
        let _g = crate::paths::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        let rendered = render_layer1_block(&"missing".to_string());
        assert!(rendered.contains("no overview yet"));
        assert!(rendered.contains("missing"));
    }
}
