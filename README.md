# Process Warden

Process Warden is a Windows process management tool that provides a terminal-based interface to monitor, manage, and control system processes.

## Features

- Real-time process monitoring with CPU and memory usage tracking
- Process sorting by PID, Name, CPU, or Memory
- Process termination, suspension, and resumption
- Process priority management (Idle through Realtime)
- **PriorityGuard** — automatic CPU hog detection and priority demotion with configurable thresholds, grace periods, and demotion tiers
- Live process name filtering
- Action log viewer
- Configurable settings (refresh interval, PriorityGuard tuning, and more)
- ETW (Event Tracing for Windows) support with automatic fallback to polling

### Planned

- CPU affinity control

## Building

ETW support is enabled by default:

```bash
cargo build --release
```

To build without ETW (polling-only):

```bash
cargo build --no-default-features --release
```

The binary will be at `target/release/procwarden.exe`

## Running

```bash
cargo run
# or run the release binary directly
./target/release/procwarden.exe
```

When built with ETW, the application automatically attempts to use Event Tracing for Windows for real-time process monitoring. If ETW fails to initialize (e.g., due to insufficient permissions), it gracefully falls back to polling.

You can force polling mode even when ETW is available:

```bash
cargo run -- --disable-etw
```

## Controls

### Process List

| Key | Action |
| ----- | -------- |
| **↑/↓** or **K/J** | Navigate process list |
| **←/→** | Change sort column (PID, Name, CPU, Memory) |
| **Space** | Toggle sort direction |
| **Tab** or **/** | Filter processes by name |
| **X** or **Delete** | Terminate process (with confirmation) |
| **Z** | Suspend / resume process |
| **P** | Set process priority |
| **S** | Open settings menu |
| **L** | Open log viewer |
| **Q** | Quit |

### Settings Menu

| Key | Action |
| ----- | -------- |
| **↑/↓** or **K/J** | Navigate settings |
| **←/→** | Adjust selected setting |
| **S** / **Esc** | Return to process list |

### Log Viewer

| Key | Action |
| ----- | -------- |
| **↑/↓** or **K/J** | Scroll log |
| **Home/End** | Jump to start / end |
| **L** / **Esc** | Return to process list |

## Settings

| Setting | Range | Default | Description |
| --------- | ------- | --------- | ------------- |
| Poll Interval | 50–10000 ms | 2000 ms | Process data refresh frequency |
| Hide Self | on/off | — | Hide ProcWarden from the process list |
| PriorityGuard Enabled | on/off | on | Automatic CPU hog detection |
| CPU Threshold | 10–100% | 65% | Absolute CPU usage threshold |
| Duration | 1–30 s | 2 s | Time a process must exceed the threshold |
| Per-Core Threshold | 30–100% | 25% | Per-core CPU detection threshold |
| Relative Multiplier | 2–50x | 4x | Multiplier relative to system average |
| EMA Alpha | 0.05–1.0 | 0.5 | Exponential moving average smoothing |
| Grace Period | 0–60 s | 3 s | Delay before demotion takes effect |
| Adaptive Sensitivity | 0.0–2.0 | 1.0 | Detection sensitivity scaling |

## Dependencies

- **ratatui** — Terminal UI framework
- **crossterm** — Cross-platform terminal handling
- **sysinfo** — System and process information
- **tokio** — Async runtime
- **windows** (optional, `etw` feature) — Event Tracing for Windows bindings

## License

This project is licensed under the [GPL-3.0](LICENSE).
