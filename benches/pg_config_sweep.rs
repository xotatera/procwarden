//! PriorityGuard configuration space search.
//!
//! Simulates synthetic workloads through a lightweight PriorityGuard engine
//! with controllable discrete time, then grid-searches parameter spaces
//! across four detection modes: Absolute, Per-Core, Relative, and Hybrid.
//!
//! Usage: cargo run --bin pg_config_sweep

use std::collections::HashMap;

/// Deterministic jitter: returns a value in [-amplitude, +amplitude] based on pid+tick.
/// Uses a simple hash to avoid pulling in rand as a dependency.
fn jitter(pid: u32, tick: u64, amplitude: f32) -> f32 {
    let mut h = (pid as u64).wrapping_mul(2654435761) ^ (tick.wrapping_mul(40503));
    h = h.wrapping_mul(h.wrapping_add(1));
    let norm = ((h % 10001) as f32 / 5000.0) - 1.0;
    norm * amplitude
}

// ---------------------------------------------------------------------------
// Detection modes and configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DetectionMode {
    /// Original: absolute CPU% threshold
    Absolute,
    /// Per-core: max single-core CPU derived from total + thread count
    PerCore,
    /// Relative: CPU vs median of all processes in snapshot
    Relative,
    /// Hybrid: OR of Absolute + PerCore + Relative
    Hybrid,
}

#[derive(Clone, Debug)]
struct DetectionConfig {
    mode: DetectionMode,
    /// Absolute CPU% threshold (used by Absolute and Hybrid)
    abs_threshold: f32,
    /// Duration in ticks before demotion
    duration: u64,
    /// Per-core CPU% threshold (used by PerCore and Hybrid)
    per_core_threshold: f32,
    /// Number of CPU cores in the simulated system
    core_count: u8,
    /// Relative multiplier: trigger if CPU > multiplier × median (used by Relative and Hybrid)
    relative_multiplier: f32,
    /// EMA smoothing alpha (1.0 = no smoothing)
    ema_alpha: f32,
    /// Grace period: ignore processes for first N ticks
    grace_period: u64,
    /// Whether repeat offender penalty is active
    repeat_offender: bool,
    /// Adaptive sensitivity: 0.0 = disabled, higher = more aggressive scaling
    adaptive_sensitivity: f32,
}

// ---------------------------------------------------------------------------
// Simulation engine (mirrors PriorityGuard logic with discrete time)
// ---------------------------------------------------------------------------

struct SimOffender {
    first_seen_tick: u64,
    current_tier: u8, // 0=not demoted, 1=below normal, 2=idle
}

struct SimEngine {
    offenders: HashMap<u32, SimOffender>,
    demotions: Vec<DemotionEvent>,
    ema_values: HashMap<u32, f32>,
    first_seen_tick_map: HashMap<u32, u64>, // PID → first tick seen (for grace period)
    demotion_history: HashMap<u32, u32>, // PID → demotion count (for repeat offender)
}

#[allow(dead_code)] // fields used for debug output in sweep analysis
struct DemotionEvent {
    pid: u32,
    tick: u64,
    first_seen_tick: u64,
    tier: u8,
}

/// Info about a single process in a tick snapshot.
struct SimProcess {
    pid: u32,
    cpu: f32,
    threads_used: u8,
}

impl SimEngine {
    fn new() -> Self {
        Self {
            offenders: HashMap::new(),
            demotions: Vec::new(),
            ema_values: HashMap::new(),
            first_seen_tick_map: HashMap::new(),
            demotion_history: HashMap::new(),
        }
    }

    fn tick(&mut self, tick: u64, processes: &[SimProcess], config: &DetectionConfig) {
        // Update first-seen times and EMA values
        for proc in processes {
            self.first_seen_tick_map.entry(proc.pid).or_insert(tick);
            let alpha = config.ema_alpha;
            self.ema_values
                .entry(proc.pid)
                .and_modify(|prev| *prev = alpha * proc.cpu + (1.0 - alpha) * *prev)
                .or_insert(proc.cpu);
        }

        // Compute median from EMA values for relative mode
        let median_cpu = if matches!(config.mode, DetectionMode::Relative | DetectionMode::Hybrid) {
            let mut cpus: Vec<f32> = self.ema_values.values().copied().collect();
            cpus.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if cpus.is_empty() {
                0.0
            } else if cpus.len().is_multiple_of(2) {
                (cpus[cpus.len() / 2 - 1] + cpus[cpus.len() / 2]) / 2.0
            } else {
                cpus[cpus.len() / 2]
            }
        } else {
            0.0
        };

        // Compute adaptive scaling factor
        let scale = if config.adaptive_sensitivity > 0.0 {
            let total_cpu: f32 = self.ema_values.values().sum();
            let system_load = (total_cpu / (config.core_count as f32 * 100.0)).clamp(0.0, 1.0);
            (1.0 - config.adaptive_sensitivity * (system_load - 0.5)).clamp(0.1, 3.0)
        } else {
            1.0
        };

        let mut still_above: HashMap<u32, bool> = HashMap::new();

        for proc in processes {
            // Grace period: skip processes seen for fewer ticks than grace_period
            if let Some(&first_tick) = self.first_seen_tick_map.get(&proc.pid) {
                if tick - first_tick < config.grace_period {
                    continue;
                }
            }

            let ema_cpu = self.ema_values.get(&proc.pid).copied().unwrap_or(proc.cpu);
            let triggered = Self::check_trigger_with_ema(proc, ema_cpu, config, median_cpu, scale);

            if triggered {
                still_above.insert(proc.pid, true);

                // Repeat offender: reduce effective duration
                let repeat_count = if config.repeat_offender {
                    self.demotion_history.get(&proc.pid).copied().unwrap_or(0)
                } else {
                    0
                };
                let effective_duration = if repeat_count > 0 {
                    (config.duration / (1 + repeat_count as u64)).max(1)
                } else {
                    config.duration
                };

                let entry = self.offenders.entry(proc.pid).or_insert(SimOffender {
                    first_seen_tick: tick,
                    current_tier: 0,
                });
                let elapsed = tick - entry.first_seen_tick;

                // Tier 1: Below Normal
                if entry.current_tier == 0 && elapsed >= effective_duration {
                    entry.current_tier = 1;
                    *self.demotion_history.entry(proc.pid).or_insert(0) += 1;
                    self.demotions.push(DemotionEvent {
                        pid: proc.pid,
                        tick,
                        first_seen_tick: entry.first_seen_tick,
                        tier: 1,
                    });
                }

                // Tier 2: Idle (after 2× effective_duration)
                if entry.current_tier == 1 && elapsed >= effective_duration * 2 {
                    entry.current_tier = 2;
                    self.demotions.push(DemotionEvent {
                        pid: proc.pid,
                        tick,
                        first_seen_tick: entry.first_seen_tick,
                        tier: 2,
                    });
                }
            }
        }

        self.offenders.retain(|pid, _| still_above.contains_key(pid));
    }

    fn check_trigger_with_ema(proc: &SimProcess, ema_cpu: f32, config: &DetectionConfig, median_cpu: f32, scale: f32) -> bool {
        let abs_t = config.abs_threshold * scale;
        let pc_t = config.per_core_threshold * scale;

        match config.mode {
            DetectionMode::Absolute => {
                ema_cpu >= abs_t
            }
            DetectionMode::PerCore => {
                let per_core = Self::derive_per_core_cpu_ema(ema_cpu, proc.threads_used, config.core_count);
                per_core >= pc_t
            }
            DetectionMode::Relative => {
                median_cpu > 0.0 && ema_cpu >= config.relative_multiplier * median_cpu
            }
            DetectionMode::Hybrid => {
                let abs_hit = ema_cpu >= abs_t;
                let per_core = Self::derive_per_core_cpu_ema(ema_cpu, proc.threads_used, config.core_count);
                let pc_hit = per_core >= pc_t;
                let rel_hit = median_cpu > 0.0
                    && ema_cpu >= config.relative_multiplier * median_cpu;
                abs_hit || pc_hit || rel_hit
            }
        }
    }

    fn derive_per_core_cpu_ema(ema_cpu: f32, threads_used: u8, core_count: u8) -> f32 {
        let threads = (threads_used as f32).max(1.0);
        let cores = core_count as f32;
        let per_thread_cpu = ema_cpu / threads;
        (per_thread_cpu * cores).min(100.0)
    }

}

// ---------------------------------------------------------------------------
// Workload scenarios
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Label {
    Hog,
    NotHog,
}

/// A process entry in a scenario with thread count for per-core derivation.
struct ScenarioProcess {
    pid: u32,
    label: Label,
    /// Number of threads this process uses (affects per-core CPU calculation)
    threads_used: u8,
}

type CpuFn = fn(u32, u64) -> f32;

struct Scenario {
    name: &'static str,
    processes: Vec<ScenarioProcess>,
    /// Total duration in seconds
    duration_ticks: u64,
    /// Returns CPU for (pid, tick)
    cpu_fn: CpuFn,
    /// Background processes for relative mode context.
    /// Each is (pid, cpu_fn). These are NOT scored (no label) but affect median.
    background: Vec<(u32, CpuFn)>,
}

/// Helper: create a ScenarioProcess with default 4 threads (multi-threaded).
fn sp(pid: u32, label: Label) -> ScenarioProcess {
    ScenarioProcess { pid, label, threads_used: 4 }
}

/// Helper: create a ScenarioProcess with specific thread count.
fn sp_t(pid: u32, label: Label, threads: u8) -> ScenarioProcess {
    ScenarioProcess { pid, label, threads_used: threads }
}

// --- CPU pattern functions (original clear-cut) ---

fn sustained_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 92.0 } else { 10.0 }
}

fn brief_spike_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick < 2 { 95.0 } else if pid == 1 { 5.0 } else { 10.0 }
}

fn medium_burst_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick < 5 { 70.0 } else { 10.0 }
}

fn gradual_ramp_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick < 8 { 10.0 + (95.0 - 10.0) * (tick as f32 / 8.0) } else { 95.0 }
}

fn oscillating_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if (tick / 2).is_multiple_of(2) { 90.0 } else { 20.0 }
}

fn normal_load_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 25.0 } else { 10.0 }
}

fn multi_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid <= 3 { 88.0 } else { 10.0 }
}

// --- Nuanced borderline scenarios ---

fn compilation_burst_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick < 4 { 75.0 } else if pid == 1 { 8.0 } else { 10.0 }
}

fn video_encode_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 65.0 } else { 10.0 }
}

fn runaway_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    (50.0 + tick as f32 * 5.0).min(100.0)
}

fn browser_leak_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 45.0 } else { 10.0 }
}

fn game_av_cpu(pid: u32, tick: u64) -> f32 {
    match pid {
        1 => 60.0,
        2 => if tick < 10 { 85.0 } else { 5.0 },
        _ => 5.0,
    }
}

fn noisy_sensor_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick.is_multiple_of(2) { 85.0 } else { 70.0 }
}

fn crypto_miner_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 95.0 } else { 5.0 }
}

fn stutter_hog_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick % 4 < 3 { 90.0 } else { 30.0 }
}

fn multi_tier_cpu(pid: u32, _tick: u64) -> f32 {
    match pid { 1 => 55.0, 2 => 75.0, 3 => 92.0, _ => 5.0 }
}

fn spike_then_sustain_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick < 1 { 95.0 } else if tick < 4 { 40.0 } else { 88.0 }
}

// --- Jittered scenarios ---

fn jittered_hog_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0 + jitter(pid, tick, 3.0); }
    (90.0 + jitter(pid, tick, 5.0)).clamp(0.0, 100.0)
}

fn threshold_hugger_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 8.0 + jitter(pid, tick, 2.0); }
    (78.0 + jitter(pid, tick, 4.0)).clamp(0.0, 100.0)
}

fn noisy_hog_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 12.0 + jitter(pid, tick, 3.0); }
    (85.0 + jitter(pid, tick, 8.0)).clamp(0.0, 100.0)
}

fn jittered_mixed_cpu(pid: u32, tick: u64) -> f32 {
    match pid {
        1 => (88.0 + jitter(pid, tick, 6.0)).clamp(0.0, 100.0),
        2 => (40.0 + jitter(pid, tick, 6.0)).clamp(0.0, 100.0),
        _ => 5.0,
    }
}

fn jittered_spike_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0 + jitter(pid, tick, 2.0); }
    if tick < 3 { (92.0 + jitter(pid, tick, 5.0)).clamp(0.0, 100.0) }
    else { (15.0 + jitter(pid, tick, 3.0)).clamp(0.0, 100.0) }
}

fn jittered_ramp_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 8.0 + jitter(pid, tick, 2.0); }
    let base = 30.0 + (95.0 - 30.0) * (tick as f32 / 15.0).min(1.0);
    (base + jitter(pid, tick, 7.0)).clamp(0.0, 100.0)
}

fn jittered_multi_hog_cpu(pid: u32, tick: u64) -> f32 {
    let base = match pid {
        1 => 83.0, 2 => 87.0, 3 => 92.0,
        _ => return 10.0 + jitter(pid, tick, 2.0),
    };
    (base + jitter(pid, tick, 4.0)).clamp(0.0, 100.0)
}

// --- Per-core specific scenarios ---

/// Single-threaded hog: 12.5% total on 8 cores = 100% on one core.
/// Invisible to absolute threshold, caught by per-core.
fn single_thread_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 12.5 } else { 5.0 }
}

/// Parallel workload: 80% total across 8 threads = ~10% per core. Legit work.
fn parallel_workload_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 80.0 } else { 5.0 }
}

/// Two-thread hog: 25% total on 8 cores, but each thread at 100% of a core.
fn two_thread_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 25.0 } else { 5.0 }
}

/// Balanced high load: 90% across all 8 cores. Legit parallel work.
fn balanced_high_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 90.0 } else { 5.0 }
}

/// 4-thread build: 50% total on 8 cores = 100% per-core per-thread.
/// Legit compilation — high per-core but short-lived (6s).
fn four_thread_build_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick < 6 { 50.0 } else { 5.0 }
}

/// 3-thread game: 37.5% total on 8 cores = 100% per-core. Legit sustained game load.
fn three_thread_game_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 37.5 } else { 5.0 }
}

/// Single-thread idle spinner: 5% total on 8 cores = 40% per-core. Not a hog.
fn idle_spinner_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 5.0 } else { 3.0 }
}

/// Single-thread moderate: 8% total = 64% per-core on 8 cores. Borderline, not a hog.
fn single_thread_moderate_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 8.0 } else { 3.0 }
}

/// Single-thread runaway: starts 5% (40% per-core), climbs to 12.5% (100% per-core).
/// Becomes a hog after reaching full core saturation.
fn single_thread_ramp_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 3.0; }
    (5.0 + tick as f32 * 0.75).min(12.5)
}

/// Mixed threading: pid=1 is 1-thread at 12% (96% per-core, hog),
/// pid=2 is 6-thread at 60% (80% per-core, legit parallel work).
fn mixed_thread_cpu(pid: u32, _tick: u64) -> f32 {
    match pid { 1 => 12.0, 2 => 60.0, _ => 5.0 }
}

/// Jittered single-thread hog: 11% ± 2% total (88% ± 16% per-core).
fn jittered_single_thread_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 3.0 + jitter(pid, tick, 1.0); }
    (11.0 + jitter(pid, tick, 2.0)).clamp(0.0, 100.0)
}

/// Two processes: one 2-thread at 20% (80% per-core, borderline),
/// one 1-thread at 12.5% (100% per-core, hog). Tests discrimination.
fn dual_core_hog_cpu(pid: u32, _tick: u64) -> f32 {
    match pid { 1 => 20.0, 2 => 12.5, _ => 5.0 }
}

// --- Relative CPU specific scenarios ---

/// Outlier on idle system: 40% CPU when everyone else is 1-3%.
fn outlier_idle_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 40.0 } else { 5.0 }
}
fn bg_idle_cpu(_pid: u32, tick: u64) -> f32 {
    1.0 + jitter(100, tick, 1.0).abs()
}

/// All processes busy: 50% target + peers at 40-50%. Not an outlier.
fn all_busy_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 50.0 } else { 5.0 }
}
fn bg_busy_cpu(pid: u32, tick: u64) -> f32 {
    40.0 + jitter(pid, tick, 5.0).abs()
}

/// Slightly above peers: 15% when peers are 8-10%. Not worth demoting.
fn slightly_above_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 15.0 } else { 5.0 }
}
fn bg_medium_cpu(pid: u32, tick: u64) -> f32 {
    8.0 + jitter(pid, tick, 1.0).abs()
}

/// Outlier on loaded system: 85% when peers are 20-30%.
fn outlier_loaded_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 85.0 } else { 5.0 }
}
fn bg_loaded_cpu(pid: u32, tick: u64) -> f32 {
    20.0 + jitter(pid, tick, 5.0).abs()
}

/// Gradual outlier: starts at 10% (fits in), ramps to 70% while bg stays at 5%.
/// Becomes a relative outlier partway through.
fn gradual_outlier_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 5.0; }
    (10.0 + tick as f32 * 4.0).min(70.0)
}
fn bg_low_cpu(pid: u32, tick: u64) -> f32 {
    4.0 + jitter(pid, tick, 1.0).abs()
}

/// Spiky outlier: 80% for 3s, drops to 10%, repeats. Not sustained enough.
fn spiky_outlier_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 5.0; }
    if tick % 6 < 3 { 80.0 } else { 10.0 }
}
fn bg_quiet_cpu(pid: u32, tick: u64) -> f32 {
    3.0 + jitter(pid, tick, 1.0).abs()
}

/// Two outliers competing: pid=1 at 60%, pid=2 at 90%, bg at 5%.
/// Only pid=2 is the real hog. Tests that relative mode picks the worse one.
fn two_outliers_cpu(pid: u32, _tick: u64) -> f32 {
    match pid { 1 => 60.0, 2 => 90.0, _ => 5.0 }
}

/// Normal desktop: many procs at varied levels, one genuine hog at 75%.
/// Background: mix of 2-15% processes.
fn desktop_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 75.0 } else { 5.0 }
}
fn bg_desktop_cpu(pid: u32, tick: u64) -> f32 {
    let base = match pid % 5 { 0 => 2.0, 1 => 5.0, 2 => 8.0, 3 => 12.0, _ => 3.0 };
    base + jitter(pid, tick, 2.0)
}

/// Streaming + downloads: target at 35% with bg at 15-25%. Moderate outlier, not a hog.
fn moderate_outlier_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 35.0 } else { 5.0 }
}
fn bg_moderate_cpu(pid: u32, tick: u64) -> f32 {
    15.0 + jitter(pid, tick, 5.0).abs()
}

/// Jittered relative hog: 65% ± 8% with bg at 5% ± 2%. Clear outlier with noise.
fn jittered_rel_hog_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 5.0; }
    (65.0 + jitter(pid, tick, 8.0)).clamp(0.0, 100.0)
}
fn bg_jittered_cpu(pid: u32, tick: u64) -> f32 {
    5.0 + jitter(pid, tick, 2.0)
}

// --- New feature-targeting scenarios ---

/// EMA test: single 95% spike for 1 tick then drops to 5%. Should NOT be demoted with EMA.
fn ema_brief_spike_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick == 0 { 95.0 } else if pid == 1 { 5.0 } else { 10.0 }
}

/// EMA test: gradual ramp from 20% to 90% over 14 ticks. Should be detected as HOG.
fn ema_ramp_up_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    (20.0 + (90.0 - 20.0) * (tick as f32 / 14.0).min(1.0)).min(90.0)
}

/// Grace period test: 80% for 3 ticks then drops to 10%. New process startup burst.
fn grace_new_process_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick < 3 { 80.0 } else { 10.0 }
}

/// Repeat offender test: 5 cycles of above/below threshold.
fn oscillator_repeat_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    // 3 ticks high, 2 ticks low, repeat
    if tick % 5 < 3 { 90.0 } else { 20.0 }
}

/// Graduated demotion test: sustained heavy load for 4× normal duration.
fn sustained_heavy_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 95.0 } else { 5.0 }
}

/// Adaptive test: high system load (background at 85%, target at 55%).
fn adaptive_high_load_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 55.0 } else { 10.0 }
}
fn bg_high_load_cpu(_pid: u32, _tick: u64) -> f32 {
    85.0
}

/// Adaptive test: low system load (background at 15%, target at 55%).
fn adaptive_low_load_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 55.0 } else { 10.0 }
}
fn bg_low_load_cpu(_pid: u32, _tick: u64) -> f32 {
    15.0
}

/// Adaptive: saturated system with borderline hog. 8 bg procs at 70% each.
/// Target at 40% — well below abs threshold normally, but system is critically overloaded
/// so adaptive should lower thresholds to catch this resource hog.
fn adaptive_saturated_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 40.0 } else { 10.0 }
}
fn bg_saturated_cpu(_pid: u32, _tick: u64) -> f32 {
    70.0
}

/// Adaptive: medium load system. 5 bg procs at 40% = ~25% system load.
/// Target at 60% — moderately high, but system isn't stressed. Should NOT be a hog.
fn adaptive_medium_load_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 60.0 } else { 10.0 }
}
fn bg_medium_load_cpu(_pid: u32, _tick: u64) -> f32 {
    40.0
}

/// Adaptive: build server scenario. Many procs competing, system at ~60% load.
/// Target at 45% is normal CI work, NOT a hog despite busy system.
fn adaptive_build_server_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 45.0 } else { 10.0 }
}
fn bg_build_server_cpu(pid: u32, tick: u64) -> f32 {
    50.0 + jitter(pid, tick, 10.0)
}

/// Adaptive: overloaded server with clear hog. System at 90% load, target at 60%.
/// Below abs threshold normally, but adaptive should lower thresholds on overloaded system.
fn adaptive_overloaded_hog_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 60.0 } else { 10.0 }
}
fn bg_overloaded_cpu(pid: u32, tick: u64) -> f32 {
    80.0 + jitter(pid, tick, 5.0)
}

/// Adaptive: spike under load. System is busy (65% load), process spikes to 55% for 4 ticks
/// then drops to 15%. Brief spike on a loaded system — should NOT be demoted.
fn adaptive_spike_under_load_cpu(pid: u32, tick: u64) -> f32 {
    if pid == 1 && tick < 4 { 55.0 } else if pid == 1 { 15.0 } else { 10.0 }
}
fn bg_moderate_load_cpu(pid: u32, tick: u64) -> f32 {
    55.0 + jitter(pid, tick, 5.0)
}

/// Adaptive: gradual system load increase with a borderline process.
/// Background ramps from 20% to 80% over 20 ticks. Target stays at 45%.
/// Below threshold normally, but should become a hog as system load increases.
fn adaptive_ramp_load_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 45.0 } else { 10.0 }
}
fn bg_ramp_load_cpu(_pid: u32, tick: u64) -> f32 {
    (20.0 + (80.0 - 20.0) * (tick as f32 / 20.0).min(1.0)).min(80.0)
}

/// Adaptive: idle system with high-CPU process. System load < 5%, target at 60%.
/// On an idle system, adaptive should RAISE thresholds — this is NOT a hog.
fn adaptive_idle_system_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 60.0 } else { 10.0 }
}
fn bg_idle_system_cpu(_pid: u32, _tick: u64) -> f32 {
    2.0
}

/// Adaptive: mixed realistic desktop. Several apps at varied loads totaling ~50% system.
/// Target at 55% is just another busy app — NOT a hog in this context.
fn adaptive_desktop_mix_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 55.0 } else { 10.0 }
}
fn bg_desktop_mix_cpu(pid: u32, tick: u64) -> f32 {
    let base = match pid % 4 { 0 => 30.0, 1 => 45.0, 2 => 55.0, _ => 25.0 };
    base + jitter(pid, tick, 5.0)
}

/// Adaptive: thermal throttle scenario. System suddenly goes from 30% to 90% load.
/// Target at 42% — below threshold normally, but should be caught after load spike.
fn adaptive_thermal_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 42.0 } else { 10.0 }
}
fn bg_thermal_cpu(_pid: u32, tick: u64) -> f32 {
    if tick < 8 { 25.0 } else { 90.0 }
}

/// Combined: process that starts high, drops, then comes back. Tests repeat offender + EMA.
fn relapse_hog_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick < 5 { 90.0 } else if tick < 10 { 15.0 } else { 92.0 }
}

// --- Multi-feature interaction scenarios ---

/// Grace + EMA + Adaptive: new process ramps up on an overloaded system.
/// Starts at 80% (startup burst during grace), drops to 20%, ramps to 50%.
/// System is overloaded (bg 75%). Grace should skip startup, EMA smooths the ramp,
/// adaptive lowers threshold to catch the 50% target. HOG.
fn combo_grace_ema_adaptive_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick < 5 { 80.0 } else if tick < 8 { 20.0 } else { 50.0 }
}
fn bg_combo_overloaded_cpu(_pid: u32, _tick: u64) -> f32 {
    75.0
}

/// Grace + EMA: new process with noisy startup, then settles into sustained hog.
/// First 4 ticks: wild 30-95% swings (startup). Then settles at 85%.
/// Grace should ignore startup, EMA should smooth transitions. HOG after grace.
fn combo_grace_ema_startup_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    match tick {
        0 => 95.0, 1 => 30.0, 2 => 88.0, 3 => 45.0, // noisy startup
        _ => 85.0,
    }
}

/// Repeat offender + Graduated: oscillating hog that keeps coming back.
/// 4 ticks high, 3 ticks low, repeat for 30 ticks. Should be caught faster
/// each cycle (repeat offender) AND escalate to tier 2 (graduated). HOG.
fn combo_repeat_graduated_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick % 7 < 4 { 92.0 } else { 15.0 }
}

/// Repeat offender + EMA: process that rapidly oscillates above/below threshold.
/// EMA should smooth the rapid oscillation, repeat offender accelerates detection.
/// 2 ticks high, 1 tick low, cycle. HOG.
fn combo_repeat_ema_rapid_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    if tick % 3 < 2 { 88.0 } else { 30.0 }
}

/// Adaptive + Per-core: single-threaded hog (10% total = 80% per-core) on overloaded system.
/// Below abs threshold, caught by per-core. Adaptive should also lower per-core threshold
/// on this overloaded system. HOG.
fn combo_adaptive_percore_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 10.0 } else { 5.0 }
}
fn bg_combo_percore_load_cpu(_pid: u32, _tick: u64) -> f32 {
    70.0
}

/// Adaptive + Per-core: 2-thread process at 18% total (72% per-core) on busy system.
/// Per-core borderline (below 80% threshold). Adaptive should lower threshold to catch it. HOG.
fn combo_adaptive_percore_borderline_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 18.0 } else { 5.0 }
}
fn bg_combo_percore_busy_cpu(pid: u32, tick: u64) -> f32 {
    65.0 + jitter(pid, tick, 5.0)
}

/// Adaptive + Per-core: 1-thread at 8% total (64% per-core) on idle system.
/// Adaptive should RAISE thresholds on idle system, making this even less of a hog. NOT hog.
fn combo_adaptive_percore_idle_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 8.0 } else { 3.0 }
}
fn bg_combo_percore_idle_cpu(_pid: u32, _tick: u64) -> f32 {
    2.0
}

/// All features: new process that ramps up on overloaded system, oscillates, escalates.
/// Tick 0-4: grace period (70% startup burst). Tick 5-10: ramps to 55%.
/// Tick 11-15: drops to 20%. Tick 16-25: back to 55% (repeat offender kicks in).
/// System overloaded (bg 80%). Tests grace + EMA + adaptive + repeat offender + graduated. HOG.
fn combo_all_features_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 10.0; }
    match tick {
        0..=4 => 70.0,     // startup burst (grace period)
        5..=10 => 55.0,    // sustained — adaptive should catch on overloaded system
        11..=15 => 20.0,   // drops below
        _ => 55.0,         // repeat offender — should be caught faster
    }
}
fn bg_combo_all_features_cpu(pid: u32, tick: u64) -> f32 {
    80.0 + jitter(pid, tick, 3.0)
}

/// All features negative: new process on idle system, brief activity, NOT a hog.
/// Tick 0-4: 60% startup (grace ignores). Tick 5-8: 35% moderate. Tick 9+: 10% idle.
/// System is idle (bg 3%). All features should agree this is NOT a hog.
fn combo_all_features_not_hog_cpu(pid: u32, tick: u64) -> f32 {
    if pid != 1 { return 5.0; }
    match tick {
        0..=4 => 60.0,   // startup burst
        5..=8 => 35.0,   // moderate work
        _ => 10.0,       // idle
    }
}
fn bg_combo_idle_cpu(_pid: u32, _tick: u64) -> f32 {
    3.0
}

/// Repeat + graduated + adaptive: sustained hog on loaded system, 4x duration.
/// Should escalate to tier 2 faster due to repeat (if previously seen).
/// Uses fresh PID so no prior history, but tests graduated + adaptive together.
fn combo_graduated_adaptive_cpu(pid: u32, _tick: u64) -> f32 {
    if pid == 1 { 50.0 } else { 5.0 }
}
fn bg_combo_graduated_load_cpu(pid: u32, tick: u64) -> f32 {
    75.0 + jitter(pid, tick, 5.0)
}

fn scenarios() -> Vec<Scenario> {
    vec![
        // --- Original clear-cut scenarios ---
        Scenario {
            name: "Sustained hog (92%, 15s)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: sustained_hog_cpu,
            background: vec![],
        },
        Scenario {
            name: "Brief spike (95%, 2s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 10, cpu_fn: brief_spike_cpu,
            background: vec![],
        },
        Scenario {
            name: "Medium burst (70%, 5s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 10, cpu_fn: medium_burst_cpu,
            background: vec![],
        },
        Scenario {
            name: "Gradual ramp (10→95%, holds)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: gradual_ramp_cpu,
            background: vec![],
        },
        Scenario {
            name: "Oscillating (90/20% every 2s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: oscillating_cpu,
            background: vec![],
        },
        Scenario {
            name: "Normal load (25% steady)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: normal_load_cpu,
            background: vec![],
        },
        Scenario {
            name: "Multi-hog (3×88%, 12s)",
            processes: vec![
                sp(1, Label::Hog), sp(2, Label::Hog), sp(3, Label::Hog),
                sp(4, Label::NotHog),
            ],
            duration_ticks: 15, cpu_fn: multi_hog_cpu,
            background: vec![],
        },

        // --- Nuanced borderline scenarios ---
        Scenario {
            name: "Compilation burst (75%, 4s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 10, cpu_fn: compilation_burst_cpu,
            background: vec![],
        },
        Scenario {
            name: "Video encode (65% sustained 20s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: video_encode_cpu,
            background: vec![],
        },
        Scenario {
            name: "Runaway (50→100%, holds)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: runaway_cpu,
            background: vec![],
        },
        Scenario {
            name: "Browser leak (45% steady 30s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 30, cpu_fn: browser_leak_cpu,
            background: vec![],
        },
        Scenario {
            name: "Game(60%)+AV scan(85%,10s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::Hog), sp(3, Label::NotHog)],
            duration_ticks: 15, cpu_fn: game_av_cpu,
            background: vec![],
        },
        Scenario {
            name: "Noisy sensor (70-85% flicker)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: noisy_sensor_cpu,
            background: vec![],
        },
        Scenario {
            name: "Crypto miner (95% instant)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: crypto_miner_cpu,
            background: vec![],
        },
        Scenario {
            name: "Stutter hog (90%×3s/30%×1s cycle)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: stutter_hog_cpu,
            background: vec![],
        },
        Scenario {
            name: "Multi-tier (55/75/92%)",
            processes: vec![
                sp(1, Label::NotHog), sp(2, Label::NotHog), sp(3, Label::Hog),
                sp(4, Label::NotHog),
            ],
            duration_ticks: 15, cpu_fn: multi_tier_cpu,
            background: vec![],
        },
        Scenario {
            name: "Spike then sustain (95→40→88%)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: spike_then_sustain_cpu,
            background: vec![],
        },

        // --- Jittered scenarios ---
        Scenario {
            name: "Jittered hog (90%±5%, 20s)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: jittered_hog_cpu,
            background: vec![],
        },
        Scenario {
            name: "Threshold hugger (78%±4%, 20s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: threshold_hugger_cpu,
            background: vec![],
        },
        Scenario {
            name: "Noisy hog (85%±8%, 20s)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: noisy_hog_cpu,
            background: vec![],
        },
        Scenario {
            name: "Jittered mixed (88%±6 vs 40%±6)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog), sp(3, Label::NotHog)],
            duration_ticks: 20, cpu_fn: jittered_mixed_cpu,
            background: vec![],
        },
        Scenario {
            name: "Jittered spike (92%±5%, 3s)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 10, cpu_fn: jittered_spike_cpu,
            background: vec![],
        },
        Scenario {
            name: "Jittered ramp (30→95%±7%)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: jittered_ramp_cpu,
            background: vec![],
        },
        Scenario {
            name: "Jittered multi-hog (83/87/92%±4%)",
            processes: vec![
                sp(1, Label::Hog), sp(2, Label::Hog), sp(3, Label::Hog),
                sp(4, Label::NotHog),
            ],
            duration_ticks: 20, cpu_fn: jittered_multi_hog_cpu,
            background: vec![],
        },

        // --- Per-core scenarios ---
        Scenario {
            name: "Single-thread hog (12.5% total, 1 thread, 8 cores)",
            processes: vec![sp_t(1, Label::Hog, 1), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: single_thread_hog_cpu,
            background: vec![],
        },
        Scenario {
            name: "Parallel workload (80% total, 8 threads)",
            processes: vec![sp_t(1, Label::NotHog, 8), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: parallel_workload_cpu,
            background: vec![],
        },
        Scenario {
            name: "Two-thread hog (25% total, 2 threads, 8 cores)",
            processes: vec![sp_t(1, Label::Hog, 2), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: two_thread_hog_cpu,
            background: vec![],
        },
        Scenario {
            name: "Balanced high (90% total, 8 threads)",
            processes: vec![sp_t(1, Label::NotHog, 8), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: balanced_high_cpu,
            background: vec![],
        },
        Scenario {
            name: "4-thread build (50% total, 4 threads, 6s burst)",
            processes: vec![sp_t(1, Label::NotHog, 4), sp(2, Label::NotHog)],
            duration_ticks: 12, cpu_fn: four_thread_build_cpu,
            background: vec![],
        },
        Scenario {
            name: "3-thread game (37.5% total, 3 threads, sustained)",
            processes: vec![sp_t(1, Label::NotHog, 3), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: three_thread_game_cpu,
            background: vec![],
        },
        Scenario {
            name: "Idle spinner (5% total, 1 thread, 40% per-core)",
            processes: vec![sp_t(1, Label::NotHog, 1), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: idle_spinner_cpu,
            background: vec![],
        },
        Scenario {
            name: "Single-thread moderate (8% total, 64% per-core)",
            processes: vec![sp_t(1, Label::NotHog, 1), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: single_thread_moderate_cpu,
            background: vec![],
        },
        Scenario {
            name: "Single-thread ramp (5→12.5%, 1 thread)",
            processes: vec![sp_t(1, Label::Hog, 1), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: single_thread_ramp_cpu,
            background: vec![],
        },
        Scenario {
            name: "Mixed threads (1t@12%=96%pc hog + 6t@60%=80%pc legit)",
            processes: vec![sp_t(1, Label::Hog, 1), sp_t(2, Label::NotHog, 6), sp(3, Label::NotHog)],
            duration_ticks: 15, cpu_fn: mixed_thread_cpu,
            background: vec![],
        },
        Scenario {
            name: "Jittered single-thread (11%±2% total, 1 thread)",
            processes: vec![sp_t(1, Label::Hog, 1), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: jittered_single_thread_cpu,
            background: vec![],
        },
        Scenario {
            name: "Dual-core (2t@20%=80%pc + 1t@12.5%=100%pc)",
            processes: vec![sp_t(1, Label::NotHog, 2), sp_t(2, Label::Hog, 1), sp(3, Label::NotHog)],
            duration_ticks: 15, cpu_fn: dual_core_hog_cpu,
            background: vec![],
        },

        // --- Relative CPU scenarios ---
        Scenario {
            name: "Outlier on idle system (40% vs 1-3% bg)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: outlier_idle_cpu,
            background: vec![(101, bg_idle_cpu), (102, bg_idle_cpu), (103, bg_idle_cpu),
                            (104, bg_idle_cpu), (105, bg_idle_cpu)],
        },
        Scenario {
            name: "All procs busy (50% vs 40-50% bg)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: all_busy_cpu,
            background: vec![(101, bg_busy_cpu), (102, bg_busy_cpu), (103, bg_busy_cpu),
                            (104, bg_busy_cpu), (105, bg_busy_cpu)],
        },
        Scenario {
            name: "Slightly above (15% vs 8-10% bg)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: slightly_above_cpu,
            background: vec![(101, bg_medium_cpu), (102, bg_medium_cpu), (103, bg_medium_cpu),
                            (104, bg_medium_cpu), (105, bg_medium_cpu)],
        },
        Scenario {
            name: "Outlier on loaded system (85% vs 20-30% bg)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: outlier_loaded_cpu,
            background: vec![(101, bg_loaded_cpu), (102, bg_loaded_cpu), (103, bg_loaded_cpu),
                            (104, bg_loaded_cpu), (105, bg_loaded_cpu)],
        },
        Scenario {
            name: "Gradual outlier (10→70% vs 5% bg)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: gradual_outlier_cpu,
            background: vec![(101, bg_low_cpu), (102, bg_low_cpu), (103, bg_low_cpu),
                            (104, bg_low_cpu), (105, bg_low_cpu)],
        },
        Scenario {
            name: "Spiky outlier (80%/10% cycle vs 3% bg)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: spiky_outlier_cpu,
            background: vec![(101, bg_quiet_cpu), (102, bg_quiet_cpu), (103, bg_quiet_cpu),
                            (104, bg_quiet_cpu), (105, bg_quiet_cpu)],
        },
        Scenario {
            name: "Two outliers (60%+90% vs 5% bg, only 90% is hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::Hog), sp(3, Label::NotHog)],
            duration_ticks: 15, cpu_fn: two_outliers_cpu,
            background: vec![(101, bg_low_cpu), (102, bg_low_cpu), (103, bg_low_cpu),
                            (104, bg_low_cpu), (105, bg_low_cpu)],
        },
        Scenario {
            name: "Desktop hog (75% vs mixed 2-15% bg)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: desktop_hog_cpu,
            background: vec![(101, bg_desktop_cpu), (102, bg_desktop_cpu), (103, bg_desktop_cpu),
                            (104, bg_desktop_cpu), (105, bg_desktop_cpu),
                            (106, bg_desktop_cpu), (107, bg_desktop_cpu), (108, bg_desktop_cpu)],
        },
        Scenario {
            name: "Moderate outlier (35% vs 15-25% bg, not hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: moderate_outlier_cpu,
            background: vec![(101, bg_moderate_cpu), (102, bg_moderate_cpu), (103, bg_moderate_cpu),
                            (104, bg_moderate_cpu), (105, bg_moderate_cpu)],
        },
        Scenario {
            name: "Jittered relative hog (65%±8% vs 5%±2% bg)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: jittered_rel_hog_cpu,
            background: vec![(101, bg_jittered_cpu), (102, bg_jittered_cpu), (103, bg_jittered_cpu),
                            (104, bg_jittered_cpu), (105, bg_jittered_cpu)],
        },

        // --- New feature-targeting scenarios ---
        Scenario {
            name: "EMA: brief spike (95% 1 tick, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 10, cpu_fn: ema_brief_spike_cpu,
            background: vec![],
        },
        Scenario {
            name: "EMA: gradual ramp (20→90%, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: ema_ramp_up_cpu,
            background: vec![],
        },
        Scenario {
            name: "Grace: new process burst (80% 3s, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 10, cpu_fn: grace_new_process_cpu,
            background: vec![],
        },
        Scenario {
            name: "Repeat offender: oscillator (5 cycles high/low, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: oscillator_repeat_cpu,
            background: vec![],
        },
        Scenario {
            name: "Graduated: sustained heavy (95%, 4× duration, HOG tier2)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: sustained_heavy_cpu,
            background: vec![],
        },
        Scenario {
            name: "Adaptive: high system load (55% target, 85% bg, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_high_load_cpu,
            background: vec![(101, bg_high_load_cpu), (102, bg_high_load_cpu), (103, bg_high_load_cpu),
                            (104, bg_high_load_cpu), (105, bg_high_load_cpu)],
        },
        Scenario {
            name: "Adaptive: low system load (55% target, 15% bg, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_low_load_cpu,
            background: vec![(101, bg_low_load_cpu), (102, bg_low_load_cpu), (103, bg_low_load_cpu),
                            (104, bg_low_load_cpu), (105, bg_low_load_cpu)],
        },
        Scenario {
            name: "Adaptive: saturated system (50% target, 70% bg×8, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_saturated_hog_cpu,
            background: vec![(101, bg_saturated_cpu), (102, bg_saturated_cpu), (103, bg_saturated_cpu),
                            (104, bg_saturated_cpu), (105, bg_saturated_cpu), (106, bg_saturated_cpu),
                            (107, bg_saturated_cpu), (108, bg_saturated_cpu)],
        },
        Scenario {
            name: "Adaptive: medium load (60% target, 40% bg, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_medium_load_cpu,
            background: vec![(101, bg_medium_load_cpu), (102, bg_medium_load_cpu), (103, bg_medium_load_cpu),
                            (104, bg_medium_load_cpu), (105, bg_medium_load_cpu)],
        },
        Scenario {
            name: "Adaptive: build server (45% target, 50% bg, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_build_server_cpu,
            background: vec![(101, bg_build_server_cpu), (102, bg_build_server_cpu), (103, bg_build_server_cpu),
                            (104, bg_build_server_cpu), (105, bg_build_server_cpu)],
        },
        Scenario {
            name: "Adaptive: overloaded + clear hog (75% target, 80% bg, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_overloaded_hog_cpu,
            background: vec![(101, bg_overloaded_cpu), (102, bg_overloaded_cpu), (103, bg_overloaded_cpu),
                            (104, bg_overloaded_cpu), (105, bg_overloaded_cpu)],
        },
        Scenario {
            name: "Adaptive: spike under load (60% 4s on busy system, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 12, cpu_fn: adaptive_spike_under_load_cpu,
            background: vec![(101, bg_moderate_load_cpu), (102, bg_moderate_load_cpu), (103, bg_moderate_load_cpu),
                            (104, bg_moderate_load_cpu), (105, bg_moderate_load_cpu)],
        },
        Scenario {
            name: "Adaptive: ramping load (55% target, bg 20→80%, HOG late)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: adaptive_ramp_load_cpu,
            background: vec![(101, bg_ramp_load_cpu), (102, bg_ramp_load_cpu), (103, bg_ramp_load_cpu),
                            (104, bg_ramp_load_cpu), (105, bg_ramp_load_cpu)],
        },
        Scenario {
            name: "Adaptive: idle system + 60% process (NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_idle_system_cpu,
            background: vec![(101, bg_idle_system_cpu), (102, bg_idle_system_cpu), (103, bg_idle_system_cpu),
                            (104, bg_idle_system_cpu), (105, bg_idle_system_cpu)],
        },
        Scenario {
            name: "Adaptive: desktop mix (55% target, varied 25-55% bg, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: adaptive_desktop_mix_cpu,
            background: vec![(101, bg_desktop_mix_cpu), (102, bg_desktop_mix_cpu), (103, bg_desktop_mix_cpu),
                            (104, bg_desktop_mix_cpu), (105, bg_desktop_mix_cpu), (106, bg_desktop_mix_cpu)],
        },
        Scenario {
            name: "Adaptive: thermal throttle (50% target, bg 25→90%, HOG after load spike)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: adaptive_thermal_cpu,
            background: vec![(101, bg_thermal_cpu), (102, bg_thermal_cpu), (103, bg_thermal_cpu),
                            (104, bg_thermal_cpu), (105, bg_thermal_cpu)],
        },
        Scenario {
            name: "Relapse hog (90→15→92%, repeat offender, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: relapse_hog_cpu,
            background: vec![],
        },

        // --- Multi-feature interaction scenarios ---
        Scenario {
            name: "Combo: grace+EMA+adaptive (new proc ramp on overloaded sys, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: combo_grace_ema_adaptive_cpu,
            background: vec![(101, bg_combo_overloaded_cpu), (102, bg_combo_overloaded_cpu),
                            (103, bg_combo_overloaded_cpu), (104, bg_combo_overloaded_cpu),
                            (105, bg_combo_overloaded_cpu), (106, bg_combo_overloaded_cpu)],
        },
        Scenario {
            name: "Combo: grace+EMA (noisy startup then sustained 85%, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: combo_grace_ema_startup_cpu,
            background: vec![],
        },
        Scenario {
            name: "Combo: repeat+graduated (4on/3off oscillator, 30s, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 30, cpu_fn: combo_repeat_graduated_cpu,
            background: vec![],
        },
        Scenario {
            name: "Combo: repeat+EMA (2on/1off rapid oscillator, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 25, cpu_fn: combo_repeat_ema_rapid_cpu,
            background: vec![],
        },
        Scenario {
            name: "Combo: adaptive+per-core (1t@10%=80%pc, overloaded sys, HOG)",
            processes: vec![sp_t(1, Label::Hog, 1), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: combo_adaptive_percore_cpu,
            background: vec![(101, bg_combo_percore_load_cpu), (102, bg_combo_percore_load_cpu),
                            (103, bg_combo_percore_load_cpu), (104, bg_combo_percore_load_cpu),
                            (105, bg_combo_percore_load_cpu), (106, bg_combo_percore_load_cpu)],
        },
        Scenario {
            name: "Combo: adaptive+per-core borderline (2t@18%=72%pc, busy sys, HOG)",
            processes: vec![sp_t(1, Label::Hog, 2), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: combo_adaptive_percore_borderline_cpu,
            background: vec![(101, bg_combo_percore_busy_cpu), (102, bg_combo_percore_busy_cpu),
                            (103, bg_combo_percore_busy_cpu), (104, bg_combo_percore_busy_cpu),
                            (105, bg_combo_percore_busy_cpu)],
        },
        Scenario {
            name: "Combo: adaptive+per-core idle (1t@8%=64%pc, idle sys, NOT hog)",
            processes: vec![sp_t(1, Label::NotHog, 1), sp(2, Label::NotHog)],
            duration_ticks: 15, cpu_fn: combo_adaptive_percore_idle_cpu,
            background: vec![(101, bg_combo_percore_idle_cpu), (102, bg_combo_percore_idle_cpu),
                            (103, bg_combo_percore_idle_cpu), (104, bg_combo_percore_idle_cpu),
                            (105, bg_combo_percore_idle_cpu)],
        },
        Scenario {
            name: "Combo: ALL features (grace+EMA+adaptive+repeat+grad, overloaded, HOG)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 30, cpu_fn: combo_all_features_cpu,
            background: vec![(101, bg_combo_all_features_cpu), (102, bg_combo_all_features_cpu),
                            (103, bg_combo_all_features_cpu), (104, bg_combo_all_features_cpu),
                            (105, bg_combo_all_features_cpu), (106, bg_combo_all_features_cpu)],
        },
        Scenario {
            name: "Combo: ALL features negative (grace+idle sys, brief activity, NOT hog)",
            processes: vec![sp(1, Label::NotHog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: combo_all_features_not_hog_cpu,
            background: vec![(101, bg_combo_idle_cpu), (102, bg_combo_idle_cpu),
                            (103, bg_combo_idle_cpu), (104, bg_combo_idle_cpu),
                            (105, bg_combo_idle_cpu)],
        },
        Scenario {
            name: "Combo: graduated+adaptive (50% sustained on loaded sys, HOG tier2)",
            processes: vec![sp(1, Label::Hog), sp(2, Label::NotHog)],
            duration_ticks: 20, cpu_fn: combo_graduated_adaptive_cpu,
            background: vec![(101, bg_combo_graduated_load_cpu), (102, bg_combo_graduated_load_cpu),
                            (103, bg_combo_graduated_load_cpu), (104, bg_combo_graduated_load_cpu),
                            (105, bg_combo_graduated_load_cpu), (106, bg_combo_graduated_load_cpu)],
        },
    ]
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

struct EvalResult {
    config: DetectionConfig,
    true_positives: u32,
    false_positives: u32,
    missed: u32,
    total_hogs: u32,
    total_non_hogs: u32,
    reaction_times: Vec<f64>,
    score: f64,
}

impl EvalResult {
    fn tp_rate(&self) -> f64 {
        if self.total_hogs == 0 { 0.0 } else { self.true_positives as f64 / self.total_hogs as f64 }
    }
    fn fp_rate(&self) -> f64 {
        if self.total_non_hogs == 0 { 0.0 } else { self.false_positives as f64 / self.total_non_hogs as f64 }
    }
    fn miss_rate(&self) -> f64 {
        if self.total_hogs == 0 { 0.0 } else { self.missed as f64 / self.total_hogs as f64 }
    }
    fn mean_reaction(&self) -> f64 {
        if self.reaction_times.is_empty() { 0.0 }
        else { self.reaction_times.iter().sum::<f64>() / self.reaction_times.len() as f64 }
    }
    fn p95_reaction(&self) -> f64 {
        if self.reaction_times.is_empty() { return 0.0; }
        let mut sorted = self.reaction_times.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = ((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1);
        sorted[idx]
    }
}

fn evaluate(config: DetectionConfig, all_scenarios: &[Scenario], max_duration: f64) -> EvalResult {
    let mut tp = 0u32;
    let mut fp = 0u32;
    let mut missed = 0u32;
    let mut total_hogs = 0u32;
    let mut total_non_hogs = 0u32;
    let mut reaction_times: Vec<f64> = Vec::new();

    for scenario in all_scenarios {
        let mut engine = SimEngine::new();

        for tick in 0..scenario.duration_ticks {
            // Build full process snapshot including background procs
            let mut procs: Vec<SimProcess> = scenario.processes.iter()
                .map(|p| SimProcess {
                    pid: p.pid,
                    cpu: (scenario.cpu_fn)(p.pid, tick),
                    threads_used: p.threads_used,
                })
                .collect();

            // Add background procs (for relative mode median calculation)
            for &(bg_pid, bg_fn) in &scenario.background {
                procs.push(SimProcess {
                    pid: bg_pid,
                    cpu: bg_fn(bg_pid, tick),
                    threads_used: 4,
                });
            }

            engine.tick(tick, &procs, &config);
        }

        let demoted_pids: HashMap<u32, &DemotionEvent> = engine.demotions.iter()
            .map(|e| (e.pid, e))
            .collect();

        for p in &scenario.processes {
            match p.label {
                Label::Hog => {
                    total_hogs += 1;
                    if let Some(event) = demoted_pids.get(&p.pid) {
                        tp += 1;
                        reaction_times.push((event.tick - event.first_seen_tick) as f64);
                    } else {
                        missed += 1;
                    }
                }
                Label::NotHog => {
                    total_non_hogs += 1;
                    if demoted_pids.contains_key(&p.pid) {
                        fp += 1;
                    }
                }
            }
        }

        // Background procs are NOT scored — they don't have labels
    }

    let tp_rate = if total_hogs == 0 { 0.0 } else { tp as f64 / total_hogs as f64 };
    let fp_rate = if total_non_hogs == 0 { 0.0 } else { fp as f64 / total_non_hogs as f64 };
    let miss_rate = if total_hogs == 0 { 0.0 } else { missed as f64 / total_hogs as f64 };
    let mean_reaction = if reaction_times.is_empty() { 0.0 }
        else { reaction_times.iter().sum::<f64>() / reaction_times.len() as f64 };
    let norm_reaction = mean_reaction / max_duration;

    let score = (tp_rate * 40.0)
        - (fp_rate * 35.0)
        - (miss_rate * 25.0)
        - (norm_reaction * 10.0);

    EvalResult {
        config,
        true_positives: tp,
        false_positives: fp,
        missed,
        total_hogs,
        total_non_hogs,
        reaction_times,
        score,
    }
}

// ---------------------------------------------------------------------------
// Grid search helpers
// ---------------------------------------------------------------------------

fn print_table(title: &str, results: &[EvalResult], top_n: usize) {
    println!("\n## {}\n", title);
    match results[0].config.mode {
        DetectionMode::Absolute => {
            println!("| Rank | Threshold | Duration | Score | TP | FP | Miss | Avg React | p95 React |");
            println!("|------|-----------|----------|-------|-----|-----|------|-----------|-----------|");
        }
        DetectionMode::PerCore => {
            println!("| Rank | PC Thresh | Duration | Score | TP | FP | Miss | Avg React | p95 React |");
            println!("|------|-----------|----------|-------|-----|-----|------|-----------|-----------|");
        }
        DetectionMode::Relative => {
            println!("| Rank | Multiplier | Duration | Score | TP | FP | Miss | Avg React | p95 React |");
            println!("|------|------------|----------|-------|-----|-----|------|-----------|-----------|");
        }
        DetectionMode::Hybrid => {
            println!("| Rank | AbsT | PCT | RelM | Dur | Score | TP | FP | Miss | Avg React |");
            println!("|------|------|-----|------|-----|-------|-----|-----|------|-----------|");
        }
    }

    for (i, r) in results.iter().take(top_n).enumerate() {
        let react = if r.reaction_times.is_empty() { "—".into() }
            else { format!("{:.1}s", r.mean_reaction()) };
        let p95 = if r.reaction_times.is_empty() { "—".into() }
            else { format!("{:.1}s", r.p95_reaction()) };

        match r.config.mode {
            DetectionMode::Absolute => {
                println!(
                    "| {} | {}% | {}s | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} | {} |",
                    i + 1, r.config.abs_threshold, r.config.duration, r.score,
                    r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0,
                    react, p95
                );
            }
            DetectionMode::PerCore => {
                println!(
                    "| {} | {}% | {}s | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} | {} |",
                    i + 1, r.config.per_core_threshold, r.config.duration, r.score,
                    r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0,
                    react, p95
                );
            }
            DetectionMode::Relative => {
                println!(
                    "| {} | {}x | {}s | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} | {} |",
                    i + 1, r.config.relative_multiplier, r.config.duration, r.score,
                    r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0,
                    react, p95
                );
            }
            DetectionMode::Hybrid => {
                println!(
                    "| {} | {}% | {}% | {}x | {}s | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} |",
                    i + 1, r.config.abs_threshold, r.config.per_core_threshold,
                    r.config.relative_multiplier, r.config.duration, r.score,
                    r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0,
                    react
                );
            }
        }
    }
}

fn default_config(mode: DetectionMode) -> DetectionConfig {
    DetectionConfig {
        mode,
        abs_threshold: 80.0,
        duration: 2,
        per_core_threshold: 90.0,
        core_count: 8,
        relative_multiplier: 5.0,
        ema_alpha: 1.0,           // 1.0 = no smoothing (backward compat)
        grace_period: 0,          // 0 = disabled
        repeat_offender: false,   // off by default for legacy sweeps
        adaptive_sensitivity: 0.0, // 0 = disabled for legacy sweeps
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let all_scenarios = scenarios();

    let max_duration = all_scenarios.iter()
        .map(|s| s.duration_ticks)
        .max()
        .unwrap_or(1) as f64;

    println!("# PriorityGuard Configuration Space Search (Multi-Mode)\n");

    // Print scenario summary
    println!("## Scenarios ({} total)\n", all_scenarios.len());
    for s in &all_scenarios {
        let hog_count = s.processes.iter().filter(|p| p.label == Label::Hog).count();
        let bg_count = s.background.len();
        let bg_str = if bg_count > 0 { format!(" + {} bg procs", bg_count) } else { String::new() };
        println!("- **{}** — {} hog(s), {}s{}", s.name, hog_count, s.duration_ticks, bg_str);
    }

    println!("\n## Fitness Function\n");
    println!("`score = (TP_rate × 40) - (FP_rate × 35) - (miss_rate × 25) - (norm_reaction × 10)`\n");

    // -----------------------------------------------------------------------
    // Sweep 1: Absolute (existing)
    // -----------------------------------------------------------------------
    let abs_thresholds = vec![30.0, 40.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0, 95.0];
    let durations = vec![1, 2, 3, 4, 5, 7, 10, 15];

    let mut abs_results: Vec<EvalResult> = Vec::new();
    for &t in &abs_thresholds {
        for &d in &durations {
            let mut cfg = default_config(DetectionMode::Absolute);
            cfg.abs_threshold = t;
            cfg.duration = d;
            abs_results.push(evaluate(cfg, &all_scenarios, max_duration));
        }
    }
    abs_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    print_table("Absolute Mode (CPU% threshold)", &abs_results, 10);

    let _best_abs_threshold = abs_results[0].config.abs_threshold;

    // -----------------------------------------------------------------------
    // Sweep 2: Per-Core
    // -----------------------------------------------------------------------
    let pc_thresholds = vec![50.0, 60.0, 70.0, 75.0, 80.0, 85.0, 90.0, 95.0];

    let mut pc_results: Vec<EvalResult> = Vec::new();
    for &t in &pc_thresholds {
        for &d in &durations {
            let mut cfg = default_config(DetectionMode::PerCore);
            cfg.per_core_threshold = t;
            cfg.duration = d;
            pc_results.push(evaluate(cfg, &all_scenarios, max_duration));
        }
    }
    pc_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    print_table("Per-Core Mode (max single-core CPU%)", &pc_results, 10);

    let _best_pc_threshold = pc_results[0].config.per_core_threshold;

    // -----------------------------------------------------------------------
    // Sweep 3: Relative
    // -----------------------------------------------------------------------
    let multipliers = vec![3.0, 5.0, 8.0, 10.0, 15.0, 20.0];

    let mut rel_results: Vec<EvalResult> = Vec::new();
    for &m in &multipliers {
        for &d in &durations {
            let mut cfg = default_config(DetectionMode::Relative);
            cfg.relative_multiplier = m;
            cfg.duration = d;
            rel_results.push(evaluate(cfg, &all_scenarios, max_duration));
        }
    }
    rel_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    print_table("Relative Mode (CPU vs median × multiplier)", &rel_results, 10);

    let _best_rel_multiplier = rel_results[0].config.relative_multiplier;

    // -----------------------------------------------------------------------
    // Sweep 4: Hybrid (constrained search around best individual params)
    // -----------------------------------------------------------------------
    // Take top-3 unique values from each individual sweep
    let top_abs: Vec<f32> = {
        let mut seen = Vec::new();
        for r in &abs_results {
            if !seen.contains(&r.config.abs_threshold) { seen.push(r.config.abs_threshold); }
            if seen.len() >= 3 { break; }
        }
        seen
    };
    let top_pc: Vec<f32> = {
        let mut seen = Vec::new();
        for r in &pc_results {
            if !seen.contains(&r.config.per_core_threshold) { seen.push(r.config.per_core_threshold); }
            if seen.len() >= 3 { break; }
        }
        seen
    };
    let top_rel: Vec<f32> = {
        let mut seen = Vec::new();
        for r in &rel_results {
            if !seen.contains(&r.config.relative_multiplier) { seen.push(r.config.relative_multiplier); }
            if seen.len() >= 3 { break; }
        }
        seen
    };
    let hybrid_durations = vec![1, 2, 3];

    let mut hybrid_results: Vec<EvalResult> = Vec::new();
    for &at in &top_abs {
        for &pt in &top_pc {
            for &rm in &top_rel {
                for &d in &hybrid_durations {
                    let mut cfg = default_config(DetectionMode::Hybrid);
                    cfg.abs_threshold = at;
                    cfg.duration = d;
                    cfg.per_core_threshold = pt;
                    cfg.relative_multiplier = rm;
                    hybrid_results.push(evaluate(cfg, &all_scenarios, max_duration));
                }
            }
        }
    }
    hybrid_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    print_table("Hybrid Mode (OR of Absolute + Per-Core + Relative)", &hybrid_results, 10);

    // -----------------------------------------------------------------------
    // Sweep 5: EMA Alpha (on top of best hybrid)
    // -----------------------------------------------------------------------
    let best_hybrid = &hybrid_results[0].config;
    let ema_alphas = vec![0.1, 0.2, 0.3, 0.5, 0.7, 1.0];

    let mut ema_results: Vec<EvalResult> = Vec::new();
    for &alpha in &ema_alphas {
        let mut cfg = best_hybrid.clone();
        cfg.ema_alpha = alpha;
        ema_results.push(evaluate(cfg, &all_scenarios, max_duration));
    }
    ema_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    println!("\n## EMA Alpha Sweep (best hybrid base)\n");
    println!("| Rank | Alpha | Score | TP | FP | Miss | Avg React |");
    println!("|------|-------|-------|-----|-----|------|-----------|");
    for (i, r) in ema_results.iter().take(6).enumerate() {
        let react = if r.reaction_times.is_empty() { "—".into() }
            else { format!("{:.1}s", r.mean_reaction()) };
        println!("| {} | {} | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} |",
            i + 1, r.config.ema_alpha, r.score,
            r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0, react);
    }

    let best_ema = ema_results[0].config.ema_alpha;

    // -----------------------------------------------------------------------
    // Sweep 6: Grace Period
    // -----------------------------------------------------------------------
    let grace_periods = vec![0, 1, 2, 3, 5, 10];

    let mut grace_results: Vec<EvalResult> = Vec::new();
    for &gp in &grace_periods {
        let mut cfg = best_hybrid.clone();
        cfg.ema_alpha = best_ema;
        cfg.grace_period = gp;
        grace_results.push(evaluate(cfg, &all_scenarios, max_duration));
    }
    grace_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    println!("\n## Grace Period Sweep\n");
    println!("| Rank | Grace | Score | TP | FP | Miss | Avg React |");
    println!("|------|-------|-------|-----|-----|------|-----------|");
    for (i, r) in grace_results.iter().take(6).enumerate() {
        let react = if r.reaction_times.is_empty() { "—".into() }
            else { format!("{:.1}s", r.mean_reaction()) };
        println!("| {} | {}s | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} |",
            i + 1, r.config.grace_period, r.score,
            r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0, react);
    }

    let best_grace = grace_results[0].config.grace_period;

    // -----------------------------------------------------------------------
    // Sweep 7: Repeat Offender toggle
    // -----------------------------------------------------------------------
    let mut ro_results: Vec<EvalResult> = Vec::new();
    for &ro in &[false, true] {
        let mut cfg = best_hybrid.clone();
        cfg.ema_alpha = best_ema;
        cfg.grace_period = best_grace;
        cfg.repeat_offender = ro;
        ro_results.push(evaluate(cfg, &all_scenarios, max_duration));
    }
    ro_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    println!("\n## Repeat Offender Toggle\n");
    println!("| Enabled | Score | TP | FP | Miss | Avg React |");
    println!("|---------|-------|-----|-----|------|-----------|");
    for r in &ro_results {
        let react = if r.reaction_times.is_empty() { "—".into() }
            else { format!("{:.1}s", r.mean_reaction()) };
        println!("| {} | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} |",
            r.config.repeat_offender, r.score,
            r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0, react);
    }

    let best_ro = ro_results[0].config.repeat_offender;

    // -----------------------------------------------------------------------
    // Sweep 8: Adaptive Sensitivity
    // -----------------------------------------------------------------------
    let sensitivities = vec![0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8, 1.0, 1.5, 2.0];

    let mut adaptive_results: Vec<EvalResult> = Vec::new();
    for &sens in &sensitivities {
        let mut cfg = best_hybrid.clone();
        cfg.ema_alpha = best_ema;
        cfg.grace_period = best_grace;
        cfg.repeat_offender = best_ro;
        cfg.adaptive_sensitivity = sens;
        adaptive_results.push(evaluate(cfg, &all_scenarios, max_duration));
    }
    adaptive_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    println!("\n## Adaptive Sensitivity Sweep\n");
    println!("| Rank | Sensitivity | Score | TP | FP | Miss | Avg React |");
    println!("|------|-------------|-------|-----|-----|------|-----------|");
    for (i, r) in adaptive_results.iter().take(10).enumerate() {
        let react = if r.reaction_times.is_empty() { "—".into() }
            else { format!("{:.1}s", r.mean_reaction()) };
        println!("| {} | {:.1} | {:.2} | {:.0}% | {:.0}% | {:.0}% | {} |",
            i + 1, r.config.adaptive_sensitivity, r.score,
            r.tp_rate() * 100.0, r.fp_rate() * 100.0, r.miss_rate() * 100.0, react);
    }

    // -----------------------------------------------------------------------
    // Final combined best config
    // -----------------------------------------------------------------------
    let best_adaptive = &adaptive_results[0].config;
    println!("\n## Final Best Configuration\n");
    println!("| Parameter | Value |");
    println!("|-----------|-------|");
    println!("| Abs Threshold | {}% |", best_adaptive.abs_threshold);
    println!("| Per-Core Threshold | {}% |", best_adaptive.per_core_threshold);
    println!("| Relative Multiplier | {}x |", best_adaptive.relative_multiplier);
    println!("| Duration | {}s |", best_adaptive.duration);
    println!("| EMA Alpha | {} |", best_adaptive.ema_alpha);
    println!("| Grace Period | {}s |", best_adaptive.grace_period);
    println!("| Repeat Offender | {} |", best_adaptive.repeat_offender);
    println!("| Adaptive Sensitivity | {} |", best_adaptive.adaptive_sensitivity);
    println!("\nFinal Score: {:.2}", adaptive_results[0].score);

    // -----------------------------------------------------------------------
    // Legacy Summary (original 4 modes)
    // -----------------------------------------------------------------------
    println!("\n## Legacy Mode Summary\n");
    println!("| Mode | Best Params | Score | TP | FP | Miss |");
    println!("|------|-------------|-------|-----|-----|------|");
    for (name, results) in [
        ("Absolute", &abs_results),
        ("Per-Core", &pc_results),
        ("Relative", &rel_results),
        ("Hybrid", &hybrid_results),
    ] {
        let b = &results[0];
        let params = match b.config.mode {
            DetectionMode::Absolute => format!("threshold={}%, dur={}s", b.config.abs_threshold, b.config.duration),
            DetectionMode::PerCore => format!("pc_threshold={}%, dur={}s", b.config.per_core_threshold, b.config.duration),
            DetectionMode::Relative => format!("multiplier={}x, dur={}s", b.config.relative_multiplier, b.config.duration),
            DetectionMode::Hybrid => format!("abs={}%, pc={}%, rel={}x, dur={}s",
                b.config.abs_threshold, b.config.per_core_threshold,
                b.config.relative_multiplier, b.config.duration),
        };
        println!(
            "| {} | {} | {:.2} | {:.0}% | {:.0}% | {:.0}% |",
            name, params, b.score,
            b.tp_rate() * 100.0, b.fp_rate() * 100.0, b.miss_rate() * 100.0
        );
    }
}
