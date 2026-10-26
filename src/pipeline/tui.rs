use std::io::{Stdout, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

use super::event::IndexEvent;
use super::serve::LiveStats;

/// Restores the terminal when the TUI ends by any path: return, error, abort, or unwind.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        // Make the guard first, so a failed EnterAlternateScreen still restores raw mode.
        let guard = TerminalGuard;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}

/// The default hook prints on the alternate screen, which then disappears. Restore first.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Raw mode turns Ctrl-C into a key event, so treat it as quit.
fn is_quit(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

pub async fn run_tui(
    mut rx: broadcast::Receiver<IndexEvent>,
    stats: Arc<RwLock<LiveStats>>,
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    // EventStream reads keys on its own thread, so no tokio worker blocks on input.
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(80));

    let mut lines: Vec<String> = Vec::new();
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            key = keys.next() => match key {
                Some(Ok(Event::Key(key))) if is_quit(&key) => return Ok(()),
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.into()),
                None => return Ok(()),
            },
        }

        while let Ok(event) = rx.try_recv() {
            lines.push(format_event(&event));
            if lines.len() > 200 {
                lines.drain(0..lines.len() - 200);
            }
        }

        let snap = stats.read().await.clone();
        terminal.draw(|f| draw(f, &snap, &lines))?;
    }
}

fn format_event(event: &IndexEvent) -> String {
    match event {
        IndexEvent::TokenTransfer {
            slot,
            amount,
            source,
            destination,
            ..
        } => format!(
            "{slot} transfer {amount} {} -> {}",
            short(source),
            short(destination)
        ),
        IndexEvent::ProgramIx {
            slot,
            program_name,
            instruction,
            ..
        } => format!("{slot} {program_name}::{instruction}"),
        IndexEvent::Gap { from_slot, to_slot } => format!("gap {from_slot}..{to_slot}"),
        IndexEvent::IdlDrift {
            program_name,
            severity,
            summary,
            ..
        } => format!("drift {program_name} {severity} {summary}"),
        IndexEvent::Slot { slot, .. } => format!("{slot} slot"),
    }
}

fn short(s: &str) -> String {
    if s.len() <= 8 {
        s.to_string()
    } else {
        format!("{}..{}", &s[..4], &s[s.len() - 4..])
    }
}

fn draw(f: &mut Frame, stats: &LiveStats, lines: &[String]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(f.size());

    let header = Paragraph::new(format!(
        "slot {}   finalized {}   events {}   token {}   program {}   gaps {}   q to quit",
        stats.slot,
        stats.finalized_slot,
        stats.events_total,
        stats.token_transfers,
        stats.program_ix,
        stats.open_gaps
    ))
    .block(Block::default().borders(Borders::ALL).title("index"));
    f.render_widget(header, chunks[0]);

    let items: Vec<ListItem> = lines
        .iter()
        .rev()
        .take(chunks[1].height.saturating_sub(2) as usize)
        .map(|l| ListItem::new(l.as_str()))
        .collect();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("events"));
    f.render_widget(list, chunks[1]);
}

// CrosstermBackend needs Stdout; keep type in scope for rustc.
#[allow(dead_code)]
type _Alias = CrosstermBackend<Stdout>;
