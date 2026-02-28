/// Shared types across the application
use crate::exemption::{ExemptionList, VerificationService};
use ratatui::widgets::TableState;
use std::collections::HashMap;

// Re-export ExemptionPickerState as ExemptionEditorState for backward compatibility
pub use crate::exemption::ExemptionPickerState as ExemptionEditorState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Pid,
    Name,
    Cpu,
    Memory,
}

impl SortColumn {
    /// Display label for the column header.
    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Pid => "PID",
            SortColumn::Name => "NAME",
            SortColumn::Cpu => "CPU",
            SortColumn::Memory => "MEMORY",
        }
    }

    /// Default sort direction for this column (true = ascending).
    pub fn default_ascending(self) -> bool {
        match self {
            SortColumn::Pid | SortColumn::Name => true,
            SortColumn::Cpu | SortColumn::Memory => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UIMode {
    Processes,
    Settings,
    Log,
    PriorityPicker,
    ExemptionEditor,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    Kill { pid: u32, name: String },
    Suspend { pid: u32, name: String },
    Resume { pid: u32, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceMode {
    Etw,
    PollingFallback,
}

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
}

pub struct SettingsState {
    pub poll_interval_ms: u64,
    pub hide_self: bool,
    pub selected_option: usize,
    pub priority_guard_enabled: bool,
    pub priority_guard_cpu_threshold: f32,
    pub priority_guard_duration_secs: u64,
    pub priority_guard_per_core_threshold: f32,
    pub priority_guard_relative_multiplier: f32,
    pub priority_guard_ema_alpha: f32,
    pub priority_guard_grace_period_secs: u64,
    pub priority_guard_adaptive_sensitivity: f32,
    pub exemptions: ExemptionList,
    pub exemption_verifier: VerificationService,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsState {
    pub const MAX_OPTION: usize = 10;

    pub fn new() -> Self {
        Self {
            poll_interval_ms: 2000,
            hide_self: false,
            selected_option: 0,
            priority_guard_enabled: true,
            priority_guard_cpu_threshold: 65.0,
            priority_guard_duration_secs: 2,
            priority_guard_per_core_threshold: 95.0,
            priority_guard_relative_multiplier: 8.0,
            priority_guard_ema_alpha: 0.3,
            priority_guard_grace_period_secs: 5,
            priority_guard_adaptive_sensitivity: 0.4,
            exemptions: ExemptionList::new(),
            exemption_verifier: VerificationService::new(),
        }
    }
}

// ExemptionEditorState has been replaced by ExemptionPickerState (re-exported above)

/// Pre-formatted row cells for rendering (avoids per-frame allocations).
pub type FormattedRow = [String; 5]; // pid, name, cpu, memory, cpu_rel

pub struct ProcessListState {
    pub processes: Vec<ProcessInfo>,
    pub formatted_rows: Vec<FormattedRow>,
    pub selected: usize,
    /// PID of the currently selected process, used to preserve selection across refreshes.
    pub selected_pid: Option<u32>,
    pub sort_column: SortColumn,
    pub sort_ascending: bool,
    pub table_state: TableState,
    pub filter_query: String,
    pub filter_cursor: usize,
    pub filter_focused: bool,
    /// Exponential moving average of relative CPU per PID: (avg, sample_count).
    pub rel_cpu_avg: HashMap<u32, (f32, u32)>,
    /// Total process count before filtering (for "X/Y" display).
    pub total_process_count: usize,
}

impl Default for ProcessListState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessListState {
    pub fn new() -> Self {
        Self {
            processes: Vec::new(),
            formatted_rows: Vec::new(),
            selected: 0,
            selected_pid: None,
            sort_column: SortColumn::Cpu,
            sort_ascending: false,
            table_state: TableState::default().with_selected(0),
            filter_query: String::new(),
            filter_cursor: 0,
            filter_focused: false,
            rel_cpu_avg: HashMap::new(),
            total_process_count: 0,
        }
    }

    /// Rebuild formatted row cache from current process list.
    pub fn rebuild_formatted_rows(&mut self) {
        let min_cpu = self
            .processes
            .iter()
            .map(|p| p.cpu)
            .filter(|&c| c > 0.0)
            .fold(f32::MAX, f32::min);
        let min_cpu = if min_cpu == f32::MAX { 0.0 } else { min_cpu };

        self.formatted_rows
            .resize_with(self.processes.len(), Default::default);
        for (i, proc) in self.processes.iter().enumerate() {
            let row = &mut self.formatted_rows[i];
            row[0].clear();
            std::fmt::Write::write_fmt(&mut row[0], format_args!("{}", proc.pid)).unwrap();
            row[1].clear();
            row[1].push_str(&proc.name);
            row[2].clear();
            std::fmt::Write::write_fmt(&mut row[2], format_args!("{:.2}%", proc.cpu)).unwrap();
            row[3].clear();
            std::fmt::Write::write_fmt(
                &mut row[3],
                format_args!("{:.1} MB", proc.memory as f64 / 1024.0 / 1024.0),
            )
            .unwrap();
            row[4].clear();
            if min_cpu > 0.0 {
                let rel = proc.cpu / min_cpu;
                const ALPHA: f32 = 0.3;
                const MIN_SAMPLES: u32 = 5;
                let entry = self
                    .rel_cpu_avg
                    .entry(proc.pid)
                    .and_modify(|(a, n)| {
                        *a = ALPHA * rel + (1.0 - ALPHA) * *a;
                        *n += 1;
                    })
                    .or_insert((rel, 1));
                if entry.1 >= MIN_SAMPLES {
                    std::fmt::Write::write_fmt(
                        &mut row[4],
                        format_args!("{:.1}x ({:.1}x)", rel, entry.0),
                    )
                    .unwrap();
                } else {
                    std::fmt::Write::write_fmt(&mut row[4], format_args!("{:.1}x", rel)).unwrap();
                }
            } else {
                row[4].push('-');
            }
        }
        self.formatted_rows.truncate(self.processes.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SortColumn::default_ascending ---

    #[test]
    fn pid_default_ascending() {
        assert!(SortColumn::Pid.default_ascending());
    }

    #[test]
    fn name_default_ascending() {
        assert!(SortColumn::Name.default_ascending());
    }

    #[test]
    fn cpu_default_descending() {
        assert!(!SortColumn::Cpu.default_ascending());
    }

    #[test]
    fn memory_default_descending() {
        assert!(!SortColumn::Memory.default_ascending());
    }

    // --- SettingsState ---

    #[test]
    fn settings_state_defaults() {
        let s = SettingsState::new();
        assert_eq!(s.poll_interval_ms, 2000);
        assert!(!s.hide_self);
        assert_eq!(s.selected_option, 0);
        assert!(s.priority_guard_enabled);
        assert_eq!(s.priority_guard_cpu_threshold, 65.0);
        assert_eq!(s.priority_guard_duration_secs, 2);
        assert_eq!(s.priority_guard_per_core_threshold, 95.0);
        assert_eq!(s.priority_guard_relative_multiplier, 8.0);
        assert_eq!(s.priority_guard_ema_alpha, 0.3);
        assert_eq!(s.priority_guard_grace_period_secs, 5);
        assert_eq!(s.priority_guard_adaptive_sensitivity, 0.4);
    }

    // --- ProcessListState ---

    #[test]
    fn process_list_state_defaults() {
        let s = ProcessListState::new();
        assert!(s.processes.is_empty());
        assert!(s.formatted_rows.is_empty());
        assert_eq!(s.selected, 0);
        assert_eq!(s.selected_pid, None);
        assert_eq!(s.sort_column, SortColumn::Cpu);
        assert!(!s.sort_ascending);
        assert!(s.filter_query.is_empty());
        assert_eq!(s.filter_cursor, 0);
        assert!(!s.filter_focused);
        assert!(s.rel_cpu_avg.is_empty());
    }

    // --- rebuild_formatted_rows ---

    fn make_procs(specs: &[(u32, &str, f32, u64)]) -> Vec<ProcessInfo> {
        specs
            .iter()
            .map(|(pid, name, cpu, mem)| ProcessInfo {
                pid: *pid,
                name: name.to_string(),
                cpu: *cpu,
                memory: *mem,
            })
            .collect()
    }

    #[test]
    fn rebuild_empty_processes() {
        let mut state = ProcessListState::new();
        state.rebuild_formatted_rows();
        assert!(state.formatted_rows.is_empty());
    }

    #[test]
    fn rebuild_single_process_formats_correctly() {
        let mut state = ProcessListState::new();
        state.processes = make_procs(&[(42, "test", 5.5, 1048576)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows.len(), 1);
        assert_eq!(state.formatted_rows[0][0], "42");
        assert_eq!(state.formatted_rows[0][1], "test");
        assert_eq!(state.formatted_rows[0][2], "5.50%");
        assert_eq!(state.formatted_rows[0][3], "1.0 MB");
    }

    #[test]
    fn rebuild_all_zero_cpu_shows_dash() {
        let mut state = ProcessListState::new();
        state.processes = make_procs(&[(1, "a", 0.0, 0), (2, "b", 0.0, 0)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows[0][4], "-");
        assert_eq!(state.formatted_rows[1][4], "-");
    }

    #[test]
    fn rebuild_relative_cpu_computed() {
        let mut state = ProcessListState::new();
        // min_cpu = 2.0, so pid=1 gets 5.0x, pid=2 gets 1.0x
        state.processes = make_procs(&[(1, "a", 10.0, 0), (2, "b", 2.0, 0)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows[0][4], "5.0x");
        assert_eq!(state.formatted_rows[1][4], "1.0x");
    }

    #[test]
    fn rebuild_ema_shows_after_min_samples() {
        let mut state = ProcessListState::new();
        state.processes = make_procs(&[(1, "a", 10.0, 0), (2, "b", 2.0, 0)]);
        // Run 5 times to reach MIN_SAMPLES
        for _ in 0..5 {
            state.rebuild_formatted_rows();
        }
        // After 5 calls, should show "X.Xx (Y.Yx)" format with parentheses
        assert!(
            state.formatted_rows[0][4].contains('('),
            "expected EMA after 5 samples: {}",
            state.formatted_rows[0][4]
        );
    }

    #[test]
    fn rebuild_ema_hidden_before_min_samples() {
        let mut state = ProcessListState::new();
        state.processes = make_procs(&[(1, "a", 10.0, 0), (2, "b", 2.0, 0)]);
        // Run 4 times (below MIN_SAMPLES=5)
        for _ in 0..4 {
            state.rebuild_formatted_rows();
        }
        assert!(
            !state.formatted_rows[0][4].contains('('),
            "no EMA before 5 samples: {}",
            state.formatted_rows[0][4]
        );
    }

    #[test]
    fn rebuild_truncates_stale_rows() {
        let mut state = ProcessListState::new();
        state.processes = make_procs(&[(1, "a", 1.0, 0), (2, "b", 2.0, 0), (3, "c", 3.0, 0)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows.len(), 3);
        // Shrink to 1 process
        state.processes = make_procs(&[(1, "a", 1.0, 0)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows.len(), 1);
    }

    #[test]
    fn rebuild_reuses_buffers() {
        let mut state = ProcessListState::new();
        state.processes = make_procs(&[(1, "a", 1.0, 0)]);
        state.rebuild_formatted_rows();
        let ptr = state.formatted_rows[0][0].as_ptr();
        // Second call should reuse the same String allocation
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows[0][0].as_ptr(), ptr);
    }

    #[test]
    fn rebuild_memory_formatting() {
        let mut state = ProcessListState::new();
        // 0 bytes
        state.processes = make_procs(&[(1, "a", 0.0, 0)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows[0][3], "0.0 MB");
        // 1.5 GB = 1610612736 bytes
        state.processes = make_procs(&[(1, "a", 0.0, 1_610_612_736)]);
        state.rebuild_formatted_rows();
        assert_eq!(state.formatted_rows[0][3], "1536.0 MB");
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_process_info() -> impl Strategy<Value = ProcessInfo> {
        (any::<u32>(), "\\PC{1,20}", 0.0f32..10000.0f32, any::<u64>()).prop_map(
            |(pid, name, cpu, memory)| ProcessInfo {
                pid,
                name,
                cpu,
                memory,
            },
        )
    }

    proptest! {
        #[test]
        fn fuzz_rebuild_formatted_rows_length(procs in prop::collection::vec(arb_process_info(), 0..50)) {
            let mut state = ProcessListState::new();
            state.processes = procs;
            state.rebuild_formatted_rows();
            prop_assert_eq!(state.formatted_rows.len(), state.processes.len());
        }

        #[test]
        fn fuzz_rebuild_repeated_calls(procs in prop::collection::vec(arb_process_info(), 1..20), repeats in 1usize..10) {
            let mut state = ProcessListState::new();
            state.processes = procs;
            for _ in 0..repeats {
                state.rebuild_formatted_rows();
            }
            prop_assert_eq!(state.formatted_rows.len(), state.processes.len());
            // Every row should have non-empty pid and name
            for (i, row) in state.formatted_rows.iter().enumerate() {
                prop_assert!(!row[0].is_empty(), "pid cell empty at row {}", i);
                prop_assert!(!row[1].is_empty(), "name cell empty at row {}", i);
                prop_assert!(!row[2].is_empty(), "cpu cell empty at row {}", i);
                prop_assert!(!row[3].is_empty(), "memory cell empty at row {}", i);
                prop_assert!(!row[4].is_empty(), "rel_cpu cell empty at row {}", i);
            }
        }

        #[test]
        fn fuzz_rebuild_extreme_values(
            cpu in prop_oneof![Just(0.0f32), Just(f32::MIN_POSITIVE), Just(f32::MAX / 2.0), Just(0.001)],
            memory in prop_oneof![Just(0u64), Just(u64::MAX), Just(1024 * 1024)],
        ) {
            let mut state = ProcessListState::new();
            state.processes = vec![
                ProcessInfo { pid: 1, name: "a".into(), cpu, memory },
                ProcessInfo { pid: 2, name: "b".into(), cpu: cpu * 0.5, memory },
            ];
            state.rebuild_formatted_rows();
            prop_assert_eq!(state.formatted_rows.len(), 2);
        }
    }
}
