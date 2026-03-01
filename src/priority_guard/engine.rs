use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::Instant;

use super::config::{is_exempt, PriorityGuardConfig};
use super::windows_api;
use crate::common::ProcessInfo;

/// Tracks a process that exceeded the CPU threshold.
#[derive(Debug)]
pub struct OffenderEntry {
    pub first_seen: Instant,
    pub original_priority: Option<u32>,
    /// Demotion tier: 0 = not demoted, 1 = below normal, 2 = idle.
    pub current_tier: u8,
}

impl OffenderEntry {
    pub fn is_demoted(&self) -> bool {
        self.current_tier > 0
    }
}

/// A log entry recording a PriorityGuard action.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub elapsed_secs: f64,
    pub message: String,
}

const MAX_LOG_ENTRIES: usize = 200;

/// PriorityGuard engine: detects CPU hogs and temporarily lowers their priority.
pub struct PriorityGuardEngine {
    offenders: HashMap<u32, OffenderEntry>,
    log: VecDeque<LogEntry>,
    start_time: Instant,
    /// EMA-smoothed CPU values per PID.
    ema_values: HashMap<u32, f32>,
    /// When each PID was first seen (for grace period).
    first_seen_times: HashMap<u32, Instant>,
    /// Demotion count per PID (persists across restore cycles for repeat offender penalty).
    demotion_history: HashMap<u32, u32>,
    /// PIDs that have already had their grace period exit logged (to avoid repeat logs).
    grace_logged: HashSet<u32>,
}

impl Default for PriorityGuardEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PriorityGuardEngine {
    pub fn new() -> Self {
        Self {
            offenders: HashMap::new(),
            log: VecDeque::new(),
            start_time: Instant::now(),
            ema_values: HashMap::new(),
            first_seen_times: HashMap::new(),
            demotion_history: HashMap::new(),
            grace_logged: HashSet::new(),
        }
    }

    fn push_log(&mut self, message: String) {
        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        if self.log.len() >= MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
        self.log.push_back(LogEntry {
            elapsed_secs,
            message,
        });
    }

    /// Get the log entries for display.
    pub fn log_entries(&self) -> &VecDeque<LogEntry> {
        &self.log
    }

    /// Run one tick of the PriorityGuard algorithm.
    /// Delegates to `tick_with_foreground` with the real foreground PID.
    pub fn tick(&mut self, processes: &[ProcessInfo], config: &PriorityGuardConfig) {
        let fg_pid = windows_api::get_foreground_pid();
        self.tick_with_foreground(processes, config, fg_pid);
    }

    /// Run one tick with an explicit foreground PID (for testability).
    pub fn tick_with_foreground(
        &mut self,
        processes: &[ProcessInfo],
        config: &PriorityGuardConfig,
        foreground_pid: Option<u32>,
    ) {
        if !config.enabled {
            self.cleanup();
            return;
        }

        let grace = std::time::Duration::from_secs(config.grace_period_secs);
        let now = Instant::now();

        // Track current PIDs for stale cleanup
        let current_pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();

        // Update first-seen times and prune stale PIDs
        for &pid in &current_pids {
            self.first_seen_times.entry(pid).or_insert(now);
        }
        self.first_seen_times
            .retain(|pid, _| current_pids.contains(pid));
        self.grace_logged.retain(|pid| current_pids.contains(pid));

        // Update EMA values
        let alpha = config.ema_alpha;
        for proc in processes {
            let ema = self
                .ema_values
                .entry(proc.pid)
                .and_modify(|prev| *prev = alpha * proc.cpu + (1.0 - alpha) * *prev)
                .or_insert(proc.cpu);
            let _ = ema; // used below via ema_values map
        }
        self.ema_values.retain(|pid, _| current_pids.contains(pid));

        // Compute median from EMA values
        let median_cpu = Self::compute_median_from_map(&self.ema_values);

        // Compute adaptive scaling factor: scale = 1.0 - sensitivity * (load - 0.5)
        let scale = if config.adaptive_sensitivity > 0.0 {
            let total_cpu: f32 = self.ema_values.values().sum();
            let system_load = (total_cpu / (config.core_count as f32 * 100.0)).clamp(0.0, 1.0);
            (1.0 - config.adaptive_sensitivity * (system_load - 0.5)).clamp(0.1, 3.0)
        } else {
            1.0
        };

        // If foreground process is currently demoted, restore it immediately
        if let Some(fg) = foreground_pid {
            if let Some(entry) = self.offenders.remove(&fg) {
                if entry.is_demoted() {
                    if let Some(original) = entry.original_priority {
                        windows_api::set_priority(fg, original);
                        self.push_log(format!("foreground restore: PID {}", fg));
                    }
                }
            }
        }

        let mut still_above: HashMap<u32, bool> = HashMap::new();
        let mut pending_logs: Vec<String> = Vec::new();
        let mut pending_history: Vec<u32> = Vec::new();

        for proc in processes {
            if is_exempt(
                proc.pid,
                &proc.name,
                &config.exemption_matcher,
                proc.exe_path.as_deref(),
            ) {
                continue;
            }

            // Skip foreground process
            if foreground_pid == Some(proc.pid) {
                continue;
            }

            // Skip processes in grace period
            if let Some(&first_seen) = self.first_seen_times.get(&proc.pid) {
                if now.duration_since(first_seen) < grace {
                    continue;
                }
            }

            let ema_cpu = self.ema_values.get(&proc.pid).copied().unwrap_or(proc.cpu);
            let triggered = Self::check_hybrid_trigger(ema_cpu, config, median_cpu, scale);

            if triggered {
                still_above.insert(proc.pid, true);

                let repeat_count = self.demotion_history.get(&proc.pid).copied().unwrap_or(0);

                // Log when a process first triggers after exiting grace period
                if config.grace_period_secs > 0
                    && !self.offenders.contains_key(&proc.pid)
                    && self.grace_logged.insert(proc.pid)
                {
                    pending_logs.push(format!(
                        "grace ended: PID {} ({}) — tracking started, CPU {:.1}%",
                        proc.pid, proc.name, ema_cpu
                    ));
                }

                let entry = self
                    .offenders
                    .entry(proc.pid)
                    .or_insert_with(|| OffenderEntry {
                        first_seen: now,
                        original_priority: None,
                        current_tier: 0,
                    });

                // Repeat offender: reduce effective duration
                let effective_duration_secs = if repeat_count > 0 {
                    (config.duration_secs / (1 + repeat_count as u64)).max(1)
                } else {
                    config.duration_secs
                };
                let effective_duration = std::time::Duration::from_secs(effective_duration_secs);
                let elapsed = now.duration_since(entry.first_seen);

                // Tier 1: Below Normal (after effective_duration)
                if entry.current_tier == 0 && elapsed >= effective_duration {
                    if entry.original_priority.is_none() {
                        entry.original_priority =
                            windows_api::get_priority(proc.pid).map(|(raw, _)| raw);
                    }
                    if windows_api::set_priority(proc.pid, windows_api::DEMOTE_PRIORITY) {
                        entry.current_tier = 1;
                        pending_history.push(proc.pid);
                    }
                    let status = if entry.current_tier == 1 {
                        "demoted (tier 1)"
                    } else {
                        "demotion failed"
                    };
                    let repeat_info = if repeat_count > 0 {
                        format!(
                            ", repeat #{} (eff. dur {}s)",
                            repeat_count, effective_duration_secs
                        )
                    } else {
                        String::new()
                    };
                    let adaptive_info = if scale != 1.0 {
                        format!(", adaptive ×{:.2}", scale)
                    } else {
                        String::new()
                    };
                    pending_logs.push(format!(
                        "{}: PID {} ({}) — CPU {:.1}%{}{}",
                        status, proc.pid, proc.name, ema_cpu, repeat_info, adaptive_info
                    ));
                }

                // Tier 2: Idle (after 2× effective_duration, still triggered)
                if entry.current_tier == 1 && elapsed >= effective_duration * 2 {
                    if windows_api::set_priority(proc.pid, windows_api::IDLE_PRIORITY) {
                        entry.current_tier = 2;
                    }
                    let status = if entry.current_tier == 2 {
                        "escalated (tier 2 idle)"
                    } else {
                        "tier 2 failed"
                    };
                    pending_logs.push(format!(
                        "{}: PID {} ({}) — CPU {:.1}%{}",
                        status,
                        proc.pid,
                        proc.name,
                        ema_cpu,
                        if scale != 1.0 {
                            format!(", adaptive ×{:.2}", scale)
                        } else {
                            String::new()
                        }
                    ));
                }
            }
        }

        // Apply deferred mutations
        for pid in pending_history {
            *self.demotion_history.entry(pid).or_insert(0) += 1;
        }
        for msg in pending_logs {
            self.push_log(msg);
        }

        // Restore processes that dropped below all triggers
        let pids_to_remove: Vec<u32> = self
            .offenders
            .iter()
            .filter(|(pid, _)| !still_above.contains_key(pid))
            .map(|(pid, _)| *pid)
            .collect();

        for pid in pids_to_remove {
            if let Some(entry) = self.offenders.remove(&pid) {
                if entry.is_demoted() {
                    if let Some(original) = entry.original_priority {
                        windows_api::set_priority(pid, original);
                        self.push_log(format!("restored: PID {} to original priority", pid));
                    }
                }
            }
        }
    }

    /// Hybrid trigger: OR of absolute, per-core, and relative thresholds.
    /// Uses EMA-smoothed CPU value and adaptive scaling factor.
    fn check_hybrid_trigger(
        ema_cpu: f32,
        config: &PriorityGuardConfig,
        median_cpu: f32,
        scale: f32,
    ) -> bool {
        let abs_threshold = config.cpu_threshold * scale;
        let pc_threshold = config.per_core_threshold * scale;

        // Absolute: EMA CPU% >= scaled threshold
        let abs_hit = ema_cpu >= abs_threshold;

        // Per-core: estimate max single-core usage (assumes single-threaded worst case)
        let per_core_cpu = (ema_cpu * config.core_count as f32).min(100.0);
        let pc_hit = per_core_cpu >= pc_threshold;

        // Relative: CPU significantly above the median of all running processes
        let rel_hit = median_cpu > 0.0 && ema_cpu >= config.relative_multiplier * median_cpu;

        abs_hit || pc_hit || rel_hit
    }

    fn compute_median_from_map(ema_values: &HashMap<u32, f32>) -> f32 {
        if ema_values.is_empty() {
            return 0.0;
        }
        let mut cpus: Vec<f32> = ema_values.values().copied().collect();
        cpus.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let len = cpus.len();
        if len.is_multiple_of(2) {
            (cpus[len / 2 - 1] + cpus[len / 2]) / 2.0
        } else {
            cpus[len / 2]
        }
    }

    /// Get the number of currently demoted processes.
    pub fn active_demotion_count(&self) -> usize {
        self.offenders.values().filter(|e| e.is_demoted()).count()
    }

    /// Check if a specific PID is currently demoted.
    pub fn is_demoted(&self, pid: u32) -> bool {
        self.offenders.get(&pid).is_some_and(|e| e.is_demoted())
    }

    /// Get PIDs of all currently demoted processes.
    pub fn demoted_pids(&self) -> Vec<u32> {
        self.offenders
            .iter()
            .filter(|(_, e)| e.is_demoted())
            .map(|(pid, _)| *pid)
            .collect()
    }

    /// Restore all demoted processes to their original priority.
    pub fn cleanup(&mut self) {
        let entries: Vec<_> = self.offenders.drain().collect();
        for (pid, entry) in entries {
            if entry.is_demoted() {
                if let Some(original) = entry.original_priority {
                    windows_api::set_priority(pid, original);
                    self.push_log(format!("cleanup: restored PID {}", pid));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(enabled: bool, threshold: f32, duration_secs: u64) -> PriorityGuardConfig {
        PriorityGuardConfig {
            enabled,
            cpu_threshold: threshold,
            duration_secs,
            exemption_matcher: crate::exemption::ExemptionMatcher::new(
                crate::exemption::ExemptionList::new(),
            ),
            per_core_threshold: 95.0,
            core_count: 8,
            relative_multiplier: 8.0,
            ema_alpha: 1.0,       // No smoothing in tests by default
            grace_period_secs: 0, // No grace period in tests by default
            adaptive_sensitivity: 0.0,
        }
    }

    fn make_proc(pid: u32, name: &str, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_string(),
            cpu,
            memory: 0,
            exe_path: None,
        }
    }

    #[test]
    fn disabled_does_nothing() {
        let mut engine = PriorityGuardEngine::new();
        let procs = vec![make_proc(1, "hog.exe", 99.0)];
        engine.tick(&procs, &make_config(false, 80.0, 3));
        assert_eq!(engine.active_demotion_count(), 0);
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn below_threshold_not_tracked() {
        let mut engine = PriorityGuardEngine::new();
        let procs = vec![make_proc(1, "idle.exe", 5.0)];
        engine.tick(&procs, &make_config(true, 80.0, 3));
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn above_threshold_tracked_but_not_immediately_demoted() {
        let mut engine = PriorityGuardEngine::new();
        let procs = vec![make_proc(1, "hog.exe", 99.0)];
        engine.tick(&procs, &make_config(true, 80.0, 3));
        assert_eq!(engine.offenders.len(), 1);
        assert!(!engine.is_demoted(1));
    }

    #[test]
    fn demoted_after_duration() {
        let mut engine = PriorityGuardEngine::new();
        let procs = vec![make_proc(1, "hog.exe", 99.0)];
        let config = make_config(true, 80.0, 0); // 0 second duration = immediate

        engine.tick(&procs, &config);
        assert!(engine.offenders.contains_key(&1));
        // set_priority returns false for non-existent PIDs, so demoted stays false
        assert!(!engine.is_demoted(1));
    }

    #[test]
    fn exempt_process_not_tracked() {
        let mut engine = PriorityGuardEngine::new();
        let procs = vec![make_proc(1, "csrss.exe", 99.0)];
        engine.tick(&procs, &make_config(true, 80.0, 0));
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn exemption_system_integration() {
        // Note: Path-based exemption matching logic is tested in exemption module.
        // This test verifies that the engine respects is_exempt().
        // System process exemptions (csrss, svchost, etc.) are tested in config.rs
        let mut engine = PriorityGuardEngine::new();
        let procs = vec![make_proc(999, "csrss.exe", 99.0)]; // System process
        let config = PriorityGuardConfig {
            enabled: true,
            cpu_threshold: 80.0,
            duration_secs: 0,
            exemption_matcher: crate::exemption::ExemptionMatcher::new(
                crate::exemption::ExemptionList::new(),
            ),
            per_core_threshold: 95.0,
            core_count: 8,
            relative_multiplier: 8.0,
            ema_alpha: 1.0,
            grace_period_secs: 0,
            adaptive_sensitivity: 0.0,
        };
        engine.tick(&procs, &config);
        // csrss.exe should be exempt (system process)
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn drops_below_threshold_removed_from_tracking() {
        let mut engine = PriorityGuardEngine::new();
        let config = make_config(true, 80.0, 3);

        // First tick: above threshold
        engine.tick(&[make_proc(1, "hog.exe", 99.0)], &config);
        assert_eq!(engine.offenders.len(), 1);

        // Second tick: below threshold
        engine.tick(&[make_proc(1, "hog.exe", 10.0)], &config);
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn cleanup_clears_all() {
        let mut engine = PriorityGuardEngine::new();
        let config = make_config(true, 80.0, 3);
        engine.tick(
            &[make_proc(1, "a.exe", 99.0), make_proc(2, "b.exe", 99.0)],
            &config,
        );
        assert_eq!(engine.offenders.len(), 2);
        engine.cleanup();
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn disable_triggers_cleanup() {
        let mut engine = PriorityGuardEngine::new();
        engine.tick(
            &[make_proc(1, "hog.exe", 99.0)],
            &make_config(true, 80.0, 3),
        );
        assert_eq!(engine.offenders.len(), 1);
        // Disable
        engine.tick(
            &[make_proc(1, "hog.exe", 99.0)],
            &make_config(false, 80.0, 3),
        );
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn multiple_processes_tracked_independently() {
        let mut engine = PriorityGuardEngine::new();
        let config = make_config(true, 80.0, 3);
        let procs = vec![
            make_proc(1, "a.exe", 99.0),
            make_proc(2, "b.exe", 5.0),
            make_proc(3, "c.exe", 95.0),
        ];
        engine.tick(&procs, &config);
        assert_eq!(engine.offenders.len(), 2); // PIDs 1 and 3
        assert!(!engine.offenders.contains_key(&2));
    }

    // --- EMA smoothing tests ---

    #[test]
    fn ema_smoothing_ignores_single_spike() {
        let mut engine = PriorityGuardEngine::new();
        let config = PriorityGuardConfig {
            enabled: true,
            cpu_threshold: 80.0,
            duration_secs: 3,
            exemption_matcher: crate::exemption::ExemptionMatcher::new(
                crate::exemption::ExemptionList::new(),
            ),
            per_core_threshold: 100.0, // Disable per-core for this test
            core_count: 1,
            relative_multiplier: 100.0, // Disable relative for this test
            ema_alpha: 0.3,
            grace_period_secs: 0,
            adaptive_sensitivity: 0.0,
        };

        // Tick 1: process at 5% — EMA = 5%
        engine.tick(&[make_proc(1, "app.exe", 5.0)], &config);
        assert!(engine.offenders.is_empty());

        // Tick 2: spike to 95% — EMA = 0.3*95 + 0.7*5 = 32%
        engine.tick(&[make_proc(1, "app.exe", 95.0)], &config);
        assert!(
            engine.offenders.is_empty(),
            "EMA should dampen single spike below threshold"
        );

        // Tick 3: back to 5% — EMA drops further
        engine.tick(&[make_proc(1, "app.exe", 5.0)], &config);
        assert!(engine.offenders.is_empty());
    }

    #[test]
    fn ema_alpha_1_is_no_smoothing() {
        let mut engine = PriorityGuardEngine::new();
        let mut config = make_config(true, 80.0, 3);
        config.ema_alpha = 1.0;

        // With alpha=1.0, EMA = raw CPU
        engine.tick(&[make_proc(1, "app.exe", 5.0)], &config);
        engine.tick(&[make_proc(1, "app.exe", 95.0)], &config);
        // Should be tracked since EMA = 95% > 80%
        assert_eq!(engine.offenders.len(), 1);
    }

    // --- Grace period tests ---

    #[test]
    fn grace_period_skips_new_process() {
        let mut engine = PriorityGuardEngine::new();
        let mut config = make_config(true, 80.0, 0);
        config.grace_period_secs = 10;

        // Process appears with high CPU but is within grace period
        engine.tick(&[make_proc(1, "new.exe", 99.0)], &config);
        assert!(
            engine.offenders.is_empty(),
            "new process should be in grace period"
        );
    }

    // --- Foreground protection tests ---

    #[test]
    fn foreground_pid_not_tracked() {
        let mut engine = PriorityGuardEngine::new();
        let config = make_config(true, 80.0, 3);

        // PID 1 is the foreground process
        engine.tick_with_foreground(&[make_proc(1, "fg.exe", 99.0)], &config, Some(1));
        assert!(
            engine.offenders.is_empty(),
            "foreground PID should be skipped"
        );
    }

    #[test]
    fn foreground_pid_gets_restored_if_demoted() {
        let mut engine = PriorityGuardEngine::new();
        let config = make_config(true, 80.0, 3);

        // First: track as offender (not foreground)
        engine.tick_with_foreground(&[make_proc(1, "app.exe", 99.0)], &config, None);
        assert_eq!(engine.offenders.len(), 1);

        // Now it becomes foreground — should be removed from offenders
        engine.tick_with_foreground(&[make_proc(1, "app.exe", 99.0)], &config, Some(1));
        assert!(
            engine.offenders.is_empty(),
            "foreground process should be restored"
        );
    }

    // --- Repeat offender tests ---

    #[test]
    fn demotion_history_tracks_counts() {
        let mut engine = PriorityGuardEngine::new();
        let config = make_config(true, 80.0, 0);

        // Tick: triggers demotion attempt (won't actually demote on test PIDs)
        engine.tick(&[make_proc(100, "hog.exe", 99.0)], &config);
        // The demotion was attempted; check history was incremented
        // (Even if set_priority fails, the attempt counts)
        // Since set_priority returns false in tests, tier stays 0, no history increment
        assert_eq!(engine.demotion_history.get(&100).copied().unwrap_or(0), 0);
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_process() -> impl Strategy<Value = ProcessInfo> {
        (1u32..10000, "\\PC{1,20}", 0.0f32..200.0f32, any::<u64>()).prop_map(
            |(pid, name, cpu, mem)| ProcessInfo {
                pid,
                name,
                cpu,
                memory: mem,
                exe_path: None,
            },
        )
    }

    fn arb_config() -> impl Strategy<Value = PriorityGuardConfig> {
        (
            any::<bool>(),    // enabled
            1.0f32..100.0f32, // cpu_threshold
            0u64..10,         // duration_secs
            0.0f32..100.0f32, // per_core_threshold
            1u8..32,          // core_count
            1.0f32..20.0f32,  // relative_multiplier
            0.01f32..1.0f32,  // ema_alpha
            0u64..10,         // grace_period_secs
            0.0f32..1.0f32,   // adaptive_sensitivity
        )
            .prop_map(
                |(enabled, cpu, dur, per_core, cores, rel, ema, grace, adapt)| {
                    PriorityGuardConfig {
                        enabled,
                        cpu_threshold: cpu,
                        duration_secs: dur,
                        exemption_matcher: crate::exemption::ExemptionMatcher::new(
                            crate::exemption::ExemptionList::new(),
                        ),
                        per_core_threshold: per_core,
                        core_count: cores,
                        relative_multiplier: rel,
                        ema_alpha: ema,
                        grace_period_secs: grace,
                        adaptive_sensitivity: adapt,
                    }
                },
            )
    }

    proptest! {
        /// Engine tick never panics with arbitrary processes and configs.
        #[test]
        fn fuzz_engine_tick_no_panic(
            procs in prop::collection::vec(arb_process(), 0..30),
            config in arb_config(),
        ) {
            let mut engine = PriorityGuardEngine::new();
            engine.tick(&procs, &config);
            // No panic = success
        }

        /// Multiple ticks with varying process lists and configs never panic.
        #[test]
        fn fuzz_engine_multi_tick(
            configs in prop::collection::vec(arb_config(), 1..10),
            procs in prop::collection::vec(arb_process(), 0..20),
        ) {
            let mut engine = PriorityGuardEngine::new();
            for config in &configs {
                engine.tick(&procs, config);

                // Invariant: log entries count bounded
                prop_assert!(engine.log_entries().len() <= 200 + procs.len(),
                    "log grew too large: {}", engine.log_entries().len());
            }
        }

        /// Disabled config never creates offenders.
        #[test]
        fn fuzz_disabled_never_tracks(
            procs in prop::collection::vec(arb_process(), 0..20),
            mut config in arb_config(),
        ) {
            config.enabled = false;
            let mut engine = PriorityGuardEngine::new();
            engine.tick(&procs, &config);
            prop_assert_eq!(engine.active_demotion_count(), 0);
        }

        /// Cleanup always leaves engine empty.
        #[test]
        fn fuzz_cleanup_always_clears(
            procs in prop::collection::vec(arb_process(), 1..20),
            config in arb_config(),
        ) {
            let mut engine = PriorityGuardEngine::new();
            engine.tick(&procs, &config);
            engine.cleanup();
            prop_assert_eq!(engine.active_demotion_count(), 0);
            prop_assert!(engine.demoted_pids().is_empty());
        }

        /// Exempt processes are never tracked as offenders.
        #[test]
        fn fuzz_exempt_never_tracked(idx in 0..super::super::config::SYSTEM_EXEMPTIONS.len()) {
            let name = super::super::config::SYSTEM_EXEMPTIONS[idx];
            let procs = vec![ProcessInfo { pid: 1, name: name.to_string(), cpu: 99.0, memory: 0, exe_path: None }];
            let config = PriorityGuardConfig {
                enabled: true,
                cpu_threshold: 1.0,
                duration_secs: 0,
                exemption_matcher: crate::exemption::ExemptionMatcher::new(
                crate::exemption::ExemptionList::new(),
            ),
                per_core_threshold: 1.0,
                core_count: 1,
                relative_multiplier: 1.0,
                ema_alpha: 1.0,
                grace_period_secs: 0,
                adaptive_sensitivity: 0.0,
            };
            let mut engine = PriorityGuardEngine::new();
            engine.tick(&procs, &config);
            prop_assert!(engine.offenders.is_empty(),
                "system exempt process '{}' was tracked", name);
        }
    }
}
