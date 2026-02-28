use crossterm::event::KeyCode;
use crate::common::{SortColumn, ProcessListState};

/// Handle process list navigation and sorting keys
pub fn handle_key(state: &mut ProcessListState, key: KeyCode) {
    match key {
        KeyCode::Up | KeyCode::Char('k') => {
            if state.selected > 0 {
                state.selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.selected < state.processes.len().saturating_sub(1) {
                state.selected += 1;
            }
        }
        KeyCode::PageUp => {
            state.selected = state.selected.saturating_sub(20);
        }
        KeyCode::PageDown => {
            let max = state.processes.len().saturating_sub(1);
            state.selected = (state.selected + 20).min(max);
        }
        KeyCode::Home => {
            state.selected = 0;
        }
        KeyCode::End => {
            state.selected = state.processes.len().saturating_sub(1);
        }
        KeyCode::Left => {
            let new_col = match state.sort_column {
                SortColumn::Pid => SortColumn::Memory,
                SortColumn::Name => SortColumn::Pid,
                SortColumn::Cpu => SortColumn::Name,
                SortColumn::Memory => SortColumn::Cpu,
            };
            state.sort_ascending = new_col.default_ascending();
            state.sort_column = new_col;
        }
        KeyCode::Right => {
            let new_col = match state.sort_column {
                SortColumn::Pid => SortColumn::Name,
                SortColumn::Name => SortColumn::Cpu,
                SortColumn::Cpu => SortColumn::Memory,
                SortColumn::Memory => SortColumn::Pid,
            };
            state.sort_ascending = new_col.default_ascending();
            state.sort_column = new_col;
        }
        KeyCode::Char(' ') => {
            state.sort_ascending = !state.sort_ascending;
        }
        _ => {}
    }

    // Track selected PID so selection survives re-sorting
    state.selected_pid = state.processes.get(state.selected).map(|p| p.pid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ProcessInfo;

    fn state_with_procs(n: usize) -> ProcessListState {
        let mut state = ProcessListState::new();
        state.processes = (0..n)
            .map(|i| ProcessInfo {
                pid: i as u32,
                name: format!("proc-{}", i),
                cpu: 0.0,
                memory: 0,
            })
            .collect();
        state
    }

    #[test]
    fn up_decrements_selection() {
        let mut state = state_with_procs(5);
        state.selected = 3;
        handle_key(&mut state, KeyCode::Up);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn up_at_zero_stays() {
        let mut state = state_with_procs(5);
        state.selected = 0;
        handle_key(&mut state, KeyCode::Up);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn down_increments_selection() {
        let mut state = state_with_procs(5);
        state.selected = 2;
        handle_key(&mut state, KeyCode::Down);
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn down_at_end_stays() {
        let mut state = state_with_procs(5);
        state.selected = 4;
        handle_key(&mut state, KeyCode::Down);
        assert_eq!(state.selected, 4);
    }

    #[test]
    fn right_cycles_sort_column() {
        let mut state = state_with_procs(1);
        state.sort_column = SortColumn::Pid;
        handle_key(&mut state, KeyCode::Right);
        assert_eq!(state.sort_column, SortColumn::Name);
        handle_key(&mut state, KeyCode::Right);
        assert_eq!(state.sort_column, SortColumn::Cpu);
        handle_key(&mut state, KeyCode::Right);
        assert_eq!(state.sort_column, SortColumn::Memory);
        handle_key(&mut state, KeyCode::Right);
        assert_eq!(state.sort_column, SortColumn::Pid);
    }

    #[test]
    fn left_cycles_sort_column_reverse() {
        let mut state = state_with_procs(1);
        state.sort_column = SortColumn::Pid;
        handle_key(&mut state, KeyCode::Left);
        assert_eq!(state.sort_column, SortColumn::Memory);
        handle_key(&mut state, KeyCode::Left);
        assert_eq!(state.sort_column, SortColumn::Cpu);
    }

    #[test]
    fn space_toggles_sort_direction() {
        let mut state = state_with_procs(1);
        let original = state.sort_ascending;
        handle_key(&mut state, KeyCode::Char(' '));
        assert_eq!(state.sort_ascending, !original);
        handle_key(&mut state, KeyCode::Char(' '));
        assert_eq!(state.sort_ascending, original);
    }

    #[test]
    fn column_switch_resets_to_default_direction() {
        let mut state = state_with_procs(1);
        // Start at PID ascending, switch to CPU — should be descending
        state.sort_column = SortColumn::Pid;
        state.sort_ascending = true;
        handle_key(&mut state, KeyCode::Right); // → Name
        assert!(state.sort_ascending); // Name defaults ascending
        handle_key(&mut state, KeyCode::Right); // → CPU
        assert!(!state.sort_ascending); // CPU defaults descending
        handle_key(&mut state, KeyCode::Right); // → Memory
        assert!(!state.sort_ascending); // Memory defaults descending
        handle_key(&mut state, KeyCode::Right); // → PID
        assert!(state.sort_ascending); // PID defaults ascending
    }

    #[test]
    fn unknown_key_is_noop() {
        let mut state = state_with_procs(3);
        state.selected = 1;
        state.sort_column = SortColumn::Cpu;
        handle_key(&mut state, KeyCode::Char('x'));
        assert_eq!(state.selected, 1);
        assert_eq!(state.sort_column, SortColumn::Cpu);
    }

    // --- vim-style j/k navigation ---

    #[test]
    fn j_moves_down() {
        let mut state = state_with_procs(5);
        state.selected = 1;
        handle_key(&mut state, KeyCode::Char('j'));
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn k_moves_up() {
        let mut state = state_with_procs(5);
        state.selected = 3;
        handle_key(&mut state, KeyCode::Char('k'));
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn k_at_zero_stays() {
        let mut state = state_with_procs(5);
        state.selected = 0;
        handle_key(&mut state, KeyCode::Char('k'));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn j_at_end_stays() {
        let mut state = state_with_procs(5);
        state.selected = 4;
        handle_key(&mut state, KeyCode::Char('j'));
        assert_eq!(state.selected, 4);
    }

    // --- PageUp/PageDown ---

    #[test]
    fn page_down_jumps_20() {
        let mut state = state_with_procs(50);
        state.selected = 5;
        handle_key(&mut state, KeyCode::PageDown);
        assert_eq!(state.selected, 25);
    }

    #[test]
    fn page_down_clamps_at_end() {
        let mut state = state_with_procs(10);
        state.selected = 5;
        handle_key(&mut state, KeyCode::PageDown);
        assert_eq!(state.selected, 9);
    }

    #[test]
    fn page_up_jumps_20() {
        let mut state = state_with_procs(50);
        state.selected = 30;
        handle_key(&mut state, KeyCode::PageUp);
        assert_eq!(state.selected, 10);
    }

    #[test]
    fn page_up_clamps_at_zero() {
        let mut state = state_with_procs(50);
        state.selected = 5;
        handle_key(&mut state, KeyCode::PageUp);
        assert_eq!(state.selected, 0);
    }

    // --- Home/End ---

    #[test]
    fn home_jumps_to_start() {
        let mut state = state_with_procs(50);
        state.selected = 25;
        handle_key(&mut state, KeyCode::Home);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn end_jumps_to_last() {
        let mut state = state_with_procs(50);
        state.selected = 5;
        handle_key(&mut state, KeyCode::End);
        assert_eq!(state.selected, 49);
    }

    #[test]
    fn home_end_on_empty_list() {
        let mut state = state_with_procs(0);
        handle_key(&mut state, KeyCode::Home);
        assert_eq!(state.selected, 0);
        handle_key(&mut state, KeyCode::End);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn page_down_on_empty_list() {
        let mut state = state_with_procs(0);
        handle_key(&mut state, KeyCode::PageDown);
        assert_eq!(state.selected, 0);
    }
}
