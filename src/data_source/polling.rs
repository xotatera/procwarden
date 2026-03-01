use super::DataSource;
use crate::common::ProcessInfo;
use anyhow::Result;
use std::collections::HashMap;
use sysinfo::{ProcessRefreshKind, System};

/// Polling-based process data source using sysinfo
pub struct PollingDataSource {
    system: System,
    num_cpus: f32,
}

impl PollingDataSource {
    pub fn new() -> Self {
        let mut system = System::new();
        // Initial refresh populates CPU list so we can count logical processors
        system.refresh_cpu_all();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All);
        let num_cpus = (system.cpus().len() as f32).max(1.0);
        Self { system, num_cpus }
    }

    #[allow(dead_code)]
    fn get_self_pid() -> u32 {
        std::process::id()
    }

    /// Refresh only memory info (skip CPU, disk, etc.) and return PID→memory map.
    pub fn get_memory_only(&mut self) -> HashMap<u32, u64> {
        self.system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            ProcessRefreshKind::new().with_memory(),
        );
        self.system
            .processes()
            .iter()
            .map(|(pid, proc)| (pid.as_u32(), proc.memory()))
            .collect()
    }

    /// Refresh memory + exe info and return PID→(memory, exe_path) map.
    pub fn get_memory_and_exe(&mut self) -> HashMap<u32, (u64, Option<std::path::PathBuf>)> {
        self.system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            ProcessRefreshKind::new()
                .with_memory()
                .with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
        );
        self.system
            .processes()
            .iter()
            .map(|(pid, proc)| {
                (
                    pid.as_u32(),
                    (proc.memory(), proc.exe().map(|p| p.to_path_buf())),
                )
            })
            .collect()
    }
}

impl DataSource for PollingDataSource {
    fn get_processes(&mut self) -> Result<Vec<ProcessInfo>> {
        self.system
            .refresh_processes(sysinfo::ProcessesToUpdate::All);

        let procs = self
            .system
            .processes()
            .iter()
            .map(|(pid, proc)| {
                let raw_name = proc.name().to_string_lossy().into_owned();
                let name = if raw_name.is_empty() {
                    // Fallback: extract filename from exe path, or use PID
                    proc.exe()
                        .and_then(|p| p.file_name())
                        .map(|f| f.to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("PID-{}", pid.as_u32()))
                } else {
                    raw_name
                };
                ProcessInfo {
                    pid: pid.as_u32(),
                    name,
                    cpu: proc.cpu_usage() / self.num_cpus,
                    memory: proc.memory(),
                    exe_path: proc.exe().map(|p| p.to_path_buf()),
                }
            })
            .collect();

        Ok(procs)
    }
}

impl Default for PollingDataSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_instance() {
        let _ = PollingDataSource::new();
    }

    #[test]
    fn default_creates_instance() {
        let _ = PollingDataSource::default();
    }

    #[test]
    fn get_processes_returns_non_empty() {
        let mut ds = PollingDataSource::new();
        let procs = ds.get_processes().unwrap();
        assert!(!procs.is_empty(), "should see at least one process");
    }

    #[test]
    fn get_processes_contains_self() {
        let mut ds = PollingDataSource::new();
        let procs = ds.get_processes().unwrap();
        let self_pid = std::process::id();
        assert!(
            procs.iter().any(|p| p.pid == self_pid),
            "process list should contain the test runner (pid {})",
            self_pid
        );
    }

    #[test]
    fn process_info_fields_populated() {
        let mut ds = PollingDataSource::new();
        let procs = ds.get_processes().unwrap();
        for p in &procs {
            assert!(
                !p.name.is_empty(),
                "process name should not be empty for pid {}",
                p.pid
            );
        }
    }

    #[test]
    fn consecutive_calls_succeed() {
        let mut ds = PollingDataSource::new();
        let first = ds.get_processes().unwrap();
        let second = ds.get_processes().unwrap();
        assert!(!first.is_empty());
        assert!(!second.is_empty());
    }

    #[test]
    fn get_self_pid_matches() {
        assert_eq!(PollingDataSource::get_self_pid(), std::process::id());
    }

    #[test]
    fn get_memory_only_returns_non_empty() {
        let mut ds = PollingDataSource::new();
        let mem = ds.get_memory_only();
        assert!(!mem.is_empty(), "should see at least one process");
    }

    #[test]
    fn get_memory_only_contains_self() {
        let mut ds = PollingDataSource::new();
        let mem = ds.get_memory_only();
        let self_pid = std::process::id();
        assert!(
            mem.contains_key(&self_pid),
            "should contain self (pid {})",
            self_pid
        );
        assert!(mem[&self_pid] > 0, "self should have non-zero memory");
    }

    #[test]
    fn num_cpus_is_reasonable() {
        let ds = PollingDataSource::new();
        assert!(ds.num_cpus >= 1.0, "should have at least 1 CPU");
        assert!(
            ds.num_cpus <= 1024.0,
            "sanity check: not more than 1024 CPUs"
        );
    }

    /// Cross-validate ALL process names against PowerShell's Get-Process.
    ///
    /// Queries every non-fallback process we report and checks the name
    /// matches what PowerShell returns (modulo the `.exe` suffix that
    /// sysinfo includes but PowerShell strips).
    ///
    /// Processes that exit between our snapshot and the PowerShell query are
    /// silently skipped — that's expected on a live system.
    #[test]
    fn names_match_powershell() {
        let mut ds = PollingDataSource::new();
        let procs = ds.get_processes().unwrap();

        // Test every process, skip PID-fallbacks (those already failed name lookup)
        let testable: Vec<_> = procs
            .iter()
            .filter(|p| !p.name.starts_with("PID-"))
            .collect();

        assert!(
            testable.len() >= 10,
            "need at least 10 testable processes, got {}",
            testable.len()
        );

        // Query PowerShell for ALL PIDs at once
        let pids_csv: String = testable
            .iter()
            .map(|p| p.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let ps_script = format!(
            "Get-Process -Id {} -ErrorAction SilentlyContinue | ForEach-Object {{ \"$($_.Id)|$($_.ProcessName)\" }}",
            pids_csv
        );

        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
            .output()
            .expect("failed to run powershell");

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse PowerShell output into a PID→name map
        let ps_names: HashMap<u32, String> = stdout
            .lines()
            .filter_map(|line| {
                let (pid_str, name) = line.split_once('|')?;
                let pid: u32 = pid_str.trim().parse().ok()?;
                Some((pid, name.trim().to_string()))
            })
            .collect();

        let mut matched = 0;
        let mut mismatched = Vec::new();

        for proc in &testable {
            let Some(ps_name) = ps_names.get(&proc.pid) else {
                // Process exited before PowerShell could query it — skip
                continue;
            };

            // sysinfo returns "notepad.exe", PowerShell returns "notepad"
            let our_name = proc
                .name
                .strip_suffix(".exe")
                .or_else(|| proc.name.strip_suffix(".EXE"))
                .unwrap_or(&proc.name);

            if our_name.eq_ignore_ascii_case(ps_name) {
                matched += 1;
            } else {
                mismatched.push(format!(
                    "PID {}: ours='{}' ps='{}'",
                    proc.pid, proc.name, ps_name
                ));
            }
        }

        let skipped = testable.len() - matched - mismatched.len();
        eprintln!(
            "PowerShell cross-check: {} total, {} matched, {} mismatched, {} skipped (exited)",
            testable.len(),
            matched,
            mismatched.len(),
            skipped
        );
        for m in &mismatched {
            eprintln!("  {}", m);
        }

        assert!(
            matched > 0,
            "should match at least one process name with PowerShell"
        );
        assert!(
            mismatched.is_empty(),
            "name mismatches ({}/{}):\n{}",
            mismatched.len(),
            matched + mismatched.len(),
            mismatched.join("\n")
        );
    }
}
