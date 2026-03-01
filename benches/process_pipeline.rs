use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use procwarden::common::{ProcessInfo, ProcessListState, SortColumn};
use procwarden::process_list::{filter_processes, format_memory, sort_processes};

fn make_procs(n: usize) -> Vec<ProcessInfo> {
    (0..n)
        .map(|i| ProcessInfo {
            pid: i as u32,
            name: format!("process-{}", i),
            cpu: (i as f32) * 0.5,
            memory: (i as u64) * 1024 * 1024,
            exe_path: None,
        })
        .collect()
}

fn bench_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("sort_processes");
    for size in [100, 500, 1000] {
        let procs = make_procs(size);
        group.bench_with_input(BenchmarkId::new("by_cpu", size), &procs, |b, procs| {
            b.iter(|| sort_processes(black_box(procs.clone()), SortColumn::Cpu, false))
        });
        group.bench_with_input(BenchmarkId::new("by_name", size), &procs, |b, procs| {
            b.iter(|| sort_processes(black_box(procs.clone()), SortColumn::Name, true))
        });
        group.bench_with_input(BenchmarkId::new("by_pid", size), &procs, |b, procs| {
            b.iter(|| sort_processes(black_box(procs.clone()), SortColumn::Pid, true))
        });
        group.bench_with_input(BenchmarkId::new("by_memory", size), &procs, |b, procs| {
            b.iter(|| sort_processes(black_box(procs.clone()), SortColumn::Memory, false))
        });
    }
    group.finish();
}

fn bench_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_processes");
    for size in [100, 500, 1000] {
        let procs = make_procs(size);
        group.bench_with_input(
            BenchmarkId::new("hide_self_off", size),
            &procs,
            |b, procs| b.iter(|| filter_processes(black_box(procs.clone()), false, "")),
        );
        group.bench_with_input(
            BenchmarkId::new("hide_self_on", size),
            &procs,
            |b, procs| b.iter(|| filter_processes(black_box(procs.clone()), true, "")),
        );
    }
    group.finish();
}

fn bench_format_memory(c: &mut Criterion) {
    c.bench_function("format_memory", |b| {
        b.iter(|| format_memory(black_box(1_572_864)))
    });
}

fn bench_rebuild_formatted_rows(c: &mut Criterion) {
    let mut group = c.benchmark_group("rebuild_formatted_rows");
    for size in [100, 500, 1000] {
        let procs = make_procs(size);
        // First call (cold — allocates strings)
        group.bench_with_input(BenchmarkId::new("cold", size), &procs, |b, procs| {
            b.iter(|| {
                let mut state = ProcessListState::new();
                state.processes = procs.clone();
                state.rebuild_formatted_rows();
                black_box(&state.formatted_rows);
            })
        });
        // Subsequent calls (warm — reuses buffers)
        group.bench_with_input(BenchmarkId::new("warm", size), &procs, |b, procs| {
            let mut state = ProcessListState::new();
            state.processes = procs.clone();
            state.rebuild_formatted_rows();
            b.iter(|| {
                state.rebuild_formatted_rows();
                black_box(&state.formatted_rows);
            })
        });
    }
    group.finish();
}

fn bench_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_pipeline");
    for size in [100, 500, 1000] {
        let procs = make_procs(size);
        group.bench_with_input(
            BenchmarkId::new("sort_filter_format", size),
            &procs,
            |b, procs| {
                let mut state = ProcessListState::new();
                b.iter(|| {
                    let filtered = filter_processes(procs.clone(), true, "");
                    let sorted = sort_processes(filtered, SortColumn::Cpu, false);
                    state.processes = sorted;
                    state.rebuild_formatted_rows();
                    black_box(&state.formatted_rows);
                })
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_sort,
    bench_filter,
    bench_format_memory,
    bench_rebuild_formatted_rows,
    bench_full_pipeline,
);
criterion_main!(benches);
