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
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use claw_fleet_task::pitem::{PItem, PItemStatus};
use claw_fleet_task::task::Task;

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
