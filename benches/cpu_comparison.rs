//! CPU comparison benchmark: ProcWarden vs Task Manager
//!
//! Measures CPU overhead of both processes under idle, moderate, and heavy load.
//! Must be run as administrator (ETW requires elevation).
//!
//! Usage: cargo build --release && target\release\cpu_comparison.exe

use std::os::windows::process::CommandExt;
use std::process::{Command, Child};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System, ProcessRefreshKind};

const CREATE_NEW_CONSOLE: u32 = 0x00000010;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
const WARMUP_SAMPLES: usize = 20; // 10 seconds warmup per scenario
const MEASURE_SAMPLES: usize = 60; // 30 seconds of measurement per scenario
const PROCWARDEN_EXE: &str = "target/release/procwarden.exe";

struct Stats {
    avg: f32,
    median: f32,
    min: f32,
    max: f32,
    p95: f32,
}

fn compute_stats(samples: &[f32]) -> Stats {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = sorted.len();
    Stats {
        avg: sorted.iter().sum::<f32>() / len as f32,
        median: sorted[len / 2],
        min: sorted[0],
        max: sorted[len - 1],
        p95: sorted[(len as f32 * 0.95) as usize],
    }
}

struct ScenarioResult {
    name: &'static str,
    pl: Stats,
    tm: Stats,
}

fn find_pid_by_name(system: &System, name: &str) -> Option<u32> {
    let name_lower = name.to_ascii_lowercase();
    system.processes().iter()
        .find(|(_, p)| p.name().to_string_lossy().to_ascii_lowercase() == name_lower)
        .map(|(pid, _)| pid.as_u32())
}

/// Sample CPU for two PIDs in one refresh call, returning raw per-core percentages.
fn sample_cpu_pair(system: &mut System, pid_a: u32, pid_b: u32) -> (f32, f32) {
    let a = sysinfo::Pid::from_u32(pid_a);
    let b = sysinfo::Pid::from_u32(pid_b);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[a, b]),
        ProcessRefreshKind::new().with_cpu(),
    );
    let cpu_a = system.process(a).map(|p| p.cpu_usage()).unwrap_or(0.0);
    let cpu_b = system.process(b).map(|p| p.cpu_usage()).unwrap_or(0.0);
    (cpu_a, cpu_b)
}

fn run_scenario(
    system: &mut System,
    pl_pid: u32,
    tm_pid: u32,
    num_cpus: f32,
    name: &'static str,
    stress_threads: usize,
) -> ScenarioResult {
    println!("  Running scenario: {} ({} stress threads)...", name, stress_threads);

    // Spawn stress workers
    let running = Arc::new(AtomicBool::new(true));
    let workers: Vec<_> = (0..stress_threads)
        .map(|_| {
            let flag = running.clone();
            thread::spawn(move || {
                while flag.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();

    // Warmup: let CPU settle
    for _ in 0..WARMUP_SAMPLES {
        sample_cpu_pair(system, pl_pid, tm_pid);
        thread::sleep(SAMPLE_INTERVAL);
    }

    // Measure
    let mut pl_samples = Vec::with_capacity(MEASURE_SAMPLES);
    let mut tm_samples = Vec::with_capacity(MEASURE_SAMPLES);

    for _ in 0..MEASURE_SAMPLES {
        thread::sleep(SAMPLE_INTERVAL);
        let (pl, tm) = sample_cpu_pair(system, pl_pid, tm_pid);
        // Store normalized (system-wide %) values
        pl_samples.push(pl / num_cpus);
        tm_samples.push(tm / num_cpus);
    }

    // Stop stress workers
    running.store(false, Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }

    // Print raw sample summary for debugging
    let pl_raw_avg: f32 = pl_samples.iter().sum::<f32>() / pl_samples.len() as f32;
    let tm_raw_avg: f32 = tm_samples.iter().sum::<f32>() / tm_samples.len() as f32;
    let pl_nonzero = pl_samples.iter().filter(|&&x| x > 0.001).count();
    let tm_nonzero = tm_samples.iter().filter(|&&x| x > 0.001).count();
    println!("    PL: avg={:.4}%, non-zero={}/{}", pl_raw_avg, pl_nonzero, MEASURE_SAMPLES);
    println!("    TM: avg={:.4}%, non-zero={}/{}", tm_raw_avg, tm_nonzero, MEASURE_SAMPLES);

    ScenarioResult {
        name,
        pl: compute_stats(&pl_samples),
        tm: compute_stats(&tm_samples),
    }
}

fn main() {
    println!("CPU Comparison Benchmark: ProcWarden vs Task Manager");
    println!("=======================================================\n");

    // Ensure release build exists
    println!("Building procwarden in release mode...");
    let build = Command::new("cargo")
        .args(["build", "--release"])
        .status()
        .expect("Failed to run cargo build");
    if !build.success() {
        eprintln!("Release build failed!");
        std::process::exit(1);
    }

    // Initialize sysinfo
    let mut system = System::new();
    system.refresh_cpu_all();
    system.refresh_processes(ProcessesToUpdate::All);

    let num_cpus = system.cpus().len().max(1) as f32;
    println!("System: {} logical CPUs\n", num_cpus as usize);

    // Launch Task Manager if not running
    let tm_pid = if let Some(pid) = find_pid_by_name(&system, "Taskmgr.exe") {
        println!("Task Manager already running (PID {})", pid);
        pid
    } else {
        println!("Launching Task Manager...");
        #[allow(clippy::zombie_processes)] // Intentionally fire-and-forget; Task Manager runs independently
        let _ = Command::new("C:\\Windows\\System32\\Taskmgr.exe")
            .spawn()
            .expect("Failed to launch Task Manager");
        thread::sleep(Duration::from_secs(3));
        system.refresh_processes(ProcessesToUpdate::All);
        find_pid_by_name(&system, "Taskmgr.exe")
            .expect("Task Manager not found after launch")
    };
    println!("Task Manager PID: {}", tm_pid);

    // Launch ProcWarden
    println!("Launching ProcWarden...");
    let mut pl_child: Child = Command::new(PROCWARDEN_EXE)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .expect("Failed to launch procwarden.exe — did you build release?");
    let pl_pid = pl_child.id();
    println!("ProcWarden PID: {}", pl_pid);

    // Verify both processes are visible
    thread::sleep(Duration::from_secs(2));
    system.refresh_processes(ProcessesToUpdate::All);
    let pl_visible = system.process(sysinfo::Pid::from_u32(pl_pid)).is_some();
    let tm_visible = system.process(sysinfo::Pid::from_u32(tm_pid)).is_some();
    println!("ProcWarden visible: {}, Task Manager visible: {}", pl_visible, tm_visible);

    if !pl_visible {
        eprintln!("ERROR: ProcWarden not visible to sysinfo! Aborting.");
        let _ = pl_child.kill();
        std::process::exit(1);
    }

    // Let both settle
    println!("\nWarming up (15s)...\n");
    thread::sleep(Duration::from_secs(15));
    system.refresh_processes(ProcessesToUpdate::All);

    let moderate_threads = (num_cpus as usize / 2).max(1);
    let heavy_threads = num_cpus as usize;

    // Run scenarios
    let results = vec![
        run_scenario(&mut system, pl_pid, tm_pid, num_cpus, "Idle", 0),
        run_scenario(&mut system, pl_pid, tm_pid, num_cpus, "Moderate", moderate_threads),
        run_scenario(&mut system, pl_pid, tm_pid, num_cpus, "Heavy", heavy_threads),
    ];

    // Cleanup
    println!("\nCleaning up...");
    let _ = pl_child.kill();
    let _ = pl_child.wait();

    // Print results
    println!("\n## Summary\n");
    println!("| {:<8} | {:<11} | {:>12} | {:>12} | {:>6} |", "Scenario", "Metric", "ProcWarden", "TaskMgr", "Ratio");
    println!("|{:-<10}|{:-<13}|{:-<14}|{:-<14}|{:-<8}|", "", "", "", "", "");
    for r in &results {
        let ratio_avg = if r.tm.avg > 0.001 {
            format!("{:.2}x", r.pl.avg / r.tm.avg)
        } else if r.pl.avg > 0.001 {
            "TM~0".to_string()
        } else {
            "both~0".to_string()
        };
        let ratio_p95 = if r.tm.p95 > 0.001 {
            format!("{:.2}x", r.pl.p95 / r.tm.p95)
        } else {
            "-".to_string()
        };
        let pl_minmax = format!("{:.4}-{:.4}%", r.pl.min, r.pl.max);
        let tm_minmax = format!("{:.4}-{:.4}%", r.tm.min, r.tm.max);
        println!("| {:<8} | {:<11} | {:>12} | {:>12} | {:>6} |", r.name, "avg", format!("{:.4}%", r.pl.avg), format!("{:.4}%", r.tm.avg), ratio_avg);
        println!("| {:<8} | {:<11} | {:>12} | {:>12} | {:>6} |", "", "median", format!("{:.4}%", r.pl.median), format!("{:.4}%", r.tm.median), "");
        println!("| {:<8} | {:<11} | {:>12} | {:>12} | {:>6} |", "", "p95", format!("{:.4}%", r.pl.p95), format!("{:.4}%", r.tm.p95), ratio_p95);
        println!("| {:<8} | {:<11} | {:>12} | {:>12} | {:>6} |", "", "min-max", pl_minmax, tm_minmax, "");
    }

    println!("\nConfig: sample={}ms, measure={} samples ({}s), warmup={} samples ({}s)",
        SAMPLE_INTERVAL.as_millis(),
        MEASURE_SAMPLES, MEASURE_SAMPLES as u64 * SAMPLE_INTERVAL.as_millis() as u64 / 1000,
        WARMUP_SAMPLES, WARMUP_SAMPLES as u64 * SAMPLE_INTERVAL.as_millis() as u64 / 1000);
}
