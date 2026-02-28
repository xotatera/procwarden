use crate::common::{ProcessInfo, SortColumn};

/// Pure function: Sort processes by given column
pub fn sort_processes(
    mut procs: Vec<ProcessInfo>,
    column: SortColumn,
    ascending: bool,
) -> Vec<ProcessInfo> {
    match column {
        SortColumn::Pid => {
            if ascending {
                procs.sort_by_key(|p| p.pid);
            } else {
                procs.sort_by(|a, b| b.pid.cmp(&a.pid));
            }
        }
        SortColumn::Name => {
            if ascending {
                procs.sort_by(|a, b| {
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase())
                });
            } else {
                procs.sort_by(|a, b| {
                    b.name
                        .to_ascii_lowercase()
                        .cmp(&a.name.to_ascii_lowercase())
                });
            }
        }
        SortColumn::Cpu => {
            if ascending {
                procs.sort_by(|a, b| {
                    a.cpu
                        .partial_cmp(&b.cpu)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            } else {
                procs.sort_by(|a, b| {
                    b.cpu
                        .partial_cmp(&a.cpu)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }
        SortColumn::Memory => {
            if ascending {
                procs.sort_by_key(|p| p.memory);
            } else {
                procs.sort_by(|a, b| b.memory.cmp(&a.memory));
            }
        }
    }
    procs
}

/// Pure function: Filter processes by hide_self setting and text query.
/// Query supports comma-separated terms with OR logic (case-insensitive).
pub fn filter_processes(
    mut procs: Vec<ProcessInfo>,
    hide_self: bool,
    query: &str,
) -> Vec<ProcessInfo> {
    if hide_self {
        let self_pid = std::process::id();
        procs.retain(|p| p.pid != self_pid);
    }

    let terms: Vec<String> = query
        .split(',')
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    if !terms.is_empty() {
        procs.retain(|p| {
            let name = p.name.to_ascii_lowercase();
            terms.iter().any(|t| name.contains(t.as_str()))
        });
    }

    procs
}

/// Pure function: Get header string for a sort column with indicator
pub fn get_column_header(
    column: SortColumn,
    current_column: SortColumn,
    ascending: bool,
) -> &'static str {
    if column != current_column {
        return column.label();
    }
    match (column, ascending) {
        (SortColumn::Pid, true) => "PID ↑",
        (SortColumn::Pid, false) => "PID ↓",
        (SortColumn::Name, true) => "NAME ↑",
        (SortColumn::Name, false) => "NAME ↓",
        (SortColumn::Cpu, true) => "CPU ↑",
        (SortColumn::Cpu, false) => "CPU ↓",
        (SortColumn::Memory, true) => "MEMORY ↑",
        (SortColumn::Memory, false) => "MEMORY ↓",
    }
}

/// Pure function: Format a process's memory usage as string
pub fn format_memory(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
}

/// Pure function: Clamp selected index to valid range
pub fn clamp_selection(selected: usize, list_len: usize) -> usize {
    if selected >= list_len {
        list_len.saturating_sub(1)
    } else {
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ProcessInfo;

    fn make_procs() -> Vec<ProcessInfo> {
        vec![
            ProcessInfo {
                pid: 10,
                name: "alpha".into(),
                cpu: 5.0,
                memory: 300,
            },
            ProcessInfo {
                pid: 1,
                name: "charlie".into(),
                cpu: 90.0,
                memory: 100,
            },
            ProcessInfo {
                pid: 5,
                name: "bravo".into(),
                cpu: 0.5,
                memory: 200,
            },
        ]
    }

    // -- sort_processes --

    #[test]
    fn sort_by_pid_ascending() {
        let result = sort_processes(make_procs(), SortColumn::Pid, true);
        let pids: Vec<u32> = result.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![1, 5, 10]);
    }

    #[test]
    fn sort_by_pid_descending() {
        let result = sort_processes(make_procs(), SortColumn::Pid, false);
        let pids: Vec<u32> = result.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![10, 5, 1]);
    }

    #[test]
    fn sort_by_name_ascending() {
        let result = sort_processes(make_procs(), SortColumn::Name, true);
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn sort_by_name_descending() {
        let result = sort_processes(make_procs(), SortColumn::Name, false);
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["charlie", "bravo", "alpha"]);
    }

    #[test]
    fn sort_by_cpu_ascending() {
        let result = sort_processes(make_procs(), SortColumn::Cpu, true);
        let cpus: Vec<f32> = result.iter().map(|p| p.cpu).collect();
        assert_eq!(cpus, vec![0.5, 5.0, 90.0]);
    }

    #[test]
    fn sort_by_cpu_descending() {
        let result = sort_processes(make_procs(), SortColumn::Cpu, false);
        let cpus: Vec<f32> = result.iter().map(|p| p.cpu).collect();
        assert_eq!(cpus, vec![90.0, 5.0, 0.5]);
    }

    #[test]
    fn sort_by_memory_ascending() {
        let result = sort_processes(make_procs(), SortColumn::Memory, true);
        let mems: Vec<u64> = result.iter().map(|p| p.memory).collect();
        assert_eq!(mems, vec![100, 200, 300]);
    }

    #[test]
    fn sort_by_memory_descending() {
        let result = sort_processes(make_procs(), SortColumn::Memory, false);
        let mems: Vec<u64> = result.iter().map(|p| p.memory).collect();
        assert_eq!(mems, vec![300, 200, 100]);
    }

    #[test]
    fn sort_empty_list() {
        let result = sort_processes(vec![], SortColumn::Pid, true);
        assert!(result.is_empty());
    }

    #[test]
    fn sort_by_name_case_insensitive() {
        let procs = vec![
            ProcessInfo {
                pid: 1,
                name: "Zebra.exe".into(),
                cpu: 0.0,
                memory: 0,
            },
            ProcessInfo {
                pid: 2,
                name: "alpha.exe".into(),
                cpu: 0.0,
                memory: 0,
            },
            ProcessInfo {
                pid: 3,
                name: "Beta.exe".into(),
                cpu: 0.0,
                memory: 0,
            },
        ];
        let result = sort_processes(procs, SortColumn::Name, true);
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.exe", "Beta.exe", "Zebra.exe"]);
    }

    // -- filter_processes --

    #[test]
    fn filter_hide_self_false_returns_all() {
        let procs = make_procs();
        let len = procs.len();
        let result = filter_processes(procs, false, "");
        assert_eq!(result.len(), len);
    }

    #[test]
    fn filter_hide_self_true_removes_self_pid() {
        let self_pid = std::process::id();
        let mut procs = make_procs();
        procs.push(ProcessInfo {
            pid: self_pid,
            name: "self".into(),
            cpu: 0.0,
            memory: 0,
        });
        let result = filter_processes(procs, true, "");
        assert!(result.iter().all(|p| p.pid != self_pid));
    }

    #[test]
    fn filter_by_query_case_insensitive() {
        let procs = make_procs();
        let result = filter_processes(procs, false, "ALPHA");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "alpha");
    }

    #[test]
    fn filter_by_query_empty_returns_all() {
        let procs = make_procs();
        let len = procs.len();
        let result = filter_processes(procs, false, "");
        assert_eq!(result.len(), len);
    }

    #[test]
    fn filter_comma_separated_or_logic() {
        let procs = make_procs();
        let result = filter_processes(procs, false, "alpha, charlie");
        assert_eq!(result.len(), 2);
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"charlie"));
    }

    #[test]
    fn filter_comma_with_no_match() {
        let procs = make_procs();
        let result = filter_processes(procs, false, "nonexistent");
        assert!(result.is_empty());
    }

    #[test]
    fn filter_trailing_comma_ignored() {
        let procs = make_procs();
        let result = filter_processes(procs, false, "alpha,");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "alpha");
    }

    // -- get_column_header --

    #[test]
    fn header_active_ascending() {
        assert_eq!(
            get_column_header(SortColumn::Cpu, SortColumn::Cpu, true),
            "CPU ↑"
        );
    }

    #[test]
    fn header_active_descending() {
        assert_eq!(
            get_column_header(SortColumn::Cpu, SortColumn::Cpu, false),
            "CPU ↓"
        );
    }

    #[test]
    fn header_inactive() {
        assert_eq!(
            get_column_header(SortColumn::Name, SortColumn::Cpu, true),
            "NAME"
        );
    }

    #[test]
    fn header_all_columns_inactive() {
        for col in [
            SortColumn::Pid,
            SortColumn::Name,
            SortColumn::Cpu,
            SortColumn::Memory,
        ] {
            let other = if col == SortColumn::Pid {
                SortColumn::Name
            } else {
                SortColumn::Pid
            };
            let h = get_column_header(col, other, true);
            assert!(!h.contains('↑') && !h.contains('↓'));
        }
    }

    // -- format_memory --

    #[test]
    fn format_memory_zero() {
        assert_eq!(format_memory(0), "0.0 MB");
    }

    #[test]
    fn format_memory_one_mb() {
        assert_eq!(format_memory(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn format_memory_fractional() {
        // 1.5 MB = 1572864 bytes
        assert_eq!(format_memory(1572864), "1.5 MB");
    }

    // -- clamp_selection --

    #[test]
    fn clamp_within_range() {
        assert_eq!(clamp_selection(2, 5), 2);
    }

    #[test]
    fn clamp_at_boundary() {
        assert_eq!(clamp_selection(4, 5), 4);
    }

    #[test]
    fn clamp_over_boundary() {
        assert_eq!(clamp_selection(10, 5), 4);
    }

    #[test]
    fn clamp_empty_list() {
        assert_eq!(clamp_selection(0, 0), 0);
    }

    #[test]
    fn clamp_empty_list_with_index() {
        assert_eq!(clamp_selection(5, 0), 0);
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use crate::common::{ProcessInfo, SortColumn};
    use proptest::prelude::*;

    fn arb_sort_column() -> impl Strategy<Value = SortColumn> {
        prop_oneof![
            Just(SortColumn::Pid),
            Just(SortColumn::Name),
            Just(SortColumn::Cpu),
            Just(SortColumn::Memory),
        ]
    }

    fn arb_process_info() -> impl Strategy<Value = ProcessInfo> {
        (any::<u32>(), ".*", 0.0f32..1000.0f32, any::<u64>()).prop_map(
            |(pid, name, cpu, memory)| ProcessInfo {
                pid,
                name,
                cpu,
                memory,
            },
        )
    }

    fn arb_process_list() -> impl Strategy<Value = Vec<ProcessInfo>> {
        prop::collection::vec(arb_process_info(), 0..50)
    }

    proptest! {
        #[test]
        fn fuzz_sort_preserves_length(procs in arb_process_list(), col in arb_sort_column(), asc in any::<bool>()) {
            let len = procs.len();
            let result = sort_processes(procs, col, asc);
            prop_assert_eq!(result.len(), len);
        }

        #[test]
        fn fuzz_sort_is_sorted(procs in arb_process_list(), col in arb_sort_column(), asc in any::<bool>()) {
            let result = sort_processes(procs, col, asc);
            for w in result.windows(2) {
                let ordered = match (col, asc) {
                    (SortColumn::Pid, true) => w[0].pid <= w[1].pid,
                    (SortColumn::Pid, false) => w[0].pid >= w[1].pid,
                    (SortColumn::Name, true) => w[0].name.to_ascii_lowercase() <= w[1].name.to_ascii_lowercase(),
                    (SortColumn::Name, false) => w[0].name.to_ascii_lowercase() >= w[1].name.to_ascii_lowercase(),
                    (SortColumn::Cpu, true) => w[0].cpu <= w[1].cpu || w[0].cpu.is_nan() || w[1].cpu.is_nan(),
                    (SortColumn::Cpu, false) => w[0].cpu >= w[1].cpu || w[0].cpu.is_nan() || w[1].cpu.is_nan(),
                    (SortColumn::Memory, true) => w[0].memory <= w[1].memory,
                    (SortColumn::Memory, false) => w[0].memory >= w[1].memory,
                };
                prop_assert!(ordered, "not sorted: {:?} vs {:?} col={:?} asc={}", w[0], w[1], col, asc);
            }
        }

        #[test]
        fn fuzz_filter_subset(procs in arb_process_list(), query in ".*") {
            let original_len = procs.len();
            let result = filter_processes(procs, false, &query);
            prop_assert!(result.len() <= original_len);
        }

        #[test]
        fn fuzz_filter_empty_query_preserves(procs in arb_process_list()) {
            let len = procs.len();
            let result = filter_processes(procs, false, "");
            prop_assert_eq!(result.len(), len);
        }

        #[test]
        fn fuzz_filter_arbitrary_query_no_panic(query in "\\PC{0,100}") {
            let procs = vec![
                ProcessInfo { pid: 1, name: "chrome.exe".into(), cpu: 1.0, memory: 100 },
                ProcessInfo { pid: 2, name: "firefox.exe".into(), cpu: 2.0, memory: 200 },
            ];
            let _ = filter_processes(procs, false, &query);
        }

        #[test]
        fn fuzz_format_memory_no_panic(bytes in any::<u64>()) {
            let result = format_memory(bytes);
            prop_assert!(!result.is_empty());
            prop_assert!(result.contains("MB"));
        }

        #[test]
        fn fuzz_clamp_selection(selected in any::<usize>(), list_len in any::<usize>()) {
            let result = clamp_selection(selected, list_len);
            if list_len == 0 {
                prop_assert_eq!(result, 0);
            } else {
                prop_assert!(result < list_len);
            }
        }
    }
}
