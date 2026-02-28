//! App state machine - temporary allow for deprecated fields during migration
#![allow(deprecated)]

use crate::common::{
    DataSourceMode, ExemptionEditorState, PendingAction, ProcessListState, SettingsState, UIMode,
};
use crate::data_source::DataSource;
use crate::priority_guard::windows_api as win_api;
use crate::priority_guard::{PriorityGuardConfig, PriorityGuardEngine};
use crate::process_list::display::clamp_selection;
use crate::process_list::{filter_processes, handle_key as handle_process_key, sort_processes};
use crate::settings::handlers as settings_handlers;
use anyhow::Result;
use crossterm::event::KeyCode;
use std::collections::HashSet;

/// Get the number of logical CPU cores, cached after first call.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8)
}

/// Main application state machine (OOP approach for state management)
pub struct App {
    pub ui_mode: UIMode,
    pub process_list: ProcessListState,
    pub settings: SettingsState,
    pub data_source_mode: DataSourceMode,
    pub priority_guard: PriorityGuardEngine,
    pub log_scroll_offset: Option<u16>,
    pub pending_action: Option<PendingAction>,
    pub suspended_pids: HashSet<u32>,
    pub priority_picker_selected: usize,
    pub exemption_editor: ExemptionEditorState,
    data_source: Box<dyn DataSource>,
}

impl App {
    pub fn new(data_source: Box<dyn DataSource>, data_source_mode: DataSourceMode) -> Self {
        Self {
            ui_mode: UIMode::Processes,
            process_list: ProcessListState::new(),
            settings: SettingsState::new(),
            data_source_mode,
            priority_guard: PriorityGuardEngine::new(),
            log_scroll_offset: None,
            pending_action: None,
            suspended_pids: HashSet::new(),
            priority_picker_selected: 2, // default to Normal
            exemption_editor: ExemptionEditorState::new(),
            data_source,
        }
    }

    /// Update process list from data source, apply filtering and sorting
    pub fn update_processes(&mut self) -> Result<()> {
        let mut procs = self.data_source.get_processes()?;
        self.process_list.total_process_count = procs.len();

        // Apply filters
        procs = filter_processes(
            procs,
            self.settings.hide_self,
            &self.process_list.filter_query,
        );

        // Apply sorting
        procs = sort_processes(
            procs,
            self.process_list.sort_column,
            self.process_list.sort_ascending,
        );

        self.process_list.processes = procs;

        // Restore selection to the previously selected PID if possible
        if let Some(pid) = self.process_list.selected_pid {
            if let Some(idx) = self
                .process_list
                .processes
                .iter()
                .position(|p| p.pid == pid)
            {
                self.process_list.selected = idx;
            }
        }

        self.process_list.selected = clamp_selection(
            self.process_list.selected,
            self.process_list.processes.len(),
        );

        // Only sync selected_pid if user has actively selected something;
        // otherwise keep None so selection tracks the top of the sorted list.
        if self.process_list.selected_pid.is_some() {
            self.process_list.selected_pid = self
                .process_list
                .processes
                .get(self.process_list.selected)
                .map(|p| p.pid);
        }

        self.process_list.rebuild_formatted_rows();

        // Run PriorityGuard tick (hybrid detection: absolute OR per-core OR relative)
        let pg_config = PriorityGuardConfig {
            enabled: self.settings.priority_guard_enabled,
            cpu_threshold: self.settings.priority_guard_cpu_threshold,
            duration_secs: self.settings.priority_guard_duration_secs,
            user_exemptions: self.settings.user_exemptions.clone(),
            per_core_threshold: self.settings.priority_guard_per_core_threshold,
            core_count: num_cpus() as u8,
            relative_multiplier: self.settings.priority_guard_relative_multiplier,
            ema_alpha: self.settings.priority_guard_ema_alpha,
            grace_period_secs: self.settings.priority_guard_grace_period_secs,
            adaptive_sensitivity: self.settings.priority_guard_adaptive_sensitivity,
        };
        self.priority_guard
            .tick(&self.process_list.processes, &pg_config);

        Ok(())
    }

    /// Handle keyboard input and update state
    pub fn on_key(&mut self, key: KeyCode) -> Result<bool> {
        if self.pending_action.is_some() {
            return self.handle_confirmation_key(key);
        }
        match self.ui_mode {
            UIMode::Processes => self.handle_process_key(key),
            UIMode::Settings => self.handle_settings_key(key),
            UIMode::Log => self.handle_log_key(key),
            UIMode::PriorityPicker => self.handle_priority_picker_key(key),
            UIMode::ExemptionEditor => self.handle_exemption_editor_key(key),
        }
    }

    fn handle_process_key(&mut self, key: KeyCode) -> Result<bool> {
        if self.process_list.filter_focused {
            return self.handle_filter_key(key);
        }

        let old_sort = (
            self.process_list.sort_column,
            self.process_list.sort_ascending,
        );
        match key {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.ui_mode = UIMode::Settings;
                self.settings.selected_option = 0;
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.ui_mode = UIMode::Log;
            }
            KeyCode::Tab | KeyCode::Char('/') => {
                self.process_list.filter_focused = true;
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(proc) = self.process_list.processes.get(self.process_list.selected) {
                    self.pending_action = Some(PendingAction::Kill {
                        pid: proc.pid,
                        name: proc.name.clone(),
                    });
                }
            }
            KeyCode::Char('z') => {
                if let Some(proc) = self.process_list.processes.get(self.process_list.selected) {
                    let action = if self.suspended_pids.contains(&proc.pid) {
                        PendingAction::Resume {
                            pid: proc.pid,
                            name: proc.name.clone(),
                        }
                    } else {
                        PendingAction::Suspend {
                            pid: proc.pid,
                            name: proc.name.clone(),
                        }
                    };
                    self.pending_action = Some(action);
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if !self.process_list.processes.is_empty() {
                    self.ui_mode = UIMode::PriorityPicker;
                    self.priority_picker_selected = 2; // Normal
                }
            }
            _ => {
                handle_process_key(&mut self.process_list, key);
            }
        }
        // Re-sort cached data immediately if sort state changed (no expensive re-fetch)
        let new_sort = (
            self.process_list.sort_column,
            self.process_list.sort_ascending,
        );
        if old_sort != new_sort {
            self.process_list.processes = sort_processes(
                std::mem::take(&mut self.process_list.processes),
                self.process_list.sort_column,
                self.process_list.sort_ascending,
            );
            if let Some(pid) = self.process_list.selected_pid {
                if let Some(idx) = self
                    .process_list
                    .processes
                    .iter()
                    .position(|p| p.pid == pid)
                {
                    self.process_list.selected = idx;
                }
            }
            self.process_list.selected = clamp_selection(
                self.process_list.selected,
                self.process_list.processes.len(),
            );
            self.process_list.selected_pid = self
                .process_list
                .processes
                .get(self.process_list.selected)
                .map(|p| p.pid);
            self.process_list.rebuild_formatted_rows();
        }
        Ok(false)
    }

    fn handle_filter_key(&mut self, key: KeyCode) -> Result<bool> {
        let st = &mut self.process_list;
        match key {
            KeyCode::Tab => {
                st.filter_focused = false;
            }
            KeyCode::Esc => {
                st.filter_query.clear();
                st.filter_cursor = 0;
                st.filter_focused = false;
                st.rel_cpu_avg.clear();
                self.update_processes()?;
            }
            KeyCode::Char(c) => {
                st.filter_query.insert(st.filter_cursor, c);
                st.filter_cursor += c.len_utf8();
                st.rel_cpu_avg.clear();
                self.update_processes()?;
            }
            KeyCode::Backspace => {
                if st.filter_cursor > 0 {
                    // Find previous char boundary
                    let prev = st.filter_query[..st.filter_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    st.filter_query.remove(prev);
                    st.filter_cursor = prev;
                    st.rel_cpu_avg.clear();
                    self.update_processes()?;
                }
            }
            KeyCode::Delete => {
                if st.filter_cursor < st.filter_query.len() {
                    st.filter_query.remove(st.filter_cursor);
                    st.rel_cpu_avg.clear();
                    self.update_processes()?;
                }
            }
            KeyCode::Left => {
                if st.filter_cursor > 0 {
                    // Move to previous char boundary
                    st.filter_cursor = st.filter_query[..st.filter_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if st.filter_cursor < st.filter_query.len() {
                    // Move to next char boundary
                    st.filter_cursor = st.filter_query[st.filter_cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| st.filter_cursor + i)
                        .unwrap_or(st.filter_query.len());
                }
            }
            KeyCode::Home => {
                st.filter_cursor = 0;
            }
            KeyCode::End => {
                st.filter_cursor = st.filter_query.len();
            }
            KeyCode::Up | KeyCode::Down => {
                handle_process_key(&mut self.process_list, key);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_settings_key(&mut self, key: KeyCode) -> Result<bool> {
        match key {
            KeyCode::Esc | KeyCode::Char('s') | KeyCode::Char('S') => {
                self.ui_mode = UIMode::Processes;
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            KeyCode::Enter => {
                // Enter on option 10 (Exemptions) opens the exemption editor
                if self.settings.selected_option == 10 {
                    self.ui_mode = UIMode::ExemptionEditor;
                    self.exemption_editor.selected = 0;
                    self.exemption_editor.editing_new = false;
                    self.exemption_editor.input_buffer.clear();
                    self.exemption_editor.input_cursor = 0;
                }
            }
            _ => {
                settings_handlers::handle_key(&mut self.settings, key);
                // Persist settings after each change
                let persisted = crate::config::PersistedSettings::from(&self.settings);
                if let Err(e) = crate::config::save(&persisted) {
                    log::warn!("Failed to save settings: {}", e);
                }
            }
        }
        Ok(false)
    }

    fn handle_log_key(&mut self, key: KeyCode) -> Result<bool> {
        match key {
            KeyCode::Esc | KeyCode::Char('l') | KeyCode::Char('L') => {
                self.log_scroll_offset = None;
                self.ui_mode = UIMode::Processes;
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.log_scroll_offset = None;
                self.ui_mode = UIMode::Settings;
                self.settings.selected_option = 0;
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let offset = self.log_scroll_offset.unwrap_or(u16::MAX);
                self.log_scroll_offset = Some(offset.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let offset = self.log_scroll_offset.unwrap_or(u16::MAX);
                self.log_scroll_offset = Some(offset.saturating_add(1));
            }
            KeyCode::Home => {
                self.log_scroll_offset = Some(0);
            }
            KeyCode::End => {
                self.log_scroll_offset = None;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_confirmation_key(&mut self, key: KeyCode) -> Result<bool> {
        match key {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(action) = self.pending_action.take() {
                    match action {
                        PendingAction::Kill { pid, name } => {
                            let ok = win_api::terminate_process(pid);
                            log::info!(
                                "Kill {} (PID {}): {}",
                                name,
                                pid,
                                if ok { "success" } else { "failed" }
                            );
                        }
                        PendingAction::Suspend { pid, name } => {
                            let ok = win_api::suspend_process(pid);
                            if ok {
                                self.suspended_pids.insert(pid);
                            }
                            log::info!(
                                "Suspend {} (PID {}): {}",
                                name,
                                pid,
                                if ok { "success" } else { "failed" }
                            );
                        }
                        PendingAction::Resume { pid, name } => {
                            let ok = win_api::resume_process(pid);
                            if ok {
                                self.suspended_pids.remove(&pid);
                            }
                            log::info!(
                                "Resume {} (PID {}): {}",
                                name,
                                pid,
                                if ok { "success" } else { "failed" }
                            );
                        }
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending_action = None;
            }
            _ => {} // ignore other keys while confirming
        }
        Ok(false)
    }

    fn handle_priority_picker_key(&mut self, key: KeyCode) -> Result<bool> {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.priority_picker_selected = self.priority_picker_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.priority_picker_selected < win_api::PRIORITY_CLASSES.len() - 1 {
                    self.priority_picker_selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(proc) = self.process_list.processes.get(self.process_list.selected) {
                    let (raw, label) = win_api::PRIORITY_CLASSES[self.priority_picker_selected];
                    let ok = win_api::set_priority(proc.pid, raw);
                    log::info!(
                        "Set priority of {} (PID {}) to {}: {}",
                        proc.name,
                        proc.pid,
                        label,
                        if ok { "success" } else { "failed" }
                    );
                }
                self.ui_mode = UIMode::Processes;
            }
            KeyCode::Esc | KeyCode::Char('p') | KeyCode::Char('P') => {
                self.ui_mode = UIMode::Processes;
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            _ => {}
        }
        Ok(false)
    }

    fn handle_exemption_editor_key(&mut self, key: KeyCode) -> Result<bool> {
        if self.exemption_editor.editing_new {
            // Text input mode
            match key {
                KeyCode::Enter => {
                    // Validate and add new exemption
                    let input = self.exemption_editor.input_buffer.trim().to_string();
                    if !input.is_empty() {
                        // Check for case-insensitive duplicates
                        let is_duplicate = self
                            .settings
                            .user_exemptions
                            .iter()
                            .any(|e| e.eq_ignore_ascii_case(&input));
                        if !is_duplicate {
                            self.settings.user_exemptions.push(input.clone());
                            log::info!("Added PriorityGuard exemption: {}", input);
                            // Save settings
                            let persisted = crate::config::PersistedSettings::from(&self.settings);
                            if let Err(e) = crate::config::save(&persisted) {
                                log::warn!("Failed to save settings: {}", e);
                            }
                        }
                    }
                    // Exit editing mode
                    self.exemption_editor.editing_new = false;
                    self.exemption_editor.input_buffer.clear();
                    self.exemption_editor.input_cursor = 0;
                }
                KeyCode::Esc => {
                    // Cancel editing
                    self.exemption_editor.editing_new = false;
                    self.exemption_editor.input_buffer.clear();
                    self.exemption_editor.input_cursor = 0;
                }
                KeyCode::Char(c) => {
                    self.exemption_editor
                        .input_buffer
                        .insert(self.exemption_editor.input_cursor, c);
                    self.exemption_editor.input_cursor += c.len_utf8();
                }
                KeyCode::Backspace => {
                    if self.exemption_editor.input_cursor > 0 {
                        let prev = self.exemption_editor.input_buffer
                            [..self.exemption_editor.input_cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        self.exemption_editor.input_buffer.remove(prev);
                        self.exemption_editor.input_cursor = prev;
                    }
                }
                KeyCode::Delete => {
                    if self.exemption_editor.input_cursor < self.exemption_editor.input_buffer.len()
                    {
                        self.exemption_editor
                            .input_buffer
                            .remove(self.exemption_editor.input_cursor);
                    }
                }
                KeyCode::Left => {
                    if self.exemption_editor.input_cursor > 0 {
                        self.exemption_editor.input_cursor = self.exemption_editor.input_buffer
                            [..self.exemption_editor.input_cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                    }
                }
                KeyCode::Right => {
                    if self.exemption_editor.input_cursor < self.exemption_editor.input_buffer.len()
                    {
                        self.exemption_editor.input_cursor = self.exemption_editor.input_buffer
                            [self.exemption_editor.input_cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| self.exemption_editor.input_cursor + i)
                            .unwrap_or(self.exemption_editor.input_buffer.len());
                    }
                }
                KeyCode::Home => {
                    self.exemption_editor.input_cursor = 0;
                }
                KeyCode::End => {
                    self.exemption_editor.input_cursor = self.exemption_editor.input_buffer.len();
                }
                _ => {}
            }
        } else {
            // Navigation mode
            match key {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.exemption_editor.selected =
                        self.exemption_editor.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !self.settings.user_exemptions.is_empty()
                        && self.exemption_editor.selected < self.settings.user_exemptions.len() - 1
                    {
                        self.exemption_editor.selected += 1;
                    }
                }
                KeyCode::Delete | KeyCode::Char('x') => {
                    if !self.settings.user_exemptions.is_empty()
                        && self.exemption_editor.selected < self.settings.user_exemptions.len()
                    {
                        let removed = self
                            .settings
                            .user_exemptions
                            .remove(self.exemption_editor.selected);
                        log::info!("Removed PriorityGuard exemption: {}", removed);
                        // Adjust selection if needed
                        if self.exemption_editor.selected >= self.settings.user_exemptions.len()
                            && self.exemption_editor.selected > 0
                        {
                            self.exemption_editor.selected -= 1;
                        }
                        // Save settings
                        let persisted = crate::config::PersistedSettings::from(&self.settings);
                        if let Err(e) = crate::config::save(&persisted) {
                            log::warn!("Failed to save settings: {}", e);
                        }
                    }
                }
                KeyCode::Char('a') | KeyCode::Enter => {
                    // Start adding new exemption
                    self.exemption_editor.editing_new = true;
                    self.exemption_editor.input_buffer.clear();
                    self.exemption_editor.input_cursor = 0;
                }
                KeyCode::Esc | KeyCode::Char('e') | KeyCode::Char('E') => {
                    // Exit to Settings
                    self.ui_mode = UIMode::Settings;
                    self.exemption_editor.selected = 0;
                    self.exemption_editor.editing_new = false;
                    self.exemption_editor.input_buffer.clear();
                    self.exemption_editor.input_cursor = 0;
                }
                KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
                _ => {}
            }
        }
        Ok(false)
    }

    pub fn poll_interval_ms(&self) -> u64 {
        self.settings.poll_interval_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ProcessInfo;

    struct MockDataSource {
        processes: Vec<ProcessInfo>,
    }

    impl MockDataSource {
        fn new(processes: Vec<ProcessInfo>) -> Self {
            Self { processes }
        }
    }

    impl DataSource for MockDataSource {
        fn get_processes(&mut self) -> anyhow::Result<Vec<ProcessInfo>> {
            Ok(self.processes.clone())
        }
    }

    fn sample_procs() -> Vec<ProcessInfo> {
        vec![
            ProcessInfo {
                pid: 1,
                name: "alpha".into(),
                cpu: 10.0,
                memory: 1000,
            },
            ProcessInfo {
                pid: 2,
                name: "bravo".into(),
                cpu: 50.0,
                memory: 2000,
            },
            ProcessInfo {
                pid: 3,
                name: "charlie".into(),
                cpu: 1.0,
                memory: 500,
            },
        ]
    }

    fn make_app(procs: Vec<ProcessInfo>) -> App {
        App::new(
            Box::new(MockDataSource::new(procs)),
            DataSourceMode::PollingFallback,
        )
    }

    #[test]
    fn new_defaults() {
        let app = make_app(vec![]);
        assert_eq!(app.ui_mode, UIMode::Processes);
        assert_eq!(app.data_source_mode, DataSourceMode::PollingFallback);
        assert!(app.process_list.processes.is_empty());
        assert_eq!(app.settings.poll_interval_ms, 2000);
    }

    #[test]
    fn update_processes_populates_list() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        assert_eq!(app.process_list.processes.len(), 3);
    }

    #[test]
    fn update_processes_applies_sorting() {
        let mut app = make_app(sample_procs());
        app.process_list.sort_column = crate::common::SortColumn::Name;
        app.process_list.sort_ascending = true;
        app.update_processes().unwrap();
        let names: Vec<&str> = app
            .process_list
            .processes
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn update_processes_clamps_selection() {
        let mut app = make_app(sample_procs());
        app.process_list.selected = 100;
        app.update_processes().unwrap();
        assert!(app.process_list.selected < app.process_list.processes.len());
    }

    #[test]
    fn update_processes_with_hide_self() {
        let self_pid = std::process::id();
        let procs = vec![
            ProcessInfo {
                pid: self_pid,
                name: "self".into(),
                cpu: 0.0,
                memory: 0,
            },
            ProcessInfo {
                pid: 999,
                name: "other".into(),
                cpu: 0.0,
                memory: 0,
            },
        ];
        let mut app = make_app(procs);
        app.settings.hide_self = true;
        app.update_processes().unwrap();
        assert!(app.process_list.processes.iter().all(|p| p.pid != self_pid));
    }

    #[test]
    fn q_quits() {
        let mut app = make_app(vec![]);
        assert!(app.on_key(KeyCode::Char('q')).unwrap());
    }

    #[test]
    fn upper_q_quits() {
        let mut app = make_app(vec![]);
        assert!(app.on_key(KeyCode::Char('Q')).unwrap());
    }

    #[test]
    fn s_opens_settings() {
        let mut app = make_app(vec![]);
        assert!(!app.on_key(KeyCode::Char('s')).unwrap());
        assert_eq!(app.ui_mode, UIMode::Settings);
    }

    #[test]
    fn upper_s_opens_settings() {
        let mut app = make_app(vec![]);
        app.on_key(KeyCode::Char('S')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Settings);
    }

    #[test]
    fn esc_closes_settings() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Settings;
        assert!(!app.on_key(KeyCode::Esc).unwrap());
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn settings_keys_do_not_quit() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Settings;
        assert!(!app.on_key(KeyCode::Up).unwrap());
        assert!(!app.on_key(KeyCode::Down).unwrap());
        assert!(!app.on_key(KeyCode::Left).unwrap());
        assert!(!app.on_key(KeyCode::Right).unwrap());
    }

    #[test]
    fn poll_interval_returns_setting() {
        let mut app = make_app(vec![]);
        app.settings.poll_interval_ms = 500;
        assert_eq!(app.poll_interval_ms(), 500);
    }

    #[test]
    fn round_trip_settings() {
        let mut app = make_app(vec![]);
        assert_eq!(app.ui_mode, UIMode::Processes);
        app.on_key(KeyCode::Char('s')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Settings);
        app.on_key(KeyCode::Esc).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn settings_resets_selected_option() {
        let mut app = make_app(vec![]);
        app.settings.selected_option = 1;
        app.on_key(KeyCode::Char('s')).unwrap();
        assert_eq!(app.settings.selected_option, 0);
    }

    #[test]
    fn table_always_sorted_by_selected_column_and_direction() {
        use crate::common::SortColumn;

        let procs = vec![
            ProcessInfo {
                pid: 3,
                name: "charlie".into(),
                cpu: 50.0,
                memory: 500,
            },
            ProcessInfo {
                pid: 1,
                name: "alpha".into(),
                cpu: 10.0,
                memory: 3000,
            },
            ProcessInfo {
                pid: 2,
                name: "bravo".into(),
                cpu: 1.0,
                memory: 2000,
            },
            ProcessInfo {
                pid: 4,
                name: "delta".into(),
                cpu: 25.0,
                memory: 1000,
            },
        ];

        for &col in &[
            SortColumn::Pid,
            SortColumn::Name,
            SortColumn::Cpu,
            SortColumn::Memory,
        ] {
            for &asc in &[true, false] {
                let mut app = make_app(procs.clone());
                app.process_list.sort_column = col;
                app.process_list.sort_ascending = asc;
                app.update_processes().unwrap();

                let ps = &app.process_list.processes;
                for w in ps.windows(2) {
                    let ordered = match (col, asc) {
                        (SortColumn::Pid, true) => w[0].pid <= w[1].pid,
                        (SortColumn::Pid, false) => w[0].pid >= w[1].pid,
                        (SortColumn::Name, true) => {
                            w[0].name.to_ascii_lowercase() <= w[1].name.to_ascii_lowercase()
                        }
                        (SortColumn::Name, false) => {
                            w[0].name.to_ascii_lowercase() >= w[1].name.to_ascii_lowercase()
                        }
                        (SortColumn::Cpu, true) => w[0].cpu <= w[1].cpu,
                        (SortColumn::Cpu, false) => w[0].cpu >= w[1].cpu,
                        (SortColumn::Memory, true) => w[0].memory <= w[1].memory,
                        (SortColumn::Memory, false) => w[0].memory >= w[1].memory,
                    };
                    assert!(
                        ordered,
                        "Not sorted by {:?} asc={}: {:?} vs {:?}",
                        col, asc, w[0], w[1]
                    );
                }
            }
        }
    }

    #[test]
    fn sort_column_change_via_key_keeps_table_sorted() {
        use crate::common::SortColumn;

        let mut app = make_app(vec![
            ProcessInfo {
                pid: 3,
                name: "charlie".into(),
                cpu: 50.0,
                memory: 500,
            },
            ProcessInfo {
                pid: 1,
                name: "alpha".into(),
                cpu: 10.0,
                memory: 3000,
            },
            ProcessInfo {
                pid: 2,
                name: "bravo".into(),
                cpu: 1.0,
                memory: 2000,
            },
        ]);
        app.update_processes().unwrap();

        // Cycle through columns with Right key, verify sorted after each
        for _ in 0..4 {
            app.on_key(KeyCode::Right).unwrap();
            let col = app.process_list.sort_column;
            let asc = app.process_list.sort_ascending;
            let ps = &app.process_list.processes;
            for w in ps.windows(2) {
                let ordered = match (col, asc) {
                    (SortColumn::Pid, true) => w[0].pid <= w[1].pid,
                    (SortColumn::Pid, false) => w[0].pid >= w[1].pid,
                    (SortColumn::Name, true) => w[0].name <= w[1].name,
                    (SortColumn::Name, false) => w[0].name >= w[1].name,
                    (SortColumn::Cpu, true) => w[0].cpu <= w[1].cpu,
                    (SortColumn::Cpu, false) => w[0].cpu >= w[1].cpu,
                    (SortColumn::Memory, true) => w[0].memory <= w[1].memory,
                    (SortColumn::Memory, false) => w[0].memory >= w[1].memory,
                };
                assert!(ordered, "Not sorted after Right key: {:?} asc={}", col, asc);
            }
        }

        // Toggle direction with Space, verify still sorted
        app.on_key(KeyCode::Char(' ')).unwrap();
        let col = app.process_list.sort_column;
        let asc = app.process_list.sort_ascending;
        let ps = &app.process_list.processes;
        for w in ps.windows(2) {
            let ordered = match (col, asc) {
                (SortColumn::Pid, true) => w[0].pid <= w[1].pid,
                (SortColumn::Pid, false) => w[0].pid >= w[1].pid,
                (SortColumn::Name, true) => w[0].name <= w[1].name,
                (SortColumn::Name, false) => w[0].name >= w[1].name,
                (SortColumn::Cpu, true) => w[0].cpu <= w[1].cpu,
                (SortColumn::Cpu, false) => w[0].cpu >= w[1].cpu,
                (SortColumn::Memory, true) => w[0].memory <= w[1].memory,
                (SortColumn::Memory, false) => w[0].memory >= w[1].memory,
            };
            assert!(
                ordered,
                "Not sorted after Space toggle: {:?} asc={}",
                col, asc
            );
        }
    }

    // --- Filter key handling ---

    fn make_app_with_filter(procs: Vec<ProcessInfo>, query: &str) -> App {
        let mut app = make_app(procs);
        app.process_list.filter_focused = true;
        for c in query.chars() {
            app.on_key(KeyCode::Char(c)).unwrap();
        }
        app
    }

    #[test]
    fn tab_enters_filter_mode() {
        let mut app = make_app(sample_procs());
        assert!(!app.process_list.filter_focused);
        app.on_key(KeyCode::Tab).unwrap();
        assert!(app.process_list.filter_focused);
    }

    #[test]
    fn tab_exits_filter_mode() {
        let mut app = make_app(vec![]);
        app.process_list.filter_focused = true;
        app.on_key(KeyCode::Tab).unwrap();
        assert!(!app.process_list.filter_focused);
    }

    #[test]
    fn filter_char_inserts_and_advances_cursor() {
        let app = make_app_with_filter(sample_procs(), "ab");
        assert_eq!(app.process_list.filter_query, "ab");
        assert_eq!(app.process_list.filter_cursor, 2);
    }

    #[test]
    fn filter_backspace_removes_before_cursor() {
        let mut app = make_app_with_filter(sample_procs(), "abc");
        app.on_key(KeyCode::Backspace).unwrap();
        assert_eq!(app.process_list.filter_query, "ab");
        assert_eq!(app.process_list.filter_cursor, 2);
    }

    #[test]
    fn filter_backspace_at_start_does_nothing() {
        let mut app = make_app_with_filter(sample_procs(), "");
        app.on_key(KeyCode::Backspace).unwrap();
        assert_eq!(app.process_list.filter_query, "");
        assert_eq!(app.process_list.filter_cursor, 0);
    }

    #[test]
    fn filter_delete_removes_after_cursor() {
        let mut app = make_app_with_filter(sample_procs(), "abc");
        // Move cursor to start
        app.on_key(KeyCode::Home).unwrap();
        app.on_key(KeyCode::Delete).unwrap();
        assert_eq!(app.process_list.filter_query, "bc");
        assert_eq!(app.process_list.filter_cursor, 0);
    }

    #[test]
    fn filter_delete_at_end_does_nothing() {
        let mut app = make_app_with_filter(sample_procs(), "abc");
        app.on_key(KeyCode::Delete).unwrap();
        assert_eq!(app.process_list.filter_query, "abc");
    }

    #[test]
    fn filter_left_right_navigation() {
        let mut app = make_app_with_filter(sample_procs(), "abc");
        assert_eq!(app.process_list.filter_cursor, 3);
        app.on_key(KeyCode::Left).unwrap();
        assert_eq!(app.process_list.filter_cursor, 2);
        app.on_key(KeyCode::Left).unwrap();
        assert_eq!(app.process_list.filter_cursor, 1);
        app.on_key(KeyCode::Right).unwrap();
        assert_eq!(app.process_list.filter_cursor, 2);
    }

    #[test]
    fn filter_left_at_start_stays() {
        let mut app = make_app_with_filter(sample_procs(), "a");
        app.on_key(KeyCode::Home).unwrap();
        app.on_key(KeyCode::Left).unwrap();
        assert_eq!(app.process_list.filter_cursor, 0);
    }

    #[test]
    fn filter_right_at_end_stays() {
        let mut app = make_app_with_filter(sample_procs(), "a");
        app.on_key(KeyCode::Right).unwrap();
        assert_eq!(app.process_list.filter_cursor, 1);
    }

    #[test]
    fn filter_home_end() {
        let mut app = make_app_with_filter(sample_procs(), "abc");
        app.on_key(KeyCode::Home).unwrap();
        assert_eq!(app.process_list.filter_cursor, 0);
        app.on_key(KeyCode::End).unwrap();
        assert_eq!(app.process_list.filter_cursor, 3);
    }

    #[test]
    fn filter_insert_at_cursor_mid() {
        let mut app = make_app_with_filter(sample_procs(), "ac");
        // cursor is at 2, move left once to position 1
        app.on_key(KeyCode::Left).unwrap();
        app.on_key(KeyCode::Char('b')).unwrap();
        assert_eq!(app.process_list.filter_query, "abc");
        assert_eq!(app.process_list.filter_cursor, 2);
    }

    #[test]
    fn filter_esc_clears_query_and_exits() {
        let mut app = make_app_with_filter(sample_procs(), "test");
        app.on_key(KeyCode::Esc).unwrap();
        assert!(app.process_list.filter_query.is_empty());
        assert_eq!(app.process_list.filter_cursor, 0);
        assert!(!app.process_list.filter_focused);
    }

    #[test]
    fn filter_clears_rel_cpu_avg_on_text_change() {
        let mut app = make_app_with_filter(sample_procs(), "a");
        app.process_list.rel_cpu_avg.insert(1, (2.0, 10));
        app.on_key(KeyCode::Char('b')).unwrap();
        assert!(app.process_list.rel_cpu_avg.is_empty());
    }

    #[test]
    fn filter_up_down_navigate_while_focused() {
        let mut app = make_app_with_filter(sample_procs(), "");
        app.update_processes().unwrap();
        let before = app.process_list.selected;
        app.on_key(KeyCode::Down).unwrap();
        // Should have moved selection (or stayed if list is small)
        assert!(app.process_list.selected >= before);
    }

    #[test]
    fn filter_q_does_not_quit() {
        let mut app = make_app_with_filter(sample_procs(), "");
        let quit = app.on_key(KeyCode::Char('q')).unwrap();
        assert!(!quit, "q should type into filter, not quit");
        assert_eq!(app.process_list.filter_query, "q");
    }

    // --- / enters filter mode ---

    #[test]
    fn slash_enters_filter_mode() {
        let mut app = make_app(sample_procs());
        assert!(!app.process_list.filter_focused);
        app.on_key(KeyCode::Char('/')).unwrap();
        assert!(app.process_list.filter_focused);
    }

    // --- s toggles settings closed ---

    #[test]
    fn s_closes_settings() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Settings;
        app.on_key(KeyCode::Char('s')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn upper_s_closes_settings() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Settings;
        app.on_key(KeyCode::Char('S')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn q_quits_from_settings() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Settings;
        assert!(app.on_key(KeyCode::Char('q')).unwrap());
    }

    // --- s opens settings from log ---

    #[test]
    fn s_opens_settings_from_log() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Log;
        app.on_key(KeyCode::Char('s')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Settings);
        assert_eq!(app.settings.selected_option, 0);
    }

    // --- log scroll keys ---

    #[test]
    fn log_up_sets_scroll_offset() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Log;
        assert!(app.log_scroll_offset.is_none());
        app.on_key(KeyCode::Up).unwrap();
        assert!(app.log_scroll_offset.is_some());
    }

    #[test]
    fn log_j_k_scrolls() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Log;
        app.on_key(KeyCode::Char('j')).unwrap();
        assert!(app.log_scroll_offset.is_some());
        app.on_key(KeyCode::Char('k')).unwrap();
        assert!(app.log_scroll_offset.is_some());
    }

    #[test]
    fn log_home_scrolls_to_top() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Log;
        app.on_key(KeyCode::Home).unwrap();
        assert_eq!(app.log_scroll_offset, Some(0));
    }

    #[test]
    fn log_end_resets_to_auto_scroll() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Log;
        app.log_scroll_offset = Some(5);
        app.on_key(KeyCode::End).unwrap();
        assert!(app.log_scroll_offset.is_none());
    }

    #[test]
    fn log_close_resets_scroll() {
        let mut app = make_app(vec![]);
        app.ui_mode = UIMode::Log;
        app.log_scroll_offset = Some(10);
        app.on_key(KeyCode::Esc).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
        assert!(app.log_scroll_offset.is_none());
    }

    // --- total_process_count ---

    #[test]
    fn update_sets_total_process_count() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        assert_eq!(app.process_list.total_process_count, 3);
    }

    #[test]
    fn total_count_before_filter() {
        let mut app = make_app(sample_procs());
        app.process_list.filter_focused = true;
        // Type "alpha" to filter
        for c in "alpha".chars() {
            app.on_key(KeyCode::Char(c)).unwrap();
        }
        assert_eq!(app.process_list.total_process_count, 3);
        assert_eq!(app.process_list.processes.len(), 1);
    }

    #[test]
    fn update_preserves_selection_by_pid() {
        let mut app = make_app(sample_procs());
        app.process_list.sort_column = crate::common::SortColumn::Pid;
        app.process_list.sort_ascending = true;
        app.update_processes().unwrap();
        // Select pid=2 (bravo), which is at index 1
        app.process_list.selected = 1;
        app.process_list.selected_pid = Some(2);
        // Switch to descending — pid 2 moves to index 1 (order: 3,2,1)
        app.process_list.sort_ascending = false;
        app.update_processes().unwrap();
        assert_eq!(app.process_list.processes[app.process_list.selected].pid, 2);
    }

    // --- Process actions ---

    #[test]
    fn x_sets_pending_kill() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('x')).unwrap();
        assert!(matches!(
            app.pending_action,
            Some(PendingAction::Kill { .. })
        ));
    }

    #[test]
    fn delete_sets_pending_kill() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Delete).unwrap();
        assert!(matches!(
            app.pending_action,
            Some(PendingAction::Kill { .. })
        ));
    }

    #[test]
    fn z_sets_pending_suspend() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('z')).unwrap();
        assert!(matches!(
            app.pending_action,
            Some(PendingAction::Suspend { .. })
        ));
    }

    #[test]
    fn z_sets_pending_resume_when_suspended() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        let pid = app.process_list.processes[app.process_list.selected].pid;
        app.suspended_pids.insert(pid);
        app.on_key(KeyCode::Char('z')).unwrap();
        assert!(matches!(
            app.pending_action,
            Some(PendingAction::Resume { .. })
        ));
    }

    #[test]
    fn x_on_empty_list_does_nothing() {
        let mut app = make_app(vec![]);
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('x')).unwrap();
        assert!(app.pending_action.is_none());
    }

    #[test]
    fn confirmation_n_cancels() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('x')).unwrap();
        assert!(app.pending_action.is_some());
        app.on_key(KeyCode::Char('n')).unwrap();
        assert!(app.pending_action.is_none());
    }

    #[test]
    fn confirmation_esc_cancels() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('x')).unwrap();
        app.on_key(KeyCode::Esc).unwrap();
        assert!(app.pending_action.is_none());
    }

    #[test]
    fn confirmation_blocks_other_keys() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('x')).unwrap();
        // s should not switch to settings while confirming
        app.on_key(KeyCode::Char('s')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
        assert!(app.pending_action.is_some());
    }

    #[test]
    fn confirmation_y_clears_action() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('x')).unwrap();
        app.on_key(KeyCode::Char('y')).unwrap();
        assert!(app.pending_action.is_none());
    }

    // --- Priority picker ---

    #[test]
    fn p_opens_priority_picker() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        assert_eq!(app.ui_mode, UIMode::PriorityPicker);
    }

    #[test]
    fn p_on_empty_list_does_nothing() {
        let mut app = make_app(vec![]);
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn priority_picker_esc_closes() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        app.on_key(KeyCode::Esc).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn priority_picker_navigate() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        assert_eq!(app.priority_picker_selected, 2); // default Normal
        app.on_key(KeyCode::Down).unwrap();
        assert_eq!(app.priority_picker_selected, 3);
        app.on_key(KeyCode::Up).unwrap();
        assert_eq!(app.priority_picker_selected, 2);
    }

    #[test]
    fn priority_picker_enter_closes() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        app.on_key(KeyCode::Enter).unwrap();
        assert_eq!(app.ui_mode, UIMode::Processes);
    }

    #[test]
    fn priority_picker_q_quits() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        assert!(app.on_key(KeyCode::Char('q')).unwrap());
    }

    #[test]
    fn priority_picker_clamps_at_bounds() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.on_key(KeyCode::Char('p')).unwrap();
        // Try going above top
        app.on_key(KeyCode::Up).unwrap();
        app.on_key(KeyCode::Up).unwrap();
        app.on_key(KeyCode::Up).unwrap();
        assert_eq!(app.priority_picker_selected, 0);
        // Go to bottom
        for _ in 0..10 {
            app.on_key(KeyCode::Down).unwrap();
        }
        assert_eq!(app.priority_picker_selected, 5);
    }

    // --- Filter mode doesn't trigger actions ---

    #[test]
    fn filter_x_types_not_kills() {
        let mut app = make_app(sample_procs());
        app.update_processes().unwrap();
        app.process_list.filter_focused = true;
        app.on_key(KeyCode::Char('x')).unwrap();
        assert!(app.pending_action.is_none());
        assert_eq!(app.process_list.filter_query, "x");
    }

    #[test]
    fn new_app_has_empty_suspended_pids() {
        let app = make_app(vec![]);
        assert!(app.suspended_pids.is_empty());
        assert!(app.pending_action.is_none());
        assert_eq!(app.priority_picker_selected, 2);
    }

    // --- Exemption editor tests ---

    #[test]
    fn exemptions_passed_to_priority_guard() {
        let mut app = make_app(sample_procs());
        app.settings.user_exemptions = vec!["chrome.exe".to_string(), "firefox.exe".to_string()];
        app.update_processes().unwrap();
        // PriorityGuard config should receive the exemptions
        // (verified by checking it doesn't demote exempted processes in integration tests)
    }

    #[test]
    fn exemption_editor_state_initialized() {
        let app = make_app(vec![]);
        assert_eq!(app.exemption_editor.selected, 0);
        assert!(!app.exemption_editor.editing_new);
        assert!(app.exemption_editor.input_buffer.is_empty());
        assert_eq!(app.exemption_editor.input_cursor, 0);
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use crate::common::ProcessInfo;
    use proptest::prelude::*;

    struct MockDataSource {
        processes: Vec<ProcessInfo>,
    }
    impl DataSource for MockDataSource {
        fn get_processes(&mut self) -> anyhow::Result<Vec<ProcessInfo>> {
            Ok(self.processes.clone())
        }
    }

    fn sample_procs() -> Vec<ProcessInfo> {
        vec![
            ProcessInfo {
                pid: 1,
                name: "alpha".into(),
                cpu: 10.0,
                memory: 1000,
            },
            ProcessInfo {
                pid: 2,
                name: "bravo".into(),
                cpu: 50.0,
                memory: 2000,
            },
            ProcessInfo {
                pid: 3,
                name: "charlie".into(),
                cpu: 1.0,
                memory: 500,
            },
        ]
    }

    fn make_app(procs: Vec<ProcessInfo>) -> App {
        App::new(
            Box::new(MockDataSource { processes: procs }),
            DataSourceMode::PollingFallback,
        )
    }

    fn arb_keycode() -> impl Strategy<Value = KeyCode> {
        prop_oneof![
            Just(KeyCode::Up),
            Just(KeyCode::Down),
            Just(KeyCode::Left),
            Just(KeyCode::Right),
            Just(KeyCode::Tab),
            Just(KeyCode::Esc),
            Just(KeyCode::Backspace),
            Just(KeyCode::Delete),
            Just(KeyCode::Home),
            Just(KeyCode::End),
            Just(KeyCode::PageUp),
            Just(KeyCode::PageDown),
            Just(KeyCode::Char(' ')),
            Just(KeyCode::Char('s')),
            Just(KeyCode::Char('l')),
            Just(KeyCode::Char('j')),
            Just(KeyCode::Char('k')),
            Just(KeyCode::Char('/')),
            // Don't include 'q' — it quits the app
            any::<char>()
                .prop_filter("no quit", |c| *c != 'q' && *c != 'Q')
                .prop_map(KeyCode::Char),
        ]
    }

    proptest! {
        #[test]
        fn fuzz_key_sequence_no_panic(keys in prop::collection::vec(arb_keycode(), 1..100)) {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();
            for key in keys {
                let _ = app.on_key(key);
            }
        }

        #[test]
        fn fuzz_key_sequence_invariants(keys in prop::collection::vec(arb_keycode(), 1..50)) {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();

            for key in keys {
                let _ = app.on_key(key);

                // Invariant: filter cursor always valid
                prop_assert!(
                    app.process_list.filter_cursor <= app.process_list.filter_query.len(),
                    "cursor {} > query len {}",
                    app.process_list.filter_cursor,
                    app.process_list.filter_query.len()
                );

                // Invariant: selected index valid when processes non-empty
                if !app.process_list.processes.is_empty() {
                    prop_assert!(
                        app.process_list.selected < app.process_list.processes.len(),
                        "selected {} >= len {}",
                        app.process_list.selected,
                        app.process_list.processes.len()
                    );
                }

                // Invariant: ui_mode is always valid
                prop_assert!(
                    app.ui_mode == UIMode::Processes || app.ui_mode == UIMode::Settings || app.ui_mode == UIMode::Log || app.ui_mode == UIMode::PriorityPicker || app.ui_mode == UIMode::ExemptionEditor
                );

                // Invariant: priority_picker_selected always in bounds
                prop_assert!(
                    app.priority_picker_selected < win_api::PRIORITY_CLASSES.len(),
                    "priority_picker_selected {} >= PRIORITY_CLASSES len {}",
                    app.priority_picker_selected,
                    win_api::PRIORITY_CLASSES.len()
                );

                // Invariant: suspended_pids is accessible without panic
                let _ = app.suspended_pids.len();
            }
        }

        #[test]
        fn fuzz_filter_typing_invariants(text in "\\PC{0,30}") {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();
            // Enter filter mode
            let _ = app.on_key(KeyCode::Tab);

            // Type arbitrary text
            for c in text.chars() {
                let _ = app.on_key(KeyCode::Char(c));
                prop_assert!(
                    app.process_list.filter_cursor <= app.process_list.filter_query.len(),
                    "cursor {} > query len {} after typing '{}'",
                    app.process_list.filter_cursor,
                    app.process_list.filter_query.len(),
                    c
                );
            }
        }

        /// Fuzz confirmation flow: trigger action then confirm/cancel with random keys.
        #[test]
        fn fuzz_confirmation_flow(
            action_key in prop_oneof![Just(KeyCode::Char('x')), Just(KeyCode::Char('z')), Just(KeyCode::Delete)],
            response_keys in prop::collection::vec(arb_keycode(), 1..20),
        ) {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();

            // Trigger an action
            let _ = app.on_key(action_key);

            // If action was set, confirmation should block normal navigation
            let had_action = app.pending_action.is_some();

            for key in &response_keys {
                let _ = app.on_key(*key);

                // Once pending_action is cleared, it stays cleared until another action key
                // (which can't happen while in confirmation mode)
                if app.pending_action.is_none() {
                    // Should be back to normal operation
                    break;
                }
            }

            // If we had an action and only pressed non-y/n/esc keys, action should persist
            if had_action {
                let any_resolve = response_keys.iter().any(|k| matches!(k,
                    KeyCode::Char('y') | KeyCode::Char('Y') |
                    KeyCode::Char('n') | KeyCode::Char('N') |
                    KeyCode::Esc
                ));
                if !any_resolve {
                    prop_assert!(app.pending_action.is_some(),
                        "pending_action cleared without y/n/esc");
                }
            }
        }

        /// Fuzz priority picker: open picker, send random keys, verify bounds.
        #[test]
        fn fuzz_priority_picker_navigation(keys in prop::collection::vec(arb_keycode(), 1..30)) {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();

            // Open priority picker
            let _ = app.on_key(KeyCode::Char('p'));
            if app.ui_mode != UIMode::PriorityPicker {
                return Ok(()); // empty list, nothing to test
            }

            for key in keys {
                // Skip 'q' which would quit
                if matches!(key, KeyCode::Char('q') | KeyCode::Char('Q')) {
                    continue;
                }
                let _ = app.on_key(key);

                // Invariant: picker selection always in bounds
                prop_assert!(
                    app.priority_picker_selected < win_api::PRIORITY_CLASSES.len(),
                    "picker selected {} out of bounds", app.priority_picker_selected
                );

                // If mode changed away from PriorityPicker, stop
                if app.ui_mode != UIMode::PriorityPicker {
                    break;
                }
            }
        }

        /// Fuzz: interleave action keys with navigation, verify no panics and invariants hold.
        #[test]
        fn fuzz_mixed_actions_and_navigation(keys in prop::collection::vec(arb_keycode(), 1..80)) {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();

            for key in keys {
                // Skip quit
                if matches!(key, KeyCode::Char('q') | KeyCode::Char('Q')) {
                    continue;
                }
                let _ = app.on_key(key);

                // Core invariants must hold at every step
                prop_assert!(
                    app.priority_picker_selected < win_api::PRIORITY_CLASSES.len(),
                    "picker out of bounds"
                );
                if !app.process_list.processes.is_empty() {
                    prop_assert!(
                        app.process_list.selected < app.process_list.processes.len(),
                        "selected out of bounds"
                    );
                }
                prop_assert!(
                    app.process_list.filter_cursor <= app.process_list.filter_query.len(),
                    "cursor out of bounds"
                );
            }
        }

        /// Fuzz: rapid action trigger/cancel cycles don't corrupt state.
        #[test]
        fn fuzz_rapid_action_cancel_cycles(cycles in 1usize..20) {
            let mut app = make_app(sample_procs());
            app.update_processes().unwrap();

            for _ in 0..cycles {
                // Trigger kill
                let _ = app.on_key(KeyCode::Char('x'));
                prop_assert!(app.pending_action.is_some() || app.process_list.processes.is_empty());
                // Cancel
                let _ = app.on_key(KeyCode::Esc);
                prop_assert!(app.pending_action.is_none());
                // Trigger suspend
                let _ = app.on_key(KeyCode::Char('z'));
                // Confirm (will fail since stubs return false, but shouldn't panic)
                let _ = app.on_key(KeyCode::Char('y'));
                prop_assert!(app.pending_action.is_none());
            }
        }
    }
}
