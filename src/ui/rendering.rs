/// Build the filter box display text.
pub fn build_filter_content(query: &str, cursor: usize, focused: bool) -> String {
    if focused {
        let (before, after) = query.split_at(cursor);
        format!("Filter: {}|{}", before, after)
    } else if query.is_empty() {
        "Filter: (Tab to focus, comma = OR)".to_string()
    } else {
        format!("Filter: {}", query)
    }
}

/// Determine whether compare mode (relative CPU column) should be active.
pub fn is_compare_active(filter_query: &str, row_count: usize) -> bool {
    !filter_query.is_empty() && row_count >= 2
}

use crate::common::{
    DataSourceMode, ExemptionEditorState, PendingAction, ProcessListState, SettingsState, UIMode,
};
use crate::priority_guard::windows_api::PRIORITY_CLASSES;
use crate::process_list::get_column_header;
use crate::settings::get_hide_self_status;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};
use std::collections::HashSet;

use crate::priority_guard::LogEntry;

/// Info about PriorityGuard state for rendering.
pub struct PriorityGuardStatus {
    pub enabled: bool,
    pub active_demotions: usize,
    pub demoted_pids: Vec<u32>,
    pub log_entries: Vec<LogEntry>,
}

/// Extra state needed for rendering process actions.
pub struct ActionState<'a> {
    pub pending_action: &'a Option<PendingAction>,
    pub suspended_pids: &'a HashSet<u32>,
    pub priority_picker_selected: usize,
    pub exemption_editor: &'a ExemptionEditorState,
}

#[allow(clippy::too_many_arguments)]
pub fn render_ui(
    f: &mut Frame,
    ui_mode: UIMode,
    process_state: &mut ProcessListState,
    settings: &SettingsState,
    data_source_mode: DataSourceMode,
    pg_status: &PriorityGuardStatus,
    log_scroll_offset: Option<u16>,
    action_state: &ActionState<'_>,
) {
    match ui_mode {
        UIMode::Processes => render_process_list(
            f,
            process_state,
            data_source_mode,
            pg_status,
            action_state.pending_action,
            action_state.suspended_pids,
        ),
        UIMode::Settings => render_settings(f, settings),
        UIMode::Log => render_log(f, &pg_status.log_entries, log_scroll_offset),
        UIMode::PriorityPicker => {
            render_process_list(
                f,
                process_state,
                data_source_mode,
                pg_status,
                action_state.pending_action,
                action_state.suspended_pids,
            );
            render_priority_picker(f, process_state, action_state.priority_picker_selected);
        }
        UIMode::ExemptionEditor => {
            render_settings(f, settings);
            render_exemption_editor(f, settings, action_state.exemption_editor);
        }
    }
}

pub fn render_process_list(
    f: &mut Frame,
    state: &mut ProcessListState,
    data_source_mode: DataSourceMode,
    pg_status: &PriorityGuardStatus,
    pending_action: &Option<PendingAction>,
    suspended_pids: &HashSet<u32>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(7),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(f.area());

    // Title
    let mut title_spans = match data_source_mode {
        DataSourceMode::Etw => vec![
            Span::styled(
                "Process Warden",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "[ETW]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ],
        DataSourceMode::PollingFallback => vec![
            Span::styled(
                "Process Warden",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "[POLLING FALLBACK - ETW unavailable, run as admin for real-time events]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ],
    };

    // PriorityGuard status indicator
    if pg_status.enabled {
        let pg_text = if pg_status.active_demotions > 0 {
            format!(" [PG: {} active]", pg_status.active_demotions)
        } else {
            " [PG: ON]".to_string()
        };
        title_spans.push(Span::styled(
            pg_text,
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let title = Paragraph::new(Line::from(title_spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Filter box
    let filter_border_color = if state.filter_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let filter_content = build_filter_content(
        &state.filter_query,
        state.filter_cursor,
        state.filter_focused,
    );
    let count_suffix = if state.filter_query.is_empty() {
        format!("  [{} processes]", state.total_process_count)
    } else {
        format!(
            "  [{}/{}]",
            state.processes.len(),
            state.total_process_count
        )
    };
    let filter_widget = Paragraph::new(format!("{}{}", filter_content, count_suffix))
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(filter_border_color)),
        );
    f.render_widget(filter_widget, chunks[1]);

    // Compare mode: show relative CPU column when filtering with 2+ results
    let compare_active = is_compare_active(&state.filter_query, state.formatted_rows.len());

    // Build table rows from pre-formatted cache (no allocations here)
    let rows: Vec<Row> = state
        .formatted_rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let proc = state.processes.get(i);
            let is_demoted = proc.is_some_and(|p| pg_status.demoted_pids.contains(&p.pid));
            let is_suspended = proc.is_some_and(|p| suspended_pids.contains(&p.pid));

            let cpu = proc.map_or(0.0, |p| p.cpu);
            let style = if is_suspended {
                Style::default().fg(Color::DarkGray)
            } else if is_demoted {
                Style::default().fg(Color::Red)
            } else if cpu >= 80.0 {
                Style::default().fg(Color::LightRed)
            } else if cpu >= 50.0 {
                Style::default().fg(Color::Yellow)
            } else if i % 2 == 0 {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };

            let name = if is_suspended {
                format!("[S] {}", row[1])
            } else if is_demoted {
                format!("[PG] {}", row[1])
            } else {
                row[1].clone()
            };

            let cells: Vec<String> = if compare_active {
                vec![
                    row[0].clone(),
                    name,
                    row[2].clone(),
                    row[3].clone(),
                    row[4].clone(),
                ]
            } else {
                vec![row[0].clone(), name, row[2].clone(), row[3].clone()]
            };
            Row::new(cells).style(style)
        })
        .collect();

    let pid_header = get_column_header(
        crate::common::SortColumn::Pid,
        state.sort_column,
        state.sort_ascending,
    );
    let name_header = get_column_header(
        crate::common::SortColumn::Name,
        state.sort_column,
        state.sort_ascending,
    );
    let cpu_header = get_column_header(
        crate::common::SortColumn::Cpu,
        state.sort_column,
        state.sort_ascending,
    );
    let memory_header = get_column_header(
        crate::common::SortColumn::Memory,
        state.sort_column,
        state.sort_ascending,
    );

    let (header, widths): (Row, Vec<Constraint>) = if compare_active {
        (
            Row::new(vec![
                pid_header,
                name_header,
                cpu_header,
                memory_header,
                "CPU (rel)",
            ])
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
            vec![
                Constraint::Length(10),
                Constraint::Percentage(40),
                Constraint::Length(15),
                Constraint::Length(15),
                Constraint::Length(20),
            ],
        )
    } else {
        (
            Row::new(vec![pid_header, name_header, cpu_header, memory_header])
                .style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .bottom_margin(1),
            vec![
                Constraint::Length(10),
                Constraint::Percentage(50),
                Constraint::Length(15),
                Constraint::Length(15),
            ],
        )
    };

    // Empty state message when no processes match
    let table_title = if state.formatted_rows.is_empty() && !state.filter_query.is_empty() {
        "Processes — No matches (ESC to clear filter)"
    } else if state.formatted_rows.is_empty() {
        "Processes — No processes"
    } else {
        "Processes"
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(table_title))
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    // Sync TableState with current selection so ratatui auto-scrolls
    state.table_state.select(Some(state.selected));
    f.render_stateful_widget(table, chunks[2], &mut state.table_state);

    // Help / confirmation bar
    let help_lines = if let Some(action) = pending_action {
        let (prompt, color) = match action {
            PendingAction::Kill { pid, name } => {
                (format!("Kill '{}' (PID {})? [Y/N]", name, pid), Color::Red)
            }
            PendingAction::Suspend { pid, name } => (
                format!("Suspend '{}' (PID {})? [Y/N]", name, pid),
                Color::Yellow,
            ),
            PendingAction::Resume { pid, name } => (
                format!("Resume '{}' (PID {})? [Y/N]", name, pid),
                Color::Green,
            ),
        };
        vec![Line::from(Span::styled(
            prompt,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))]
    } else {
        vec![Line::from(vec![
            Span::styled("↑↓/j k", Style::default().fg(Color::Cyan)),
            Span::raw(" Nav  "),
            Span::styled("←→", Style::default().fg(Color::Cyan)),
            Span::raw(" Sort  "),
            Span::styled("Tab /", Style::default().fg(Color::Cyan)),
            Span::raw(" Filter  "),
            Span::styled("x/Del", Style::default().fg(Color::Red)),
            Span::raw(" Kill  "),
            Span::styled("z", Style::default().fg(Color::Yellow)),
            Span::raw(" Suspend  "),
            Span::styled("p", Style::default().fg(Color::Magenta)),
            Span::raw(" Priority  "),
            Span::styled("s", Style::default().fg(Color::Cyan)),
            Span::raw(" Settings  "),
            Span::styled("l", Style::default().fg(Color::Cyan)),
            Span::raw(" Log  "),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::raw(" Quit"),
        ])]
    };
    let help = Paragraph::new(help_lines)
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[3]);
}

pub fn render_settings(f: &mut Frame, settings: &SettingsState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(f.area());

    // Title
    let title = Paragraph::new("Settings")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Settings options
    let mut lines = vec![];

    let hide_self_status = get_hide_self_status(settings.hide_self);
    let pg_enabled_status = if settings.priority_guard_enabled {
        "ON"
    } else {
        "OFF"
    };
    let adaptive_display = if settings.priority_guard_adaptive_sensitivity == 0.0 {
        "OFF".to_string()
    } else {
        format!("{:.1}", settings.priority_guard_adaptive_sensitivity)
    };

    let options: &[(usize, String, Option<&str>)] = &[
        (
            0,
            format!("  Poll Interval: {} ms ", settings.poll_interval_ms),
            Some("  (less = faster updates, higher CPU usage)"),
        ),
        (
            1,
            format!("  Hide Self (ProcWarden): {} ", hide_self_status),
            None,
        ),
        (
            2,
            format!("  PriorityGuard: {} ", pg_enabled_status),
            Some("  (auto-demote CPU hogs to Below Normal priority)"),
        ),
        (
            3,
            format!(
                "  CPU Threshold: {:.0}% ",
                settings.priority_guard_cpu_threshold
            ),
            None,
        ),
        (
            4,
            format!("  Duration: {}s ", settings.priority_guard_duration_secs),
            Some("  (how long a process must exceed threshold before demotion)"),
        ),
        (
            5,
            format!(
                "  Per-Core Threshold: {:.0}% ",
                settings.priority_guard_per_core_threshold
            ),
            Some("  (max estimated single-core CPU to trigger demotion)"),
        ),
        (
            6,
            format!(
                "  Relative Multiplier: {:.0}x ",
                settings.priority_guard_relative_multiplier
            ),
            Some("  (trigger if CPU > multiplier × median of all processes)"),
        ),
        (
            7,
            format!("  EMA Alpha: {:.2} ", settings.priority_guard_ema_alpha),
            Some("  (CPU smoothing: lower = smoother, 1.0 = no smoothing)"),
        ),
        (
            8,
            format!(
                "  Grace Period: {}s ",
                settings.priority_guard_grace_period_secs
            ),
            Some("  (ignore newly launched processes for this duration)"),
        ),
        (
            9,
            format!("  Adaptive Sensitivity: {} ", adaptive_display),
            Some("  (0 = off, higher = more aggressive load-based scaling)"),
        ),
        (
            10,
            format!("  Exemptions: [{}] ", settings.user_exemptions.len()),
            Some("  (processes that PriorityGuard will never demote)"),
        ),
    ];

    for (idx, label, hint) in options {
        // Insert PriorityGuard section header before option 2
        if *idx == 2 {
            lines.push(Line::from(""));
            lines.push(Line::from("  --- PriorityGuard ---"));
            lines.push(Line::from(""));
        }

        let line = if settings.selected_option == *idx {
            Line::from(vec![Span::styled(
                label.clone(),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )])
        } else {
            Line::from(label.as_str())
        };
        lines.push(line);

        if let Some(hint_text) = hint {
            lines.push(Line::from(*hint_text));
        }
        // Don't add trailing blank after last option
        if *idx < 10 {
            lines.push(Line::from(""));
        }
    }

    let settings_widget = Paragraph::new(lines)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title("Settings"));
    f.render_widget(settings_widget, chunks[1]);

    // Help text
    let help_lines = vec![Line::from(vec![
        Span::raw("Navigation: "),
        Span::styled("↑↓", Style::default().fg(Color::Cyan)),
        Span::raw(" | Adjust: "),
        Span::styled("←→", Style::default().fg(Color::Cyan)),
        Span::raw(" | Edit: "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" | Close: "),
        Span::styled("s/ESC", Style::default().fg(Color::Cyan)),
        Span::raw(" | Quit: "),
        Span::styled("q", Style::default().fg(Color::Cyan)),
    ])];
    let help = Paragraph::new(help_lines)
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

pub fn render_log(f: &mut Frame, log_entries: &[LogEntry], scroll_offset: Option<u16>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Min(7),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(f.area());

    // Title
    let title = Paragraph::new("PriorityGuard Log")
        .style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Log entries (newest at bottom)
    let lines: Vec<Line> = if log_entries.is_empty() {
        vec![Line::from(Span::styled(
            "  No PriorityGuard events yet. Enable PriorityGuard in Settings (s) to start.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        log_entries
            .iter()
            .map(|entry| {
                let timestamp = format!("[{:>8.1}s] ", entry.elapsed_secs);
                let color = if entry.message.starts_with("demoted")
                    || entry.message.starts_with("demotion failed")
                {
                    Color::Red
                } else if entry.message.starts_with("restored")
                    || entry.message.starts_with("cleanup")
                {
                    Color::Green
                } else if entry.message.starts_with("grace") {
                    Color::Cyan
                } else {
                    Color::Yellow // escalated, foreground restore, tier 2, etc.
                };
                Line::from(vec![
                    Span::styled(timestamp, Style::default().fg(Color::DarkGray)),
                    Span::styled(&entry.message, Style::default().fg(color)),
                ])
            })
            .collect()
    };

    let total = log_entries.len() as u16;
    let visible = chunks[1].height.saturating_sub(2); // borders
    let max_scroll = total.saturating_sub(visible);
    let scroll_pos = match scroll_offset {
        Some(offset) => offset.min(max_scroll),
        None => max_scroll, // auto-scroll to bottom
    };
    let log_widget = Paragraph::new(lines)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title("Events"))
        .scroll((scroll_pos, 0));
    f.render_widget(log_widget, chunks[1]);

    // Help text
    let help_lines = vec![Line::from(vec![
        Span::styled("↑↓/j k", Style::default().fg(Color::Cyan)),
        Span::raw(" Scroll  "),
        Span::styled("Home/End", Style::default().fg(Color::Cyan)),
        Span::raw(" Top/Bottom  "),
        Span::raw("Close: "),
        Span::styled("ESC/l", Style::default().fg(Color::Cyan)),
        Span::raw("  Settings: "),
        Span::styled("s", Style::default().fg(Color::Cyan)),
        Span::raw("  Quit: "),
        Span::styled("q", Style::default().fg(Color::Cyan)),
    ])];
    let help = Paragraph::new(help_lines)
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

fn render_priority_picker(f: &mut Frame, state: &ProcessListState, selected: usize) {
    let proc_name = state
        .processes
        .get(state.selected)
        .map(|p| p.name.as_str())
        .unwrap_or("?");

    let area = f.area();
    let popup_width = 40u16;
    let popup_height = (PRIORITY_CLASSES.len() as u16) + 4; // borders + title + spacing
    let x = area.width.saturating_sub(popup_width) / 2;
    let y = area.height.saturating_sub(popup_height) / 2;
    let popup_area = ratatui::layout::Rect::new(
        x,
        y,
        popup_width.min(area.width),
        popup_height.min(area.height),
    );

    // Clear background
    f.render_widget(ratatui::widgets::Clear, popup_area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, (_raw, label)) in PRIORITY_CLASSES.iter().enumerate() {
        let line = if i == selected {
            Line::from(Span::styled(
                format!("  > {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(format!("    {} ", label))
        };
        lines.push(line);
    }

    let title = format!("Set Priority: {}", proc_name);
    let picker = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(picker, popup_area);
}

fn render_exemption_editor(
    f: &mut Frame,
    settings: &SettingsState,
    editor_state: &ExemptionEditorState,
) {
    let area = f.area();
    let popup_width = 60u16;
    let popup_height = 20u16;
    let x = area.width.saturating_sub(popup_width) / 2;
    let y = area.height.saturating_sub(popup_height) / 2;
    let popup_area = ratatui::layout::Rect::new(
        x,
        y,
        popup_width.min(area.width),
        popup_height.min(area.height),
    );

    // Clear background
    f.render_widget(ratatui::widgets::Clear, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(popup_area);

    // Exemption list
    let mut lines: Vec<Line> = Vec::new();
    if settings.user_exemptions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No exemptions. Press 'a' to add process name.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (i, exemption) in settings.user_exemptions.iter().enumerate() {
            let line = if i == editor_state.selected && !editor_state.editing_new {
                Line::from(Span::styled(
                    format!("  > {} ", exemption),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(format!("    {} ", exemption))
            };
            lines.push(line);
        }
    }

    let list_widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("PriorityGuard Exemptions")
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(list_widget, chunks[0]);

    // Input box (when adding new exemption)
    if editor_state.editing_new {
        let (before, after) = editor_state
            .input_buffer
            .split_at(editor_state.input_cursor);
        let input_content = format!("{}|{}", before, after);
        let input_widget = Paragraph::new(input_content)
            .style(Style::default().fg(Color::White))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Add Exemption (Enter to save, Esc to cancel)")
                    .border_style(Style::default().fg(Color::Cyan)),
            );
        f.render_widget(input_widget, chunks[1]);
    } else {
        let input_widget = Paragraph::new("").block(
            Block::default()
                .borders(Borders::ALL)
                .title("Add Exemption"),
        );
        f.render_widget(input_widget, chunks[1]);
    }

    // Help text
    let help_lines = vec![Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Cyan)),
        Span::raw(" Nav  "),
        Span::styled("Del/x", Style::default().fg(Color::Red)),
        Span::raw(" Remove  "),
        Span::styled("a/Enter", Style::default().fg(Color::Green)),
        Span::raw(" Add  "),
        Span::styled("e/ESC", Style::default().fg(Color::Cyan)),
        Span::raw(" Back"),
    ])];
    let help = Paragraph::new(help_lines)
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

/// Build confirmation prompt text for a pending action (pure function for testing).
pub fn build_confirmation_prompt(action: &PendingAction) -> String {
    match action {
        PendingAction::Kill { pid, name } => format!("Kill '{}' (PID {})? [Y/N]", name, pid),
        PendingAction::Suspend { pid, name } => format!("Suspend '{}' (PID {})? [Y/N]", name, pid),
        PendingAction::Resume { pid, name } => format!("Resume '{}' (PID {})? [Y/N]", name, pid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_filter_content ---

    #[test]
    fn filter_content_unfocused_empty() {
        assert_eq!(
            build_filter_content("", 0, false),
            "Filter: (Tab to focus, comma = OR)"
        );
    }

    #[test]
    fn filter_content_unfocused_with_query() {
        assert_eq!(build_filter_content("chrome", 6, false), "Filter: chrome");
    }

    #[test]
    fn filter_content_focused_empty() {
        assert_eq!(build_filter_content("", 0, true), "Filter: |");
    }

    #[test]
    fn filter_content_focused_cursor_at_end() {
        assert_eq!(build_filter_content("abc", 3, true), "Filter: abc|");
    }

    #[test]
    fn filter_content_focused_cursor_at_start() {
        assert_eq!(build_filter_content("abc", 0, true), "Filter: |abc");
    }

    #[test]
    fn filter_content_focused_cursor_mid() {
        assert_eq!(build_filter_content("abc", 1, true), "Filter: a|bc");
    }

    // --- is_compare_active ---

    #[test]
    fn compare_inactive_empty_query() {
        assert!(!is_compare_active("", 5));
    }

    #[test]
    fn compare_inactive_single_result() {
        assert!(!is_compare_active("chrome", 1));
    }

    #[test]
    fn compare_inactive_zero_results() {
        assert!(!is_compare_active("chrome", 0));
    }

    #[test]
    fn compare_active_with_query_and_results() {
        assert!(is_compare_active("chrome", 2));
    }

    #[test]
    fn compare_active_many_results() {
        assert!(is_compare_active("a", 100));
    }

    // --- build_confirmation_prompt ---

    #[test]
    fn confirmation_prompt_kill() {
        let action = PendingAction::Kill {
            pid: 123,
            name: "test.exe".into(),
        };
        assert_eq!(
            build_confirmation_prompt(&action),
            "Kill 'test.exe' (PID 123)? [Y/N]"
        );
    }

    #[test]
    fn confirmation_prompt_suspend() {
        let action = PendingAction::Suspend {
            pid: 456,
            name: "app.exe".into(),
        };
        assert_eq!(
            build_confirmation_prompt(&action),
            "Suspend 'app.exe' (PID 456)? [Y/N]"
        );
    }

    #[test]
    fn confirmation_prompt_resume() {
        let action = PendingAction::Resume {
            pid: 789,
            name: "srv.exe".into(),
        };
        assert_eq!(
            build_confirmation_prompt(&action),
            "Resume 'srv.exe' (PID 789)? [Y/N]"
        );
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_pending_action() -> impl Strategy<Value = PendingAction> {
        let name = "\\PC{0,50}";
        let pid = any::<u32>();
        prop_oneof![
            (pid, name).prop_map(|(p, n)| PendingAction::Kill { pid: p, name: n }),
            (pid, name).prop_map(|(p, n)| PendingAction::Suspend { pid: p, name: n }),
            (pid, name).prop_map(|(p, n)| PendingAction::Resume { pid: p, name: n }),
        ]
    }

    proptest! {
        /// Fuzz build_confirmation_prompt with arbitrary names and PIDs.
        #[test]
        fn fuzz_confirmation_prompt_no_panic(action in arb_pending_action()) {
            let result = build_confirmation_prompt(&action);
            prop_assert!(!result.is_empty());
            prop_assert!(result.contains("[Y/N]"));
            // Must contain the action verb
            prop_assert!(
                result.starts_with("Kill") || result.starts_with("Suspend") || result.starts_with("Resume"),
                "unexpected prefix: {}", result
            );
        }

        /// Fuzz build_confirmation_prompt contains the PID and name.
        #[test]
        fn fuzz_confirmation_prompt_contains_pid(pid in any::<u32>()) {
            let action = PendingAction::Kill { pid, name: "test".into() };
            let result = build_confirmation_prompt(&action);
            prop_assert!(result.contains(&pid.to_string()), "missing PID {} in: {}", pid, result);
        }

        /// Fuzz build_filter_content with arbitrary queries and valid char-boundary cursor positions.
        #[test]
        fn fuzz_build_filter_content(query in "\\PC{0,100}", focused in any::<bool>()) {
            // Only use valid char boundary positions (split_at requires char boundaries)
            let valid_positions: Vec<usize> = query.char_indices().map(|(i, _)| i)
                .chain(std::iter::once(query.len()))
                .collect();
            for cursor in valid_positions {
                let result = build_filter_content(&query, cursor, focused);
                prop_assert!(!result.is_empty());
                prop_assert!(result.starts_with("Filter:"), "bad prefix: {}", result);
            }
        }

        /// Fuzz is_compare_active with arbitrary inputs.
        #[test]
        fn fuzz_is_compare_active(query in "\\PC{0,20}", count in 0usize..100) {
            let result = is_compare_active(&query, count);
            if query.is_empty() || count < 2 {
                prop_assert!(!result);
            } else {
                prop_assert!(result);
            }
        }
    }
}
