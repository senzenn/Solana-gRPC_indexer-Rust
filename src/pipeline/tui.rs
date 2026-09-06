use std::io::{stdout, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use tokio::sync::{broadcast, RwLock};
use std::sync::Arc;

use super::event::IndexEvent;
use super::serve::LiveStats;

pub async fn run_tui(
    mut rx: broadcast::Receiver<IndexEvent>,
    stats: Arc<RwLock<LiveStats>>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut lines: Vec<String> = Vec::new();
    let result = loop {
        while let Ok(event) = rx.try_recv() {
            lines.push(format_event(&event));
            if lines.len() > 200 {
                lines.drain(0..lines.len() - 200);
            }
        }

        let snap = stats.read().await.clone();
        terminal.draw(|f| draw(f, &snap, &lines))?;

        if event::poll(Duration::from_millis(80))?
            && let Event::Key(key) = event::read()?
                && (key.code == KeyCode::Char('q') || key.code == KeyCode::Esc) {
                    break Ok(());
                }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn format_event(event: &IndexEvent) -> String {
    match event {
        IndexEvent::TokenTransfer {
            slot,
            amount,
            source,
            destination,
            ..
        } => format!("{slot} transfer {amount} {} -> {}", short(source), short(destination)),
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
        "slot {}   events {}   token {}   program {}   gaps {}   q to quit",
        stats.slot, stats.events_total, stats.token_transfers, stats.program_ix, stats.open_gaps
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
