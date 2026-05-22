//! Read-only ratatui front-end for the foreground `fleet-task new` /
//! `fleet-task resume` invocations. Phase 2 keeps it minimal:
//!
//! - Top region (full width): P-item list grouped by status — shows id + desc.
//! - Bottom status bar: task title + status + http port + pid + hint.
//!
//! All P-item operations stay with the master agent; the TUI only renders
//! and lets the operator quit with `q` / `Ctrl-C`.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::{event, execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use std::path::PathBuf;

use tui_textarea::TextArea;

use claw_fleet_task::pitem::{PItem, PItemStatus};
use claw_fleet_task::registry;
use claw_fleet_task::task::{list_tasks, Task, TaskStatus};

pub struct TuiContext {
    pub task_id: String,
    pub http_port: u16,
}

pub fn run(ctx: TuiContext, stop: Arc<AtomicBool>) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = run_loop(&mut term, &ctx, &stop);

    let _ = execute!(term.backend_mut(), terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    result
}

fn run_loop<B: ratatui::backend::Backend>(
    term: &mut Terminal<B>,
    ctx: &TuiContext,
    stop: &Arc<AtomicBool>,
) -> io::Result<()> {
    while !stop.load(Ordering::SeqCst) {
        let task = claw_fleet_task::task::get_task(&ctx.task_id).ok();
        term.draw(|f| draw(f, ctx, task.as_ref()))?;
        if event::poll(Duration::from_millis(250))? {
            if let event::Event::Key(k) = event::read()? {
                if should_quit(&k) {
                    stop.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    }
    Ok(())
}

fn should_quit(k: &event::KeyEvent) -> bool {
    matches!(k.code, event::KeyCode::Char('q') | event::KeyCode::Char('Q'))
        || (k.modifiers.contains(event::KeyModifiers::CONTROL)
            && matches!(k.code, event::KeyCode::Char('c')))
}

fn draw(f: &mut Frame, ctx: &TuiContext, task: Option<&Task>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(f.area());

    let title = match task {
        Some(t) => format!("P-items ({})", t.plan.len()),
        None => "P-items (task not found)".into(),
    };
    let items: Vec<ListItem> = match task {
        Some(t) => render_p_items(t),
        None => Vec::new(),
    };
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL));
    f.render_widget(list, chunks[0]);

    let status = render_status(ctx, task);
    let bar = Paragraph::new(status)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(bar, chunks[1]);
}

fn render_p_items(task: &Task) -> Vec<ListItem<'static>> {
    let mut items: Vec<&PItem> = task.plan.items.values().collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
        .into_iter()
        .map(|p| {
            let (label, color) = status_label(&p.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {:8} ", label), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::raw(p.id.clone()),
                Span::raw("  "),
                Span::styled(p.desc.clone(), Style::default().fg(Color::Gray)),
            ]))
        })
        .collect()
}

fn status_label(status: &PItemStatus) -> (&'static str, Color) {
    match status {
        PItemStatus::WaitDeps => ("WAIT", Color::DarkGray),
        PItemStatus::WaitResource => ("WAITR", Color::Cyan),
        PItemStatus::Running => ("RUN", Color::Yellow),
        PItemStatus::Done => ("DONE", Color::Green),
        PItemStatus::Skipped => ("SKIP", Color::Gray),
        PItemStatus::Failed(_) => ("FAIL", Color::Red),
        PItemStatus::WaitHumanGate => ("GATE", Color::Magenta),
    }
}

// ── Launchpad (default `fleet-task` entry) ───────────────────────────────────
// One TUI session with two screens:
//   - NewTask (default): a prompt textarea over `$PWD`; Ctrl-Enter starts a
//     new task in `$PWD` with the entered prompt + auto-derived title.
//   - Picker: the multi-task list from `run_picker`; Enter resumes the
//     highlighted task. Reachable via Ctrl-T from NewTask; Esc/q goes back.
// Mirrors `claude`'s "bare invocation = prompt waiting" UX while keeping
// the resume path one keystroke away.

#[derive(Debug, Clone)]
pub enum LaunchAction {
    StartNew {
        workspace: PathBuf,
        prompt: String,
        title: String,
    },
    Resume {
        task_id: String,
    },
    Quit,
}

pub fn run_launchpad(default_workspace: PathBuf) -> io::Result<LaunchAction> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = launchpad_loop(&mut term, default_workspace);

    let _ = execute!(term.backend_mut(), terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    result
}

enum LaunchScreen {
    NewTask,
    Picker,
}

fn launchpad_loop<B: ratatui::backend::Backend>(
    term: &mut Terminal<B>,
    workspace: PathBuf,
) -> io::Result<LaunchAction> {
    let mut screen = LaunchScreen::NewTask;

    let mut textarea = TextArea::default();
    textarea.set_block(
        Block::default()
            .title(" Prompt ")
            .borders(Borders::ALL),
    );
    textarea.set_placeholder_text("What should the master agent do?");

    let mut tasks = list_tasks(None);
    tasks.sort_by_key(|t| {
        let primary = match t.status {
            TaskStatus::Running => 0,
            TaskStatus::Paused => 1,
            TaskStatus::Ready | TaskStatus::Planning | TaskStatus::Drafting => 2,
            TaskStatus::Done | TaskStatus::Abandoned => 3,
        };
        (primary, -t.created_at)
    });
    let runtime: std::collections::HashMap<String, registry::RegistryEntry> =
        registry::list_alive()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.task_id.clone(), e))
            .collect();
    let mut picker_state = ListState::default();
    if !tasks.is_empty() {
        picker_state.select(Some(0));
    }

    let workspace_is_git = workspace.join(".git").exists();

    loop {
        term.draw(|f| match screen {
            LaunchScreen::NewTask => {
                draw_new_task(f, &workspace, workspace_is_git, &textarea);
            }
            LaunchScreen::Picker => {
                draw_picker(f, &tasks, &runtime, &mut picker_state);
            }
        })?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let raw = event::read()?;
        let event::Event::Key(k) = raw else { continue };

        let ctrl = k.modifiers.contains(event::KeyModifiers::CONTROL);

        match screen {
            LaunchScreen::NewTask => {
                if ctrl && matches!(k.code, event::KeyCode::Char('c')) {
                    return Ok(LaunchAction::Quit);
                }
                if matches!(k.code, event::KeyCode::Esc) {
                    return Ok(LaunchAction::Quit);
                }
                // Ctrl-T → switch to picker (resume an existing task).
                if ctrl && matches!(k.code, event::KeyCode::Char('t')) {
                    if tasks.is_empty() {
                        // No tasks; flash a hint in the textarea by skipping.
                        continue;
                    }
                    screen = LaunchScreen::Picker;
                    continue;
                }
                // Ctrl-Enter or Ctrl-S → submit.
                if ctrl
                    && (matches!(k.code, event::KeyCode::Enter)
                        || matches!(k.code, event::KeyCode::Char('s')))
                {
                    if !workspace_is_git {
                        // Can't start a task outside a git repo. Surface in
                        // the textarea title rather than panic.
                        textarea.set_block(
                            Block::default()
                                .title(" Prompt — workspace is not a git repo, cd into one first ")
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(Color::Red)),
                        );
                        continue;
                    }
                    let prompt = textarea.lines().join("\n").trim().to_string();
                    if prompt.is_empty() {
                        continue;
                    }
                    let title = derive_title(&prompt);
                    return Ok(LaunchAction::StartNew {
                        workspace: workspace.clone(),
                        prompt,
                        title,
                    });
                }
                // Forward everything else to the textarea (typing, arrows,
                // backspace, plain Enter for newlines).
                textarea.input(raw);
            }
            LaunchScreen::Picker => {
                // Esc / q / Ctrl-T → back to NewTask.
                if matches!(k.code, event::KeyCode::Esc | event::KeyCode::Char('q'))
                    || (ctrl && matches!(k.code, event::KeyCode::Char('t')))
                {
                    screen = LaunchScreen::NewTask;
                    continue;
                }
                if ctrl && matches!(k.code, event::KeyCode::Char('c')) {
                    return Ok(LaunchAction::Quit);
                }
                match k.code {
                    event::KeyCode::Up | event::KeyCode::Char('k') => {
                        let i = picker_state.selected().unwrap_or(0);
                        picker_state.select(Some(if i == 0 { tasks.len() - 1 } else { i - 1 }));
                    }
                    event::KeyCode::Down | event::KeyCode::Char('j') => {
                        let i = picker_state.selected().unwrap_or(0);
                        picker_state.select(Some((i + 1) % tasks.len()));
                    }
                    event::KeyCode::Enter => {
                        if let Some(i) = picker_state.selected() {
                            return Ok(LaunchAction::Resume {
                                task_id: tasks[i].id.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw_new_task(
    f: &mut Frame,
    workspace: &std::path::Path,
    workspace_is_git: bool,
    textarea: &TextArea,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // workspace header
            Constraint::Min(0),     // textarea
            Constraint::Length(3),  // hint bar
        ])
        .split(f.area());

    let ws_color = if workspace_is_git {
        Color::Green
    } else {
        Color::Red
    };
    let ws_line = Line::from(vec![
        Span::styled("workspace ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            workspace.display().to_string(),
            Style::default().fg(ws_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            if workspace_is_git { "(git repo ✓)" } else { "(not a git repo ✗)" },
            Style::default().fg(ws_color),
        ),
    ]);
    let header = Paragraph::new(ws_line)
        .block(Block::default().title(" New task ").borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    f.render_widget(textarea, chunks[1]);

    let hint = Line::from(vec![
        Span::styled("Ctrl-Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" start   "),
        Span::styled("Ctrl-T", Style::default().fg(Color::Yellow)),
        Span::raw(" resume from task list   "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" quit"),
    ]);
    let bar = Paragraph::new(hint).block(Block::default().borders(Borders::ALL));
    f.render_widget(bar, chunks[2]);
}

fn derive_title(prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or("").trim();
    if first_line.chars().count() <= 40 {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(40).collect();
        format!("{truncated}…")
    }
}

// ── Multi-task picker ────────────────────────────────────────────────────────

/// Outcome of `run_picker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// User pressed Enter on `task_id`.
    Selected(String),
    /// User pressed q / Ctrl-C without selecting a task.
    Quit,
    /// No tasks exist on disk; nothing to pick from.
    Empty,
}

pub fn run_picker() -> io::Result<PickerOutcome> {
    let mut tasks = list_tasks(None);
    if tasks.is_empty() {
        return Ok(PickerOutcome::Empty);
    }
    // Show running first, then by created_at desc.
    tasks.sort_by_key(|t| {
        let primary = match t.status {
            TaskStatus::Running => 0,
            TaskStatus::Paused => 1,
            TaskStatus::Ready | TaskStatus::Planning | TaskStatus::Drafting => 2,
            TaskStatus::Done | TaskStatus::Abandoned => 3,
        };
        (primary, -t.created_at)
    });

    let runtime: std::collections::HashMap<String, registry::RegistryEntry> =
        registry::list_alive()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.task_id.clone(), e))
            .collect();

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = picker_loop(&mut term, &tasks, &runtime);

    let _ = execute!(term.backend_mut(), terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    result
}

fn picker_loop<B: ratatui::backend::Backend>(
    term: &mut Terminal<B>,
    tasks: &[Task],
    runtime: &std::collections::HashMap<String, registry::RegistryEntry>,
) -> io::Result<PickerOutcome> {
    let mut state = ListState::default();
    state.select(Some(0));

    loop {
        term.draw(|f| draw_picker(f, tasks, runtime, &mut state))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let event::Event::Key(k) = event::read()? else { continue };
        if should_quit(&k) {
            return Ok(PickerOutcome::Quit);
        }
        match k.code {
            event::KeyCode::Up | event::KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0);
                state.select(Some(if i == 0 { tasks.len() - 1 } else { i - 1 }));
            }
            event::KeyCode::Down | event::KeyCode::Char('j') => {
                let i = state.selected().unwrap_or(0);
                state.select(Some((i + 1) % tasks.len()));
            }
            event::KeyCode::Enter => {
                if let Some(i) = state.selected() {
                    return Ok(PickerOutcome::Selected(tasks[i].id.clone()));
                }
            }
            _ => {}
        }
    }
}

fn draw_picker(
    f: &mut Frame,
    tasks: &[Task],
    runtime: &std::collections::HashMap<String, registry::RegistryEntry>,
    state: &mut ListState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(f.area());

    let items: Vec<ListItem> = tasks
        .iter()
        .map(|t| {
            let live = runtime.contains_key(&t.id);
            let short = if t.id.len() >= 8 { &t.id[..8] } else { &t.id };
            let status = format!("{:?}", t.status).to_lowercase();
            let total = t.plan.len();
            let done = t.plan.items.values()
                .filter(|p| matches!(p.status, PItemStatus::Done))
                .count();
            ListItem::new(Line::from(vec![
                Span::styled(
                    if live { " ● " } else { "   " }.to_string(),
                    Style::default().fg(if live { Color::Green } else { Color::DarkGray }),
                ),
                Span::styled(format!("{:<8}  ", short), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:<10} ", status), task_status_color(&t.status)),
                Span::raw(format!("{done}/{total}  ")),
                Span::styled(t.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
            ]))
        })
        .collect();

    let list = ratatui::widgets::List::new(items)
        .block(
            Block::default()
                .title(format!("Tasks ({})", tasks.len()))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, chunks[0], state);

    let hint = Line::from(vec![
        Span::styled("↑↓/jk", Style::default().fg(Color::Yellow)),
        Span::raw(" move   "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" resume task   "),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw(" quit"),
    ]);
    let bar = Paragraph::new(hint).block(Block::default().borders(Borders::ALL));
    f.render_widget(bar, chunks[1]);
}

fn task_status_color(status: &TaskStatus) -> Style {
    let c = match status {
        TaskStatus::Running => Color::Yellow,
        TaskStatus::Paused => Color::Magenta,
        TaskStatus::Ready | TaskStatus::Planning => Color::Cyan,
        TaskStatus::Drafting => Color::DarkGray,
        TaskStatus::Done => Color::Green,
        TaskStatus::Abandoned => Color::DarkGray,
    };
    Style::default().fg(c)
}

fn render_status(ctx: &TuiContext, task: Option<&Task>) -> Line<'static> {
    let (title, status_str) = match task {
        Some(t) => (t.title.clone(), format!("{:?}", t.status)),
        None => (ctx.task_id.clone(), "?".into()),
    };
    Line::from(vec![
        Span::styled("task ", Style::default().fg(Color::DarkGray)),
        Span::raw(title),
        Span::styled("  status ", Style::default().fg(Color::DarkGray)),
        Span::raw(status_str),
        Span::styled("  port ", Style::default().fg(Color::DarkGray)),
        Span::raw(ctx.http_port.to_string()),
        Span::styled("  pid ", Style::default().fg(Color::DarkGray)),
        Span::raw(std::process::id().to_string()),
        Span::styled("  ", Style::default()),
        Span::styled("[q]uit", Style::default().fg(Color::DarkGray)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_fleet_task::pitem::PItem;
    use claw_fleet_task::plan::DagPlan;

    fn pitem_with_status(id: &str, status: PItemStatus) -> PItem {
        PItem {
            id: id.into(),
            desc: format!("desc {id}"),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }
    }

    #[test]
    fn render_p_items_sorts_by_id() {
        use claw_fleet_task::paths::fleet_home_lock;
        use claw_fleet_task::task::{create_task, TaskInput};

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

        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let mut task = create_task(TaskInput {
            project_id: "p".into(),
            title: "T".into(),
            description: String::new(),
        })
        .unwrap();
        task.plan = DagPlan::from_items(vec![
            pitem_with_status("p2", PItemStatus::WaitDeps),
            pitem_with_status("p1", PItemStatus::Running),
        ]);
        let items = render_p_items(&task);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn quit_keys_recognise_q_caps_q_and_ctrl_c() {
        use event::{KeyCode, KeyEvent, KeyModifiers};
        assert!(should_quit(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(should_quit(&KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE)));
        assert!(should_quit(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!should_quit(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)));
    }
}
