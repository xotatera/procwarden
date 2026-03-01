//! Tabbed exemption picker state management.

use crate::common::ProcessInfo;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerTab {
    RunningProcesses,
    Browse,
    ManualEntry,
}

impl PickerTab {
    pub fn next(self) -> Self {
        match self {
            PickerTab::RunningProcesses => PickerTab::Browse,
            PickerTab::Browse => PickerTab::ManualEntry,
            PickerTab::ManualEntry => PickerTab::RunningProcesses,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            PickerTab::RunningProcesses => "Running Processes",
            PickerTab::Browse => "Browse",
            PickerTab::ManualEntry => "Manual Entry",
        }
    }
}

/// A process candidate for exemption.
#[derive(Debug, Clone)]
pub struct ProcessCandidate {
    pub pid: u32,
    pub name: String,
    pub path: PathBuf,
}

impl ProcessCandidate {
    /// Filter processes to those with valid file paths.
    /// Uses sysinfo to get exe path for each process.
    pub fn from_process_info(procs: &[ProcessInfo]) -> Vec<Self> {
        use sysinfo::{Pid, System};

        let mut sys = System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All);

        procs
            .iter()
            .filter_map(|p| {
                let pid = Pid::from_u32(p.pid);
                sys.process(pid)
                    .and_then(|proc| proc.exe().map(|path| path.to_path_buf()))
                    .filter(|path| path.exists()) // Only valid, existing paths
                    .map(|path| ProcessCandidate {
                        pid: p.pid,
                        name: p.name.clone(),
                        path,
                    })
            })
            .collect()
    }
}

/// Tabbed exemption picker state.
#[derive(Debug, Clone)]
pub struct ExemptionPickerState {
    pub active_tab: PickerTab,
    pub running_selected: usize,
    pub running_candidates: Vec<ProcessCandidate>,
    pub browse_path: PathBuf,
    pub manual_input: String,
    pub manual_cursor: usize,
}

impl ExemptionPickerState {
    pub fn new() -> Self {
        Self {
            active_tab: PickerTab::RunningProcesses,
            running_selected: 0,
            running_candidates: vec![],
            browse_path: std::env::current_dir().unwrap_or_else(|_| {
                #[cfg(windows)]
                {
                    PathBuf::from("C:\\")
                }
                #[cfg(not(windows))]
                {
                    PathBuf::from("/")
                }
            }),
            manual_input: String::new(),
            manual_cursor: 0,
        }
    }

    pub fn refresh_candidates(&mut self, processes: &[ProcessInfo]) {
        self.running_candidates = ProcessCandidate::from_process_info(processes);
        self.running_selected = self
            .running_selected
            .min(self.running_candidates.len().saturating_sub(1));
    }

    pub fn selected_candidate(&self) -> Option<&ProcessCandidate> {
        if self.active_tab == PickerTab::RunningProcesses {
            self.running_candidates.get(self.running_selected)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.active_tab = PickerTab::RunningProcesses;
        self.running_selected = 0;
        self.manual_input.clear();
        self.manual_cursor = 0;
    }

    pub fn navigate_up(&mut self) {
        if self.active_tab == PickerTab::RunningProcesses {
            self.running_selected = self.running_selected.saturating_sub(1);
        }
    }

    pub fn navigate_down(&mut self) {
        if self.active_tab == PickerTab::RunningProcesses {
            let max = self.running_candidates.len().saturating_sub(1);
            if self.running_selected < max {
                self.running_selected += 1;
            }
        }
    }

    pub fn switch_tab(&mut self) {
        self.active_tab = self.active_tab.next();
    }
}

impl Default for ExemptionPickerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_tab_next_cycles() {
        assert_eq!(PickerTab::RunningProcesses.next(), PickerTab::Browse);
        assert_eq!(PickerTab::Browse.next(), PickerTab::ManualEntry);
        assert_eq!(PickerTab::ManualEntry.next(), PickerTab::RunningProcesses);
    }

    #[test]
    fn picker_tab_labels() {
        assert_eq!(PickerTab::RunningProcesses.label(), "Running Processes");
        assert_eq!(PickerTab::Browse.label(), "Browse");
        assert_eq!(PickerTab::ManualEntry.label(), "Manual Entry");
    }

    #[test]
    fn exemption_picker_state_new() {
        let state = ExemptionPickerState::new();
        assert_eq!(state.active_tab, PickerTab::RunningProcesses);
        assert_eq!(state.running_selected, 0);
        assert!(state.running_candidates.is_empty());
        assert!(state.manual_input.is_empty());
        assert_eq!(state.manual_cursor, 0);
    }

    #[test]
    fn process_candidate_from_process_info_filters() {
        // Test with real process info - should get at least current process
        let procs = vec![ProcessInfo {
            pid: std::process::id(),
            name: "test".to_string(),
            cpu: 0.0,
            memory: 0,
            exe_path: None,
        }];

        let candidates = ProcessCandidate::from_process_info(&procs);
        // Current process should have a valid path
        assert!(!candidates.is_empty());
    }

    #[test]
    fn refresh_candidates_updates_list() {
        let mut state = ExemptionPickerState::new();
        let procs = vec![ProcessInfo {
            pid: std::process::id(),
            name: "test".to_string(),
            cpu: 0.0,
            memory: 0,
            exe_path: None,
        }];

        state.refresh_candidates(&procs);
        assert!(!state.running_candidates.is_empty());
    }

    #[test]
    fn refresh_candidates_clamps_selection() {
        let mut state = ExemptionPickerState::new();
        state.running_selected = 999;

        let procs = vec![ProcessInfo {
            pid: std::process::id(),
            name: "test".to_string(),
            cpu: 0.0,
            memory: 0,
            exe_path: None,
        }];

        state.refresh_candidates(&procs);
        assert!(state.running_selected < state.running_candidates.len());
    }

    #[test]
    fn selected_candidate_returns_some_on_running_tab() {
        let mut state = ExemptionPickerState::new();
        state.running_candidates = vec![ProcessCandidate {
            pid: 123,
            name: "test".to_string(),
            path: PathBuf::from("C:\\test.exe"),
        }];

        assert!(state.selected_candidate().is_some());
    }

    #[test]
    fn selected_candidate_returns_none_on_other_tabs() {
        let mut state = ExemptionPickerState::new();
        state.active_tab = PickerTab::Browse;
        state.running_candidates = vec![ProcessCandidate {
            pid: 123,
            name: "test".to_string(),
            path: PathBuf::from("C:\\test.exe"),
        }];

        assert!(state.selected_candidate().is_none());
    }

    #[test]
    fn reset_clears_state() {
        let mut state = ExemptionPickerState::new();
        state.active_tab = PickerTab::Browse;
        state.running_selected = 5;
        state.manual_input = "test".to_string();
        state.manual_cursor = 4;

        state.reset();

        assert_eq!(state.active_tab, PickerTab::RunningProcesses);
        assert_eq!(state.running_selected, 0);
        assert!(state.manual_input.is_empty());
        assert_eq!(state.manual_cursor, 0);
    }

    #[test]
    fn navigate_up_decrements_selection() {
        let mut state = ExemptionPickerState::new();
        state.running_selected = 5;

        state.navigate_up();
        assert_eq!(state.running_selected, 4);

        state.running_selected = 0;
        state.navigate_up();
        assert_eq!(state.running_selected, 0); // Saturating
    }

    #[test]
    fn navigate_down_increments_selection() {
        let mut state = ExemptionPickerState::new();
        state.running_candidates = vec![
            ProcessCandidate {
                pid: 1,
                name: "a".to_string(),
                path: PathBuf::from("a"),
            },
            ProcessCandidate {
                pid: 2,
                name: "b".to_string(),
                path: PathBuf::from("b"),
            },
        ];

        state.navigate_down();
        assert_eq!(state.running_selected, 1);

        state.navigate_down();
        assert_eq!(state.running_selected, 1); // Clamped at max
    }

    #[test]
    fn switch_tab_cycles() {
        let mut state = ExemptionPickerState::new();
        assert_eq!(state.active_tab, PickerTab::RunningProcesses);

        state.switch_tab();
        assert_eq!(state.active_tab, PickerTab::Browse);

        state.switch_tab();
        assert_eq!(state.active_tab, PickerTab::ManualEntry);

        state.switch_tab();
        assert_eq!(state.active_tab, PickerTab::RunningProcesses);
    }
}
