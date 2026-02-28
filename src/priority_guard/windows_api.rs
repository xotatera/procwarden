//! Windows API wrappers for process priority management.

#[cfg(all(windows, feature = "etw"))]
use windows::Win32::System::Threading::{
    OpenProcess, GetPriorityClass, SetPriorityClass,
    PROCESS_QUERY_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
    IDLE_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS,
    NORMAL_PRIORITY_CLASS, ABOVE_NORMAL_PRIORITY_CLASS,
    HIGH_PRIORITY_CLASS, REALTIME_PRIORITY_CLASS,
};

/// Priority class values for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
    Unknown(u32),
}

impl PriorityClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            PriorityClass::Idle => "Idle",
            PriorityClass::BelowNormal => "Below Normal",
            PriorityClass::Normal => "Normal",
            PriorityClass::AboveNormal => "Above Normal",
            PriorityClass::High => "High",
            PriorityClass::Realtime => "Realtime",
            PriorityClass::Unknown(_) => "Unknown",
        }
    }
}

#[cfg(all(windows, feature = "etw"))]
fn raw_to_priority_class(raw: u32) -> PriorityClass {
    match raw {
        x if x == IDLE_PRIORITY_CLASS.0 => PriorityClass::Idle,
        x if x == BELOW_NORMAL_PRIORITY_CLASS.0 => PriorityClass::BelowNormal,
        x if x == NORMAL_PRIORITY_CLASS.0 => PriorityClass::Normal,
        x if x == ABOVE_NORMAL_PRIORITY_CLASS.0 => PriorityClass::AboveNormal,
        x if x == HIGH_PRIORITY_CLASS.0 => PriorityClass::High,
        x if x == REALTIME_PRIORITY_CLASS.0 => PriorityClass::Realtime,
        other => PriorityClass::Unknown(other),
    }
}

/// Get the priority class of a process by PID.
/// Returns None if the process cannot be opened.
#[cfg(all(windows, feature = "etw"))]
pub fn get_priority(pid: u32) -> Option<(u32, PriorityClass)> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION, false, pid).ok()?;
        let raw = GetPriorityClass(handle);
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if raw == 0 {
            None
        } else {
            Some((raw, raw_to_priority_class(raw)))
        }
    }
}

/// Set the priority class of a process by PID.
/// Returns true on success.
#[cfg(all(windows, feature = "etw"))]
pub fn set_priority(pid: u32, priority_class: u32) -> bool {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SET_INFORMATION, false, pid) else {
            return false;
        };
        let result = SetPriorityClass(handle, windows::Win32::System::Threading::PROCESS_CREATION_FLAGS(priority_class));
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        result.as_bool()
    }
}

/// Stub for non-Windows or non-ETW builds.
#[cfg(not(all(windows, feature = "etw")))]
pub fn get_priority(_pid: u32) -> Option<(u32, PriorityClass)> {
    None
}

#[cfg(not(all(windows, feature = "etw")))]
pub fn set_priority(_pid: u32, _priority_class: u32) -> bool {
    false
}

/// Terminate a process by PID. Returns true on success.
#[cfg(all(windows, feature = "etw"))]
pub fn terminate_process(pid: u32) -> bool {
    use windows::Win32::System::Threading::TerminateProcess;
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) else {
            return false;
        };
        let result = TerminateProcess(handle, 1);
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        result.as_bool()
    }
}

#[cfg(not(all(windows, feature = "etw")))]
pub fn terminate_process(_pid: u32) -> bool {
    false
}

/// Suspend all threads of a process by PID. Returns true if at least one thread was suspended.
#[cfg(all(windows, feature = "etw"))]
pub fn suspend_process(pid: u32) -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next,
        TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{OpenThread, SuspendThread};
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else {
            return false;
        };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut suspended_any = false;
        if Thread32First(snap, &mut entry).as_bool() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    if let Ok(thread) = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) {
                        SuspendThread(thread);
                        let _ = windows::Win32::Foundation::CloseHandle(thread);
                        suspended_any = true;
                    }
                }
                if !Thread32Next(snap, &mut entry).as_bool() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        suspended_any
    }
}

#[cfg(not(all(windows, feature = "etw")))]
pub fn suspend_process(_pid: u32) -> bool {
    false
}

/// Resume all threads of a process by PID. Returns true if at least one thread was resumed.
#[cfg(all(windows, feature = "etw"))]
pub fn resume_process(pid: u32) -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next,
        TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{OpenThread, ResumeThread};
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else {
            return false;
        };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed_any = false;
        if Thread32First(snap, &mut entry).as_bool() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    if let Ok(thread) = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) {
                        ResumeThread(thread);
                        let _ = windows::Win32::Foundation::CloseHandle(thread);
                        resumed_any = true;
                    }
                }
                if !Thread32Next(snap, &mut entry).as_bool() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        resumed_any
    }
}

#[cfg(not(all(windows, feature = "etw")))]
pub fn resume_process(_pid: u32) -> bool {
    false
}

/// Raw priority class constants for the priority picker.
pub const PRIORITY_CLASSES: &[(u32, &str)] = &[
    (0x40, "Idle"),
    (0x4000, "Below Normal"),
    (0x20, "Normal"),
    (0x8000, "Above Normal"),
    (0x80, "High"),
    (0x100, "Realtime"),
];

/// The "below normal" priority class raw value for demotion.
#[cfg(all(windows, feature = "etw"))]
pub const DEMOTE_PRIORITY: u32 = BELOW_NORMAL_PRIORITY_CLASS.0;

#[cfg(not(all(windows, feature = "etw")))]
pub const DEMOTE_PRIORITY: u32 = 0x4000; // BELOW_NORMAL_PRIORITY_CLASS

/// The "idle" priority class raw value for tier-2 demotion.
#[cfg(all(windows, feature = "etw"))]
pub const IDLE_PRIORITY: u32 = IDLE_PRIORITY_CLASS.0;

#[cfg(not(all(windows, feature = "etw")))]
pub const IDLE_PRIORITY: u32 = 0x40; // IDLE_PRIORITY_CLASS

/// Get the PID of the foreground window's process.
#[cfg(all(windows, feature = "etw"))]
pub fn get_foreground_pid() -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }
        let mut pid: u32 = 0;
        let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if tid == 0 || pid == 0 {
            None
        } else {
            Some(pid)
        }
    }
}

#[cfg(not(all(windows, feature = "etw")))]
pub fn get_foreground_pid() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_class_display() {
        assert_eq!(PriorityClass::Normal.as_str(), "Normal");
        assert_eq!(PriorityClass::BelowNormal.as_str(), "Below Normal");
        assert_eq!(PriorityClass::Unknown(999).as_str(), "Unknown");
    }

    #[test]
    fn priority_classes_constant_valid() {
        assert_eq!(PRIORITY_CLASSES.len(), 6);
        for (raw, label) in PRIORITY_CLASSES {
            assert!(*raw > 0, "raw value should be nonzero: {}", label);
            assert!(!label.is_empty(), "label should not be empty");
        }
    }

    #[test]
    fn priority_classes_raw_values_unique() {
        let mut seen = std::collections::HashSet::new();
        for (raw, _) in PRIORITY_CLASSES {
            assert!(seen.insert(raw), "duplicate raw value: {}", raw);
        }
    }

    #[test]
    fn stubs_return_expected_defaults() {
        // On non-ETW builds, stubs return false/None
        #[cfg(not(all(windows, feature = "etw")))]
        {
            assert!(!terminate_process(0));
            assert!(!suspend_process(0));
            assert!(!resume_process(0));
            assert!(get_priority(0).is_none());
            assert!(!set_priority(0, 0));
        }
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// PriorityClass::as_str never panics for any variant.
        #[test]
        fn fuzz_priority_class_as_str(raw in any::<u32>()) {
            let pc = PriorityClass::Unknown(raw);
            let s = pc.as_str();
            prop_assert_eq!(s, "Unknown");
        }

        /// PRIORITY_CLASSES index access is always safe within bounds.
        #[test]
        fn fuzz_priority_classes_index(idx in 0usize..6) {
            let (raw, label) = PRIORITY_CLASSES[idx];
            prop_assert!(raw > 0);
            prop_assert!(!label.is_empty());
        }

        /// Stubs don't panic with any PID.
        #[test]
        fn fuzz_stub_functions_no_panic(pid in any::<u32>()) {
            #[cfg(not(all(windows, feature = "etw")))]
            {
                let _ = terminate_process(pid);
                let _ = suspend_process(pid);
                let _ = resume_process(pid);
                let _ = get_priority(pid);
                let _ = set_priority(pid, 0x20);
            }
            // On ETW builds, we still don't want to actually kill random PIDs,
            // so just verify the functions exist and compile
            #[cfg(all(windows, feature = "etw"))]
            {
                let _ = get_priority(pid);
                // Don't call terminate/suspend/resume with arbitrary PIDs on real Windows
            }
        }
    }
}
