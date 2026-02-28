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
    "procwarden.exe", // This is a horrible hack to prevent self-throttling, TODO: find a better way to identify our own process without relying on the name.
];

/// Check if a process name is exempt (system or user list).
pub fn is_exempt(name: &str, user_exemptions: &[String]) -> bool {
    SYSTEM_EXEMPTIONS.iter().any(|s| s.eq_ignore_ascii_case(name))
        || user_exemptions.iter().any(|s| s.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_process_is_exempt() {
        assert!(is_exempt("csrss.exe", &[]));
        assert!(is_exempt("svchost.exe", &[]));
        assert!(is_exempt("System", &[]));
    }

    #[test]
    fn system_exemption_case_insensitive() {
        assert!(is_exempt("CSRSS.EXE", &[]));
        assert!(is_exempt("Svchost.Exe", &[]));
    }

    #[test]
    fn user_exemption_matches() {
        let exemptions = vec!["myapp.exe".to_string()];
        assert!(is_exempt("myapp.exe", &exemptions));
    }

    #[test]
    fn user_exemption_case_insensitive() {
        let exemptions = vec!["MyApp.exe".to_string()];
        assert!(is_exempt("myapp.exe", &exemptions));
    }

    #[test]
    fn non_exempt_process() {
        assert!(!is_exempt("chrome.exe", &[]));
        assert!(!is_exempt("notepad.exe", &[]));
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Fuzz is_exempt with arbitrary process names — should never panic.
        #[test]
        fn fuzz_is_exempt_no_panic(name in "\\PC{0,100}", exemptions in prop::collection::vec("\\PC{1,30}", 0..10)) {
            let _ = is_exempt(&name, &exemptions);
        }

        /// System exemptions are always case-insensitive matches.
        #[test]
        fn fuzz_system_exemptions_case_insensitive(idx in 0..SYSTEM_EXEMPTIONS.len()) {
            let name = SYSTEM_EXEMPTIONS[idx];
            // Original case
            prop_assert!(is_exempt(name, &[]));
            // Uppercase
            prop_assert!(is_exempt(&name.to_ascii_uppercase(), &[]));
            // Lowercase
            prop_assert!(is_exempt(&name.to_ascii_lowercase(), &[]));
        }

        /// User exemption lookup is case-insensitive and symmetric.
        #[test]
        fn fuzz_user_exemption_case_insensitive(name in "[a-zA-Z0-9_.]{1,30}") {
            let exemptions = vec![name.clone()];
            prop_assert!(is_exempt(&name, &exemptions));
            prop_assert!(is_exempt(&name.to_ascii_uppercase(), &exemptions));
            prop_assert!(is_exempt(&name.to_ascii_lowercase(), &exemptions));
        }

        /// A name not in system or user exemptions returns false.
        #[test]
        fn fuzz_non_exempt_returns_false(name in "[a-z]{5,15}_unique_test\\.exe") {
            // These names are unlikely to match system exemptions
            let is_system = SYSTEM_EXEMPTIONS.iter().any(|s| s.eq_ignore_ascii_case(&name));
            if !is_system {
                prop_assert!(!is_exempt(&name, &[]));
            }
        }
    }
}
