//! Terminal UI using ratatui

use std::sync::Arc;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use tokio::sync::mpsc;
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
    layout::{Layout, Constraint, Direction, Rect},
    style::{Style, Color, Modifier},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap, Gauge, Scrollbar, ScrollbarOrientation, ScrollbarState},
    text::{Span, Line, Text},
};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers, KeyEventKind},
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use orchestrator_common::{Error, Result};
use orchestrator_common::types::{Proposal, Approval, ApprovalResponse, RiskLevel, ProposalKind, Component};
use tempfile::NamedTempFile;

/// TUI application
pub struct TuiApp {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    proposal_rx: mpsc::Receiver<Proposal>,
    approval_tx: mpsc::Sender<ApprovalResponse>,
    scroll_state: ScrollbarState,
    diff_scroll: usize,
    max_diff_scroll: usize,
}

impl TuiApp {
    pub fn new(proposal_rx: mpsc::Receiver<Proposal>, approval_tx: mpsc::Sender<ApprovalResponse>) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self { 
            terminal, 
            proposal_rx, 
            approval_tx,
            scroll_state: ScrollbarState::default(),
            diff_scroll: 0,
            max_diff_scroll: 0,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        loop {
            // Check for new proposal
            if let Ok(proposal) = self.proposal_rx.try_recv() {
                let response = self.show_proposal(&proposal).await?;
                self.approval_tx.send(response).await?;
            }

            // Handle keyboard events
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn show_proposal(&mut self, proposal: &Proposal) -> Result<ApprovalResponse> {
        let mut approved = false;
        let mut modified = None;
        let mut reason = None;
        let mut show_diff = true;
        self.diff_scroll = 0;

        // Pre-calculate max scroll for diff
        let diff_lines = proposal.diff.as_deref().unwrap_or("No diff provided").lines().count();
        self.max_diff_scroll = diff_lines.saturating_sub(20);
        self.scroll_state = self.scroll_state.content_length(diff_lines);

        loop {
            self.terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),   // Title
                        Constraint::Min(8),      // Proposal details
                        Constraint::Min(8),      // Impact
                        Constraint::Min(10),     // Diff (expandable)
                        Constraint::Length(3),   // Controls
                    ].as_ref())
                    .split(f.area());

                // Title
                let title = Paragraph::new(format!("Proposal: {} (ID: {})", proposal.description, proposal.id))
                    .block(Block::default().borders(Borders::ALL).title("Approval Required"))
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(title, chunks[0]);

                // Details
                let details = self.build_details(proposal);
                let details_widget = Paragraph::new(details)
                    .block(Block::default().borders(Borders::ALL).title("Details"))
                    .wrap(Wrap { trim: true });
                f.render_widget(details_widget, chunks[1]);

                // Impact
                let impact = self.build_impact(proposal);
                let impact_widget = Paragraph::new(impact)
                    .block(Block::default().borders(Borders::ALL).title("Estimated Impact"));
                f.render_widget(impact_widget, chunks[2]);

                // Diff
                if show_diff {
                    let diff_text = self.build_diff(proposal);
                    let diff_widget = Paragraph::new(diff_text)
                        .block(Block::default().borders(Borders::ALL).title("Diff (↑/↓ to scroll)"))
                        .wrap(Wrap { trim: false })
                        .scroll((self.diff_scroll as u16, 0));
                    f.render_widget(diff_widget, chunks[3]);

                    // Scrollbar
                    let scrollbar = Scrollbar::default()
                        .orientation(ScrollbarOrientation::VerticalRight)
                        .begin_symbol(Some("▲"))
                        .end_symbol(Some("▼"));
                    f.render_stateful_widget(scrollbar, chunks[3], &mut self.scroll_state);
                }

                // Controls
                let controls = self.build_controls(show_diff);
                f.render_widget(controls, chunks[4]);
            })?;

            // Handle input
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            approved = true;
                            break;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            approved = false;
                            if reason.is_none() {
                                reason = Some("Rejected by user".to_string());
                            }
                            break;
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            // Open editor for modification
                            match self.open_editor_for_modification(proposal).await {
                                Ok(new_proposal) => {
                                    modified = Some(new_proposal);
                                    reason = Some("Proposal modified in editor".to_string());
                                    approved = true;
                                    break;
                                }
                                Err(e) => {
                                    reason = Some(format!("Editor error: {}", e));
                                }
                            }
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            // Input reason
                            if let Some(r) = self.input_reason_dialog().await? {
                                reason = Some(r);
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            // Toggle diff visibility
                            show_diff = !show_diff;
                        }
                        KeyCode::Up => {
                            if self.diff_scroll > 0 {
                                self.diff_scroll -= 1;
                                self.scroll_state = self.scroll_state.position(self.diff_scroll);
                            }
                        }
                        KeyCode::Down => {
                            if self.diff_scroll < self.max_diff_scroll {
                                self.diff_scroll += 1;
                                self.scroll_state = self.scroll_state.position(self.diff_scroll);
                            }
                        }
                        KeyCode::PageUp => {
                            self.diff_scroll = self.diff_scroll.saturating_sub(10);
                            self.scroll_state = self.scroll_state.position(self.diff_scroll);
                        }
                        KeyCode::PageDown => {
                            self.diff_scroll = (self.diff_scroll + 10).min(self.max_diff_scroll);
                            self.scroll_state = self.scroll_state.position(self.diff_scroll);
                        }
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            return Err(Error::InvalidState("User quit".to_string()));
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(ApprovalResponse {
            proposal_id: proposal.id,
            approved,
            modified_proposal: modified,
            reason,
        })
    }

    fn build_details(&self, proposal: &Proposal) -> Text<'static> {
        let risk_style = risk_color(proposal.risk_level);
        let kind_str = format!("{:?}", proposal.kind);
        
        let affected: Vec<String> = proposal.affected_components.iter()
            .map(|c| format!("{:?}", c))
            .collect();

        Text::from(vec![
            Line::from(vec![
                Span::styled("Kind: ", Style::default().fg(Color::Cyan)),
                Span::raw(kind_str),
            ]),
            Line::from(vec![
                Span::styled("Trigger: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{:?}", proposal.trigger)),
            ]),
            Line::from(vec![
                Span::styled("Risk: ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{:?}", proposal.risk_level), risk_style),
            ]),
            Line::from(vec![
                Span::styled("Affected: ", Style::default().fg(Color::Cyan)),
                Span::raw(affected.join(", ")),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Rationale: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(proposal.rationale.clone()),
        ])
    }

    fn build_impact(&self, proposal: &Proposal) -> Text<'static> {
        let impact = &proposal.estimated_impact;
        
        fn fmt_delta(val: f64, positive_good: bool) -> (String, Style) {
            let sign = if val >= 0.0 { "+" } else { "" };
            let style = if (val >= 0.0) == positive_good {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            (format!("{}{:.1}%", sign, val), style)
        }

        let (perf_str, perf_style) = fmt_delta(impact.performance_delta_pct, true);
        let (cpu_str, cpu_style) = fmt_delta(impact.cpu_delta_pct, false);
        let (lat_str, lat_style) = fmt_delta(impact.latency_delta_pct, false);
        let (bat_str, bat_style) = fmt_delta(impact.battery_delta_pct, false);
        let mem_str = format!("{:+} MiB", impact.memory_delta_mib);
        let mem_style = if impact.memory_delta_mib <= 0 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };

        Text::from(vec![
            Line::from(vec![
                Span::styled("Performance: ", Style::default().fg(Color::Cyan)),
                Span::styled(perf_str, perf_style),
            ]),
            Line::from(vec![
                Span::styled("CPU Usage:   ", Style::default().fg(Color::Cyan)),
                Span::styled(cpu_str, cpu_style),
            ]),
            Line::from(vec![
                Span::styled("Memory:      ", Style::default().fg(Color::Cyan)),
                Span::styled(mem_str, mem_style),
            ]),
            Line::from(vec![
                Span::styled("Latency:     ", Style::default().fg(Color::Cyan)),
                Span::styled(lat_str, lat_style),
            ]),
            Line::from(vec![
                Span::styled("Battery:     ", Style::default().fg(Color::Cyan)),
                Span::styled(bat_str, bat_style),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Stability Risk: ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{:?}", impact.stability_risk), risk_color(impact.stability_risk)),
            ]),
            Line::from(vec![
                Span::styled("Security Risk:  ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{:?}", impact.security_risk), risk_color(impact.security_risk)),
            ]),
            Line::from(vec![
                Span::styled("Compat Risk:    ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{:?}", impact.compatibility_risk), risk_color(impact.compatibility_risk)),
            ]),
        ])
    }

    fn build_diff(&self, proposal: &Proposal) -> Text<'static> {
        let diff = proposal.diff.as_deref().unwrap_or("No diff provided");
        let lines: Vec<Line> = diff.lines()
            .map(|line| {
                let style = if line.starts_with('+') {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') {
                    Style::default().fg(Color::Red)
                } else if line.starts_with("@@") {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(line, style))
            })
            .collect();
        
        Text::from(lines)
    }

    fn build_controls(&self, show_diff: bool) -> Paragraph<'static> {
        let diff_hint = if show_diff { "[D] Hide Diff" } else { "[D] Show Diff" };
        let controls_text = format!(
            "Keys: [Y] Approve  [N] Reject  [M] Modify  [R] Reason  {}  [Q] Quit  |  Diff: ↑/↓ PgUp/PgDn",
            diff_hint
        );
        
        Paragraph::new(controls_text)
            .block(Block::default().borders(Borders::ALL).title("Controls"))
            .style(Style::default().fg(Color::Green))
    }

    /// Open external editor for proposal modification
    async fn open_editor_for_modification(&self, proposal: &Proposal) -> Result<Proposal> {
        // Create a temporary file with the proposal as YAML for editing
        let mut temp_file = NamedTempFile::new()?;
        let yaml = serde_yaml::to_string(proposal)?;
        temp_file.write_all(yaml.as_bytes())?;
        temp_file.flush()?;
        let path = temp_file.path().to_path_buf();

        // Determine editor
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        
        // Launch editor
        let status = Command::new(&editor)
            .arg(&path)
            .status()?;

        if !status.success() {
            return Err(Error::InvalidState("Editor exited with error".to_string()));
        }

        // Read modified content
        let modified_content = tokio::fs::read_to_string(&path).await?;
        let modified_proposal: Proposal = serde_yaml::from_str(&modified_content)?;
        
        // Validate that ID wasn't changed
        if modified_proposal.id != proposal.id {
            return Err(Error::InvalidState("Proposal ID cannot be changed".to_string()));
        }

        Ok(modified_proposal)
    }

    /// Input reason dialog
    async fn input_reason_dialog(&self) -> Result<Option<String>> {
        // Save terminal state
        disable_raw_mode()?;
        execute!(std::io::stdout(), LeaveAlternateScreen)?;

        // Prompt for reason
        print!("Enter rejection/modification reason: ");
        io::stdout().flush()?;
        let mut reason = String::new();
        io::stdin().read_line(&mut reason)?;
        let reason = reason.trim().to_string();

        // Restore terminal
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen)?;

        if reason.is_empty() {
            Ok(None)
        } else {
            Ok(Some(reason))
        }
    }
}

fn risk_color(risk: RiskLevel) -> Style {
    match risk {
        RiskLevel::None => Style::default().fg(Color::Green),
        RiskLevel::Low => Style::default().fg(Color::Green),
        RiskLevel::Medium => Style::default().fg(Color::Yellow),
        RiskLevel::High => Style::default().fg(Color::Red),
        RiskLevel::Critical => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

impl Drop for TuiApp {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}