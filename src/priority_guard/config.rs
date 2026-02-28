/// PriorityGuard configuration, built from SettingsState each tick.
#[derive(Debug)]
pub struct PriorityGuardConfig {
    pub enabled: bool,
    pub cpu_threshold: f32,
    pub duration_secs: u64,
    pub user_exemptions: Vec<String>,
    /// Per-core CPU% threshold. Derived: total_cpu * core_count / threads.
    pub per_core_threshold: f32,
    /// Number of logical CPU cores (for per-core derivation).
    pub core_count: u8,
    /// Relative multiplier: trigger if CPU > multiplier × median of all processes.
    pub relative_multiplier: f32,
    /// EMA smoothing alpha (0.0–1.0). Higher = more responsive, lower = smoother.
    pub ema_alpha: f32,
    /// Grace period: ignore newly seen processes for this many seconds.
    pub grace_period_secs: u64,
    /// Adaptive sensitivity: 0.0 = disabled, higher = more aggressive scaling with system load.
    /// scale = 1.0 - sensitivity * (load - 0.5)
    pub adaptive_sensitivity: f32,
}

/// System-critical processes that should never have their priority changed.
/// Some of the entries here are hackish and may need refinement, but this is a starting point to prevent major system instability. Note that some of these processes may not always be present or may have different names on different Windows versions, so this list is not exhaustive or guaranteed to be correct in all environments.
/// TODO: Consider adding more processes or using a more robust method of identifying critical system processes (e.g., by checking process integrity level or other attributes instead of just name).
pub const SYSTEM_EXEMPTIONS: &[&str] = &[
    "csrss.exe",
    "lsass.exe",
    "smss.exe",
    "services.exe",
    "svchost.exe",
    "wininit.exe",
    "winlogon.exe",
    "dwm.exe",
    "System",
    "Registry",
    "Memory Compression",
];

/// Check if a process is exempt (self, system, or user list).
pub fn is_exempt(pid: u32, name: &str, user_exemptions: &[String]) -> bool {
    // Check if this is the current process (self-protection)
    if pid == std::process::id() {
        return true;
    }

    // Check if process is system-critical (integrity level or session ID)
    if super::windows_api::is_system_critical(pid) {
        return true;
    }

    // Check name-based system exemptions (fallback for compatibility)
    if SYSTEM_EXEMPTIONS
        .iter()
        .any(|s| s.eq_ignore_ascii_case(name))
    {
        return true;
    }

    // Check user exemptions
    user_exemptions.iter().any(|s| s.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_process_is_exempt() {
        let current_pid = std::process::id();
        assert!(is_exempt(current_pid, "anything.exe", &[]));
        assert!(is_exempt(current_pid, "procwarden.exe", &[]));
    }

    #[test]
    fn system_process_is_exempt() {
        assert!(is_exempt(9999, "csrss.exe", &[]));
        assert!(is_exempt(9999, "svchost.exe", &[]));
        assert!(is_exempt(9999, "System", &[]));
    }

    #[test]
    fn system_exemption_case_insensitive() {
        assert!(is_exempt(9999, "CSRSS.EXE", &[]));
        assert!(is_exempt(9999, "Svchost.Exe", &[]));
    }

    #[test]
    fn user_exemption_matches() {
        let exemptions = vec!["myapp.exe".to_string()];
        assert!(is_exempt(9999, "myapp.exe", &exemptions));
    }

    #[test]
    fn user_exemption_case_insensitive() {
        let exemptions = vec!["MyApp.exe".to_string()];
        assert!(is_exempt(9999, "myapp.exe", &exemptions));
    }

    #[test]
    fn non_exempt_process() {
        assert!(!is_exempt(9999, "chrome.exe", &[]));
        assert!(!is_exempt(9999, "notepad.exe", &[]));
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Fuzz is_exempt with arbitrary process names — should never panic.
        #[test]
        fn fuzz_is_exempt_no_panic(pid in 1u32..100000, name in "\\PC{0,100}", exemptions in prop::collection::vec("\\PC{1,30}", 0..10)) {
            let _ = is_exempt(pid, &name, &exemptions);
        }

        /// Current process PID is always exempt.
        #[test]
        fn fuzz_self_process_always_exempt(name in "\\PC{1,50}") {
            let current_pid = std::process::id();
            prop_assert!(is_exempt(current_pid, &name, &[]));
        }

        /// System exemptions are always case-insensitive matches.
        #[test]
        fn fuzz_system_exemptions_case_insensitive(idx in 0..SYSTEM_EXEMPTIONS.len()) {
            let name = SYSTEM_EXEMPTIONS[idx];
            let pid = 9999u32; // Non-self PID
            // Original case
            prop_assert!(is_exempt(pid, name, &[]));
            // Uppercase
            prop_assert!(is_exempt(pid, &name.to_ascii_uppercase(), &[]));
            // Lowercase
            prop_assert!(is_exempt(pid, &name.to_ascii_lowercase(), &[]));
        }

        /// User exemption lookup is case-insensitive and symmetric.
        #[test]
        fn fuzz_user_exemption_case_insensitive(name in "[a-zA-Z0-9_.]{1,30}") {
            let exemptions = vec![name.clone()];
            let pid = 9999u32;
            prop_assert!(is_exempt(pid, &name, &exemptions));
            prop_assert!(is_exempt(pid, &name.to_ascii_uppercase(), &exemptions));
            prop_assert!(is_exempt(pid, &name.to_ascii_lowercase(), &exemptions));
        }

        /// A name not in system or user exemptions returns false (unless it's self).
        #[test]
        fn fuzz_non_exempt_returns_false(name in "[a-z]{5,15}_unique_test\\.exe") {
            // Use a PID that's definitely not our own
            let pid = if std::process::id() == 9999 { 8888 } else { 9999 };
            // These names are unlikely to match system exemptions
            let is_system = SYSTEM_EXEMPTIONS.iter().any(|s| s.eq_ignore_ascii_case(&name));
            if !is_system {
                prop_assert!(!is_exempt(pid, &name, &[]));
            }
        }
    }
}
