use crate::common::ProcessInfo;
use crate::data_source::DataSource;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::sync::mpsc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

/// Helper: create a wide (UTF-16) null-terminated string.
fn to_wide(s: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    wide.push(0);
    wide
}

/// The NT Kernel Logger session name (system-wide kernel trace).
const KERNEL_LOGGER_NAME: &str = "NT Kernel Logger";

/// SystemTraceControlGuid – required as Wnode.Guid for kernel trace sessions.
const SYSTEM_TRACE_CONTROL_GUID: GUID = GUID::from_u128(0x9e814aad_3204_11d2_9a82_006008a86939);

/// Process sub-provider GUID – used to identify process events in callbacks.
const PROCESS_GUID: GUID = GUID::from_u128(0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c);

/// Thread sub-provider GUID – used to identify thread and CSwitch events.
const THREAD_GUID: GUID = GUID::from_u128(0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c);

/// EnableFlags for kernel trace: process + thread + context switch.
const ENABLE_FLAGS: u32 = 0x01 | 0x02 | 0x10; // PROCESS | THREAD | CSWITCH

// ---------------------------------------------------------------------------
// Shared state between callback and get_processes()
// ---------------------------------------------------------------------------

/// State shared between the ETW callback thread and the main thread.
struct EtwSharedState {
    /// Process names from ETW start/end events (cold path).
    names: Mutex<HashMap<u32, String>>,
    /// CSwitch accumulation state (hot path).
    cswitch: Mutex<CSwitchState>,
}

/// Per-context-switch accumulation state.
struct CSwitchState {
    /// Thread ID → owning PID.
    thread_to_pid: HashMap<u32, u32>,
    /// Thread ID → QPC timestamp when it started running.
    thread_run_start: HashMap<u32, i64>,
    /// PID → accumulated QPC ticks of CPU time since last drain.
    pid_cpu_ticks: HashMap<u32, i64>,
    /// First and last event timestamps in the current accumulation window.
    window_first_ts: Option<i64>,
    window_last_ts: i64,
}

impl CSwitchState {
    fn new() -> Self {
        Self {
            thread_to_pid: HashMap::new(),
            thread_run_start: HashMap::new(),
            pid_cpu_ticks: HashMap::new(),
            window_first_ts: None,
            window_last_ts: 0,
        }
    }
}

/// A data source driven by Event Tracing for Windows (ETW).
///
/// Uses the NT Kernel Logger to receive process start/stop, thread start/stop,
/// and context switch events. CPU usage is computed from CSwitch events.
/// Memory is still obtained from sysinfo polling.
pub struct EtwDataSource {
    shared: Arc<EtwSharedState>,
    shutdown_flag: Arc<AtomicBool>,
    _worker: Option<thread::JoinHandle<()>>,
    poller: crate::data_source::polling::PollingDataSource,
    num_cpus: usize,
}

impl EtwDataSource {
    pub fn new() -> Result<Self> {
        log::info!("EtwDataSource: Initializing kernel process tracing...");

        let num_cpus = {
            let mut info = SYSTEM_INFO::default();
            unsafe { GetSystemInfo(&mut info) };
            (info.dwNumberOfProcessors as usize).max(1)
        };

        let shared = Arc::new(EtwSharedState {
            names: Mutex::new(HashMap::new()),
            cswitch: Mutex::new(CSwitchState::new()),
        });

        let shutdown_flag = Arc::new(AtomicBool::new(false));

        // Populate initial process list using polling.
        let mut poller = crate::data_source::polling::PollingDataSource::new();
        {
            if let Ok(procs) = poller.get_processes() {
                let mut names = shared.names.lock().unwrap();
                for proc in procs {
                    names.insert(proc.pid, proc.name.clone());
                }
                log::info!(
                    "EtwDataSource: Initialized with {} existing processes",
                    names.len()
                );
            }
        }

        let shared_clone = shared.clone();
        let sf = shutdown_flag.clone();
        let (tx, rx) = mpsc::channel::<Result<()>>();

        let handle = thread::spawn(move || {
            etw_worker_thread(shared_clone, sf, tx);
        });

        match rx.recv() {
            Ok(Ok(())) => {
                log::info!("EtwDataSource: ETW session started successfully");
            }
            Ok(Err(e)) => {
                return Err(anyhow!("ETW session failed to start: {}", e));
            }
            Err(_) => {
                return Err(anyhow!("ETW worker thread exited unexpectedly"));
            }
        }

        Ok(Self {
            shared,
            shutdown_flag,
            _worker: Some(handle),
            poller,
            num_cpus,
        })
    }
}

impl super::DataSource for EtwDataSource {
    fn get_processes(&mut self) -> Result<Vec<ProcessInfo>> {
        // 1. Drain CPU accumulators
        let (pid_ticks, wall_ticks) = {
            let mut cs = self.shared.cswitch.lock().unwrap();
            let window_span = match cs.window_first_ts {
                Some(first) if cs.window_last_ts > first => {
                    (cs.window_last_ts - first) as f64 * self.num_cpus as f64
                }
                _ => 0.0,
            };
            cs.window_first_ts = None;
            cs.window_last_ts = 0;
            (std::mem::take(&mut cs.pid_cpu_ticks), window_span)
        };

        // 3. Get memory + exe_path from sysinfo (skip CPU/disk)
        let memory_and_exe_map = self.poller.get_memory_and_exe();

        // 4. Build result from names + CPU + memory + exe_path
        let names = self.shared.names.lock().unwrap();
        let mut result = Vec::with_capacity(names.len());
        for (&pid, name) in names.iter() {
            let cpu_pct = pid_ticks.get(&pid).map_or(0.0, |&ticks| {
                if wall_ticks > 0.0 {
                    (ticks as f64 / wall_ticks) * 100.0
                } else {
                    0.0
                }
            });
            let (memory, exe_path) = memory_and_exe_map
                .get(&pid)
                .map(|(m, p)| (*m, p.clone()))
                .unwrap_or((0, None));
            result.push(ProcessInfo {
                pid,
                name: name.clone(),
                cpu: cpu_pct as f32,
                memory,
                exe_path,
            });
        }
        Ok(result)
    }
}

impl Drop for EtwDataSource {
    fn drop(&mut self) {
        log::info!("EtwDataSource: Shutting down...");
        self.shutdown_flag.store(true, Ordering::Relaxed);
        stop_trace_session();
    }
}

// ---------------------------------------------------------------------------
// ETW session management
// ---------------------------------------------------------------------------

/// Allocate an `EVENT_TRACE_PROPERTIES` buffer with room for the session name.
unsafe fn alloc_properties(
    session_name_wide: &[u16],
) -> Result<(*mut EVENT_TRACE_PROPERTIES, std::alloc::Layout)> {
    let name_bytes = std::mem::size_of_val(session_name_wide);
    let total = mem::size_of::<EVENT_TRACE_PROPERTIES>() + name_bytes;
    let layout = std::alloc::Layout::from_size_align(total, 8)
        .map_err(|e| anyhow!("Layout error: {}", e))?;
    let buf = std::alloc::alloc_zeroed(layout);
    if buf.is_null() {
        return Err(anyhow!("Failed to allocate EVENT_TRACE_PROPERTIES"));
    }
    let props = buf as *mut EVENT_TRACE_PROPERTIES;
    (*props).Wnode.BufferSize = total as u32;
    (*props).LoggerNameOffset = mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
    Ok((props, layout))
}

/// Stop any existing NT Kernel Logger session (best-effort).
fn stop_trace_session() {
    unsafe {
        let name = to_wide(KERNEL_LOGGER_NAME);
        let Ok((props, layout)) = alloc_properties(&name) else {
            return;
        };
        let _ = ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(name.as_ptr()),
            props,
            EVENT_TRACE_CONTROL_STOP,
        );
        std::alloc::dealloc(props as *mut u8, layout);
    }
}

/// ETW event record callback — dispatches process, thread, and CSwitch events.
///
/// # Safety
/// Called by Windows; `event_record` is guaranteed non-null by the OS.
/// `UserContext` must point to a valid `EtwSharedState`.
unsafe extern "system" fn event_record_callback(event_record: *mut EVENT_RECORD) {
    if event_record.is_null() {
        return;
    }
    let record = &*event_record;
    let header = &record.EventHeader;
    let provider = header.ProviderId;

    let context = record.UserContext as *const EtwSharedState;
    if context.is_null() {
        return;
    }
    let state = &*context;

    if provider == PROCESS_GUID {
        handle_process_event(state, record);
    } else if provider == THREAD_GUID {
        handle_thread_event(state, record);
    }
}

/// Handle process start/end events — updates the names map.
unsafe fn handle_process_event(state: &EtwSharedState, record: &EVENT_RECORD) {
    let opcode = record.EventHeader.EventDescriptor.Opcode;
    let user_data = record.UserData as *const u8;
    let user_data_len = record.UserDataLength as usize;
    let ptr_size = mem::size_of::<usize>();

    match opcode {
        // Process Start
        1 => {
            if user_data.is_null() || user_data_len < ptr_size + 4 {
                return;
            }
            let pid = *(user_data.add(ptr_size) as *const u32);
            let name = extract_image_name(user_data, user_data_len, pid);
            if let Ok(mut names) = state.names.lock() {
                names.insert(pid, name);
            }
        }
        // Process End
        2 => {
            if user_data.is_null() || user_data_len < ptr_size + 4 {
                return;
            }
            let pid = *(user_data.add(ptr_size) as *const u32);
            if let Ok(mut names) = state.names.lock() {
                names.remove(&pid);
            }
            // Clean up any threads belonging to this process
            if let Ok(mut cs) = state.cswitch.lock() {
                cs.pid_cpu_ticks.remove(&pid);
            }
        }
        _ => {}
    }
}

/// Handle thread start/end/DCStart/DCEnd and CSwitch events.
unsafe fn handle_thread_event(state: &EtwSharedState, record: &EVENT_RECORD) {
    let opcode = record.EventHeader.EventDescriptor.Opcode;
    let user_data = record.UserData as *const u8;
    let user_data_len = record.UserDataLength as usize;

    match opcode {
        // Thread Start (1) or DCStart (3) — register thread→PID mapping
        1 | 3 => {
            // Payload: ProcessId(u32) at offset 0, ThreadId(u32) at offset 4
            if user_data.is_null() || user_data_len < 8 {
                return;
            }
            let process_id = *(user_data as *const u32);
            let thread_id = *(user_data.add(4) as *const u32);
            if let Ok(mut cs) = state.cswitch.lock() {
                cs.thread_to_pid.insert(thread_id, process_id);
            }
        }
        // Thread End (2) or DCEnd (4) — remove thread mapping
        2 | 4 => {
            if user_data.is_null() || user_data_len < 8 {
                return;
            }
            let thread_id = *(user_data.add(4) as *const u32);
            if let Ok(mut cs) = state.cswitch.lock() {
                cs.thread_to_pid.remove(&thread_id);
                cs.thread_run_start.remove(&thread_id);
            }
        }
        // CSwitch (36) — the hot path
        36 => {
            // Payload: NewThreadId(u32) at offset 0, OldThreadId(u32) at offset 4
            if user_data.is_null() || user_data_len < 8 {
                return;
            }
            let new_thread_id = *(user_data as *const u32);
            let old_thread_id = *(user_data.add(4) as *const u32);
            let timestamp = record.EventHeader.TimeStamp;

            if let Ok(mut cs) = state.cswitch.lock() {
                // Track event window timestamps
                if cs.window_first_ts.is_none() {
                    cs.window_first_ts = Some(timestamp);
                }
                cs.window_last_ts = timestamp;

                // Old thread being switched out: accumulate its CPU time
                if let Some(&start_ts) = cs.thread_run_start.get(&old_thread_id) {
                    let elapsed_ticks = timestamp - start_ts;
                    if elapsed_ticks > 0 {
                        if let Some(&pid) = cs.thread_to_pid.get(&old_thread_id) {
                            *cs.pid_cpu_ticks.entry(pid).or_insert(0) += elapsed_ticks;
                        }
                    }
                    cs.thread_run_start.remove(&old_thread_id);
                }

                // New thread being switched in: record start time
                if cs.thread_to_pid.contains_key(&new_thread_id) {
                    cs.thread_run_start.insert(new_thread_id, timestamp);
                }
            }
        }
        _ => {}
    }
}

/// Best-effort extraction of the ImageFileName from process-start event data.
///
/// The ETW Process event layout (after the fixed fields) contains a variable-
/// length SID followed by a null-terminated ANSI ImageFileName. Since the SID
/// length varies, we scan the tail for the last null-terminated printable ASCII
/// string, which is the image name.
unsafe fn extract_image_name(user_data: *const u8, len: usize, pid: u32) -> String {
    let ptr_size = mem::size_of::<usize>();
    let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
    if len <= min_offset {
        return format!("PID-{}", pid);
    }

    let tail = std::slice::from_raw_parts(user_data.add(min_offset), len - min_offset);

    // Scan for all null-terminated ASCII strings in the tail and pick the best one.
    // The ImageFileName is typically the last ASCII string in the payload, after
    // a variable-length SID and optional Flags field.
    let mut best: Option<&str> = None;
    let mut i = 0;
    while i < tail.len() {
        // Find start of a printable ASCII run
        if tail[i] >= 0x20 && tail[i] < 0x7f {
            let start = i;
            while i < tail.len() && tail[i] >= 0x20 && tail[i] < 0x7f {
                i += 1;
            }
            // Must be null-terminated (or at end of buffer)
            if i < tail.len() && tail[i] == 0 {
                if let Ok(s) = std::str::from_utf8(&tail[start..i]) {
                    // Require at least 2 chars and an alphanumeric to avoid
                    // picking up stray punctuation (e.g. lone `"`) from binary data.
                    if s.len() >= 2 && s.bytes().any(|b| b.is_ascii_alphanumeric()) {
                        best = Some(s);
                    }
                }
            }
        }
        i += 1;
    }

    if let Some(name) = best {
        return name.to_string();
    }

    format!("PID-{}", pid)
}

/// Worker thread: sets up the ETW session, signals readiness, then blocks
/// in `ProcessTrace` until the session is stopped via `ControlTraceW`.
fn etw_worker_thread(
    shared: Arc<EtwSharedState>,
    _shutdown_flag: Arc<AtomicBool>,
    ready_tx: mpsc::Sender<Result<()>>,
) {
    log::info!("EtwDataSource worker: Starting ETW kernel trace session");

    stop_trace_session();

    let session = match setup_etw_session(&shared) {
        Ok(s) => {
            let _ = ready_tx.send(Ok(()));
            s
        }
        Err(e) => {
            log::error!("EtwDataSource: Setup failed: {:?}", e);
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    run_trace_loop(&session);
    cleanup_session(session);

    log::info!("EtwDataSource worker: Stopped");
}

/// Handles returned from the setup phase.
struct EtwSession {
    trace_handle: CONTROLTRACE_HANDLE,
    open_handle: PROCESSTRACE_HANDLE,
    props: *mut EVENT_TRACE_PROPERTIES,
    layout: std::alloc::Layout,
    state_ptr: *const EtwSharedState,
}

unsafe impl Send for EtwSession {}

/// Phase 1: create and open the trace session.
fn setup_etw_session(shared: &Arc<EtwSharedState>) -> Result<EtwSession> {
    unsafe {
        let session_name = to_wide(KERNEL_LOGGER_NAME);
        let (props, layout) = alloc_properties(&session_name)?;

        (*props).Wnode.Guid = SYSTEM_TRACE_CONTROL_GUID;
        (*props).Wnode.ClientContext = 1; // QPC timestamps
        (*props).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*props).EnableFlags = EVENT_TRACE_FLAG(ENABLE_FLAGS);
        (*props).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;

        let mut trace_handle = CONTROLTRACE_HANDLE::default();
        let err = StartTraceW(&mut trace_handle, PCWSTR(session_name.as_ptr()), props);

        if err.0 != 0 {
            let msg = match err.0 {
                5 => "Access denied (admin privileges required)",
                183 => "Session already exists",
                _ => "Unknown error",
            };
            std::alloc::dealloc(props as *mut u8, layout);
            return Err(anyhow!("StartTraceW failed ({}): {}", err.0, msg));
        }

        log::info!("EtwDataSource worker: Kernel trace session created");

        let mut logger_name = to_wide(KERNEL_LOGGER_NAME);
        let mut logfile: EVENT_TRACE_LOGFILEW = mem::zeroed();
        logfile.LoggerName = PWSTR(logger_name.as_mut_ptr());
        logfile.Anonymous1.ProcessTraceMode =
            PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        logfile.Anonymous2.EventRecordCallback = Some(event_record_callback);

        let state_ptr = Arc::into_raw(shared.clone());
        logfile.Context = state_ptr as *mut c_void;

        let open_handle = OpenTraceW(&mut logfile);

        if open_handle.Value == u64::MAX {
            let _ = ControlTraceW(
                trace_handle,
                PCWSTR::null(),
                props,
                EVENT_TRACE_CONTROL_STOP,
            );
            std::alloc::dealloc(props as *mut u8, layout);
            let _ = Arc::from_raw(state_ptr);
            return Err(anyhow!("OpenTraceW failed"));
        }

        log::info!("EtwDataSource worker: Real-time trace opened, ready for events");

        Ok(EtwSession {
            trace_handle,
            open_handle,
            props,
            layout,
            state_ptr,
        })
    }
}

/// Phase 2: block in `ProcessTrace` until the session is stopped.
fn run_trace_loop(session: &EtwSession) {
    let traces = [session.open_handle];
    let err = unsafe { ProcessTrace(&traces, None, None) };
    if err.0 != 0 {
        log::warn!("ProcessTrace returned error code {}", err.0);
    }
}

/// Phase 3: close handles, stop session, deallocate, reclaim Arc.
fn cleanup_session(session: EtwSession) {
    log::info!("EtwDataSource worker: Cleaning up trace session");
    unsafe {
        let _ = CloseTrace(session.open_handle);
        let _ = ControlTraceW(
            session.trace_handle,
            PCWSTR::null(),
            session.props,
            EVENT_TRACE_CONTROL_STOP,
        );
        std::alloc::dealloc(session.props as *mut u8, session.layout);
        let _ = Arc::from_raw(session.state_ptr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- to_wide --

    #[test]
    fn to_wide_ascii() {
        let w = to_wide("ABC");
        assert_eq!(w, vec![0x41, 0x42, 0x43, 0x00]);
    }

    #[test]
    fn to_wide_empty() {
        let w = to_wide("");
        assert_eq!(w, vec![0x00]);
    }

    #[test]
    fn to_wide_unicode() {
        let w = to_wide("héllo");
        assert_eq!(w.last(), Some(&0x00));
        assert!(w.len() > 1);
        assert_eq!(w[0], b'h' as u16);
        assert_eq!(w[1], 0x00E9);
    }

    #[test]
    fn to_wide_null_terminated() {
        let w = to_wide("test");
        assert_eq!(*w.last().unwrap(), 0u16);
    }

    // -- extract_image_name --

    #[test]
    fn extract_name_from_valid_payload() {
        let ptr_size = mem::size_of::<usize>();
        let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
        let name_bytes = b"notepad.exe\0";
        let total = min_offset + name_bytes.len();
        let mut buf = vec![0u8; total];
        buf[min_offset..].copy_from_slice(name_bytes);

        let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), 42) };
        assert_eq!(result, "notepad.exe");
    }

    #[test]
    fn extract_name_too_short_returns_fallback() {
        let buf = [0u8; 4];
        let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), 99) };
        assert_eq!(result, "PID-99");
    }

    #[test]
    fn extract_name_no_ascii_returns_fallback() {
        let ptr_size = mem::size_of::<usize>();
        let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
        let mut buf = vec![0u8; min_offset + 4];
        buf[min_offset] = 0x01;
        buf[min_offset + 1] = 0x02;
        buf[min_offset + 2] = 0x00;

        let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), 7) };
        assert_eq!(result, "PID-7");
    }

    #[test]
    fn extract_name_no_null_terminator_returns_fallback() {
        let ptr_size = mem::size_of::<usize>();
        let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
        let buf = vec![0x41u8; min_offset + 5];

        let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), 3) };
        assert_eq!(result, "PID-3");
    }

    #[test]
    fn extract_name_skips_sid_before_image_name() {
        // Simulate: fixed fields + Flags(4) + SID binary data + "chrome.exe\0"
        let ptr_size = mem::size_of::<usize>();
        let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
        // Flags (4 bytes) + fake SID (12 bytes of binary junk) + image name
        let sid_and_flags: &[u8] = &[
            0x00, 0x00, 0x00, 0x00, // Flags
            0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x05, 0x12, // SID bytes
            0x00, 0x00, 0x00, 0x18, // more SID
        ];
        let name_bytes = b"chrome.exe\0";
        let total = min_offset + sid_and_flags.len() + name_bytes.len();
        let mut buf = vec![0u8; total];
        buf[min_offset..min_offset + sid_and_flags.len()].copy_from_slice(sid_and_flags);
        buf[min_offset + sid_and_flags.len()..].copy_from_slice(name_bytes);

        let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), 42) };
        assert_eq!(result, "chrome.exe");
    }

    // -- alloc_properties --

    #[test]
    fn alloc_properties_sets_buffer_size() {
        let name = to_wide("TestSession");
        unsafe {
            let (props, layout) = alloc_properties(&name).unwrap();
            let expected_total =
                mem::size_of::<EVENT_TRACE_PROPERTIES>() + name.len() * mem::size_of::<u16>();
            assert_eq!((*props).Wnode.BufferSize, expected_total as u32);
            assert_eq!(
                (*props).LoggerNameOffset,
                mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32
            );
            std::alloc::dealloc(props as *mut u8, layout);
        }
    }

    #[test]
    fn alloc_properties_zeroed() {
        let name = to_wide("Test");
        unsafe {
            let (props, layout) = alloc_properties(&name).unwrap();
            assert_eq!((*props).Wnode.ClientContext, 0);
            assert_eq!((*props).LogFileMode, 0);
            std::alloc::dealloc(props as *mut u8, layout);
        }
    }

    // -- constants --

    #[test]
    fn system_trace_control_guid_correct() {
        assert_eq!(
            SYSTEM_TRACE_CONTROL_GUID,
            GUID::from_u128(0x9e814aad_3204_11d2_9a82_006008a86939)
        );
    }

    #[test]
    fn process_guid_correct() {
        assert_eq!(
            PROCESS_GUID,
            GUID::from_u128(0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c)
        );
    }

    #[test]
    fn thread_guid_correct() {
        assert_eq!(
            THREAD_GUID,
            GUID::from_u128(0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c)
        );
    }

    #[test]
    fn enable_flags_includes_all_required() {
        assert_eq!(ENABLE_FLAGS & 0x01, 0x01); // PROCESS
        assert_eq!(ENABLE_FLAGS & 0x02, 0x02); // THREAD
        assert_eq!(ENABLE_FLAGS & 0x10, 0x10); // CSWITCH
    }

    // -- Callback tests with EtwSharedState --

    fn make_test_state() -> EtwSharedState {
        EtwSharedState {
            names: Mutex::new(HashMap::new()),
            cswitch: Mutex::new(CSwitchState::new()),
        }
    }

    #[test]
    fn callback_ignores_non_process_events() {
        let state = make_test_state();
        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = GUID::from_u128(0x00000000_0000_0000_0000_000000000000);
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }
        assert!(state.names.lock().unwrap().is_empty());
    }

    #[test]
    fn callback_ignores_null_context() {
        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = PROCESS_GUID;
            record.EventHeader.EventDescriptor.Opcode = 1;
            record.UserContext = std::ptr::null_mut();
            event_record_callback(&mut record);
        }
    }

    #[test]
    fn callback_handles_process_start() {
        let state = make_test_state();
        let ptr_size = mem::size_of::<usize>();
        let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
        let name = b"test.exe\0";
        let total = min_offset + name.len();
        let mut buf = vec![0u8; total];
        buf[ptr_size..ptr_size + 4].copy_from_slice(&42u32.to_ne_bytes());
        buf[min_offset..].copy_from_slice(name);

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = PROCESS_GUID;
            record.EventHeader.EventDescriptor.Opcode = 1;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = total as u16;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let names = state.names.lock().unwrap();
        assert_eq!(names.get(&42).unwrap(), "test.exe");
    }

    #[test]
    fn callback_handles_process_end() {
        let state = make_test_state();
        state.names.lock().unwrap().insert(42, "test.exe".into());

        let ptr_size = mem::size_of::<usize>();
        let payload_len = ptr_size + 4;
        let mut buf = vec![0u8; payload_len];
        buf[ptr_size..ptr_size + 4].copy_from_slice(&42u32.to_ne_bytes());

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = PROCESS_GUID;
            record.EventHeader.EventDescriptor.Opcode = 2;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = payload_len as u16;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        assert!(!state.names.lock().unwrap().contains_key(&42));
    }

    #[test]
    fn callback_ignores_unknown_opcode() {
        let state = make_test_state();
        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = PROCESS_GUID;
            record.EventHeader.EventDescriptor.Opcode = 99;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }
        assert!(state.names.lock().unwrap().is_empty());
    }

    // -- Thread event tests --

    #[test]
    fn thread_start_registers_mapping() {
        let state = make_test_state();
        // Thread Start payload: ProcessId(u32)=100, ThreadId(u32)=200
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&100u32.to_ne_bytes());
        buf[4..8].copy_from_slice(&200u32.to_ne_bytes());

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 1; // Start
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        assert_eq!(cs.thread_to_pid.get(&200), Some(&100));
    }

    #[test]
    fn thread_dcstart_registers_mapping() {
        let state = make_test_state();
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&100u32.to_ne_bytes());
        buf[4..8].copy_from_slice(&200u32.to_ne_bytes());

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 3; // DCStart
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        assert_eq!(cs.thread_to_pid.get(&200), Some(&100));
    }

    #[test]
    fn thread_end_removes_mapping() {
        let state = make_test_state();
        {
            let mut cs = state.cswitch.lock().unwrap();
            cs.thread_to_pid.insert(200, 100);
            cs.thread_run_start.insert(200, 5000);
        }

        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&100u32.to_ne_bytes());
        buf[4..8].copy_from_slice(&200u32.to_ne_bytes());

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 2; // End
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        assert!(!cs.thread_to_pid.contains_key(&200));
        assert!(!cs.thread_run_start.contains_key(&200));
    }

    // -- CSwitch tests --

    #[test]
    fn cswitch_accumulates_cpu_ticks() {
        let state = make_test_state();
        {
            let mut cs = state.cswitch.lock().unwrap();
            cs.thread_to_pid.insert(10, 100); // thread 10 → PID 100
            cs.thread_to_pid.insert(20, 200); // thread 20 → PID 200
            cs.thread_run_start.insert(10, 1000); // thread 10 started at t=1000
        }

        // CSwitch: old=10 (switching out), new=20 (switching in), timestamp=2000
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&20u32.to_ne_bytes()); // NewThreadId
        buf[4..8].copy_from_slice(&10u32.to_ne_bytes()); // OldThreadId

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 36;
            record.EventHeader.TimeStamp = 2000;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        // PID 100 should have 1000 ticks (2000 - 1000)
        assert_eq!(cs.pid_cpu_ticks.get(&100), Some(&1000i64));
        // Thread 10 run_start should be removed (switched out)
        assert!(!cs.thread_run_start.contains_key(&10));
        // Thread 20 should have run_start = 2000 (switched in)
        assert_eq!(cs.thread_run_start.get(&20), Some(&2000i64));
    }

    #[test]
    fn cswitch_ignores_unknown_thread() {
        let state = make_test_state();
        // No threads registered at all

        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&99u32.to_ne_bytes()); // NewThreadId
        buf[4..8].copy_from_slice(&88u32.to_ne_bytes()); // OldThreadId

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 36;
            record.EventHeader.TimeStamp = 5000;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        assert!(cs.pid_cpu_ticks.is_empty());
        // Unknown new thread should not get a run_start entry
        assert!(!cs.thread_run_start.contains_key(&99));
    }

    #[test]
    fn cswitch_rapid_cycle_accumulates_correctly() {
        let state = make_test_state();
        {
            let mut cs = state.cswitch.lock().unwrap();
            cs.thread_to_pid.insert(10, 100);
            cs.thread_to_pid.insert(20, 200);
            cs.thread_run_start.insert(10, 1000);
        }

        // CSwitch 1: old=10, new=20 at t=2000
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&20u32.to_ne_bytes());
        buf[4..8].copy_from_slice(&10u32.to_ne_bytes());
        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 36;
            record.EventHeader.TimeStamp = 2000;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        // CSwitch 2: old=20, new=10 at t=3500
        buf[0..4].copy_from_slice(&10u32.to_ne_bytes());
        buf[4..8].copy_from_slice(&20u32.to_ne_bytes());
        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 36;
            record.EventHeader.TimeStamp = 3500;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        // PID 100: 1000 ticks from first switch (2000-1000)
        assert_eq!(cs.pid_cpu_ticks.get(&100), Some(&1000i64));
        // PID 200: 1500 ticks from second switch (3500-2000)
        assert_eq!(cs.pid_cpu_ticks.get(&200), Some(&1500i64));
    }

    #[test]
    fn cswitch_thread_end_clears_run_start() {
        let state = make_test_state();
        {
            let mut cs = state.cswitch.lock().unwrap();
            cs.thread_to_pid.insert(10, 100);
            cs.thread_run_start.insert(10, 5000);
        }

        // Thread end for thread 10
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&100u32.to_ne_bytes());
        buf[4..8].copy_from_slice(&10u32.to_ne_bytes());
        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = THREAD_GUID;
            record.EventHeader.EventDescriptor.Opcode = 2;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = 8;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        let cs = state.cswitch.lock().unwrap();
        assert!(!cs.thread_run_start.contains_key(&10));
        assert!(!cs.thread_to_pid.contains_key(&10));
    }

    #[test]
    fn cpu_percentage_calculation() {
        // Simulate: 1 second elapsed, 10MHz QPC, 4 CPUs, PID has 5M ticks
        let qpc_freq: u64 = 10_000_000;
        let num_cpus: usize = 4;
        let elapsed_secs: f64 = 1.0;
        let pid_ticks: i64 = 5_000_000;

        let wall_ticks = elapsed_secs * qpc_freq as f64 * num_cpus as f64;
        let cpu_pct = (pid_ticks as f64 / wall_ticks) * 100.0;

        // 5M / 40M = 12.5%
        assert!((cpu_pct - 12.5).abs() < 0.001);
    }

    #[test]
    fn cpu_percentage_zero_elapsed() {
        let wall_ticks = 0.0f64;
        let pid_ticks = 1000i64;
        let cpu_pct = if wall_ticks > 0.0 {
            (pid_ticks as f64 / wall_ticks) * 100.0
        } else {
            0.0
        };
        assert_eq!(cpu_pct, 0.0);
    }

    #[test]
    fn process_end_cleans_up_cpu_ticks() {
        let state = make_test_state();
        state.names.lock().unwrap().insert(42, "test.exe".into());
        state
            .cswitch
            .lock()
            .unwrap()
            .pid_cpu_ticks
            .insert(42, 99999);

        let ptr_size = mem::size_of::<usize>();
        let payload_len = ptr_size + 4;
        let mut buf = vec![0u8; payload_len];
        buf[ptr_size..ptr_size + 4].copy_from_slice(&42u32.to_ne_bytes());

        unsafe {
            let mut record: EVENT_RECORD = mem::zeroed();
            record.EventHeader.ProviderId = PROCESS_GUID;
            record.EventHeader.EventDescriptor.Opcode = 2;
            record.UserData = buf.as_mut_ptr() as *mut c_void;
            record.UserDataLength = payload_len as u16;
            record.UserContext = &state as *const _ as *mut c_void;
            event_record_callback(&mut record);
        }

        assert!(!state.names.lock().unwrap().contains_key(&42));
        assert!(!state
            .cswitch
            .lock()
            .unwrap()
            .pid_cpu_ticks
            .contains_key(&42));
    }

    // -- Worker thread / session setup --

    #[test]
    fn setup_etw_session_returns_error_or_session() {
        let shared = Arc::new(make_test_state());
        stop_trace_session();
        match setup_etw_session(&shared) {
            Ok(session) => {
                assert_ne!(session.open_handle.Value, u64::MAX);
                assert!(!session.props.is_null());
                cleanup_session(session);
            }
            Err(e) => {
                let msg = format!("{}", e);
                assert!(
                    msg.contains("Access denied") || msg.contains("failed"),
                    "unexpected setup error: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn stop_trace_session_is_idempotent() {
        stop_trace_session();
        stop_trace_session();
    }

    #[test]
    fn worker_thread_signals_ready_before_blocking() {
        let shared = Arc::new(make_test_state());
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();

        let s = shared.clone();
        let sf = shutdown.clone();
        let handle = thread::spawn(move || {
            etw_worker_thread(s, sf, tx);
        });

        let result = rx.recv_timeout(std::time::Duration::from_secs(5));
        assert!(
            result.is_ok(),
            "worker thread did not signal readiness in time"
        );

        if result.unwrap().is_ok() {
            shutdown.store(true, Ordering::Relaxed);
            stop_trace_session();
        }
        let _ = handle.join();
    }

    #[test]
    fn worker_thread_exits_on_shutdown() {
        let shared = Arc::new(make_test_state());
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();

        let s = shared.clone();
        let sf = shutdown.clone();
        let handle = thread::spawn(move || {
            etw_worker_thread(s, sf, tx);
        });

        let result = rx.recv_timeout(std::time::Duration::from_secs(5));
        assert!(result.is_ok());

        shutdown.store(true, Ordering::Relaxed);
        stop_trace_session();

        let join = handle.join();
        assert!(join.is_ok(), "worker thread did not exit cleanly");
    }

    // -- Integration: EtwDataSource --

    #[test]
    fn etw_data_source_new_and_drop() {
        match EtwDataSource::new() {
            Ok(ds) => drop(ds),
            Err(e) => {
                let msg = format!("{}", e);
                assert!(
                    msg.contains("Access denied") || msg.contains("failed"),
                    "unexpected error: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn etw_data_source_get_processes_or_fallback() {
        match EtwDataSource::new() {
            Ok(mut ds) => {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let procs = ds.get_processes().unwrap();
                assert!(!procs.is_empty());
            }
            Err(e) => {
                let msg = format!("{}", e);
                assert!(
                    msg.contains("Access denied") || msg.contains("failed"),
                    "unexpected error: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn etw_new_fails_gracefully_without_admin() {
        let _ = EtwDataSource::new();
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn fuzz_to_wide_always_null_terminated(s in "\\PC{0,100}") {
            let wide = to_wide(&s);
            prop_assert!(!wide.is_empty());
            prop_assert_eq!(*wide.last().unwrap(), 0u16, "must be null-terminated");
        }

        #[test]
        fn fuzz_to_wide_length(s in "\\PC{0,50}") {
            let wide = to_wide(&s);
            // UTF-16 length >= UTF-8 char count, plus null terminator
            prop_assert!(wide.len() > s.chars().count());
        }

        #[test]
        fn fuzz_extract_image_name_no_panic(buf in prop::collection::vec(any::<u8>(), 0..512), pid in any::<u32>()) {
            let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), pid) };
            prop_assert!(!result.is_empty(), "should never return empty string");
            // Must be valid UTF-8 (it's a String so it always is, but verify)
            prop_assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        }

        #[test]
        fn fuzz_extract_image_name_fallback_on_short(len in 0usize..=32, pid in any::<u32>()) {
            let buf = vec![0u8; len];
            let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), pid) };
            // Short buffers should always return PID-X fallback
            let ptr_size = std::mem::size_of::<usize>();
            let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
            if len <= min_offset {
                prop_assert_eq!(result, format!("PID-{}", pid));
            }
        }

        #[test]
        fn fuzz_extract_image_name_valid_payload(
            name_bytes in prop::collection::vec(0x20u8..0x7f, 2..50),
            pid in any::<u32>(),
        ) {
            let ptr_size = std::mem::size_of::<usize>();
            let min_offset = ptr_size + 4 + 4 + 4 + 4 + ptr_size;
            let mut buf = vec![0u8; min_offset + name_bytes.len() + 1]; // +1 for null
            buf[min_offset..min_offset + name_bytes.len()].copy_from_slice(&name_bytes);
            buf[min_offset + name_bytes.len()] = 0; // null terminator

            let result = unsafe { extract_image_name(buf.as_ptr(), buf.len(), pid) };
            let expected = std::str::from_utf8(&name_bytes).unwrap();
            let has_alnum = expected.bytes().any(|b| b.is_ascii_alphanumeric());
            if has_alnum {
                prop_assert_eq!(result, expected);
            } else {
                // No alphanumeric chars → falls back to PID-X
                prop_assert_eq!(result, format!("PID-{}", pid));
            }
        }
    }
}
