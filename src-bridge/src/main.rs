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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{s, HRESULT, PCSTR, PCWSTR};

use windows::Win32::Foundation::{
    CloseHandle, FreeLibrary, GetLastError, ERROR_ALREADY_EXISTS, ERROR_BAD_LENGTH,
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, ERROR_PIPE_CONNECTED,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};

use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};

use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, CreateRemoteThread, GetExitCodeThread, IsWow64Process2,
    OpenProcess, WaitForSingleObject, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};

use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualAllocEx,
    VirtualFreeEx, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, FILE_MAP_WRITE, MEM_COMMIT, MEM_RELEASE,
    MEM_RESERVE, PAGE_READWRITE,
};

use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;

use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
};

use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};

use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, NAMED_PIPE_MODE,
    PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386,
    IMAGE_FILE_MACHINE_UNKNOWN,
};
// ── Helpers ──────────────────────────────────────────────────────────────

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

type RemoteThreadStart = unsafe extern "system" fn(*mut std::ffi::c_void) -> u32;

unsafe fn remote_thread_start(address: usize) -> RemoteThreadStart {
    unsafe { std::mem::transmute::<usize, RemoteThreadStart>(address) }
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

const SP_STATE_INITIALIZING: u32 = 0x49;
const SP_STATE_DISABLED: u32 = 0x44;
const SP_STATE_ENABLED: u32 = 0x45;
const SP_STATE_FAILED: u32 = 0x46;

/// Read the DLL-owned handshake mapping.
///
/// `Ok(None)` is deliberately reserved for an absent mapping. Permission and
/// mapping failures are status probe errors, not evidence that the DLL is gone.
fn read_speedpatch_state(pid: u32) -> Result<Option<SpeedpatchState>, String> {
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

        let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, std::mem::size_of::<u32>());

        if view.Value.is_null() {
            let gle = GetLastError();
            let _ = CloseHandle(h);

            return Err(format!(
                "MapViewOfFile(DzsSpeedy.{pid}, FILE_MAP_READ) failed: gle={} (0x{:08x})",
                gle.0, gle.0
            ));
        }

        let state = (&*(view.Value as *const AtomicU32)).load(Ordering::Acquire);

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        match state {
            SP_STATE_INITIALIZING => Ok(Some(SpeedpatchState::Initializing)),
            SP_STATE_DISABLED => Ok(Some(SpeedpatchState::Disabled)),
            SP_STATE_ENABLED => Ok(Some(SpeedpatchState::Enabled)),
            SP_STATE_FAILED => Ok(Some(SpeedpatchState::Failed)),
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
        if state.load(Ordering::Acquire) == SP_STATE_FAILED {
            let _ = UnmapViewOfFile(view);
            let _ = CloseHandle(h);
            return Err(format!(
                "SP_Initialize failed for pid={pid}; restart the target before changing state"
            ));
        }

        state.store(
            if enabled {
                SP_STATE_ENABLED
            } else {
                SP_STATE_DISABLED
            },
            Ordering::Release,
        );

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        Ok(())
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
    untrack_target(pid);
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

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static ACTIVE_REMOTE_OPERATIONS: AtomicU32 = AtomicU32::new(0);

struct RemoteOperationLease;

impl RemoteOperationLease {
    fn new() -> Self {
        ACTIVE_REMOTE_OPERATIONS.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for RemoteOperationLease {
    fn drop(&mut self) {
        ACTIVE_REMOTE_OPERATIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn finish_injection_success(pid: u32) {
    clear_injection_stage(pid);
    clear_injection_failure(pid);
    track_target(pid);

    if SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
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

enum RemoteLoadError {
    Failed(String),
    Pending {
        detail: String,
        thread: HANDLE,
        remote_mem: usize,
    },
}

struct RemoteInitError {
    detail: String,
    safe_to_unload: bool,
    pending_thread: Option<HANDLE>,
}

enum FinishLoadedOutcome {
    Complete,
    Pending(String),
}

/// Inject using one same-architecture path: LoadLibraryW, then SP_Initialize.
fn do_inject(pid: u32) -> Result<(), String> {
    if SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
        return Err("bridge shutdown is in progress; injection was not started".into());
    }
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
    let state = read_speedpatch_state(pid)?;
    match (existing_module.as_ref(), state) {
        (Some(_), Some(SpeedpatchState::Enabled)) => {
            clear_injection_failure(pid);
            track_target(pid);
            return Ok(());
        }
        (Some(_), Some(SpeedpatchState::Disabled)) => {
            clear_injection_failure(pid);
            return do_enable(pid);
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
            return Err(injection_failure(pid).unwrap_or_else(|| {
                format!(
                    "SP_Initialize failed for pid={pid}; the target must be restarted before injection can be retried"
                )
            }));
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
                "{} is already loaded in pid={pid} from {}, but its status mapping is absent. Automatic recovery is disabled so injection always follows one fixed LoadLibraryW -> SP_Initialize chain.{previous} Restart the target process before retrying.",
                dll_name, module.path
            ));
        }
        (None, None) => {
            clear_injection_failure(pid);
        }
    }

    let h_proc = unsafe {
        OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_QUERY_INFORMATION
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ,
            false,
            pid,
        )
    }
    .map_err(|e| {
        format!(
            "OpenProcess(pid={pid}) failed: {e:?}. If already admin, target may be protected or higher integrity."
        )
    })?;

    let operation = RemoteOperationLease::new();
    set_injection_stage(pid, InjectionStage::Loading);
    match inject_via_load_library_w(pid, &h_proc, &dll_str, dll_name) {
        Ok(module) => {
            set_injection_stage(pid, InjectionStage::Initializing);
            match finish_loaded_module(pid, h_proc, &dll_str, module.base, operation)? {
                FinishLoadedOutcome::Complete => Ok(()),
                FinishLoadedOutcome::Pending(detail) => Err(format!("INJECTION_PENDING: {detail}")),
            }
        }
        Err(RemoteLoadError::Failed(detail)) => {
            clear_injection_stage(pid);
            unsafe {
                let _ = CloseHandle(h_proc);
            }
            Err(format!(
                "CreateRemoteThread + LoadLibraryW failed for pid={pid}, dll={dll_str}: {detail}"
            ))
        }
        Err(RemoteLoadError::Pending {
            detail,
            thread,
            remote_mem,
        }) => {
            reap_remote_load_async(
                pid,
                h_proc,
                thread,
                remote_mem,
                dll_str.clone(),
                dll_name.to_string(),
                operation,
            );
            Err(format!(
                "INJECTION_PENDING: CreateRemoteThread + LoadLibraryW is still running for pid={pid}, dll={dll_str}: {detail}"
            ))
        }
    }
}

fn finish_loaded_module(
    pid: u32,
    h_proc: HANDLE,
    dll_path: &str,
    remote_module: usize,
    operation: RemoteOperationLease,
) -> Result<FinishLoadedOutcome, String> {
    if let Err(init_error) = initialize_remote_speedpatch(&h_proc, dll_path, remote_module) {
        if let Some(thread) = init_error.pending_thread {
            set_injection_stage(pid, InjectionStage::Initializing);
            reap_remote_init_async(pid, h_proc, thread, remote_module, operation);
            return Ok(FinishLoadedOutcome::Pending(init_error.detail));
        }

        let cleanup = if init_error.safe_to_unload {
            match remote_free_library(pid, &h_proc, remote_module) {
                Ok(()) => "; the failed load reference was released".to_string(),
                Err(error) => format!("; failed to release the loaded DLL: {error}"),
            }
        } else {
            "; DLL remains loaded because initialization completion is uncertain".to_string()
        };

        unsafe {
            let _ = CloseHandle(h_proc);
        }
        let detail = format!(
            "LoadLibraryW loaded speedpatch for pid={pid}, but SP_Initialize failed: {}{}",
            init_error.detail, cleanup
        );
        record_injection_failure(pid, detail.clone());
        return Err(detail);
    }

    unsafe {
        let _ = CloseHandle(h_proc);
    }
    finish_injection_success(pid);
    dbg_log(&format!(
        "do_inject pid={pid}: LoadLibraryW + SP_Initialize OK"
    ));
    Ok(FinishLoadedOutcome::Complete)
}

fn initialize_remote_speedpatch(
    h_proc: &HANDLE,
    dll_path: &str,
    remote_module: usize,
) -> Result<(), RemoteInitError> {
    let dll_name = speedpatch_dll(BRIDGE_IS64);
    let dll_wide = to_wide(dll_path);
    let local_module =
        unsafe { LoadLibraryW(PCWSTR::from_raw(dll_wide.as_ptr())) }.map_err(|e| {
            RemoteInitError {
                detail: format!("LoadLibraryW(local {dll_name}) failed: {e:?}"),
                safe_to_unload: true,
                pending_thread: None,
            }
        })?;

    let init_rva = unsafe { GetProcAddress(local_module, s!("SP_Initialize")) }
        .ok_or_else(|| "GetProcAddress(SP_Initialize) failed".to_string())
        .and_then(|proc| {
            (proc as usize)
                .checked_sub(local_module.0 as usize)
                .ok_or_else(|| "SP_Initialize address is below local module base".to_string())
        });
    unsafe {
        let _ = FreeLibrary(local_module);
    }
    let init_rva = init_rva.map_err(|detail| RemoteInitError {
        detail,
        safe_to_unload: true,
        pending_thread: None,
    })?;
    let remote_init = remote_module + init_rva;

    let h_thread = unsafe {
        CreateRemoteThread(
            *h_proc,
            None,
            0,
            Some(remote_thread_start(remote_init)),
            None,
            0,
            None,
        )
    }
    .map_err(|e| RemoteInitError {
        detail: format!("CreateRemoteThread(SP_Initialize) failed: {e:?}"),
        safe_to_unload: true,
        pending_thread: None,
    })?;

    let wait_result = unsafe { WaitForSingleObject(h_thread, 15_000) };
    if wait_result == WAIT_TIMEOUT {
        return Err(RemoteInitError {
            detail: "SP_Initialize exceeded 15s and is still running".into(),
            safe_to_unload: false,
            pending_thread: Some(h_thread),
        });
    }
    if wait_result != WAIT_OBJECT_0 {
        let detail = if wait_result == WAIT_FAILED {
            let gle = unsafe { GetLastError() };
            format!("WaitForSingleObject(SP_Initialize) failed: gle={}", gle.0)
        } else {
            format!(
                "WaitForSingleObject(SP_Initialize) returned 0x{:08x}",
                wait_result.0
            )
        };
        unsafe {
            let _ = CloseHandle(h_thread);
        }
        return Err(RemoteInitError {
            detail,
            safe_to_unload: false,
            pending_thread: None,
        });
    }

    let mut exit_code = 0u32;
    let exit_result = unsafe { GetExitCodeThread(h_thread, &mut exit_code) };
    unsafe {
        let _ = CloseHandle(h_thread);
    }
    exit_result.map_err(|e| RemoteInitError {
        detail: format!("GetExitCodeThread(SP_Initialize) failed: {e:?}"),
        safe_to_unload: false,
        pending_thread: None,
    })?;

    decode_initialize_exit_code(exit_code)
}

fn decode_initialize_exit_code(code: u32) -> Result<(), RemoteInitError> {
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
        _ => match code {
            0 => return Ok(()),
            1 => "MinHook initialization failed".into(),
            2 => "one or more time hooks failed to install".into(),
            3 => "shared status mapping initialization failed".into(),
            170 => "SP_Initialize is already running".into(),
            _ => format!("SP_Initialize returned code 0x{code:08x}"),
        },
    };

    let safe_to_unload = matches!(kind, 0x0100_0000 | 0x0200_0000 | 0x0400_0000 | 0x0500_0000)
        || matches!(code, 1..=3);
    Err(RemoteInitError {
        detail,
        safe_to_unload,
        pending_thread: None,
    })
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
            Err(error) if error.code() == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
                // Windows returns ERROR_INVALID_PARAMETER once a PID no longer exists.
                // Status polling should report that as not injected, not as a transport error.
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
        if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
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
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) => {
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

fn confirm_remote_module(
    pid: u32,
    module_name: &str,
    expected_path: &str,
) -> Result<RemoteModule, String> {
    let mut last_error = None;
    for attempt in 0..50 {
        match find_remote_module(pid, module_name, Some(expected_path)) {
            Ok(Some(module)) => return Ok(module),
            Ok(None) => {
                last_error = Some(format!(
                    "{module_name} is not present in the module snapshot"
                ));
            }
            Err(error) => last_error = Some(error),
        }
        if attempt < 49 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    Err(last_error.unwrap_or_else(|| format!("could not confirm {module_name} in pid={pid}")))
}

fn local_module_containing_address(address: usize) -> Result<(String, usize), String> {
    let pid = std::process::id();
    let snapshot = create_module_snapshot(pid)?
        .ok_or_else(|| format!("could not snapshot bridge modules for pid={pid}"))?;
    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };

    if let Err(error) = unsafe { Module32FirstW(snapshot, &mut entry) } {
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        return Err(format!(
            "Module32FirstW(bridge pid={pid}) failed while resolving address 0x{address:x}: {error:?}"
        ));
    }

    let result = loop {
        let base = entry.modBaseAddr as usize;
        let end = base.saturating_add(entry.modBaseSize as usize);
        if address >= base && address < end {
            break Ok((module_text(&entry.szModule), base));
        }

        match unsafe { Module32NextW(snapshot, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) => {
                break Err(format!(
                    "no bridge module contains procedure address 0x{address:x}"
                ));
            }
            Err(error) => {
                break Err(format!(
                    "Module32NextW(bridge pid={pid}) failed while resolving address 0x{address:x}: {error:?}"
                ));
            }
        }
    };

    unsafe {
        let _ = CloseHandle(snapshot);
    }
    result
}

fn remote_module_base(pid: u32, module_name: &str) -> Result<usize, String> {
    find_remote_module(pid, module_name, None)?
        .map(|module| module.base)
        .ok_or_else(|| format!("module {module_name} not found in pid={pid}"))
}

fn remote_system_proc(pid: u32, proc_name: &str) -> Result<(usize, usize, usize), String> {
    let kernel32_w = to_wide("kernel32.dll");
    let local_kernel32 = unsafe { GetModuleHandleW(PCWSTR::from_raw(kernel32_w.as_ptr())) }
        .map_err(|e| format!("GetModuleHandleW(kernel32.dll): {e:?}"))?;
    let proc_cstr = std::ffi::CString::new(proc_name).unwrap();
    let local_proc = unsafe {
        GetProcAddress(
            local_kernel32,
            PCSTR::from_raw(proc_cstr.as_ptr() as *const u8),
        )
    }
    .ok_or_else(|| format!("GetProcAddress {proc_name}"))? as usize;
    // Forwarded kernel32 exports may resolve inside KernelBase on some Windows
    // builds. Compute the RVA from the module that actually owns the address.
    let (owner_name, local_base) = local_module_containing_address(local_proc)?;
    let rva = local_proc
        .checked_sub(local_base)
        .ok_or_else(|| format!("{proc_name} address is below local {owner_name} base"))?;
    let remote_base = remote_module_base(pid, &owner_name)?;
    dbg_log(&format!(
        "remote_system_proc: pid={pid} proc={proc_name} owner={owner_name} local=0x{local_proc:x} rva=0x{rva:x} remote=0x{:x}",
        remote_base + rva
    ));
    Ok((remote_base + rva, local_proc, rva))
}
fn inject_via_load_library_w(
    pid: u32,
    h_proc: &HANDLE,
    dll_path: &str,
    dll_name: &str,
) -> Result<RemoteModule, RemoteLoadError> {
    let path_bytes: Vec<u8> = dll_path
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .chain([0u8, 0u8])
        .collect();

    let path_len = path_bytes.len();
    let (load_lib, local_load_lib, load_lib_rva) =
        remote_system_proc(pid, "LoadLibraryW").map_err(RemoteLoadError::Failed)?;

    let remote_mem = unsafe {
        VirtualAllocEx(
            *h_proc,
            None,
            path_len,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };

    if remote_mem.is_null() {
        let gle = unsafe { GetLastError() };

        return Err(RemoteLoadError::Failed(format!(
            "VirtualAllocEx failed: gle={} (0x{:08x})",
            gle.0, gle.0
        )));
    }

    let mut written = 0usize;
    let write_result = unsafe {
        WriteProcessMemory(
            *h_proc,
            remote_mem,
            path_bytes.as_ptr() as _,
            path_len,
            Some(&mut written),
        )
    };
    if let Err(e) = write_result {
        unsafe {
            let _ = VirtualFreeEx(*h_proc, remote_mem, 0, MEM_RELEASE);
        }
        return Err(RemoteLoadError::Failed(format!(
            "WriteProcessMemory(path) failed: {e:?}"
        )));
    }
    if written != path_len {
        unsafe {
            let _ = VirtualFreeEx(*h_proc, remote_mem, 0, MEM_RELEASE);
        }
        return Err(RemoteLoadError::Failed(format!(
            "WriteProcessMemory(path) wrote {written}/{path_len} bytes"
        )));
    }

    dbg_log(&format!(
        "inject_via_load_library_w: pid={} local=0x{:x} rva=0x{:x} remote=0x{:x}",
        pid, local_load_lib, load_lib_rva, load_lib
    ));
    let h_thread = unsafe {
        CreateRemoteThread(
            *h_proc,
            None,
            0,
            Some(remote_thread_start(load_lib)),
            Some(remote_mem),
            0,
            None,
        )
    }
    .map_err(|e| {
        unsafe {
            let _ = VirtualFreeEx(*h_proc, remote_mem, 0, MEM_RELEASE);
        }

        RemoteLoadError::Failed(format!("CreateRemoteThread failed: {e:?}"))
    })?;

    let wait_result = unsafe { WaitForSingleObject(h_thread, 15_000) };
    if wait_result != WAIT_OBJECT_0 {
        let detail = if wait_result == WAIT_TIMEOUT {
            "remote LoadLibrary thread timed out after 15s".to_string()
        } else if wait_result == WAIT_FAILED {
            let gle = unsafe { GetLastError() };
            format!(
                "WaitForSingleObject failed: gle={} (0x{:08x})",
                gle.0, gle.0
            )
        } else {
            format!(
                "WaitForSingleObject returned unexpected value 0x{:08x}",
                wait_result.0
            )
        };

        return Err(RemoteLoadError::Pending {
            detail,
            thread: h_thread,
            remote_mem: remote_mem as usize,
        });
    }

    let mut exit_code = 0u32;
    let exit_result = unsafe { GetExitCodeThread(h_thread, &mut exit_code) };

    unsafe {
        let _ = VirtualFreeEx(*h_proc, remote_mem, 0, MEM_RELEASE);

        let _ = CloseHandle(h_thread);
    }

    let exit_detail = match exit_result {
        Ok(()) => format!("remote thread exit_code=0x{exit_code:08x}"),
        Err(error) => format!("GetExitCodeThread failed: {error:?}"),
    };

    // GetExitCodeThread is only 32-bit and truncates HMODULE on x64. Confirm
    // success from the target's module list instead of interpreting that value.
    let module = confirm_remote_module(pid, dll_name, dll_path).map_err(|detail| {
        RemoteLoadError::Failed(format!(
            "LoadLibraryW thread completed but {dll_name} could not be confirmed in pid={pid} ({exit_detail}): {detail}. The target loader rejected the DLL, a dependency is missing, or target policy blocked it."
        ))
    })?;

    dbg_log(&format!(
        "inject_via_load_library_w: module confirmed at 0x{:x}, path={}, {}",
        module.base, module.path, exit_detail
    ));
    Ok(module)
}

fn reap_remote_load_async(
    pid: u32,
    h_proc: HANDLE,
    h_thread: HANDLE,
    remote_mem: usize,
    dll_path: String,
    dll_name: String,
    operation: RemoteOperationLease,
) {
    let process_raw = h_proc.0 as usize;
    let thread_raw = h_thread.0 as usize;
    std::thread::spawn(move || unsafe {
        let process = HANDLE(process_raw as *mut std::ffi::c_void);
        let thread = HANDLE(thread_raw as *mut std::ffi::c_void);
        let wait_result = WaitForSingleObject(thread, u32::MAX);
        if wait_result != WAIT_OBJECT_0 {
            let detail = format!(
                "remote LoadLibraryW wait failed for pid={pid}: result=0x{:08x}; path memory was retained because thread completion is unknown",
                wait_result.0
            );
            record_injection_failure(pid, detail.clone());
            dbg_log(&detail);
            let _ = CloseHandle(thread);
            let _ = CloseHandle(process);
            return;
        }

        let mut exit_code = 0u32;
        let exit_detail = match GetExitCodeThread(thread, &mut exit_code) {
            Ok(()) => format!("remote thread exit_code=0x{exit_code:08x}"),
            Err(error) => format!("GetExitCodeThread failed: {error:?}"),
        };
        let _ = VirtualFreeEx(process, remote_mem as *mut std::ffi::c_void, 0, MEM_RELEASE);
        let _ = CloseHandle(thread);

        let module = match confirm_remote_module(pid, &dll_name, &dll_path) {
            Ok(module) => module,
            Err(error) => {
                let detail = format!(
                    "LoadLibraryW completed asynchronously for pid={pid}, but {dll_name} could not be confirmed ({exit_detail}): {error}"
                );
                record_injection_failure(pid, detail.clone());
                dbg_log(&detail);
                let _ = CloseHandle(process);
                return;
            }
        };

        set_injection_stage(pid, InjectionStage::Initializing);
        match finish_loaded_module(pid, process, &dll_path, module.base, operation) {
            Ok(FinishLoadedOutcome::Complete) => dbg_log(&format!(
                "remote LoadLibraryW continuation: pid={pid} completed SP_Initialize successfully"
            )),
            Ok(FinishLoadedOutcome::Pending(detail)) => dbg_log(&format!(
                "remote LoadLibraryW continuation: pid={pid} SP_Initialize remains pending: {detail}"
            )),
            Err(error) => dbg_log(&format!(
                "remote LoadLibraryW continuation: pid={pid} SP_Initialize failed: {error}"
            )),
        }
    });
}

fn reap_remote_init_async(
    pid: u32,
    h_proc: HANDLE,
    h_thread: HANDLE,
    remote_module: usize,
    _operation: RemoteOperationLease,
) {
    let process_raw = h_proc.0 as usize;
    let thread_raw = h_thread.0 as usize;
    std::thread::spawn(move || unsafe {
        let process = HANDLE(process_raw as *mut std::ffi::c_void);
        let thread = HANDLE(thread_raw as *mut std::ffi::c_void);
        let wait_result = WaitForSingleObject(thread, u32::MAX);
        if wait_result == WAIT_OBJECT_0 {
            let mut exit_code = 0u32;
            match GetExitCodeThread(thread, &mut exit_code) {
                Ok(()) => match decode_initialize_exit_code(exit_code) {
                    Ok(()) => {
                        finish_injection_success(pid);
                        dbg_log(&format!(
                            "remote SP_Initialize reaper: pid={pid} completed successfully"
                        ));
                    }
                    Err(error) => {
                        let cleanup = if error.safe_to_unload {
                            remote_free_library(pid, &process, remote_module)
                                .map(|_| "load reference released".to_string())
                                .unwrap_or_else(|cleanup_error| {
                                    format!("load reference cleanup failed: {cleanup_error}")
                                })
                        } else {
                            "DLL left loaded".to_string()
                        };
                        let detail = format!(
                            "remote SP_Initialize failed for pid={pid}: {}; {cleanup}",
                            error.detail
                        );
                        record_injection_failure(pid, detail.clone());
                        dbg_log(&detail);
                    }
                },
                Err(error) => {
                    let detail = format!(
                        "remote SP_Initialize completed for pid={pid}, but GetExitCodeThread failed: {error:?}"
                    );
                    record_injection_failure(pid, detail.clone());
                    dbg_log(&detail);
                }
            }
        } else {
            let detail = format!(
                "remote SP_Initialize reaper: pid={pid} wait failed result=0x{:08x}",
                wait_result.0
            );
            record_injection_failure(pid, detail.clone());
            dbg_log(&detail);
        }
        let _ = CloseHandle(thread);
        let _ = CloseHandle(process);
    });
}

fn reap_thread_handle_async(label: String, h_thread: HANDLE) {
    let thread_raw = h_thread.0 as usize;
    std::thread::spawn(move || unsafe {
        let thread = HANDLE(thread_raw as *mut std::ffi::c_void);
        let wait_result = WaitForSingleObject(thread, u32::MAX);
        dbg_log(&format!(
            "{label} reaper finished with wait result=0x{:08x}",
            wait_result.0
        ));
        let _ = CloseHandle(thread);
    });
}

fn remote_free_library(pid: u32, h_proc: &HANDLE, remote_module: usize) -> Result<(), String> {
    let (free_library, _, _) = remote_system_proc(pid, "FreeLibrary")?;
    let h_thread = unsafe {
        CreateRemoteThread(
            *h_proc,
            None,
            0,
            Some(remote_thread_start(free_library)),
            Some(remote_module as *mut std::ffi::c_void),
            0,
            None,
        )
    }
    .map_err(|error| format!("CreateRemoteThread(FreeLibrary) failed: {error:?}"))?;

    let wait_result = unsafe { WaitForSingleObject(h_thread, 5_000) };
    if wait_result != WAIT_OBJECT_0 {
        let detail = if wait_result == WAIT_TIMEOUT {
            "FreeLibrary timed out after 5s; cleanup continues asynchronously".to_string()
        } else if wait_result == WAIT_FAILED {
            let gle = unsafe { GetLastError() };
            format!(
                "WaitForSingleObject(FreeLibrary) failed: gle={}; cleanup ownership transferred to a reaper",
                gle.0
            )
        } else {
            format!(
                "WaitForSingleObject(FreeLibrary) returned 0x{:08x}; cleanup ownership transferred to a reaper",
                wait_result.0
            )
        };
        reap_thread_handle_async(format!("remote FreeLibrary pid={pid}"), h_thread);
        return Err(detail);
    }

    let mut exit_code = 0u32;
    let result = unsafe { GetExitCodeThread(h_thread, &mut exit_code) };
    unsafe {
        let _ = CloseHandle(h_thread);
    }
    result.map_err(|error| format!("GetExitCodeThread(FreeLibrary) failed: {error:?}"))?;
    if exit_code == 0 {
        return Err("remote FreeLibrary returned FALSE".into());
    }
    Ok(())
}

fn do_eject(pid: u32) -> Result<(), String> {
    let dll_name = speedpatch_dll(BRIDGE_IS64);
    let local_dll = exe_dir()?.join(dll_name);
    let local_dll_path = local_dll.to_string_lossy().to_string();
    let module = find_remote_module(pid, dll_name, Some(&local_dll_path))?
        .ok_or_else(|| format!("{dll_name} is not loaded in pid={pid}"))?;

    let local_dll_w = to_wide(&local_dll_path);
    let local_module = unsafe { LoadLibraryW(PCWSTR::from_raw(local_dll_w.as_ptr())) }
        .map_err(|error| format!("LoadLibraryW(local {dll_name}) failed: {error:?}"))?;
    let shutdown_rva = unsafe { GetProcAddress(local_module, s!("SP_Shutdown")) }
        .ok_or_else(|| "GetProcAddress(SP_Shutdown) failed".to_string())
        .and_then(|proc| {
            (proc as usize)
                .checked_sub(local_module.0 as usize)
                .ok_or_else(|| "SP_Shutdown address is below local module base".to_string())
        });
    unsafe {
        let _ = FreeLibrary(local_module);
    }
    let remote_shutdown = module.base + shutdown_rva?;

    let h_proc = unsafe {
        OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_QUERY_INFORMATION
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ,
            false,
            pid,
        )
    }
    .map_err(|error| format!("OpenProcess(pid={pid}) failed: {error:?}"))?;
    let h_shutdown = match unsafe {
        CreateRemoteThread(
            h_proc,
            None,
            0,
            Some(remote_thread_start(remote_shutdown)),
            None,
            0,
            None,
        )
    } {
        Ok(thread) => thread,
        Err(error) => {
            unsafe {
                let _ = CloseHandle(h_proc);
            }
            return Err(format!("CreateRemoteThread(SP_Shutdown) failed: {error:?}"));
        }
    };

    let wait_result = unsafe { WaitForSingleObject(h_shutdown, 5_000) };
    if wait_result != WAIT_OBJECT_0 {
        let detail = if wait_result == WAIT_TIMEOUT {
            "SP_Shutdown timed out after 5s; DLL was not unloaded".to_string()
        } else if wait_result == WAIT_FAILED {
            let gle = unsafe { GetLastError() };
            format!("WaitForSingleObject(SP_Shutdown) failed: gle={}", gle.0)
        } else {
            format!(
                "WaitForSingleObject(SP_Shutdown) returned 0x{:08x}",
                wait_result.0
            )
        };
        unsafe {
            let _ = CloseHandle(h_shutdown);
            let _ = CloseHandle(h_proc);
        }
        return Err(detail);
    }

    let mut shutdown_code = 0u32;
    let exit_result = unsafe { GetExitCodeThread(h_shutdown, &mut shutdown_code) };
    unsafe {
        let _ = CloseHandle(h_shutdown);
    }
    if let Err(error) = exit_result {
        unsafe {
            let _ = CloseHandle(h_proc);
        }
        return Err(format!(
            "GetExitCodeThread(SP_Shutdown) failed: {error:?}; DLL was not unloaded"
        ));
    }
    if shutdown_code != 0 {
        unsafe {
            let _ = CloseHandle(h_proc);
        }
        if shutdown_code == windows::Win32::Foundation::ERROR_NOT_SUPPORTED.0 {
            return Err(
                "live ejection is intentionally disabled because active time-hook call stacks cannot be unloaded safely; acceleration was disabled, and the DLL will unload when the target exits"
                    .into(),
            );
        }
        return Err(format!(
            "SP_Shutdown returned win32_error={shutdown_code}; DLL was not unloaded"
        ));
    }

    let unload_result = remote_free_library(pid, &h_proc, module.base);
    unsafe {
        let _ = CloseHandle(h_proc);
    }
    unload_result?;

    if let Some(still_loaded) = find_remote_module(pid, dll_name, Some(&local_dll_path))? {
        return Err(format!(
            "FreeLibrary returned success, but {dll_name} remains loaded at 0x{:x}; another loader reference exists",
            still_loaded.base
        ));
    }
    untrack_target(pid);
    Ok(())
}

fn do_enable(pid: u32) -> Result<(), String> {
    match read_speedpatch_state(pid)? {
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
            return Err(format!(
                "SP_Initialize failed for pid={pid}; restart the target before retrying"
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
    match read_speedpatch_state(pid)? {
        Some(SpeedpatchState::Enabled) => Ok(true),
        Some(SpeedpatchState::Disabled) => Ok(false),
        Some(SpeedpatchState::Initializing) => {
            Err(format!("SP_Initialize is still running for pid={pid}"))
        }
        Some(SpeedpatchState::Failed) => Err(format!(
            "SP_Initialize failed for pid={pid}; restart the target before retrying"
        )),
        None => Err(format!("no DzsSpeedy.{pid} mapping")),
    }
}

/// Check the exact state of the one supported injection chain.
fn do_status(pid: u32) -> Result<InjectionStatus, String> {
    match read_speedpatch_state(pid)? {
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
            return Ok(InjectionStatus::Failed(
                injection_failure(pid).unwrap_or_else(|| {
                    format!(
                        "SP_Initialize failed for pid={pid}; restart the target before retrying"
                    )
                }),
            ));
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

        "SHUTDOWN" => "OK shutting down".into(),

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
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
    disable_tracked_targets();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while ACTIVE_REMOTE_OPERATIONS.load(Ordering::Acquire) != 0
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    let remaining = ACTIVE_REMOTE_OPERATIONS.load(Ordering::Acquire);
    if remaining != 0 {
        dbg_log(&format!(
            "shutdown grace period expired with {remaining} remote operation(s) still active"
        ));
    }

    // An injection may have completed after the first snapshot. Its success
    // path also observes SHUTDOWN_REQUESTED, and this second pass closes the race.
    disable_tracked_targets();
    dbg_log("bridge shutdown complete");
    std::process::exit(0);
}

fn start_shutdown_watcher() -> Result<(), String> {
    let event_name = to_wide(SHUTDOWN_EVENT);
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::from_raw(event_name.as_ptr())) }
        .map_err(|error| format!("CreateEventW({SHUTDOWN_EVENT}) failed: {error:?}"))?;
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
        if existing_bridge_pipe_alive() {
            std::process::exit(0);
        }

        std::process::exit(2);
    }

    if let Err(error) = start_shutdown_watcher() {
        dbg_log(&error);
    }

    pipe_server();
}

#[cfg(test)]
mod tests {
    use super::{decode_initialize_exit_code, normalize_module_path};

    #[test]
    fn decodes_precise_hook_creation_failure_as_safe_to_unload() {
        let code = 0x0200_0000 | (9 << 16) | 8;
        let error = match decode_initialize_exit_code(code) {
            Err(error) => error,
            Ok(()) => panic!("hook failure was accepted"),
        };

        assert!(error.safe_to_unload);
        assert!(error.detail.contains("GetTickCount"));
        assert!(error.detail.contains("MH_ERROR_UNSUPPORTED_FUNCTION"));
    }

    #[test]
    fn treats_enable_and_restart_required_failures_as_unsafe_to_unload() {
        for code in [0x0300_0000 | 10, 0x0700_0000] {
            let error = match decode_initialize_exit_code(code) {
                Err(error) => error,
                Ok(()) => panic!("unsafe initialization failure was accepted"),
            };
            assert!(!error.safe_to_unload);
        }
    }

    #[test]
    fn normalizes_extended_windows_module_paths() {
        assert_eq!(
            normalize_module_path(r"\\?\E:/Apps/DzsSpeedy/speedpatch64.dll"),
            r"e:\apps\dzsspeedy\speedpatch64.dll"
        );
    }
}
