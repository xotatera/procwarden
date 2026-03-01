# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.4.1] - 2026-03-02

### Fixed

- **CRITICAL: Path-based exemptions were not working** - The exemption system added in v0.4.0 was non-functional because `is_exempt()` was called with `None` for the exe_path parameter
  - Added `exe_path: Option<PathBuf>` field to `ProcessInfo` struct
  - Updated both data sources (polling and ETW) to populate exe_path from `sysinfo`
  - Fixed `PriorityGuardEngine::evaluate()` to pass `proc.exe_path` to `is_exempt()`
  - Exempted processes are now correctly excluded from PriorityGuard actions
- **Duplicate exemptions bug** - Users could add the same process multiple times with different hashes
  - Modified `ExemptionList::add()` to check for existing path before adding (idempotent)
  - Uses case-insensitive path matching on Windows
  - Added test: `exemption_list_prevents_duplicates`

### Changed

- Total test count: 336 tests (added 1 new test for duplicate prevention)

## [0.4.0] - 2026-03-01

### Added

- **Path+Hash Based Exemption System**: Complete rewrite of PriorityGuard exemptions
  - Dual cryptographic hashing (SHA256 + Blake3) for secure process verification
  - Path-based matching with case-insensitive support on Windows
  - Hash verification with non-blocking warnings when executable changes
  - New `src/exemption/` module with clean architecture:
    - Domain types: `Exemption`, `ExemptionList`, `ExemptionMatcher`
    - Trait-based hashing with `HashAlgorithm` trait
    - Verification service for hash validation
    - JSON persistence with automatic migration from v1 format
- **Tabbed Exemption Picker UI**:
  - Running Processes tab: Select from currently running processes with valid exe paths
  - Browse tab: Placeholder for future file system browser
  - Manual Entry tab: Type executable path directly with full cursor support
  - Tab switching with Tab key, Esc to exit
  - Refresh command ('r') to update running process list
- 45 new unit tests with property-based fuzzing using proptest
- 7 new integration tests for exemption picker UI (335 total tests)

### Changed

- **BREAKING**: Exemptions now require full executable path instead of just filename
- Old filename-based exemptions are automatically cleared on upgrade with migration message
- PriorityGuard `is_exempt()` function now accepts optional `exe_path` parameter
- ExemptionEditorState replaced with ExemptionPickerState (backward-compatible type alias)
- Settings display shows exemption count instead of list

### Fixed

- Critical performance bug: `is_exempt()` no longer creates System instance for every process check
- Manual entry mode bug: 'e' and 'E' keys now type characters instead of exiting picker
- Removed duplicate System refresh in ProcessCandidate construction

### Security

- Cryptographic hash verification (SHA256 + Blake3) prevents exe tampering
- Path-based matching eliminates filename collision attacks
- Hash mismatch warnings logged when executable changes (non-blocking)

### Dependencies

- Added `sha2` 0.10 for SHA256 hashing
- Added `blake3` 1.5 for Blake3 hashing
- Added `hex` 0.4 for hash encoding
- Added `rand` 0.8 (dev) for testing

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
