# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-02-28

### Added

- Real-time process monitoring with CPU and memory usage tracking
- Process management: kill (with confirmation), suspend/resume, priority adjustment (Idle through Realtime)
- PriorityGuard engine for automatic CPU hog detection and demotion
  - Configurable thresholds (absolute CPU%, per-core%, relative multiplier)
  - Grace periods and adaptive sensitivity
  - EMA smoothing for stability
- Sortable process list by PID, Name, CPU, or Memory with direction toggling
- Live filtering by process name
- Settings menu with configurable poll interval, PriorityGuard parameters, and EMA alpha
- Log viewer with scrollable action history
- ETW (Event Tracing for Windows) data source for efficient kernel-level monitoring
- Automatic fallback to polling (sysinfo) when ETW is unavailable
- `--disable-etw` CLI flag to force polling mode
- Hide-self option to remove ProcWarden from the process list
- TUI built with ratatui and crossterm
