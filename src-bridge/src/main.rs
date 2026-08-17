//! DzsSpeedy Bridge — named-pipe server (Rust).

//!

//! Receives text commands from the main DzsSpeedy process:

//!   INJECT <pid>  EJECT <pid>  ENABLE <pid>  DISABLE <pid>

//!   ISENABLED <pid>  SETSPEED <factor>  GETSPEED  SHUTDOWN

//!

//!   STATUS <pid>  — check injection + enabled status

//!

//! Responses:  OK [value]  or  ERROR <message>

#![windows_subsystem = "windows"]

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;

use std::os::windows::ffi::OsStrExt;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{s, HRESULT, PCWSTR};

use windows::Win32::Foundation::{
    CloseHandle, FreeLibrary, GetLastError, LocalFree, BOOL, ERROR_ALREADY_EXISTS,
    ERROR_BAD_LENGTH, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_IO_PENDING,
    ERROR_NO_MORE_FILES, ERROR_PARTIAL_COPY, ERROR_PIPE_CONNECTED, HANDLE, HINSTANCE, HLOCAL, HWND,
    INVALID_HANDLE_VALUE, LPARAM, LRESULT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};

use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};

use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, IsWow64Process2, OpenProcess, ResetEvent, WaitForSingleObject,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    FILE_MAP_READ, FILE_MAP_WRITE, PAGE_READWRITE,
};

use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Thread32First, Thread32Next,
    MODULEENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPTHREAD, THREADENTRY32,
};

use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, NAMED_PIPE_MODE,
    PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386,
    IMAGE_FILE_MACHINE_UNKNOWN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HHOOK, WH_GETMESSAGE, WM_NULL,
};
// ── Helpers ──────────────────────────────────────────────────────────────

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HookThreadCandidate {
    thread_id: u32,
    visible: bool,
}

fn ordered_hook_threads(
    window_candidates: &[HookThreadCandidate],
    process_threads: &[u32],
) -> Vec<u32> {
    let mut ordered = Vec::new();
    for visible in [true, false] {
        for candidate in window_candidates {
            if candidate.visible == visible
                && candidate.thread_id != 0
                && !ordered.contains(&candidate.thread_id)
            {
                ordered.push(candidate.thread_id);
            }
        }
    }
    for &thread_id in process_threads {
        if thread_id != 0 && !ordered.contains(&thread_id) {
            ordered.push(thread_id);
        }
    }
    ordered
}

struct EnumWindowsContext {
    pid: u32,
    candidates: Vec<HookThreadCandidate>,
}

unsafe extern "system" fn collect_target_window_threads(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut EnumWindowsContext) };
    let mut window_pid = 0u32;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };
    if window_pid == context.pid && thread_id != 0 {
        let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
        if !context
            .candidates
            .iter()
            .any(|candidate| candidate.thread_id == thread_id)
        {
            context
                .candidates
                .push(HookThreadCandidate { thread_id, visible });
        }
    }
    BOOL::from(true)
}

fn target_process_threads(pid: u32) -> Result<Vec<u32>, String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(|error| {
        format!("CreateToolhelp32Snapshot(threads, pid={pid}) failed: {error:?}")
    })?;
    let mut threads = Vec::new();
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };

    let first = unsafe { Thread32First(snapshot, &mut entry) };
    if let Err(error) = first {
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
            return Ok(threads);
        }
        return Err(format!("Thread32First(pid={pid}) failed: {error:?}"));
    }

    loop {
        if entry.th32OwnerProcessID == pid && entry.th32ThreadID != 0 {
            threads.push(entry.th32ThreadID);
        }
        match unsafe { Thread32Next(snapshot, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) => break,
            Err(error) => {
                unsafe {
                    let _ = CloseHandle(snapshot);
                }
                return Err(format!("Thread32Next(pid={pid}) failed: {error:?}"));
            }
        }
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    Ok(threads)
}

fn target_hook_threads(pid: u32) -> Result<Vec<u32>, String> {
    let mut context = EnumWindowsContext {
        pid,
        candidates: Vec::new(),
    };
    unsafe {
        EnumWindows(
            Some(collect_target_window_threads),
            LPARAM(&mut context as *mut EnumWindowsContext as isize),
        )
        .map_err(|error| format!("EnumWindows(pid={pid}) failed: {error:?}"))?;
    }
    let process_threads = target_process_threads(pid)?;
    let candidates = ordered_hook_threads(&context.candidates, &process_threads);
    if candidates.is_empty() {
        return Err(format!(
            "no hook thread candidate is available for pid={pid}"
        ));
    }
    Ok(candidates)
}

fn exe_dir() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("current_exe() failed: {error}"))?;
    executable
        .parent()
        .map(|path| path.to_path_buf())
        .ok_or_else(|| {
            format!(
                "current executable has no parent directory: {}",
                executable.display()
            )
        })
}

#[cfg(target_arch = "x86_64")]
const BRIDGE_IS64: bool = true;

#[cfg(target_arch = "x86")]
const BRIDGE_IS64: bool = false;

/// Query target architecture with query-only rights. Never default to x64 on
/// failure: that routes a 32-bit target to the wrong bridge/DLL path.
fn query_process_is64(pid: u32) -> Result<bool, String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .or_else(|_| OpenProcess(PROCESS_QUERY_INFORMATION, false, pid))
            .map_err(|e| format!("OpenProcess(query architecture, pid={pid}): {e:?}"))?;

        let mut process_machine = IMAGE_FILE_MACHINE_UNKNOWN;
        let mut native_machine = IMAGE_FILE_MACHINE_UNKNOWN;
        let result = IsWow64Process2(h, &mut process_machine, Some(&mut native_machine));
        let error = result.as_ref().err().map(|_| GetLastError());
        let _ = CloseHandle(h);

        if let Some(error) = error {
            return Err(format!("IsWow64Process2(pid={pid}) failed: {error:?}"));
        }
        match process_machine {
            IMAGE_FILE_MACHINE_I386 => Ok(false),
            IMAGE_FILE_MACHINE_AMD64 => Ok(true),
            IMAGE_FILE_MACHINE_ARM64 => Err(format!(
                "pid={pid} is native ARM64; this build only provides x86/x64 bridges"
            )),
            IMAGE_FILE_MACHINE_UNKNOWN => match native_machine {
                IMAGE_FILE_MACHINE_AMD64 => Ok(true),
                IMAGE_FILE_MACHINE_I386 => Ok(false),
                IMAGE_FILE_MACHINE_ARM64 => Err(format!(
                    "pid={pid} is native ARM64; this build only provides x86/x64 bridges"
                )),
                IMAGE_FILE_MACHINE_UNKNOWN => Err(format!(
                    "IsWow64Process2(pid={pid}) returned unknown native architecture"
                )),
                other => Err(format!(
                    "IsWow64Process2(pid={pid}) returned unsupported native machine 0x{:04x}",
                    other.0
                )),
            },
            other => Err(format!(
                "IsWow64Process2(pid={pid}) returned unsupported process machine 0x{:04x}",
                other.0
            )),
        }
    }
}

#[cfg(target_arch = "x86_64")]
const OWN_SPEEDPATCH: &str = "speedpatch64.dll";

#[cfg(target_arch = "x86")]
const OWN_SPEEDPATCH: &str = "speedpatch32.dll";

fn speedpatch_dll(is64: bool) -> &'static str {
    if is64 {
        "speedpatch64.dll"
    } else {
        "speedpatch32.dll"
    }
}

/// `DzsSpeedy.<pid>` — same name as speedpatch `GetProcessFileMapName`.
fn speedpatch_map_name(pid: u32) -> Vec<u16> {
    to_wide(&format!("DzsSpeedy.{pid}"))
}

/// Global speed factor — must match speedpatch `GLOBAL_SPEED_MAP_NAME` (cross-process).
fn global_speed_map_name() -> Vec<u16> {
    to_wide("DzsSpeedy.SpeedFactor")
}

fn write_global_speed_factor(factor: f64) -> Result<(), String> {
    let name = global_speed_map_name();

    let size = std::mem::size_of::<f64>();

    unsafe {
        let h = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            size as u32,
            PCWSTR::from_raw(name.as_ptr()),
        )
        .map_err(|e| format!("CreateFileMapping(DzsSpeedy.SpeedFactor): {e:?}"))?;

        let view = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, size);

        if view.Value.is_null() {
            let _ = CloseHandle(h);

            return Err("MapViewOfFile(DzsSpeedy.SpeedFactor) failed".into());
        }

        *(view.Value as *mut f64) = factor;

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        Ok(())
    }
}

fn read_global_speed_factor() -> Option<f64> {
    let name = global_speed_map_name();

    let size = std::mem::size_of::<f64>();

    unsafe {
        let h = OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR::from_raw(name.as_ptr())).ok()?;

        let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, size);

        if view.Value.is_null() {
            let _ = CloseHandle(h);

            return None;
        }

        let v = *(view.Value as *const f64);

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        if v > 0.0 && v <= 10000.0 {
            Some(v)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeedpatchState {
    Initializing,
    Disabled,
    Enabled,
    Failed,
}

enum InjectionStatus {
    Initializing,
    Disabled,
    Enabled,
    Failed(String),
    NotInjected,
}

#[derive(Debug, Clone, Copy)]
struct SpeedpatchHandshake {
    state: SpeedpatchState,
    init_result: u32,
    hook_thread_id: u32,
    callback_completed: bool,
}

const SP_STATE_INITIALIZING: u32 = 0x49;
const SP_STATE_DISABLED: u32 = 0x44;
const SP_STATE_ENABLED: u32 = 0x45;
const SP_STATE_FAILED: u32 = 0x46;
const SP_HOOK_COMPLETED_BIT: u32 = 1 << 31;

/// Read the DLL-owned handshake mapping.
///
/// `Ok(None)` is deliberately reserved for an absent mapping. Permission and
/// mapping failures are status probe errors, not evidence that the DLL is gone.
fn read_speedpatch_handshake(pid: u32) -> Result<Option<SpeedpatchHandshake>, String> {
    let name = speedpatch_map_name(pid);

    unsafe {
        let h = match OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR::from_raw(name.as_ptr())) {
            Ok(h) => h,
            Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "OpenFileMapping(DzsSpeedy.{pid}, FILE_MAP_READ) failed: {e:?}"
                ));
            }
        };

        let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, std::mem::size_of::<u32>() * 3);

        if view.Value.is_null() {
            let gle = GetLastError();
            let _ = CloseHandle(h);

            return Err(format!(
                "MapViewOfFile(DzsSpeedy.{pid}, FILE_MAP_READ) failed: gle={} (0x{:08x})",
                gle.0, gle.0
            ));
        }

        let values = view.Value as *const AtomicU32;
        let state = (&*values).load(Ordering::Acquire);
        let init_result = (&*values.add(1)).load(Ordering::Acquire);
        let raw_hook_thread_id = (&*values.add(2)).load(Ordering::Acquire);
        let callback_completed = raw_hook_thread_id & SP_HOOK_COMPLETED_BIT != 0;
        let hook_thread_id = raw_hook_thread_id & !SP_HOOK_COMPLETED_BIT;

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        match state {
            0 => Ok(Some(SpeedpatchHandshake {
                state: SpeedpatchState::Initializing,
                init_result,
                hook_thread_id,
                callback_completed,
            })),
            SP_STATE_INITIALIZING => Ok(Some(SpeedpatchHandshake {
                state: SpeedpatchState::Initializing,
                init_result,
                hook_thread_id,
                callback_completed,
            })),
            SP_STATE_DISABLED => Ok(Some(SpeedpatchHandshake {
                state: SpeedpatchState::Disabled,
                init_result,
                hook_thread_id,
                callback_completed,
            })),
            SP_STATE_ENABLED => Ok(Some(SpeedpatchHandshake {
                state: SpeedpatchState::Enabled,
                init_result,
                hook_thread_id,
                callback_completed,
            })),
            SP_STATE_FAILED => Ok(Some(SpeedpatchHandshake {
                state: SpeedpatchState::Failed,
                init_result,
                hook_thread_id,
                callback_completed,
            })),
            value => Err(format!(
                "DzsSpeedy.{pid} contains unsupported state value 0x{value:08x}; DLL/bridge protocol mismatch"
            )),
        }
    }
}

fn write_speedpatch_enabled(pid: u32, enabled: bool) -> Result<(), String> {
    let name = speedpatch_map_name(pid);

    unsafe {
        let h = OpenFileMappingW(FILE_MAP_WRITE.0, false, PCWSTR::from_raw(name.as_ptr()))
            .map_err(|e| format!("OpenFileMapping(DzsSpeedy.{pid}): {e:?}"))?;

        let view = MapViewOfFile(h, FILE_MAP_WRITE, 0, 0, std::mem::size_of::<u32>());

        if view.Value.is_null() {
            let gle = GetLastError();
            let _ = CloseHandle(h);
            return Err(format!(
                "MapViewOfFile(DzsSpeedy.{pid}, FILE_MAP_WRITE) failed: gle={} (0x{:08x})",
                gle.0, gle.0
            ));
        }

        let state = &*(view.Value as *const AtomicU32);
        let desired = if enabled {
            SP_STATE_ENABLED
        } else {
            SP_STATE_DISABLED
        };
        let result = loop {
            let current = state.load(Ordering::Acquire);
            match current {
                value if value == desired => break Ok(()),
                SP_STATE_FAILED => {
                    break Err(format!(
                        "SP_Initialize failed for pid={pid}; restart the target before changing state"
                    ));
                }
                SP_STATE_INITIALIZING if enabled => {
                    break Err(format!(
                        "INJECTION_PENDING: SP_Initialize is still running for pid={pid}"
                    ));
                }
                SP_STATE_INITIALIZING | SP_STATE_DISABLED | SP_STATE_ENABLED => {
                    if state
                        .compare_exchange(current, desired, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        break Ok(());
                    }
                }
                value => {
                    break Err(format!(
                        "DzsSpeedy.{pid} contains unsupported state value 0x{value:08x}; DLL/bridge protocol mismatch"
                    ));
                }
            }
        };

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        result
    }
}

fn tracked_targets() -> &'static Mutex<HashSet<u32>> {
    static TARGETS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn track_target(pid: u32) {
    if let Ok(mut targets) = tracked_targets().lock() {
        targets.insert(pid);
    }
}

fn untrack_target(pid: u32) {
    if let Ok(mut targets) = tracked_targets().lock() {
        targets.remove(&pid);
    }
}

#[derive(Debug, Clone, Copy)]
enum InjectionStage {
    Loading,
    Initializing,
}

fn injection_stages() -> &'static Mutex<HashMap<u32, InjectionStage>> {
    static STAGES: OnceLock<Mutex<HashMap<u32, InjectionStage>>> = OnceLock::new();
    STAGES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn injection_failures() -> &'static Mutex<HashMap<u32, String>> {
    static FAILURES: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    FAILURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_injection_stage(pid: u32, stage: InjectionStage) {
    if let Ok(mut stages) = injection_stages().lock() {
        stages.insert(pid, stage);
    }
}

fn injection_stage(pid: u32) -> Option<InjectionStage> {
    injection_stages()
        .lock()
        .ok()
        .and_then(|stages| stages.get(&pid).copied())
}

fn clear_injection_stage(pid: u32) {
    if let Ok(mut stages) = injection_stages().lock() {
        stages.remove(&pid);
    }
}

fn record_injection_failure(pid: u32, detail: String) {
    clear_injection_stage(pid);
    if let Ok(mut failures) = injection_failures().lock() {
        failures.insert(pid, detail);
    }
}

fn injection_failure(pid: u32) -> Option<String> {
    injection_failures()
        .lock()
        .ok()
        .and_then(|failures| failures.get(&pid).cloned())
}

fn clear_injection_failure(pid: u32) {
    if let Ok(mut failures) = injection_failures().lock() {
        failures.remove(&pid);
    }
}

const OPERATION_SHUTDOWN_BIT: u32 = 1 << 31;
const OPERATION_COUNT_MASK: u32 = OPERATION_SHUTDOWN_BIT - 1;
static REMOTE_OPERATION_STATE: AtomicU32 = AtomicU32::new(0);

struct RemoteOperationLease;

fn try_acquire_operation_slot(state: &AtomicU32) -> Result<(), String> {
    loop {
        let current = state.load(Ordering::Acquire);
        if current & OPERATION_SHUTDOWN_BIT != 0 {
            return Err("bridge shutdown is in progress; operation was not started".into());
        }
        if current & OPERATION_COUNT_MASK == OPERATION_COUNT_MASK {
            return Err("too many concurrent bridge operations".into());
        }
        if state
            .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(());
        }
    }
}

impl RemoteOperationLease {
    fn try_acquire() -> Result<Self, String> {
        try_acquire_operation_slot(&REMOTE_OPERATION_STATE)?;
        Ok(Self)
    }
}

impl Drop for RemoteOperationLease {
    fn drop(&mut self) {
        REMOTE_OPERATION_STATE.fetch_sub(1, Ordering::AcqRel);
    }
}

fn shutdown_requested() -> bool {
    REMOTE_OPERATION_STATE.load(Ordering::Acquire) & OPERATION_SHUTDOWN_BIT != 0
}

fn request_shutdown() {
    REMOTE_OPERATION_STATE.fetch_or(OPERATION_SHUTDOWN_BIT, Ordering::AcqRel);
}

fn active_remote_operations() -> u32 {
    REMOTE_OPERATION_STATE.load(Ordering::Acquire) & OPERATION_COUNT_MASK
}

/// How long shutdown waits for in-flight operation leases before exiting
/// anyway. In-flight completions observe the shutdown bit and disable
/// speedpatch, so exiting early is safe: it can never leave a target
/// accelerated.
const SHUTDOWN_DRAIN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);

/// Wait until all remote-operation leases are released, or until `max_wait`
/// elapses. Returns true when drained, false on deadline. This is the bound
/// that prevents a stuck pending-injection monitor from pinning the bridge in
/// its shutdown state forever (the "bridge shutdown is in progress" zombie).
fn wait_for_operations_drain(state: &AtomicU32, max_wait: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + max_wait;
    loop {
        if state.load(Ordering::Acquire) & OPERATION_COUNT_MASK == 0 {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Commands whose responses must change to an explicit error once shutdown
/// starts, so a dying bridge can never masquerade as healthy.
fn command_is_shutdown_gated(cmd: &str) -> bool {
    matches!(cmd, "GETSPEED" | "PING" | "VERSION")
}

fn finish_injection_success(pid: u32) {
    clear_injection_stage(pid);
    clear_injection_failure(pid);
    track_target(pid);

    if shutdown_requested() {
        match write_speedpatch_enabled(pid, false) {
            Ok(()) => dbg_log(&format!(
                "injection completed during shutdown; disabled speedpatch for pid={pid}"
            )),
            Err(error) => dbg_log(&format!(
                "injection completed during shutdown, but disable failed for pid={pid}: {error}"
            )),
        }
    }
}

fn disable_tracked_targets() {
    let pids = tracked_targets()
        .lock()
        .map(|targets| targets.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for pid in pids {
        match write_speedpatch_enabled(pid, false) {
            Ok(()) => dbg_log(&format!("shutdown: disabled speedpatch for pid={pid}")),
            Err(error) => dbg_log(&format!(
                "shutdown: could not disable speedpatch for pid={pid}: {error}"
            )),
        }
    }
}

// ── Core operations ──────────────────────────────────────────────────────

#[derive(Debug)]
struct RemoteModule {
    base: usize,
    path: String,
}

struct InitializationError {
    detail: String,
}

type WindowsHookProc = unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT;

struct LocalSecurityDescriptor(PSECURITY_DESCRIPTOR);

impl LocalSecurityDescriptor {
    fn completion_event() -> Result<Self, String> {
        // Administrators/SYSTEM retain full control. Any authenticated target
        // may only signal the event, and the low integrity label permits that
        // write even when the bridge itself is elevated.
        let sddl = to_wide("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x0002;;;AU)S:(ML;;NW;;;LW)");
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR::from_raw(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|error| format!("build completion-event security descriptor failed: {error:?}"))?;
        Ok(Self(descriptor))
    }
}

impl Drop for LocalSecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(HLOCAL(self.0 .0));
        }
    }
}

struct HookCompletionEvent {
    handle: HANDLE,
    pid: u32,
}

// Kernel event handles are valid across threads. This wrapper owns exactly
// one handle and exposes only reset/wait semantics.
unsafe impl Send for HookCompletionEvent {}

impl HookCompletionEvent {
    fn create(pid: u32) -> Result<Self, String> {
        let name = to_wide(&format!("DzsSpeedyHookComplete.{pid}"));
        let descriptor = LocalSecurityDescriptor::completion_event()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: BOOL::from(false),
        };
        let handle = unsafe {
            CreateEventW(
                Some(&attributes),
                true,
                false,
                PCWSTR::from_raw(name.as_ptr()),
            )
        }
        .map_err(|error| format!("CreateEventW(DzsSpeedyHookComplete.{pid}) failed: {error:?}"))?;
        if let Err(error) = unsafe { ResetEvent(handle) } {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(format!(
                "ResetEvent(DzsSpeedyHookComplete.{pid}) failed: {error:?}"
            ));
        }
        Ok(Self { handle, pid })
    }

    fn is_signaled(&self) -> Result<bool, String> {
        let wait = unsafe { WaitForSingleObject(self.handle, 0) };
        if wait == WAIT_OBJECT_0 {
            return Ok(true);
        }
        if wait == WAIT_TIMEOUT {
            return Ok(false);
        }
        if wait == WAIT_FAILED {
            let error = unsafe { GetLastError() };
            return Err(format!(
                "WaitForSingleObject(DzsSpeedyHookComplete.{}) failed: win32_error={}",
                self.pid, error.0
            ));
        }
        Err(format!(
            "WaitForSingleObject(DzsSpeedyHookComplete.{}) returned 0x{:08x}",
            self.pid, wait.0
        ))
    }
}

impl Drop for HookCompletionEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

struct TargetProcessHandle {
    handle: HANDLE,
    pid: u32,
}

// Kernel process handles are valid across threads. This wrapper owns exactly
// one handle and only exposes wait operations.
unsafe impl Send for TargetProcessHandle {}

impl TargetProcessHandle {
    fn open(pid: u32) -> Result<Self, String> {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.map_err(|error| {
            if error.code() == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) {
                format!("TARGET_EXITED: pid={pid} exited before hook installation")
            } else {
                format!("OpenProcess(PROCESS_SYNCHRONIZE, pid={pid}) failed: {error:?}")
            }
        })?;
        Ok(Self { handle, pid })
    }

    fn has_exited(&self) -> Result<bool, String> {
        let wait = unsafe { WaitForSingleObject(self.handle, 0) };
        if wait == WAIT_OBJECT_0 {
            return Ok(true);
        }
        if wait == WAIT_TIMEOUT {
            return Ok(false);
        }
        if wait == WAIT_FAILED {
            let error = unsafe { GetLastError() };
            return Err(format!(
                "WaitForSingleObject(target pid={}) failed: win32_error={}",
                self.pid, error.0
            ));
        }
        Err(format!(
            "WaitForSingleObject(target pid={}) returned 0x{:08x}",
            self.pid, wait.0
        ))
    }
}

impl Drop for TargetProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HandshakeProgress {
    Pending,
    Complete {
        callback_thread: u32,
        state: SpeedpatchState,
    },
    Failed {
        callback_thread: u32,
        init_result: u32,
    },
    Invalid(String),
}

fn classify_handshake(handshake: SpeedpatchHandshake) -> HandshakeProgress {
    if !handshake.callback_completed
        || handshake.hook_thread_id == 0
        || handshake.init_result == ERROR_IO_PENDING.0
    {
        return HandshakeProgress::Pending;
    }

    match handshake.state {
        SpeedpatchState::Initializing => HandshakeProgress::Pending,
        SpeedpatchState::Enabled | SpeedpatchState::Disabled if handshake.init_result == 0 => {
            HandshakeProgress::Complete {
                callback_thread: handshake.hook_thread_id,
                state: handshake.state,
            }
        }
        SpeedpatchState::Failed if handshake.init_result != 0 => HandshakeProgress::Failed {
            callback_thread: handshake.hook_thread_id,
            init_result: handshake.init_result,
        },
        SpeedpatchState::Enabled | SpeedpatchState::Disabled if handshake.init_result != 0 => {
            // The callback publishes its result before the terminal state. A
            // concurrent logical disable may therefore expose this transient
            // pair; wait for FAILED instead of replacing the native error with
            // a protocol-mismatch message.
            HandshakeProgress::Pending
        }
        _ => HandshakeProgress::Invalid(format!(
            "inconsistent callback handshake: state={:?}, init_result=0x{:08x}, callback_thread={}",
            handshake.state, handshake.init_result, handshake.hook_thread_id
        )),
    }
}

struct InstalledWindowsHook {
    hook: HHOOK,
    thread_id: u32,
}

struct InstalledWindowsHooks {
    hooks: Vec<InstalledWindowsHook>,
    local_module: Option<windows::Win32::Foundation::HMODULE>,
}

// HHOOK and this process-local HMODULE may be released from another bridge
// thread. The container is move-only and remains their sole owner.
unsafe impl Send for InstalledWindowsHooks {}

impl InstalledWindowsHooks {
    fn cleanup(&mut self) -> Result<(), String> {
        let mut retained = Vec::new();
        let mut errors = Vec::new();
        unsafe {
            for installed in self.hooks.drain(..) {
                if let Err(error) = UnhookWindowsHookEx(installed.hook) {
                    errors.push(format!(
                        "thread={}, hook={:?}: {error:?}",
                        installed.thread_id, installed.hook
                    ));
                    retained.push(installed);
                }
            }
            self.hooks = retained;
            if self.hooks.is_empty() {
                if let Some(module) = self.local_module.take() {
                    let _ = FreeLibrary(module);
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "UnhookWindowsHookEx failed; retaining the local DLL reference for active hooks: {}",
                errors.join("; ")
            ))
        }
    }

    fn release_after_target_exit(&mut self) {
        self.hooks.clear();
        if let Some(module) = self.local_module.take() {
            unsafe {
                let _ = FreeLibrary(module);
            }
        }
    }
}

impl Drop for InstalledWindowsHooks {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            dbg_log(&format!("hook cleanup incomplete: {error}"));
        }
    }
}

enum HookWaitOutcome {
    Complete {
        callback_thread: u32,
        state: SpeedpatchState,
    },
    Pending {
        callback_thread: u32,
        detail: String,
    },
}

enum HookInjectionOutcome {
    Complete {
        callback_thread: u32,
        state: SpeedpatchState,
    },
    Pending {
        process: TargetProcessHandle,
        callback_thread: u32,
        detail: String,
        hooks: Option<InstalledWindowsHooks>,
        completion: HookCompletionEvent,
        initial_log_len: u64,
    },
}

fn initialization_failure_detail(pid: u32, init_result: u32) -> String {
    match decode_initialize_exit_code(init_result) {
        Ok(()) => {
            format!("speedpatch reported FAILED for pid={pid} without an initialization error code")
        }
        Err(error) => error.detail,
    }
}

fn persisted_initialization_failure(pid: u32, handshake: SpeedpatchHandshake) -> String {
    injection_failure(pid).unwrap_or_else(|| {
        if handshake.init_result != 0 {
            format!(
                "SP_HookProc initialization failed for pid={pid}, callback_thread={}: {}",
                handshake.hook_thread_id,
                initialization_failure_detail(pid, handshake.init_result)
            )
        } else {
            format!(
                "SP_Initialize failed for pid={pid} without a persisted native error; restart the target before retrying"
            )
        }
    })
}

fn speedpatch_log_tail(pid: u32, initial_len: u64) -> Option<String> {
    let path = std::env::temp_dir().join(format!("dzsspeedy-speedpatch-{pid}.log"));
    let bytes = std::fs::read(path).ok()?;
    let start = usize::try_from(initial_len).ok()?.min(bytes.len());
    let start = start + (start % 2);
    let end = bytes.len() - (bytes.len() % 2);
    if start > end {
        return None;
    }
    Some(String::from_utf16_lossy(
        &bytes[start..end]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    ))
}

fn latest_speedpatch_install_failure(pid: u32, initial_len: u64) -> Option<String> {
    speedpatch_log_tail(pid, initial_len)?
        .lines()
        .rev()
        .find(|line| line.contains("SP_Install:") && line.contains("FAILED"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

fn completed_without_terminal_handshake_detail(
    pid: u32,
    callback_thread: u32,
    initial_log_len: u64,
) -> String {
    let native_detail = latest_speedpatch_install_failure(pid, initial_log_len)
        .map(|line| format!(" Native DLL error: {line}"))
        .unwrap_or_default();
    format!(
        "SP_HookProc signaled completion for pid={pid}, callback_thread={callback_thread}, but the terminal handshake is unavailable.{native_detail} DLL log: {}",
        std::env::temp_dir()
            .join(format!("dzsspeedy-speedpatch-{pid}.log"))
            .display()
    )
}

/// How long the bridge waits before checking whether the hook DLL actually
/// entered the target. When the target accepts a WH_GETMESSAGE hook but never
/// dispatches it (windowless game loop, or protection blocking the hook-DLL
/// injection), the DLL never loads and SP_HookProc can never run. Failing fast
/// here turns a silent 15s+ wait into an actionable error, and keeps the
/// pending-injection monitor from being spawned for a target that can never
/// complete the chain.
const DLL_INJECTION_PROBE_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

fn wait_for_hook_callback(
    pid: u32,
    process: &TargetProcessHandle,
    completion: &HookCompletionEvent,
    hooked_threads: &[u32],
    posted_threads: &[u32],
    initial_log_len: u64,
    dll_path: &str,
) -> Result<HookWaitOutcome, String> {
    let mut observed_handshake = false;
    let mut callback_thread = 0;
    let mut terminal_result = None;
    let dll_probe_deadline = std::time::Instant::now() + DLL_INJECTION_PROBE_GRACE;
    let mut dll_probe_done = false;

    for _ in 0..300 {
        if process.has_exited()? {
            return Err(format!(
                "TARGET_EXITED: pid={pid} exited while waiting for SP_HookProc"
            ));
        }

        if let Some(handshake) = read_speedpatch_handshake(pid)? {
            observed_handshake |= handshake.hook_thread_id != 0;
            callback_thread = handshake.hook_thread_id;
            match classify_handshake(handshake) {
                HandshakeProgress::Complete {
                    callback_thread,
                    state,
                } => {
                    dbg_log(&format!(
                        "inject_via_windows_hook: initialized pid={pid} callback_thread={callback_thread} state={state:?} result=0x{:08x}",
                        handshake.init_result
                    ));
                    terminal_result = Some(Ok(HookWaitOutcome::Complete {
                        callback_thread,
                        state,
                    }));
                }
                HandshakeProgress::Failed {
                    callback_thread,
                    init_result,
                } => {
                    terminal_result = Some(Err(format!(
                        "SP_HookProc initialization failed for pid={pid}, callback_thread={callback_thread}: {}",
                        initialization_failure_detail(pid, init_result)
                    )));
                }
                HandshakeProgress::Invalid(detail) => terminal_result = Some(Err(detail)),
                HandshakeProgress::Pending => {
                    set_injection_stage(pid, InjectionStage::Initializing);
                }
            }
        }

        let completion_signaled = match completion.is_signaled() {
            Ok(signaled) => signaled,
            Err(error) => {
                dbg_log(&format!(
                    "inject_via_windows_hook: completion event probe failed for pid={pid}: {error}"
                ));
                false
            }
        };

        // When the DLL cannot create its handshake mapping, the completion
        // event is the only protocol object left. It is emitted after
        // CallNextHookEx, so it is sufficient to release the bridge-owned hook
        // in this mapping-absent failure path.
        let handshake_present = match read_speedpatch_handshake(pid) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(error) => {
                dbg_log(&format!(
                    "inject_via_windows_hook: handshake probe failed after completion for pid={pid}: {error}"
                ));
                true
            }
        };

        if terminal_result.is_some() {
            return terminal_result.take().unwrap_or_else(|| {
                Err(completed_without_terminal_handshake_detail(
                    pid,
                    callback_thread,
                    initial_log_len,
                ))
            });
        }
        if completion_signaled && !handshake_present {
            dbg_log(&format!(
                "inject_via_windows_hook: completion event signaled without a handshake for pid={pid}"
            ));
            return Err(completed_without_terminal_handshake_detail(
                pid,
                callback_thread,
                initial_log_len,
            ));
        }

        if shutdown_requested() {
            if observed_handshake || terminal_result.is_some() {
                return Ok(HookWaitOutcome::Pending {
                    callback_thread,
                    detail: format!(
                        "bridge shutdown is waiting for SP_HookProc initialization in pid={pid}"
                    ),
                });
            }
            return Err(format!(
                "bridge shutdown interrupted SetWindowsHookExW injection before callback entry for pid={pid}, hooked_threads={hooked_threads:?}"
            ));
        }

        // Fast-fail probe: if the hook DLL never appears in the target within
        // DLL_INJECTION_PROBE_GRACE, SP_HookProc can never run there — the
        // hook was accepted but the injection was rejected or is never
        // dispatched. Report that instead of waiting out the full callback
        // window and then leaving the pending monitor spinning forever.
        if !dll_probe_done
            && !observed_handshake
            && !completion_signaled
            && std::time::Instant::now() >= dll_probe_deadline
        {
            dll_probe_done = true;
            let dll_name = speedpatch_dll(BRIDGE_IS64);
            match find_remote_module(pid, dll_name, Some(dll_path)) {
                Ok(Some(module)) => {
                    dbg_log(&format!(
                        "inject_via_windows_hook: {dll_name} present in pid={pid} at 0x{:x} after {}s; handshake still pending",
                        module.base, DLL_INJECTION_PROBE_GRACE.as_secs()
                    ));
                }
                Ok(None) => {
                    if !process.has_exited()? {
                        return Err(format!(
                            "SetWindowsHookExW installed for pid={pid}, hooked_threads={hooked_threads:?}, posted_threads={posted_threads:?}, but {dll_name} was not loaded into the target within {}s of waking it. The target either rejects the hook-DLL injection (anti-cheat protection) or never dispatches WH_GETMESSAGE callbacks, so SP_HookProc can never run. DLL log: {}",
                            DLL_INJECTION_PROBE_GRACE.as_secs(),
                            std::env::temp_dir()
                                .join(format!("dzsspeedy-speedpatch-{pid}.log"))
                                .display()
                        ));
                    }
                }
                Err(error) => {
                    dbg_log(&format!(
                        "inject_via_windows_hook: module probe failed for pid={pid}: {error}"
                    ));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    if process.has_exited()? {
        return Err(format!(
            "TARGET_EXITED: pid={pid} exited while waiting for SP_HookProc"
        ));
    }
    if completion.is_signaled()? && read_speedpatch_handshake(pid)?.is_none() {
        return terminal_result.take().unwrap_or_else(|| {
            Err(completed_without_terminal_handshake_detail(
                pid,
                callback_thread,
                initial_log_len,
            ))
        });
    }
    if observed_handshake || terminal_result.is_some() {
        return Ok(HookWaitOutcome::Pending {
            callback_thread,
            detail: format!(
                "SP_HookProc entered pid={pid} on thread={callback_thread}, but its kernel completion event is still pending after 15s"
            ),
        });
    }

    let native_detail = latest_speedpatch_install_failure(pid, initial_log_len)
        .map(|line| format!(" Native DLL error: {line}"))
        .unwrap_or_default();
    Err(format!(
        "SetWindowsHookExW installed for pid={pid}, hooked_threads={hooked_threads:?}, posted_threads={posted_threads:?}, but SP_HookProc did not publish its handshake within 15s.{native_detail} DLL log: {}",
        std::env::temp_dir()
            .join(format!("dzsspeedy-speedpatch-{pid}.log"))
            .display()
    ))
}

fn inject_via_windows_hook(pid: u32, dll_path: &str) -> Result<HookInjectionOutcome, String> {
    let mut process = Some(TargetProcessHandle::open(pid)?);
    let completion = HookCompletionEvent::create(pid)?;
    let thread_ids = target_hook_threads(pid)?;
    let log_path = std::env::temp_dir().join(format!("dzsspeedy-speedpatch-{pid}.log"));
    let initial_log_len = std::fs::metadata(log_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let dll_wide = to_wide(dll_path);
    let local_module = unsafe { LoadLibraryW(PCWSTR::from_raw(dll_wide.as_ptr())) }
        .map_err(|error| format!("LoadLibraryW(local hook DLL {dll_path}) failed: {error:?}"))?;
    let hook_proc = match unsafe { GetProcAddress(local_module, s!("SP_HookProc")) } {
        Some(proc) => proc,
        None => {
            unsafe {
                let _ = FreeLibrary(local_module);
            }
            return Err("GetProcAddress(SP_HookProc) failed".into());
        }
    };
    let hook_proc = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, WindowsHookProc>(hook_proc)
    };

    let mut installed = Vec::new();
    let mut install_errors = Vec::new();
    for &thread_id in &thread_ids {
        match unsafe {
            SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(hook_proc),
                HINSTANCE(local_module.0),
                thread_id,
            )
        } {
            Ok(hook) => installed.push(InstalledWindowsHook { hook, thread_id }),
            Err(error) => install_errors.push(format!("thread={thread_id}: {error:?}")),
        }
    }
    if installed.is_empty() {
        unsafe {
            let _ = FreeLibrary(local_module);
        }
        return Err(format!(
            "SetWindowsHookExW(WH_GETMESSAGE, pid={pid}) failed for all candidates {:?}: {}",
            thread_ids,
            install_errors.join("; ")
        ));
    }
    let hooks = InstalledWindowsHooks {
        hooks: installed,
        local_module: Some(local_module),
    };
    let mut hooks = hooks;
    let hooked_threads = hooks
        .hooks
        .iter()
        .map(|installed| installed.thread_id)
        .collect::<Vec<_>>();
    dbg_log(&format!(
        "inject_via_windows_hook: installed pid={pid} candidates={thread_ids:?} hooked={hooked_threads:?} install_errors={install_errors:?}"
    ));

    let mut posted_threads = Vec::new();
    let mut post_errors = Vec::new();
    for installed in &hooks.hooks {
        match unsafe { PostThreadMessageW(installed.thread_id, WM_NULL, None, None) } {
            Ok(()) => posted_threads.push(installed.thread_id),
            Err(error) => {
                post_errors.push(format!("thread={}: {error:?}", installed.thread_id));
            }
        }
    }
    let wait_result = if posted_threads.is_empty() {
        Err(format!(
            "SetWindowsHookExW installed for pid={pid}, but PostThreadMessageW(WM_NULL) failed for all hooked threads {hooked_threads:?}: {}",
            post_errors.join("; ")
        ))
    } else {
        dbg_log(&format!(
            "inject_via_windows_hook: woke pid={pid} posted={posted_threads:?} post_errors={post_errors:?}"
        ));
        wait_for_hook_callback(
            pid,
            process.as_ref().expect("target process handle missing"),
            &completion,
            &hooked_threads,
            &posted_threads,
            initial_log_len,
            dll_path,
        )
    };

    let wait_result = match wait_result {
        Ok(HookWaitOutcome::Pending {
            callback_thread,
            detail,
        }) => {
            return Ok(HookInjectionOutcome::Pending {
                process: process.take().expect("target process handle missing"),
                callback_thread,
                detail,
                hooks: Some(hooks),
                completion,
                initial_log_len,
            });
        }
        result => result,
    };

    let callback_finished = matches!(&wait_result, Ok(HookWaitOutcome::Complete { .. }))
        || read_speedpatch_handshake(pid)
            .ok()
            .flatten()
            .is_some_and(|handshake| handshake.callback_completed)
        || (wait_result.is_err()
            && completion.is_signaled().unwrap_or(false)
            && matches!(read_speedpatch_handshake(pid), Ok(None)));
    if !callback_finished {
        let detail = match &wait_result {
            Ok(HookWaitOutcome::Pending { detail, .. }) => detail.clone(),
            Ok(HookWaitOutcome::Complete { .. }) => {
                "SP_HookProc completed without a completion signal".to_string()
            }
            Err(error) => error.clone(),
        };
        let callback_thread = match &wait_result {
            Ok(HookWaitOutcome::Complete {
                callback_thread, ..
            })
            | Ok(HookWaitOutcome::Pending {
                callback_thread, ..
            }) => *callback_thread,
            Err(_) => 0,
        };
        return Ok(HookInjectionOutcome::Pending {
            process: process.take().expect("target process handle missing"),
            callback_thread,
            detail,
            hooks: Some(hooks),
            completion,
            initial_log_len,
        });
    }

    if let Err(cleanup_error) = hooks.cleanup() {
        let wait_detail = match &wait_result {
            Ok(HookWaitOutcome::Complete { .. }) => {
                "SP_HookProc completed before hook cleanup failed".to_string()
            }
            Ok(HookWaitOutcome::Pending { detail, .. }) | Err(detail) => detail.clone(),
        };
        let detail = format!("{wait_detail}; hook cleanup remains pending: {cleanup_error}");
        return Ok(HookInjectionOutcome::Pending {
            process: process.take().expect("target process handle missing"),
            callback_thread: match wait_result {
                Ok(HookWaitOutcome::Complete {
                    callback_thread, ..
                })
                | Ok(HookWaitOutcome::Pending {
                    callback_thread, ..
                }) => callback_thread,
                Err(_) => 0,
            },
            detail,
            hooks: Some(hooks),
            completion,
            initial_log_len,
        });
    }

    if process
        .as_ref()
        .expect("target process handle missing")
        .has_exited()?
    {
        return Err(format!(
            "TARGET_EXITED: pid={pid} exited after hook cleanup"
        ));
    }

    match wait_result {
        Ok(HookWaitOutcome::Complete {
            callback_thread,
            state,
        }) => {
            dbg_log(&format!(
                "hook cleanup complete for pid={pid}, callback_thread={callback_thread}, state={state:?}"
            ));
            Ok(HookInjectionOutcome::Complete {
                callback_thread,
                state,
            })
        }
        Ok(HookWaitOutcome::Pending {
            callback_thread,
            detail,
        }) => Ok(HookInjectionOutcome::Pending {
            process: process.take().expect("target process handle missing"),
            callback_thread,
            detail,
            hooks: None,
            completion,
            initial_log_len,
        }),
        Err(error) => {
            if process
                .as_ref()
                .expect("target process handle missing")
                .has_exited()?
            {
                return Err(format!(
                    "TARGET_EXITED: pid={pid} exited after hook callback"
                ));
            }

            match read_speedpatch_handshake(pid)? {
                Some(handshake) => match classify_handshake(handshake) {
                    HandshakeProgress::Complete {
                        callback_thread,
                        state,
                    } => Ok(HookInjectionOutcome::Complete {
                        callback_thread,
                        state,
                    }),
                    HandshakeProgress::Failed {
                        callback_thread,
                        init_result,
                    } => Err(format!(
                        "SP_HookProc initialization failed for pid={pid}, callback_thread={callback_thread}: {}",
                        initialization_failure_detail(pid, init_result)
                    )),
                    HandshakeProgress::Invalid(detail) => Err(detail),
                    HandshakeProgress::Pending => Ok(HookInjectionOutcome::Pending {
                        process: process.take().expect("target process handle missing"),
                        callback_thread: handshake.hook_thread_id,
                        detail: error,
                        hooks: None,
                        completion,
                        initial_log_len,
                    }),
                },
                None => {
                    let dll_name = speedpatch_dll(BRIDGE_IS64);
                    match find_remote_module(pid, dll_name, Some(dll_path))? {
                        Some(module) => Ok(HookInjectionOutcome::Pending {
                            process: process.take().expect("target process handle missing"),
                            callback_thread: 0,
                            detail: format!(
                                "{error}; {dll_name} remains loaded at 0x{:x}, so the kernel completion event is still being monitored",
                                module.base
                            ),
                            hooks: None,
                            completion,
                            initial_log_len,
                        }),
                        None => {
                            if process
                                .as_ref()
                                .expect("target process handle missing")
                                .has_exited()?
                            {
                                Err(format!(
                                    "TARGET_EXITED: pid={pid} exited after hook cleanup"
                                ))
                            } else {
                                Err(error)
                            }
                        }
                    }
                }
            }
        }
    }
}

struct PendingInjectionMonitor {
    pid: u32,
    process: TargetProcessHandle,
    operation: RemoteOperationLease,
    pending_detail: String,
    hooks: Option<InstalledWindowsHooks>,
    completion: HookCompletionEvent,
    dll_path: String,
    initial_log_len: u64,
}

/// How long a pending-injection monitor keeps waiting for the target's
/// terminal handshake after bridge shutdown starts. When the grace expires
/// the monitor releases its hook handles, writes the final disable, and
/// terminates — releasing the operation lease so the bridge can exit even
/// when the target never publishes a handshake (hung/frozen target, hook
/// callback never delivered).
const SHUTDOWN_PENDING_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Upper bound on a pending-injection monitor that sees no progress at all:
/// hooks installed but SP_HookProc never runs and the handshake never
/// appears. Without this bound the monitor loops forever (and after its
/// spawn, STATUS reports INITIALIZING indefinitely — the eternal UI spinner),
/// because neither the target-exit nor the handshake/completion termination
/// conditions can ever become true.
const PENDING_INJECTION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

fn monitor_pending_injection(monitor: PendingInjectionMonitor) {
    std::thread::spawn(move || {
        let PendingInjectionMonitor {
            pid,
            process,
            operation,
            pending_detail,
            mut hooks,
            completion,
            dll_path,
            initial_log_len,
        } = monitor;
        let _operation = operation;
        let mut terminal: Option<Result<(u32, SpeedpatchState), String>> = None;
        let mut terminal_without_completion = false;
        let mut last_cleanup_error = None;
        let mut shutdown_grace_started: Option<std::time::Instant> = None;
        let mut last_shutdown_disable_log: Option<std::time::Instant> = None;
        let monitor_deadline = std::time::Instant::now() + PENDING_INJECTION_DEADLINE;
        loop {
            match process.has_exited() {
                Ok(true) => {
                    if let Some(mut owned_hooks) = hooks.take() {
                        owned_hooks.release_after_target_exit();
                    }
                    clear_injection_stage(pid);
                    clear_injection_failure(pid);
                    untrack_target(pid);
                    dbg_log(&format!(
                        "pending hook monitor: TARGET_EXITED pid={pid} before terminal handshake"
                    ));
                    return;
                }
                Ok(false) => {}
                Err(error) => {
                    dbg_log(&format!("pending hook monitor: {error}"));
                }
            }

            let callback_completed = match read_speedpatch_handshake(pid) {
                Ok(Some(handshake)) => handshake.callback_completed,
                Ok(None) | Err(_) => false,
            };
            let completion_signaled = match completion.is_signaled() {
                Ok(signaled) => signaled,
                Err(error) => {
                    dbg_log(&format!(
                    "pending hook monitor: completion event probe failed for pid={pid}: {error}"
                ));
                    false
                }
            };
            if completion_signaled && !callback_completed {
                dbg_log(&format!(
                "pending hook monitor: completion event signaled after callback return before marker for pid={pid}"
            ));
            }
            let handshake_absent = matches!(read_speedpatch_handshake(pid), Ok(None));
            let callback_finished = callback_completed || (completion_signaled && handshake_absent);

            let mut cleanup_complete = false;
            if callback_finished {
                if let Some(owned_hooks) = hooks.as_mut() {
                    match owned_hooks.cleanup() {
                        Ok(()) => {
                            cleanup_complete = true;
                        }
                        Err(error) => {
                            if last_cleanup_error.as_deref() != Some(error.as_str()) {
                                dbg_log(&format!(
                                "pending hook monitor: hook cleanup still pending for pid={pid}: {error}"
                            ));
                                last_cleanup_error = Some(error);
                            }
                        }
                    }
                }
            }
            if cleanup_complete {
                hooks = None;
                last_cleanup_error = None;
                dbg_log(&format!(
                    "pending hook monitor: hook cleanup completed for pid={pid}"
                ));
            }

            if terminal.is_none() {
                match read_speedpatch_handshake(pid) {
                    Ok(Some(handshake)) => match classify_handshake(handshake) {
                        HandshakeProgress::Complete {
                            callback_thread,
                            state,
                        } => terminal = Some(Ok((callback_thread, state))),
                        HandshakeProgress::Failed {
                            callback_thread,
                            init_result,
                        } => {
                            terminal = Some(Err(format!(
                                "SP_HookProc initialization failed for pid={pid}, callback_thread={callback_thread}: {}",
                                initialization_failure_detail(pid, init_result)
                            )));
                        }
                        HandshakeProgress::Invalid(detail) => terminal = Some(Err(detail)),
                        HandshakeProgress::Pending => {
                            set_injection_stage(pid, InjectionStage::Initializing);
                            if callback_finished {
                                terminal = Some(Err(completed_without_terminal_handshake_detail(
                                    pid,
                                    handshake.hook_thread_id,
                                    initial_log_len,
                                )));
                            }
                        }
                    },
                    Ok(None) if hooks.is_none() => {
                        if callback_finished {
                            terminal = Some(Err(completed_without_terminal_handshake_detail(
                                pid,
                                0,
                                initial_log_len,
                            )));
                        } else {
                            let dll_name = speedpatch_dll(BRIDGE_IS64);
                            match find_remote_module(pid, dll_name, Some(&dll_path)) {
                                    Ok(None) => match process.has_exited() {
                                        Ok(true) => {
                                            clear_injection_stage(pid);
                                            clear_injection_failure(pid);
                                            untrack_target(pid);
                                            dbg_log(&format!(
                                                "pending hook monitor: TARGET_EXITED pid={pid} during module probe"
                                            ));
                                            return;
                                        }
                                        Ok(false) => {
                                            terminal = Some(Err(pending_detail.clone()));
                                            terminal_without_completion = true;
                                        }
                                        Err(error) => dbg_log(&format!(
                                            "pending hook monitor: target recheck failed for pid={pid}: {error}"
                                        )),
                                    },
                                    Ok(Some(_)) => {}
                                    Err(error) => dbg_log(&format!(
                                        "pending hook monitor: module probe failed for pid={pid}: {error}"
                                    )),
                                }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        dbg_log(&format!(
                            "pending hook monitor: handshake read failed for pid={pid}: {error}"
                        ));
                    }
                }
            }

            // Deadline: no terminal state after PENDING_INJECTION_DEADLINE.
            // Reaching this point means neither the handshake nor the
            // completion event can ever become visible (SP_HookProc never
            // ran), so waiting longer only keeps STATUS in INITIALIZING and
            // the UI spinner spinning. Release the hooks, record an explicit
            // failure, and let the publish block below terminate the monitor.
            if !shutdown_requested() && std::time::Instant::now() >= monitor_deadline {
                if terminal.is_none() {
                    let dll_name = speedpatch_dll(BRIDGE_IS64);
                    terminal = Some(Err(format!(
                        "injection did not complete within {}s for pid={pid}: SP_HookProc never published a handshake and {dll_name} was never observed in the target, so the hook-DLL injection was likely rejected (anti-cheat protection) or no target thread dispatches WH_GETMESSAGE messages. Restart the target before retrying. Last detail: {pending_detail}",
                        PENDING_INJECTION_DEADLINE.as_secs()
                    )));
                    terminal_without_completion = true;
                }
                if let Some(mut owned_hooks) = hooks.take() {
                    match owned_hooks.cleanup() {
                        Ok(()) => dbg_log(&format!(
                            "pending hook monitor: deadline cleanup released hooks for pid={pid}"
                        )),
                        Err(error) => dbg_log(&format!(
                            "pending hook monitor: deadline hook cleanup still pending for pid={pid}: {error}"
                        )),
                    }
                }
                dbg_log(&format!(
                    "pending hook monitor: deadline reached for pid={pid}; publishing failure"
                ));
            }

            if hooks.is_none() && (callback_finished || terminal_without_completion) {
                if let Some(result) = terminal.take() {
                    match process.has_exited() {
                        Ok(true) => {
                            clear_injection_stage(pid);
                            clear_injection_failure(pid);
                            untrack_target(pid);
                            dbg_log(&format!(
                                "pending hook monitor: TARGET_EXITED pid={pid} before terminal result publication"
                            ));
                            return;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            dbg_log(&format!(
                                "pending hook monitor: terminal target recheck failed for pid={pid}: {error}"
                            ));
                            terminal = Some(result);
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            continue;
                        }
                    }
                    match result {
                        Ok((callback_thread, state)) => {
                            finish_injection_success(pid);
                            dbg_log(&format!(
                                "pending hook monitor: pid={pid} callback_thread={callback_thread} completed with {state:?}"
                            ));
                        }
                        Err(detail) => {
                            record_injection_failure(pid, detail.clone());
                            track_target(pid);
                            dbg_log(&format!("pending hook monitor: {detail}"));
                        }
                    }
                    return;
                }
            }

            if shutdown_requested() {
                if shutdown_grace_started.is_none() {
                    shutdown_grace_started = Some(std::time::Instant::now());
                }
                if let Err(error) = write_speedpatch_enabled(pid, false) {
                    // Throttle: this failure repeats every ~50 ms while the
                    // target mapping is absent (DLL never initialized). One
                    // line per second keeps the log readable without losing
                    // the diagnostic signal.
                    let now = std::time::Instant::now();
                    let should_log = last_shutdown_disable_log
                        .map(|last| now.duration_since(last) >= std::time::Duration::from_secs(1))
                        .unwrap_or(true);
                    if should_log {
                        last_shutdown_disable_log = Some(now);
                        dbg_log(&format!(
                            "pending hook monitor: shutdown disable pending for pid={pid}: {error}"
                        ));
                    }
                }
                if shutdown_grace_started
                    .expect("shutdown grace timestamp set above")
                    .elapsed()
                    >= SHUTDOWN_PENDING_GRACE
                {
                    if let Some(mut owned_hooks) = hooks.take() {
                        match owned_hooks.cleanup() {
                            Ok(()) => dbg_log(&format!(
                                "pending hook monitor: shutdown cleanup released hooks for pid={pid}"
                            )),
                            Err(error) => dbg_log(&format!(
                                "pending hook monitor: shutdown hook cleanup still pending for pid={pid}: {error}"
                            )),
                        }
                    }
                    clear_injection_stage(pid);
                    clear_injection_failure(pid);
                    untrack_target(pid);
                    dbg_log(&format!(
                        "pending hook monitor: shutdown grace expired for pid={pid}; abandoning pending injection"
                    ));
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// Inject using the one supported same-architecture GUI-hook path.
fn do_inject(pid: u32) -> Result<(), String> {
    let operation = RemoteOperationLease::try_acquire()?;
    if let Some(stage) = injection_stage(pid) {
        return Err(format!(
            "INJECTION_PENDING: the existing {:?} stage is still running for pid={pid}",
            stage
        ));
    }

    let target_is64 = query_process_is64(pid)?;
    if target_is64 != BRIDGE_IS64 {
        return Err(format!(
            "bridge/target architecture mismatch: bridge_is64={} target_is64={} pid={pid}",
            BRIDGE_IS64, target_is64
        ));
    }

    let dll_name = speedpatch_dll(BRIDGE_IS64);
    let dll_path = exe_dir()?.join(dll_name);
    let dll_str = dll_path.to_string_lossy().to_string();
    if !dll_path.is_file() {
        return Err(format!(
            "speedpatch DLL missing next to bridge: {} (bridge arch={}, target is64={})",
            dll_path.display(),
            OWN_SPEEDPATCH,
            target_is64
        ));
    }

    let existing_module = find_remote_module(pid, dll_name, Some(&dll_str))?;
    let handshake = read_speedpatch_handshake(pid)?;
    let state = handshake.map(|value| value.state);
    match (existing_module.as_ref(), state) {
        (Some(_), Some(SpeedpatchState::Enabled)) => {
            clear_injection_failure(pid);
            track_target(pid);
            return Ok(());
        }
        (Some(_), Some(SpeedpatchState::Disabled)) => {
            clear_injection_failure(pid);
            return do_enable_inner(pid);
        }
        (Some(_), Some(SpeedpatchState::Initializing)) => {
            set_injection_stage(pid, InjectionStage::Initializing);
            track_target(pid);
            return Err(format!(
                "INJECTION_PENDING: SP_Initialize is still running for pid={pid}"
            ));
        }
        (Some(_), Some(SpeedpatchState::Failed)) => {
            untrack_target(pid);
            return Err(persisted_initialization_failure(
                pid,
                handshake.expect("failed state without handshake"),
            ));
        }
        (None, Some(state)) => {
            untrack_target(pid);
            return Err(format!(
                "DzsSpeedy.{pid} reports {state:?}, but the expected DLL is not loaded from {dll_str}; refusing a false-positive injection state"
            ));
        }
        (Some(module), None) => {
            untrack_target(pid);
            let previous = injection_failure(pid)
                .map(|detail| format!(" Previous failure: {detail}."))
                .unwrap_or_default();
            return Err(format!(
                "{} is already loaded in pid={pid} from {}, but its status mapping is absent. Automatic recovery is disabled so injection always follows one fixed SetWindowsHookExW -> SP_HookProc chain.{previous} Restart the target process before retrying.",
                dll_name, module.path
            ));
        }
        (None, None) => {
            clear_injection_failure(pid);
        }
    }

    track_target(pid);
    set_injection_stage(pid, InjectionStage::Loading);
    match inject_via_windows_hook(pid, &dll_str) {
        Ok(HookInjectionOutcome::Complete {
            callback_thread,
            state,
        }) => {
            finish_injection_success(pid);
            dbg_log(&format!(
                "do_inject pid={pid}: SetWindowsHookExW + SP_HookProc complete thread={callback_thread} state={state:?}"
            ));
            if state == SpeedpatchState::Enabled {
                Ok(())
            } else {
                Err(format!(
                    "INJECTION_DISABLED: SP_HookProc initialized pid={pid} on thread={callback_thread}, but acceleration was disabled before completion"
                ))
            }
        }
        Ok(HookInjectionOutcome::Pending {
            process,
            callback_thread,
            detail,
            hooks,
            completion,
            initial_log_len,
        }) => {
            let response =
                format!("INJECTION_PENDING: {detail}; callback_thread={callback_thread}");
            set_injection_stage(pid, InjectionStage::Initializing);
            monitor_pending_injection(PendingInjectionMonitor {
                pid,
                process,
                operation,
                pending_detail: response.clone(),
                hooks,
                completion,
                dll_path: dll_str,
                initial_log_len,
            });
            Err(response)
        }
        Err(detail) => {
            let failure = format!(
                "SetWindowsHookExW injection failed for pid={pid}, dll={dll_str}: {detail}"
            );
            record_injection_failure(pid, failure.clone());
            Err(failure)
        }
    }
}

fn decode_initialize_exit_code(code: u32) -> Result<(), InitializationError> {
    let kind = code & 0xff00_0000;
    let detail = match kind {
        0x0100_0000 => format!(
            "MH_Initialize failed: {} ({})",
            minhook_status_name(code & 0xffff),
            code & 0xffff
        ),
        0x0200_0000 => format!(
            "MH_CreateHook failed for {}: {} ({})",
            hook_name((code >> 16) & 0xff),
            minhook_status_name(code & 0xffff),
            code & 0xffff
        ),
        0x0300_0000 => format!(
            "MH_EnableHook(MH_ALL_HOOKS) failed: {} ({})",
            minhook_status_name(code & 0xffff),
            code & 0xffff
        ),
        0x0400_0000 => format!(
            "CreateFileMapping(DzsSpeedy status) failed: win32_error={}",
            code & 0x00ff_ffff
        ),
        0x0500_0000 => format!(
            "MapViewOfFile(DzsSpeedy status) failed: win32_error={}",
            code & 0x00ff_ffff
        ),
        0x0600_0000 => format!(
            "MinHook rollback failed: {} ({})",
            minhook_status_name(code & 0xffff),
            code & 0xffff
        ),
        0x0700_0000 => {
            "a previous hook-enable rollback left speedpatch non-retryable; restart the target process before injecting again".into()
        }
        0x0800_0000 => format!(
            "speedpatch hook callback could not acquire its DLL self-reference: win32_error={}",
            code & 0x00ff_ffff
        ),
        0x0900_0000 => format!(
            "speedpatch hook callback could not open its bridge completion event: win32_error={}",
            code & 0x00ff_ffff
        ),
        _ => match code {
            0 => return Ok(()),
            1 => "MinHook initialization failed".into(),
            2 => "one or more time hooks failed to install".into(),
            3 => "shared status mapping initialization failed".into(),
            170 => "SP_Initialize is already running".into(),
            _ => format!("SP_Initialize returned code 0x{code:08x}"),
        },
    };

    Err(InitializationError { detail })
}

fn hook_name(id: u32) -> &'static str {
    match id {
        1 => "Sleep",
        2 => "SleepEx",
        3 => "SetWaitableTimer",
        4 => "SetWaitableTimerEx",
        5 => "SetTimer",
        6 => "timeGetTime",
        7 => "timeSetEvent",
        8 => "GetMessageTime",
        9 => "GetTickCount",
        10 => "GetTickCount64",
        11 => "QueryPerformanceCounter",
        12 => "GetSystemTimeAsFileTime",
        13 => "GetSystemTimePreciseAsFileTime",
        _ => "unknown hook",
    }
}

fn minhook_status_name(status: u32) -> &'static str {
    match status {
        0 => "MH_OK",
        1 => "MH_ERROR_ALREADY_INITIALIZED",
        2 => "MH_ERROR_NOT_INITIALIZED",
        3 => "MH_ERROR_ALREADY_CREATED",
        4 => "MH_ERROR_NOT_CREATED",
        5 => "MH_ERROR_ENABLED",
        6 => "MH_ERROR_DISABLED",
        7 => "MH_ERROR_NOT_EXECUTABLE",
        8 => "MH_ERROR_UNSUPPORTED_FUNCTION",
        9 => "MH_ERROR_MEMORY_ALLOC",
        10 => "MH_ERROR_MEMORY_PROTECT",
        11 => "MH_ERROR_MODULE_NOT_FOUND",
        12 => "MH_ERROR_FUNCTION_NOT_FOUND",
        0xffff => "MH_UNKNOWN",
        _ => "MH_STATUS_UNKNOWN",
    }
}

fn normalize_module_path(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn module_text(buffer: &[u16]) -> String {
    String::from_utf16_lossy(buffer)
        .trim_end_matches('\0')
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleSnapshotErrorKind {
    TargetGone,
    Fatal,
}

fn module_snapshot_error_kind(error: HRESULT) -> ModuleSnapshotErrorKind {
    if error == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0)
        || error == HRESULT::from_win32(ERROR_PARTIAL_COPY.0)
    {
        ModuleSnapshotErrorKind::TargetGone
    } else {
        ModuleSnapshotErrorKind::Fatal
    }
}

fn create_module_snapshot(pid: u32) -> Result<Option<HANDLE>, String> {
    let mut last_error = None;
    for attempt in 0..8 {
        match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) } {
            Ok(snapshot) => return Ok(Some(snapshot)),
            Err(error)
                if error.code() == HRESULT::from_win32(ERROR_BAD_LENGTH.0) && attempt < 7 =>
            {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error)
                if module_snapshot_error_kind(error.code()) == ModuleSnapshotErrorKind::TargetGone =>
            {
                // During process teardown Windows can report either an invalid PID or
                // ERROR_PARTIAL_COPY while the module list is disappearing. Status polling
                // should treat both as a missing target rather than surface a transient error.
                return Ok(None);
            }
            Err(error) => {
                return Err(format!(
                    "CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid={pid}) failed: {error:?}"
                ));
            }
        }
    }
    Err(format!(
        "CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid={pid}) kept returning ERROR_BAD_LENGTH: {:?}",
        last_error
    ))
}

fn find_remote_module(
    pid: u32,
    module_name: &str,
    expected_path: Option<&str>,
) -> Result<Option<RemoteModule>, String> {
    let Some(snapshot) = create_module_snapshot(pid)? else {
        return Ok(None);
    };
    let wanted_name = module_name.to_ascii_lowercase();
    let wanted_path = expected_path.map(normalize_module_path);
    let mut collisions = Vec::new();
    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };

    let first = unsafe { Module32FirstW(snapshot, &mut entry) };
    if let Err(error) = first {
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0)
            || module_snapshot_error_kind(error.code()) == ModuleSnapshotErrorKind::TargetGone
        {
            return Ok(None);
        }
        return Err(format!("Module32FirstW(pid={pid}) failed: {error:?}"));
    }

    let result = loop {
        let name = module_text(&entry.szModule).to_ascii_lowercase();
        if name == wanted_name {
            let path = module_text(&entry.szExePath);
            let path_matches = wanted_path
                .as_ref()
                .map(|wanted| normalize_module_path(&path) == *wanted)
                .unwrap_or(true);
            if path_matches {
                break Ok(Some(RemoteModule {
                    base: entry.modBaseAddr as usize,
                    path,
                }));
            }
            collisions.push(path);
        }

        match unsafe { Module32NextW(snapshot, &mut entry) } {
            Ok(()) => {}
            Err(error)
                if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0)
                    || module_snapshot_error_kind(error.code()) == ModuleSnapshotErrorKind::TargetGone =>
            {
                break Ok(None)
            }
            Err(error) => break Err(format!("Module32NextW(pid={pid}) failed: {error:?}")),
        }
    };

    unsafe {
        let _ = CloseHandle(snapshot);
    }
    let module = result?;
    if module.is_none() && !collisions.is_empty() {
        return Err(format!(
            "pid={pid} already contains {module_name} from a different path: {}",
            collisions.join(", ")
        ));
    }
    Ok(module)
}

fn do_eject(pid: u32) -> Result<(), String> {
    let _operation = RemoteOperationLease::try_acquire()?;
    do_disable_inner(pid)?;
    dbg_log(&format!(
        "do_eject pid={pid}: acceleration disabled; DLL remains resident until target exit"
    ));
    untrack_target(pid);
    Ok(())
}

fn do_enable(pid: u32) -> Result<(), String> {
    let _operation = RemoteOperationLease::try_acquire()?;
    do_enable_inner(pid)
}

fn do_enable_inner(pid: u32) -> Result<(), String> {
    let handshake = read_speedpatch_handshake(pid)?;
    match handshake.map(|value| value.state) {
        Some(SpeedpatchState::Enabled) => {
            track_target(pid);
            return Ok(());
        }
        Some(SpeedpatchState::Initializing) => {
            return Err(format!(
                "INJECTION_PENDING: SP_Initialize is still running for pid={pid}"
            ));
        }
        Some(SpeedpatchState::Failed) => {
            return Err(persisted_initialization_failure(
                pid,
                handshake.expect("failed state without handshake"),
            ));
        }
        Some(SpeedpatchState::Disabled) | None => {}
    }

    for attempt in 0..30 {
        match write_speedpatch_enabled(pid, true) {
            Ok(()) => {
                track_target(pid);
                return Ok(());
            }

            Err(e) if attempt + 1 < 30 => {
                std::thread::sleep(std::time::Duration::from_millis(50));

                let _ = e;
            }

            Err(e) => return Err(e),
        }
    }

    Err(format!(
        "ENABLE {pid}: DzsSpeedy.{pid} mapping not found (inject first)"
    ))
}

fn do_disable(pid: u32) -> Result<(), String> {
    let _operation = RemoteOperationLease::try_acquire()?;
    do_disable_inner(pid)
}

fn do_disable_inner(pid: u32) -> Result<(), String> {
    match do_status(pid)? {
        InjectionStatus::NotInjected => {
            untrack_target(pid);
            return Ok(());
        }
        InjectionStatus::Failed(detail) => return Err(detail),
        InjectionStatus::Initializing | InjectionStatus::Disabled | InjectionStatus::Enabled => {}
    }

    for attempt in 0..30 {
        match write_speedpatch_enabled(pid, false) {
            Ok(()) => {
                track_target(pid);
                return Ok(());
            }

            Err(e) if attempt + 1 < 30 => {
                std::thread::sleep(std::time::Duration::from_millis(50));

                let _ = e;
            }

            Err(e) => return Err(e),
        }
    }

    Err(format!("DISABLE {pid}: failed to write DzsSpeedy.{pid}"))
}

fn do_is_enabled(pid: u32) -> Result<bool, String> {
    let handshake = read_speedpatch_handshake(pid)?;
    match handshake.map(|value| value.state) {
        Some(SpeedpatchState::Enabled) => Ok(true),
        Some(SpeedpatchState::Disabled) => Ok(false),
        Some(SpeedpatchState::Initializing) => {
            Err(format!("SP_Initialize is still running for pid={pid}"))
        }
        Some(SpeedpatchState::Failed) => Err(persisted_initialization_failure(
            pid,
            handshake.expect("failed state without handshake"),
        )),
        None => Err(format!("no DzsSpeedy.{pid} mapping")),
    }
}

/// Check the exact state of the one supported injection chain.
fn do_status(pid: u32) -> Result<InjectionStatus, String> {
    let handshake = read_speedpatch_handshake(pid)?;
    match handshake.map(|value| value.state) {
        Some(SpeedpatchState::Enabled) => {
            clear_injection_stage(pid);
            clear_injection_failure(pid);
            track_target(pid);
            return Ok(InjectionStatus::Enabled);
        }
        Some(SpeedpatchState::Disabled) => {
            clear_injection_stage(pid);
            clear_injection_failure(pid);
            track_target(pid);
            return Ok(InjectionStatus::Disabled);
        }
        Some(SpeedpatchState::Initializing) => {
            set_injection_stage(pid, InjectionStage::Initializing);
            return Ok(InjectionStatus::Initializing);
        }
        Some(SpeedpatchState::Failed) => {
            clear_injection_stage(pid);
            untrack_target(pid);
            return Ok(InjectionStatus::Failed(persisted_initialization_failure(
                pid,
                handshake.expect("failed state without handshake"),
            )));
        }
        None => {}
    }

    if injection_stage(pid).is_some() {
        return Ok(InjectionStatus::Initializing);
    }
    if let Some(detail) = injection_failure(pid) {
        return Ok(InjectionStatus::Failed(detail));
    }

    let dll_name = speedpatch_dll(BRIDGE_IS64);
    let dll_path = exe_dir()?.join(dll_name).to_string_lossy().to_string();
    if let Some(module) = find_remote_module(pid, dll_name, Some(&dll_path))? {
        return Ok(InjectionStatus::Failed(format!(
            "{} is loaded at 0x{:x} from {}, but DzsSpeedy.{pid} is absent; SP_Initialize did not complete",
            dll_name,
            module.base,
            module.path
        )));
    }

    untrack_target(pid);
    Ok(InjectionStatus::NotInjected)
}

fn do_set_speed(factor: f64) {
    let _ = write_global_speed_factor(factor);

    let dll_wide = to_wide(OWN_SPEEDPATCH);

    unsafe {
        let Ok(h) = LoadLibraryW(PCWSTR::from_raw(dll_wide.as_ptr())) else {
            return;
        };

        let set_speed: Option<unsafe extern "C" fn(f64)> =
            std::mem::transmute(GetProcAddress(h, s!("SP_SetSpeed")));

        if let Some(f) = set_speed {
            f(factor);
        }
        let _ = FreeLibrary(h);
    }
}

fn do_get_speed() -> f64 {
    if let Some(v) = read_global_speed_factor() {
        return v;
    }

    let dll_wide = to_wide(OWN_SPEEDPATCH);

    unsafe {
        let Ok(h) = LoadLibraryW(PCWSTR::from_raw(dll_wide.as_ptr())) else {
            return 1.0;
        };

        let get_speed: Option<unsafe extern "C" fn() -> f64> =
            std::mem::transmute(GetProcAddress(h, s!("SP_GetSpeed")));

        let speed = if let Some(f) = get_speed { f() } else { 1.0 };
        let _ = FreeLibrary(h);
        speed
    }
}

// ── Command dispatch ─────────────────────────────────────────────────────

fn handle_command(line: &str) -> String {
    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("").to_uppercase();
    let raw = line.trim().to_string();
    dbg_log(&format!("cmd in: {raw}"));

    let resp = match cmd.as_str() {
        "SHUTDOWN" => "OK shutting down".into(),
        // A bridge that is shutting down must not masquerade as healthy:
        // health probes answer with an explicit error so clients (GUI health
        // check, singleton takeover probe) can tell a dying bridge apart from
        // a live one.
        _ if shutdown_requested() && command_is_shutdown_gated(&cmd) => {
            "ERROR bridge is shutting down".into()
        }
        "INJECT" => {
            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_inject(pid) {
                Ok(()) => "OK".into(),
                Err(e) => format!("ERROR {e}"),
            }
        }

        "EJECT" => {
            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_eject(pid) {
                Ok(()) => "OK".into(),
                Err(e) => format!("ERROR {e}"),
            }
        }

        "ENABLE" => {
            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_enable(pid) {
                Ok(()) => "OK".into(),
                Err(e) => format!("ERROR {e}"),
            }
        }

        "DISABLE" => {
            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_disable(pid) {
                Ok(()) => "OK".into(),
                Err(e) => format!("ERROR {e}"),
            }
        }

        "ISENABLED" => {
            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_is_enabled(pid) {
                Ok(true) => "OK 1".into(),
                Ok(false) => "OK 0".into(),

                Err(e) => format!("ERROR {e}"),
            }
        }

        "SETSPEED" => {
            let f: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);

            do_set_speed(f);

            "OK".into()
        }

        "GETSPEED" => {
            let s = do_get_speed();

            format!("OK {s:.6}")
        }

        "STATUS" => {
            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_status(pid) {
                Ok(InjectionStatus::Enabled) => "OK ENABLED".into(),
                Ok(InjectionStatus::Disabled) => "OK DISABLED".into(),
                Ok(InjectionStatus::Initializing) => "OK INITIALIZING".into(),
                Ok(InjectionStatus::Failed(detail)) => format!("OK FAILED {detail}"),
                Ok(InjectionStatus::NotInjected) => "OK NOT_INJECTED".into(),

                Err(e) => format!("ERROR {e}"),
            }
        }

        "PING" | "VERSION" => "OK bridge-filemap-v3".into(),

        _ => "ERROR unknown command".into(),
    };

    dbg_log(&format!("cmd out: {raw} -> {resp}"));

    resp
}

fn write_resp(h_pipe: HANDLE, msg: &str) {
    let mut written = 0u32;

    unsafe {
        let _ = WriteFile(h_pipe, Some(msg.as_bytes()), Some(&mut written), None);
    }
}

// ── Pipe server ───────────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
const PIPE_NAME: &str = r"\\.\pipe\DzsSpeedyBridge64";

#[cfg(target_arch = "x86")]
const PIPE_NAME: &str = r"\\.\pipe\DzsSpeedyBridge32";

#[cfg(target_arch = "x86_64")]
const SHUTDOWN_EVENT: &str = "Global\\DzsSpeedyBridge64Shutdown";

#[cfg(target_arch = "x86")]
const SHUTDOWN_EVENT: &str = "Global\\DzsSpeedyBridge32Shutdown";

fn shutdown_bridge() -> ! {
    request_shutdown();
    disable_tracked_targets();

    if !wait_for_operations_drain(&REMOTE_OPERATION_STATE, SHUTDOWN_DRAIN_DEADLINE) {
        dbg_log(&format!(
            "bridge shutdown drain deadline reached with {} in-flight operation(s); exiting anyway (completion paths observe the shutdown bit)",
            active_remote_operations()
        ));
    }

    // An injection may have completed after the first snapshot. Its success
    // path also observes the shutdown bit, and this second pass closes the race.
    disable_tracked_targets();
    dbg_log("bridge shutdown complete");
    std::process::exit(0);
}

fn start_shutdown_watcher() -> Result<(), String> {
    let event_name = to_wide(SHUTDOWN_EVENT);
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::from_raw(event_name.as_ptr())) }
        .map_err(|error| format!("CreateEventW({SHUTDOWN_EVENT}) failed: {error:?}"))?;
    // The event is a manual-reset Global object. If it already exists from a
    // previous bridge instance it may still be in the signaled state; reset it
    // so a freshly started bridge never shuts down immediately at startup.
    unsafe {
        let _ = ResetEvent(event);
    }
    let event_raw = event.0 as usize;

    std::thread::spawn(move || unsafe {
        let event = HANDLE(event_raw as *mut std::ffi::c_void);
        let wait_result = WaitForSingleObject(event, u32::MAX);
        if wait_result == WAIT_OBJECT_0 {
            dbg_log("shutdown event received");
            let _ = CloseHandle(event);
            shutdown_bridge();
        }
        dbg_log(&format!(
            "shutdown event wait failed: result=0x{:08x}",
            wait_result.0
        ));
        let _ = CloseHandle(event);
    });
    Ok(())
}

fn pipe_server() {
    loop {
        let name_wide = to_wide(PIPE_NAME);

        let h_pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR::from_raw(name_wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(3), // PIPE_ACCESS_DUPLEX
                NAMED_PIPE_MODE(PIPE_TYPE_MESSAGE.0 | PIPE_READMODE_MESSAGE.0 | PIPE_WAIT.0),
                255,
                4096,
                4096,
                0,
                None,
            )
        };

        if h_pipe == INVALID_HANDLE_VALUE {
            std::thread::sleep(std::time::Duration::from_secs(1));

            continue;
        }

        let connected = unsafe { ConnectNamedPipe(h_pipe, None) };

        let connected = connected.is_ok() || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;

        if !connected {
            unsafe {
                let _ = CloseHandle(h_pipe);
            }

            continue;
        }

        let mut buf = [0u8; 4096];

        loop {
            let mut nread = 0u32;

            let ok = unsafe { ReadFile(h_pipe, Some(&mut buf), Some(&mut nread), None) };

            if ok.is_err() || nread == 0 {
                break;
            }

            let text = String::from_utf8_lossy(&buf[..nread as usize]);

            for line in text.lines() {
                let line = line.trim();

                if line.is_empty() {
                    continue;
                }

                if line.eq_ignore_ascii_case("SHUTDOWN") {
                    write_resp(h_pipe, "OK shutting down\n");

                    unsafe {
                        let _ = CloseHandle(h_pipe);
                    }

                    shutdown_bridge();
                }
                let resp = handle_command(line);

                write_resp(h_pipe, &format!("{resp}\n"));
            }
        }

        unsafe {
            let _ = CloseHandle(h_pipe);
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
const BRIDGE_MUTEX: &str = "Global\\DzsSpeedyBridge64Mutex";

#[cfg(target_arch = "x86")]
const BRIDGE_MUTEX: &str = "Global\\DzsSpeedyBridge32Mutex";

fn existing_bridge_pipe_alive() -> bool {
    let name_wide = to_wide(PIPE_NAME);

    unsafe {
        let h = CreateFileW(
            PCWSTR::from_raw(name_wide.as_ptr()),
            0xC0000000 | 0x40000000,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        );

        let Ok(h) = h else {
            return false;
        };

        if h == INVALID_HANDLE_VALUE {
            return false;
        }

        let mode = NAMED_PIPE_MODE(PIPE_READMODE_MESSAGE.0);

        let _ = SetNamedPipeHandleState(h, Some(&mode), None, None);

        let msg = b"GETSPEED\n";

        let mut written = 0u32;

        let _ = WriteFile(h, Some(msg), Some(&mut written), None);

        let mut buf = [0u8; 256];

        let mut nread = 0u32;

        let ok = ReadFile(h, Some(&mut buf), Some(&mut nread), None);

        let _ = CloseHandle(h);

        ok.is_ok()
            && nread > 0
            && String::from_utf8_lossy(&buf[..nread as usize])
                .trim()
                .starts_with("OK")
    }
}

/// Only one bridge instance per arch may own the named pipe server.
fn acquire_bridge_singleton() -> bool {
    let name = to_wide(BRIDGE_MUTEX);

    unsafe {
        let Ok(h) = CreateMutexW(None, true, PCWSTR::from_raw(name.as_ptr())) else {
            return false;
        };

        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);

            return false;
        }

        // HANDLE is a Copy wrapper around a raw kernel handle with no Drop
        // impl, so binding it to `_` intentionally leaks the handle for the
        // lifetime of this process: the named mutex stays owned and a second
        // bridge of the same arch cannot acquire the singleton (verified by
        // the "second bridge defers" harness check). The kernel reclaims the
        // handle on process exit.
        let _ = h;

        true
    }
}

fn dbg_log(msg: &str) {
    // Bridge 是 windows_subsystem = "windows" — stderr 不可见。
    // 写文件做诊断：%TEMP%\dzsspeedy-bridge.log
    // 不锁定文件、无缓冲刷新；性能影响可忽略（仅诊断路径调用）。
    let path = std::env::temp_dir().join("dzsspeedy-bridge.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            "[{}] [pid={}] {}",
            chrono_like_now(),
            std::process::id(),
            msg
        );
        let _ = f.flush();
    }
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    // 简易 ISO-ish 时间戳（避免拉 chrono 依赖）
    format!("{}.{:03}", secs, ms)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    dbg_log(&format!(
        "main: bridge launched, exe={} args={:?}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        args
    ));

    if args.len() > 1 {
        let line = args[1..].join(" ");

        if let Err(error) = writeln!(std::io::stdout(), "{}", handle_command(&line)) {
            dbg_log(&format!("one-shot response could not be written: {error}"));
        }

        return;
    }

    if !acquire_bridge_singleton() {
        // Another bridge instance owns the pipe. If it is healthy, defer to it
        // (exit 0, as before). If it is shutting down or wedged, wait (bounded)
        // for it to leave and then take over — otherwise a freshly started app
        // binds to the dying bridge and every INJECT fails with "bridge
        // shutdown is in progress" until the stale process is killed manually.
        let takeover_deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        loop {
            if existing_bridge_pipe_alive() {
                std::process::exit(0);
            }
            if acquire_bridge_singleton() {
                break;
            }
            if std::time::Instant::now() >= takeover_deadline {
                dbg_log("singleton takeover deadline reached; giving up");
                std::process::exit(2);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    if let Err(error) = start_shutdown_watcher() {
        dbg_log(&error);
    }

    pipe_server();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::core::HRESULT;

    use super::{
        classify_handshake, command_is_shutdown_gated, decode_initialize_exit_code,
        module_snapshot_error_kind, normalize_module_path, ordered_hook_threads,
        persisted_initialization_failure, try_acquire_operation_slot, wait_for_operations_drain,
        HandshakeProgress, HookThreadCandidate, ModuleSnapshotErrorKind, SpeedpatchHandshake,
        SpeedpatchState, TargetProcessHandle, OPERATION_SHUTDOWN_BIT,
    };

    #[test]
    fn orders_visible_hidden_and_process_threads_for_the_single_hook_path() {
        let candidates = [
            HookThreadCandidate {
                thread_id: 41,
                visible: false,
            },
            HookThreadCandidate {
                thread_id: 42,
                visible: true,
            },
            HookThreadCandidate {
                thread_id: 43,
                visible: true,
            },
        ];

        assert_eq!(
            ordered_hook_threads(&candidates, &[44, 41, 45]),
            vec![42, 43, 41, 44, 45]
        );
    }

    #[test]
    fn falls_back_to_process_threads_when_no_window_thread_exists() {
        assert_eq!(ordered_hook_threads(&[], &[0, 44, 44, 45]), vec![44, 45]);
    }

    #[test]
    fn rejects_zero_thread_ids() {
        let candidates = [HookThreadCandidate {
            thread_id: 0,
            visible: true,
        }];

        assert!(ordered_hook_threads(&candidates, &[0]).is_empty());
    }

    #[test]
    fn does_not_accept_a_terminal_state_before_callback_metadata() {
        for handshake in [
            SpeedpatchHandshake {
                state: SpeedpatchState::Enabled,
                init_result: windows::Win32::Foundation::ERROR_IO_PENDING.0,
                hook_thread_id: 71,
                callback_completed: false,
            },
            SpeedpatchHandshake {
                state: SpeedpatchState::Enabled,
                init_result: 0,
                hook_thread_id: 0,
                callback_completed: false,
            },
        ] {
            assert_eq!(classify_handshake(handshake), HandshakeProgress::Pending);
        }
    }

    #[test]
    fn does_not_accept_terminal_handshake_before_callback_completion_marker() {
        assert_eq!(
            classify_handshake(SpeedpatchHandshake {
                state: SpeedpatchState::Enabled,
                init_result: 0,
                hook_thread_id: 76,
                callback_completed: false,
            }),
            HandshakeProgress::Pending
        );
    }

    #[test]
    fn accepts_success_that_shutdown_kept_disabled() {
        assert_eq!(
            classify_handshake(SpeedpatchHandshake {
                state: SpeedpatchState::Disabled,
                init_result: 0,
                hook_thread_id: 72,
                callback_completed: true,
            }),
            HandshakeProgress::Complete {
                callback_thread: 72,
                state: SpeedpatchState::Disabled,
            }
        );
    }

    #[test]
    fn preserves_failed_callback_result() {
        assert_eq!(
            classify_handshake(SpeedpatchHandshake {
                state: SpeedpatchState::Failed,
                init_result: 0x0209_0008,
                hook_thread_id: 73,
                callback_completed: true,
            }),
            HandshakeProgress::Failed {
                callback_thread: 73,
                init_result: 0x0209_0008,
            }
        );
    }

    #[test]
    fn waits_for_failed_state_after_failure_result_is_published() {
        assert_eq!(
            classify_handshake(SpeedpatchHandshake {
                state: SpeedpatchState::Disabled,
                init_result: 0x0209_0008,
                hook_thread_id: 74,
                callback_completed: false,
            }),
            HandshakeProgress::Pending
        );
    }

    #[test]
    fn decodes_persisted_failure_after_bridge_restart() {
        let detail = persisted_initialization_failure(
            4_242_424,
            SpeedpatchHandshake {
                state: SpeedpatchState::Failed,
                init_result: 0x0209_0008,
                hook_thread_id: 75,
                callback_completed: true,
            },
        );

        assert!(detail.contains("callback_thread=75"));
        assert!(detail.contains("GetTickCount"));
        assert!(detail.contains("MH_ERROR_UNSUPPORTED_FUNCTION"));
    }

    #[test]
    fn shutdown_bit_atomically_closes_operation_admission() {
        let state = AtomicU32::new(0);
        try_acquire_operation_slot(&state).expect("acquire before shutdown");
        state.fetch_or(OPERATION_SHUTDOWN_BIT, Ordering::AcqRel);

        assert!(try_acquire_operation_slot(&state).is_err());
        assert_eq!(state.load(Ordering::Acquire), OPERATION_SHUTDOWN_BIT | 1);
    }

    #[test]
    fn drain_returns_immediately_when_no_leases_are_held() {
        let state = AtomicU32::new(0);
        let started = std::time::Instant::now();
        assert!(wait_for_operations_drain(&state, std::time::Duration::from_secs(5)));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn drain_respects_deadline_when_a_lease_is_held_forever() {
        // Regression test for the zombie-bridge root cause: a stuck
        // pending-injection monitor holds its lease forever; shutdown must
        // give up after a bounded wait instead of waiting forever.
        let state = AtomicU32::new(0);
        try_acquire_operation_slot(&state).expect("acquire held lease");
        let started = std::time::Instant::now();
        assert!(!wait_for_operations_drain(&state, std::time::Duration::from_millis(400)));
        let elapsed = started.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(350),
            "drain returned too early: {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "drain exceeded the deadline: {elapsed:?}"
        );
        state.fetch_sub(1, Ordering::AcqRel);
        assert!(wait_for_operations_drain(&state, std::time::Duration::from_millis(400)));
    }

    #[test]
    fn shutdown_gating_covers_health_probes_only() {
        for probe in ["GETSPEED", "PING", "VERSION"] {
            assert!(command_is_shutdown_gated(probe), "{probe} must be gated");
        }
        for command in [
            "INJECT", "EJECT", "ENABLE", "DISABLE", "ISENABLED", "STATUS", "SETSPEED", "SHUTDOWN",
        ] {
            assert!(
                !command_is_shutdown_gated(command),
                "{command} must not be probe-gated"
            );
        }
    }

    #[test]
    fn detects_target_process_exit_without_waiting_for_hook_timeout() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .expect("spawn target-exit fixture");
        let process = TargetProcessHandle::open(child.id()).expect("open target process handle");
        assert!(!process.has_exited().expect("query live target"));

        child.kill().expect("terminate target-exit fixture");
        child.wait().expect("wait for target-exit fixture");
        assert!(process.has_exited().expect("query exited target"));
    }

    #[test]
    fn decodes_precise_hook_creation_failure() {
        let code = 0x0200_0000 | (9 << 16) | 8;
        let error = match decode_initialize_exit_code(code) {
            Err(error) => error,
            Ok(()) => panic!("hook failure was accepted"),
        };

        assert!(error.detail.contains("GetTickCount"));
        assert!(error.detail.contains("MH_ERROR_UNSUPPORTED_FUNCTION"));
    }

    #[test]
    fn decodes_enable_and_restart_required_failures() {
        let enable_error = decode_initialize_exit_code(0x0300_0000 | 10).unwrap_err();
        assert!(enable_error.detail.contains("MH_EnableHook"));

        let restart_error = decode_initialize_exit_code(0x0700_0000).unwrap_err();
        assert!(restart_error.detail.contains("restart the target"));
    }

    #[test]
    fn decodes_hook_self_reference_failure() {
        let error = match decode_initialize_exit_code(0x0800_0000 | 5) {
            Err(error) => error,
            Ok(()) => panic!("self-reference failure was accepted"),
        };

        assert!(error.detail.contains("self-reference"));
        assert!(error.detail.contains("win32_error=5"));
    }

    #[test]
    fn decodes_hook_completion_event_failure() {
        let error = match decode_initialize_exit_code(0x0900_0000 | 5) {
            Err(error) => error,
            Ok(()) => panic!("completion event failure was accepted"),
        };

        assert!(error.detail.contains("completion event"));
        assert!(error.detail.contains("win32_error=5"));
    }

    #[test]
    fn treats_process_teardown_snapshot_errors_as_target_gone() {
        assert_eq!(
            module_snapshot_error_kind(HRESULT::from_win32(
                windows::Win32::Foundation::ERROR_INVALID_PARAMETER.0,
            )),
            ModuleSnapshotErrorKind::TargetGone
        );
        assert_eq!(
            module_snapshot_error_kind(HRESULT::from_win32(
                windows::Win32::Foundation::ERROR_PARTIAL_COPY.0,
            )),
            ModuleSnapshotErrorKind::TargetGone
        );
        assert_eq!(
            module_snapshot_error_kind(HRESULT::from_win32(
                windows::Win32::Foundation::ERROR_ACCESS_DENIED.0,
            )),
            ModuleSnapshotErrorKind::Fatal
        );
    }

    #[test]
    fn normalizes_extended_windows_module_paths() {
        assert_eq!(
            normalize_module_path(r"\\?\E:/Apps/DzsSpeedy/speedpatch64.dll"),
            r"e:\apps\dzsspeedy\speedpatch64.dll"
        );
    }
}
