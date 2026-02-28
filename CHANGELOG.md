# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.3.0] - 2026-02-28

### Changed

- Upgraded `windows` crate from 0.48 to 0.62 for improved Windows API support
- PriorityGuard now uses PID-based self-process detection instead of hardcoded process name

### Added

- Multi-criteria system process detection for PriorityGuard exemptions:
  - Process integrity level checking (High/System integrity = critical process)
  - Session ID checking (Session 0 = system services)
  - Manual FFI bindings for Windows Security APIs (OpenProcessToken, GetTokenInformation, GetSidSubAuthority)
- Robust system process protection that works across different Windows versions

### Fixed

- Removed hardcoded "procwarden.exe" exemption that failed when executable was renamed
- Fixed breaking changes from windows crate upgrade (`.as_bool()` → `.is_ok()`, PROCESSTRACE_HANDLE field access)

### Security

- Enhanced system process protection prevents accidental priority changes to critical Windows services

## [0.2.0] - 2026-02-28

### Added

- PriorityGuard process exemptions UI

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
