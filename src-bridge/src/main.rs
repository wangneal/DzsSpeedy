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



use std::ffi::OsStr;

use std::os::windows::ffi::OsStrExt;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use std::fs::OpenOptions;
use std::io::Write;

use windows::core::{PCSTR, PCWSTR, PWSTR, s};

use windows::Win32::Foundation::{

    CloseHandle, GetLastError, HANDLE, BOOL, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED,

    HINSTANCE, HWND, LPARAM, INVALID_HANDLE_VALUE,

};

use windows::Win32::Storage::FileSystem::{

    CreateFileW, WriteFile, ReadFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,

    OPEN_EXISTING,

};

use windows::Win32::System::Threading::{

    CreateMutexW, CreateRemoteThread, GetCurrentProcess, IsWow64Process, OpenProcess,

    QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_CREATE_THREAD,

    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,

    PROCESS_NAME_WIN32,

};

use windows::Win32::System::Memory::{

    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualAllocEx,

    VirtualFreeEx, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, FILE_MAP_WRITE, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,

    PAGE_READWRITE, PAGE_EXECUTE_READWRITE,

};

use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};

use windows::Win32::System::Diagnostics::ToolHelp::{

    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W,

    TH32CS_SNAPMODULE,

};

use windows::Win32::System::LibraryLoader::{

    GetModuleHandleW, GetProcAddress, LoadLibraryW,

};

use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, WH_GETMESSAGE, WM_NULL,
};
use windows::Win32::System::Pipes::{

    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, PIPE_READMODE_MESSAGE,

    PIPE_TYPE_MESSAGE, PIPE_WAIT, NAMED_PIPE_MODE,

};



fn retained_hooks() -> &'static Mutex<Vec<(u32, usize)>> {
    static HOOKS: OnceLock<Mutex<Vec<(u32, usize)>>> = OnceLock::new();
    HOOKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn retain_hooks(pid: u32, hooks: Vec<HHOOK>) {
    let mut guard = retained_hooks().lock().unwrap();
    for hook in hooks {
        guard.push((pid, hook.0 as usize));
    }
}

fn release_retained_hooks(pid: u32) {
    let mut guard = retained_hooks().lock().unwrap();
    let mut kept = Vec::with_capacity(guard.len());
    for (hook_pid, raw_hook) in guard.drain(..) {
        if hook_pid == pid {
            let _ = unsafe { UnhookWindowsHookEx(HHOOK(raw_hook as _)) };
        } else {
            kept.push((hook_pid, raw_hook));
        }
    }
    *guard = kept;
}

fn retained_hook_count() -> usize {
    retained_hooks().lock().map(|hooks| hooks.len()).unwrap_or(0)
}
fn retained_hook_count_for_pid(pid: u32) -> usize {
    retained_hooks()
        .lock()
        .map(|hooks| hooks.iter().filter(|(hook_pid, _)| *hook_pid == pid).count())
        .unwrap_or(0)
}
// ── Helpers ──────────────────────────────────────────────────────────────



fn to_wide(s: &str) -> Vec<u16> {

    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()

}



fn exe_dir() -> PathBuf {

    let mut buf = vec![0u16; 260];

    let mut len = buf.len() as u32;

    let h = unsafe { GetCurrentProcess() };

    unsafe {

        let _ = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR::from_raw(buf.as_mut_ptr()), &mut len);

    }

    let s = String::from_utf16_lossy(&buf[..len as usize]);

    PathBuf::from(&s).parent().unwrap_or(&PathBuf::from(".")).to_path_buf()

}



fn is_process_64bit(pid: u32) -> bool {

    unsafe {

        let Ok(h) = OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) else { return true };

        let mut wow64 = BOOL::default();

        if IsWow64Process(h, &mut wow64).is_err() {

            let _ = CloseHandle(h);

            return true;

        }

        let _ = CloseHandle(h);

        !wow64.as_bool()

    }

}



#[cfg(target_arch = "x86_64")]

const OWN_SPEEDPATCH: &str = "speedpatch64.dll";

#[cfg(target_arch = "x86")]

const OWN_SPEEDPATCH: &str = "speedpatch32.dll";



fn speedpatch_dll(is64: bool) -> &'static str {

    if is64 { "speedpatch64.dll" } else { "speedpatch32.dll" }

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



fn read_speedpatch_enabled(pid: u32) -> Option<bool> {

    let name = speedpatch_map_name(pid);

    unsafe {

        let h = OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR::from_raw(name.as_ptr())).ok()?;

        let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, std::mem::size_of::<bool>());

        if view.Value.is_null() {

            let _ = CloseHandle(h);

            return None;

        }

        let enabled = *(view.Value as *const bool);

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        Some(enabled)

    }

}



fn write_speedpatch_enabled(pid: u32, enabled: bool) -> Result<(), String> {

    let name = speedpatch_map_name(pid);

    unsafe {

        let h = OpenFileMappingW(FILE_MAP_WRITE.0, false, PCWSTR::from_raw(name.as_ptr()))

            .map_err(|e| format!("OpenFileMapping(DzsSpeedy.{pid}): {e:?}"))?;

        let view = MapViewOfFile(h, FILE_MAP_WRITE, 0, 0, std::mem::size_of::<bool>());

        if view.Value.is_null() {

            let _ = CloseHandle(h);

            return Err(format!("MapViewOfFile(DzsSpeedy.{pid}) failed"));

        }

        *(view.Value as *mut bool) = enabled;

        let _ = UnmapViewOfFile(view);

        let _ = CloseHandle(h);

        Ok(())

    }

}



struct EnumWindowsCtx {
    pid: u32,
    threads: Vec<u32>,
}

unsafe extern "system" fn enum_windows_for_pid(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut EnumWindowsCtx);
    let mut window_pid = 0u32;
    let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
    if window_pid == ctx.pid && thread_id != 0 {
        if IsWindowVisible(hwnd).as_bool() && !ctx.threads.contains(&thread_id) {
            ctx.threads.push(thread_id);
        }
    }
    true.into()
}

fn target_window_threads(pid: u32) -> Vec<u32> {
    let mut ctx = EnumWindowsCtx { pid, threads: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(enum_windows_for_pid), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.threads
}

fn try_windows_hook_x86(pid: u32, dll_path: &str) -> Result<(), String> {
    let threads = target_window_threads(pid);
    dbg_log(&format!("try_windows_hook_x86: pid={} window_threads={:?}", pid, threads));
    if threads.is_empty() {
        return Err(format!("no visible window thread found for pid={pid}"));
    }

    let dll_w = to_wide(dll_path);
    let hmod = unsafe { LoadLibraryW(PCWSTR::from_raw(dll_w.as_ptr())) }
        .map_err(|e| format!("LoadLibraryW(local hook dll): {e:?}"))?;
    let hook_proc = unsafe { GetProcAddress(hmod, s!("SP_HookProc")) }
        .ok_or("GetProcAddress SP_HookProc failed")?;

    let mut hooks: Vec<HHOOK> = Vec::new();
    for thread_id in threads {
        let hook = unsafe {
            SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(std::mem::transmute(hook_proc)),
                HINSTANCE(hmod.0),
                thread_id,
            )
        };
        match hook {
            Ok(h) => {
                dbg_log(&format!("try_windows_hook_x86: SetWindowsHookExW OK thread={} hook={:?}", thread_id, h));
                hooks.push(h);
                let _ = unsafe { PostThreadMessageW(thread_id, WM_NULL, None, None) };
            }
            Err(e) => {
                dbg_log(&format!("try_windows_hook_x86: SetWindowsHookExW FAILED thread={} err={:?}", thread_id, e));
            }
        }
    }

    if hooks.is_empty() {
        return Err("SetWindowsHookExW failed for all target window threads".into());
    }

    for _ in 0..40 {
        if read_speedpatch_enabled(pid).is_some() {
            retain_hooks(pid, hooks);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    for hook in hooks {
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
    Err(format!("SetWindowsHookExW installed but DzsSpeedy.{pid} mapping not created"))
}
// ── Core operations ──────────────────────────────────────────────────────



/// Try injecting via LoadLibraryW, fall back to LoadLibraryA.

fn do_inject(pid: u32) -> Result<(), String> {

    // DLL already loaded: LoadLibrary won't run DllMain again — must ENABLE mapping explicitly.

    match do_status(pid) {

        Some(true) => return Ok(()),

        Some(false) => return do_enable(pid),

        None => {}

    }

    let is64 = is_process_64bit(pid);

    let dll_path = exe_dir().join(speedpatch_dll(is64));

    let dll_str = dll_path.to_string_lossy().to_string();



    if !dll_path.is_file() {

        return Err(format!(

            "speedpatch DLL missing next to bridge: {} (bridge arch={}, target is64={})",

            dll_path.display(),

            OWN_SPEEDPATCH,

            is64

        ));

    }



    let mut detail = String::new();

    if !is64 {
        match try_windows_hook_x86(pid, &dll_str) {
            Ok(()) => {
                dbg_log(&format!("do_inject pid={}: SetWindowsHookEx + FileMapping OK", pid));
                return do_enable(pid);
            }
            Err(e) => {
                dbg_log(&format!("do_inject pid={}: SetWindowsHookEx path failed: {}", pid, e));
                detail.push_str(&format!("Hook:{e}; "));
            }
        }
    }

    let h_proc = unsafe {

        OpenProcess(

            PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION

            | PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_VM_READ,

            false, pid,

        )

    }.map_err(|e| {

        format!(

            "OpenProcess(pid={pid}) failed: {e:?}. If already admin, target may be protected or higher integrity."

        )

    })?;



    if !is64 {
        match try_ldr_load_dll_x86(pid, &h_proc, &dll_str) {
            Ok(()) => {
                wait_post_inject(pid);
                if do_status(pid).is_some() {
                    dbg_log(&format!("do_inject pid={}: LdrLoadDll + FileMapping OK", pid));
                    unsafe { let _ = CloseHandle(h_proc); }
                    return do_enable(pid);
                }
                dbg_log(&format!("do_inject pid={}: LdrLoadDll returned OK but FileMapping DzsSpeedy.{} not seen within 2s", pid, pid));
            }
            Err(e) => {
                dbg_log(&format!("do_inject pid={}: LdrLoadDll path failed: {}", pid, e));
                detail.push_str(&format!("Ldr:{e}; "));
            }
        }
    }
    let mut detail = String::new();

    if let Err(e) = try_inject_impl(pid, &h_proc, &dll_str, "LoadLibraryW", true) {

        detail.push_str(&format!("W:{e}; "));

    } else {

        wait_post_inject(pid);

        if do_status(pid).is_none() {

            dbg_log(&format!("do_inject pid={}: LoadLibraryW returned OK but FileMapping DzsSpeedy.{} not seen within 2s",
                      pid, pid));

            detail.push_str("W:DLL loaded but module mapping not initialized; ");

        } else {

            dbg_log(&format!("do_inject pid={}: LoadLibraryW + FileMapping OK", pid));

            unsafe { let _ = CloseHandle(h_proc); }

            return do_enable(pid);

        }

    }

    if let Err(e) = try_inject_impl(pid, &h_proc, &dll_str, "LoadLibraryA", false) {

        detail.push_str(&format!("A:{e}"));

    } else {

        wait_post_inject(pid);

        if do_status(pid).is_none() {

            dbg_log(&format!("do_inject pid={}: LoadLibraryA returned OK but FileMapping DzsSpeedy.{} not seen within 2s",
                      pid, pid));

            detail.push_str("A:DLL loaded but module mapping not initialized");

        } else {

            dbg_log(&format!("do_inject pid={}: LoadLibraryA + FileMapping OK", pid));

            unsafe { let _ = CloseHandle(h_proc); }

            return do_enable(pid);

        }

    }



    unsafe { let _ = CloseHandle(h_proc); }

    Err(format!(

        "remote LoadLibrary failed for pid={pid}, dll={dll_str}. {detail} \
         Common causes: game anti-cheat blocking unsigned DLL, or 32/64 bridge mismatch."

    ))

}



fn wait_post_inject(pid: u32) {

    for _ in 0..40 {

        if read_speedpatch_enabled(pid).is_some() {

            return;

        }

        std::thread::sleep(std::time::Duration::from_millis(50));

    }

}



fn remote_module_base(pid: u32, module_name: &str) -> Result<usize, String> {
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) }
        .map_err(|e| format!("CreateToolhelp32Snapshot({pid}): {e:?}"))?;

    let mut me = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
    let wanted = module_name.to_ascii_lowercase();
    let mut found = None;

    unsafe {
        if Module32FirstW(snap, &mut me).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&me.szModule)
                    .trim_end_matches('\0')
                    .to_ascii_lowercase();
                if name == wanted {
                    found = Some(me.modBaseAddr as usize);
                    break;
                }
                if Module32NextW(snap, &mut me).is_err() { break; }
            }
        }
        let _ = CloseHandle(snap);
    }

    found.ok_or_else(|| format!("module {module_name} not found in pid={pid}"))
}

fn remote_module_proc(pid: u32, module_name: &str, proc_name: &str) -> Result<(usize, usize, usize), String> {
    let module_w = to_wide(module_name);
    let local_module = unsafe { GetModuleHandleW(PCWSTR::from_raw(module_w.as_ptr())) }
        .map_err(|e| format!("GetModuleHandleW({module_name}): {e:?}"))?;
    let proc_cstr = std::ffi::CString::new(proc_name).unwrap();
    let local_proc = unsafe { GetProcAddress(local_module, PCSTR::from_raw(proc_cstr.as_ptr() as *const u8)) }
        .ok_or_else(|| format!("GetProcAddress {module_name}!{proc_name}"))? as usize;
    let local_base = local_module.0 as usize;
    let rva = local_proc
        .checked_sub(local_base)
        .ok_or_else(|| format!("{module_name}!{proc_name} address is below local module base"))?;
    let remote_base = remote_module_base(pid, module_name)?;
    Ok((remote_base + rva, local_proc, rva))
}

fn write_remote(h_proc: HANDLE, remote: usize, bytes: &[u8], label: &str) -> Result<(), String> {
    let ok = unsafe { WriteProcessMemory(h_proc, remote as *const _, bytes.as_ptr() as _, bytes.len(), None) };
    ok.map_err(|e| format!("WriteProcessMemory({label}, {} bytes): {e:?}", bytes.len()))
}

fn read_remote_u32(h_proc: HANDLE, remote: usize, label: &str) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    let mut read = 0usize;
    unsafe {
        ReadProcessMemory(h_proc, remote as *const _, buf.as_mut_ptr() as _, buf.len(), Some(&mut read))
    }.map_err(|e| format!("ReadProcessMemory({label}): {e:?}"))?;
    if read != 4 {
        return Err(format!("ReadProcessMemory({label}) read {read}/4"));
    }
    Ok(u32::from_le_bytes(buf))
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn try_ldr_load_dll_x86(pid: u32, h_proc: &HANDLE, dll_path: &str) -> Result<(), String> {
    let (ldr_load_dll, local_ldr_load_dll, ldr_rva) = remote_module_proc(pid, "ntdll.dll", "LdrLoadDll")?;
    dbg_log(&format!("try_ldr_load_dll_x86: pid={} LdrLoadDll local=0x{:x} rva=0x{:x} remote=0x{:x}",
              pid, local_ldr_load_dll, ldr_rva, ldr_load_dll));
    log_remote_proc_bytes(h_proc, ldr_load_dll, local_ldr_load_dll, "LdrLoadDll");

    if ldr_load_dll > u32::MAX as usize {
        return Err(format!("LdrLoadDll remote address 0x{ldr_load_dll:x} is not 32-bit"));
    }

    let path_wide = to_wide(dll_path);
    let path_bytes: Vec<u8> = path_wide.iter().flat_map(|c| c.to_le_bytes()).collect();
    let path_len = path_bytes.len();
    let unicode_off = (path_len + 3) & !3;
    let hmodule_off = unicode_off + 8;
    let status_off = hmodule_off + 4;
    let code_off = status_off + 4;
    let total_size = code_off + 64;

    let remote_base = unsafe {
        VirtualAllocEx(*h_proc, None, total_size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE)
    };
    if remote_base.is_null() {
        let gle = unsafe { GetLastError() };
        return Err(format!("VirtualAllocEx(LdrLoadDll block) gle={}", gle.0));
    }

    let base = remote_base as usize;
    let path_remote = base;
    let unicode_remote = base + unicode_off;
    let hmodule_remote = base + hmodule_off;
    let status_remote = base + status_off;
    let code_remote = base + code_off;

    let result = (|| -> Result<(), String> {
        write_remote(*h_proc, path_remote, &path_bytes, "ldr path")?;

        let mut unicode = Vec::with_capacity(8);
        let length = (path_len - 2) as u16;
        let maximum_length = path_len as u16;
        unicode.extend_from_slice(&length.to_le_bytes());
        unicode.extend_from_slice(&maximum_length.to_le_bytes());
        unicode.extend_from_slice(&(path_remote as u32).to_le_bytes());
        write_remote(*h_proc, unicode_remote, &unicode, "UNICODE_STRING32")?;
        write_remote(*h_proc, hmodule_remote, &[0, 0, 0, 0], "LdrLoadDll hmodule")?;
        write_remote(*h_proc, status_remote, &[0xcc, 0xcc, 0xcc, 0xcc], "LdrLoadDll status")?;

        let mut code = Vec::with_capacity(40);
        code.push(0x68);
        push_u32(&mut code, hmodule_remote as u32);
        code.push(0x68);
        push_u32(&mut code, unicode_remote as u32);
        code.extend_from_slice(&[0x6a, 0x00]);
        code.extend_from_slice(&[0x6a, 0x00]);
        code.push(0xb8);
        push_u32(&mut code, ldr_load_dll as u32);
        code.extend_from_slice(&[0xff, 0xd0]);
        code.push(0xa3);
        push_u32(&mut code, status_remote as u32);
        code.push(0xa1);
        push_u32(&mut code, hmodule_remote as u32);
        code.extend_from_slice(&[0xc2, 0x04, 0x00]);
        write_remote(*h_proc, code_remote, &code, "LdrLoadDll x86 stub")?;

        let h_thread = unsafe {
            CreateRemoteThread(*h_proc, None, 0,
                Some(std::mem::transmute(code_remote)), Some(remote_base), 0, None)
        }.map_err(|e| format!("CreateRemoteThread(LdrLoadDll): {e:?}"))?;

        unsafe { WaitForSingleObject(h_thread, 5000); }
        let mut exit_code = 0u32;
        let exit_ok = unsafe {
            use windows::Win32::System::Threading::GetExitCodeThread;
            GetExitCodeThread(h_thread, &mut exit_code)
        };
        unsafe { let _ = CloseHandle(h_thread); }
        exit_ok.map_err(|_| "GetExitCodeThread(LdrLoadDll) failed".to_string())?;

        let status = read_remote_u32(*h_proc, status_remote, "LdrLoadDll status")?;
        let hmodule = read_remote_u32(*h_proc, hmodule_remote, "LdrLoadDll hmodule")?;
        dbg_log(&format!("try_ldr_load_dll_x86: exit=0x{:08x} status=0x{:08x} hmodule=0x{:08x} dll={}",
                  exit_code, status, hmodule, dll_path));

        if status != 0 || hmodule == 0 {
            return Err(format!("LdrLoadDll failed status=0x{status:08x} hmodule=0x{hmodule:08x} exit=0x{exit_code:08x}"));
        }

        Ok(())
    })();

    unsafe { let _ = VirtualFreeEx(*h_proc, remote_base, 0, MEM_RELEASE); }
    result
}
fn remote_kernel32_proc(pid: u32, proc_name: &str) -> Result<(usize, usize, usize), String> {
    let kernel32_w = to_wide("kernel32.dll");
    let local_kernel32 = unsafe { GetModuleHandleW(PCWSTR::from_raw(kernel32_w.as_ptr())) }
        .map_err(|e| format!("GetModuleHandleW(kernel32.dll): {e:?}"))?;
    let proc_cstr = std::ffi::CString::new(proc_name).unwrap();
    let local_proc = unsafe { GetProcAddress(local_kernel32, PCSTR::from_raw(proc_cstr.as_ptr() as *const u8)) }
        .ok_or_else(|| format!("GetProcAddress {proc_name}"))? as usize;
    let local_base = local_kernel32.0 as usize;
    let rva = local_proc
        .checked_sub(local_base)
        .ok_or_else(|| format!("{proc_name} address is below local kernel32 base"))?;
    let remote_base = remote_module_base(pid, "kernel32.dll")?;
    Ok((remote_base + rva, local_proc, rva))
}
fn log_remote_proc_bytes(h_proc: &HANDLE, remote_proc: usize, local_proc: usize, fn_name: &str) {
    let mut remote = [0u8; 16];
    let mut read = 0usize;
    let read_ok = unsafe {
        ReadProcessMemory(
            *h_proc,
            remote_proc as *const _,
            remote.as_mut_ptr() as _,
            remote.len(),
            Some(&mut read),
        )
    };

    let local = unsafe { std::slice::from_raw_parts(local_proc as *const u8, 16) };
    let local_hex = local.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
    let remote_hex = remote.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
    dbg_log(&format!(
        "try_inject_impl: {} bytes local=[{}] remote_read={:?}/{} remote=[{}]",
        fn_name, local_hex, read_ok, read, remote_hex
    ));
}
/// Attempt injection with a specific LoadLibrary variant.

/// `wide` = true → use wide-char path (LoadLibraryW), false → ANSI (LoadLibraryA).

fn try_inject_impl(pid: u32, h_proc: &HANDLE, dll_path: &str, fn_name: &str, wide: bool) -> Result<(), String> {

    let path_bytes: Vec<u8> = if wide {

        dll_path.encode_utf16()

            .flat_map(|c| c.to_le_bytes())

            .chain([0u8, 0u8])

            .collect()

    } else {

        dll_path.as_bytes().iter()

            .copied()

            .chain([0u8])

            .collect()

    };

    let path_len = path_bytes.len();



    let remote_mem = unsafe {

        VirtualAllocEx(*h_proc, None, path_len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)

    };

    if remote_mem.is_null() {

        let gle = unsafe { GetLastError() };

        return Err(format!("VirtualAllocEx gle={}", gle.0));

    }



    unsafe {

        let _ = WriteProcessMemory(*h_proc, remote_mem, path_bytes.as_ptr() as _, path_len, None);

    }
    let (load_lib, local_load_lib, load_lib_rva) = remote_kernel32_proc(pid, fn_name)?;
    dbg_log(&format!("try_inject_impl: pid={} {} local=0x{:x} rva=0x{:x} remote=0x{:x}",
              pid, fn_name, local_load_lib, load_lib_rva, load_lib));
    log_remote_proc_bytes(h_proc, load_lib, local_load_lib, fn_name);
    let h_thread = unsafe {

        CreateRemoteThread(*h_proc, None, 0,

            Some(std::mem::transmute(load_lib)), Some(remote_mem), 0, None)

    }.map_err(|e| {

        unsafe { let _ = VirtualFreeEx(*h_proc, remote_mem, 0, MEM_RELEASE); }

        format!("CreateRemoteThread {e:?}")

    })?;



    unsafe { WaitForSingleObject(h_thread, 5000); }



    let mut exit_code = 0u32;

    let ok = unsafe {

        use windows::Win32::System::Threading::GetExitCodeThread;

        GetExitCodeThread(h_thread, &mut exit_code)

    };



    unsafe {

        let _ = VirtualFreeEx(*h_proc, remote_mem, 0, MEM_RELEASE);

        let _ = CloseHandle(h_thread);

    }



    if ok.is_err() {

        return Err("GetExitCodeThread failed".into());

    }

    // LoadLibrary 失败的退出码：
    //   0                  → LoadLibraryW/A 返回 NULL（路径错、找不到、签名拦截）
    //   259 (STILL_ACTIVE) → 线程 5s 内未结束（极少见）
    //   0xC0000142         → STATUS_DLL_INIT_FAILED：DllMain 返回 FALSE（最常见）
    //   0xC0000005         → STATUS_ACCESS_VIOLATION：DllMain 内部崩溃
    //   0xC000007B         → STATUS_INVALID_IMAGE_FORMAT：架构不匹配
    //   0xFFFFFFFF         → 线程异常（hmodule=INVALID_HANDLE_VALUE）
    //   其他 0xC0000xxx    → NTSTATUS 错误
    let is_failure = exit_code == 0
        || exit_code == 259
        || exit_code == 0xC0000142
        || exit_code == 0xC0000005
        || exit_code == 0xC000007B
        || exit_code == 0xFFFFFFFF
        || (exit_code >= 0xC0000000 && exit_code <= 0xC0000FFF);

    if is_failure {

        let gle = unsafe { GetLastError() };

        dbg_log(&format!("try_inject_impl: {} returned exit_code=0x{:08x} gle={} dll={}",
                  fn_name, exit_code, gle.0, dll_path));

        return Err(format!(

            "LoadLibrary failed or timed out (exit_code=0x{:08x}, gle={}). \
             0xC0000142 = DllMain returned FALSE (most common: MH init/anti-cheat/blocked). \
             0xC000007B = architecture mismatch. 0xC0000005 = crash in DllMain. \
             Path blocked or rejected by target process.",

            exit_code, gle.0

        ));

    }

    dbg_log(&format!("try_inject_impl: {} success (hmodule=0x{:x}) dll={}",
              fn_name, exit_code, dll_path));

    Ok(())

}



fn do_eject(pid: u32) -> Result<(), String> {

    let is64 = is_process_64bit(pid);

    let dll_name = speedpatch_dll(is64);

    let had_retained_hook = retained_hook_count_for_pid(pid) > 0;



    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) }

        .map_err(|e| format!("CreateToolhelp32Snapshot: {e:?}"))?;



    let mut me = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };

    let mut h_mod: Option<*mut std::ffi::c_void> = None;



    unsafe {

        if Module32FirstW(snap, &mut me).is_ok() {

            loop {

                let mod_name = String::from_utf16_lossy(&me.szModule)

                    .trim_end_matches('\0').to_lowercase();

                if mod_name == dll_name.to_lowercase() {

                    h_mod = Some(me.hModule.0 as _);

                    break;

                }

                if Module32NextW(snap, &mut me).is_err() { break; }

            }

        }

    }

    unsafe { let _ = CloseHandle(snap); }



    let h_mod_ptr = h_mod.ok_or("module not found in target")?;



    let h_proc = unsafe {

        OpenProcess(

            PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION,

            false, pid,

        )

    }.map_err(|e| format!("OpenProcess: {e:?}"))?;



    let local_dll = exe_dir().join(&dll_name);

    let local_dll_w = to_wide(&local_dll.to_string_lossy());

    let local_mod = unsafe { LoadLibraryW(PCWSTR::from_raw(local_dll_w.as_ptr())) }

        .map_err(|e| format!("LoadLibraryW(local {dll_name}): {e:?}"))?;

    let shutdown_proc = unsafe { GetProcAddress(local_mod, s!("SP_Shutdown")) }

        .ok_or("GetProcAddress SP_Shutdown failed")? as usize;

    let shutdown_rva = shutdown_proc

        .checked_sub(local_mod.0 as usize)

        .ok_or("SP_Shutdown address below module base")?;

    let remote_shutdown = h_mod_ptr as usize + shutdown_rva;



    let h_shutdown = unsafe {

        CreateRemoteThread(h_proc, None, 0,

            Some(std::mem::transmute(remote_shutdown)), None, 0, None)

    }.map_err(|e| format!("CreateRemoteThread(SP_Shutdown): {e:?}"))?;

    unsafe { WaitForSingleObject(h_shutdown, 5000); }

    unsafe { let _ = CloseHandle(h_shutdown); }



    if had_retained_hook {

        release_retained_hooks(pid);

        unsafe { let _ = CloseHandle(h_proc); }

        return Ok(());

    }



    let kernel32 = unsafe {

        GetModuleHandleW(PCWSTR::from_raw(to_wide("kernel32.dll").as_ptr()))

    }.map_err(|_| "GetModuleHandleW failed")?;

    let free_lib = unsafe { GetProcAddress(kernel32, s!("FreeLibrary")) }

        .ok_or("GetProcAddress FreeLibrary failed")?;



    let h_thread = unsafe {

        CreateRemoteThread(h_proc, None, 0,

            Some(std::mem::transmute(free_lib)), Some(h_mod_ptr), 0, None)

    }.map_err(|e| format!("CreateRemoteThread(FreeLibrary): {e:?}"))?;



    unsafe { WaitForSingleObject(h_thread, 5000); }

    unsafe { let _ = CloseHandle(h_thread); let _ = CloseHandle(h_proc); }

    Ok(())

}


fn do_enable(pid: u32) -> Result<(), String> {

    if read_speedpatch_enabled(pid) == Some(true) {

        return Ok(());

    }

    for attempt in 0..30 {

        match write_speedpatch_enabled(pid, true) {

            Ok(()) => return Ok(()),

            Err(e) if attempt + 1 < 30 => {

                std::thread::sleep(std::time::Duration::from_millis(50));

                let _ = e;

            }

            Err(e) => return Err(e),

        }

    }

    Err(format!("ENABLE {pid}: DzsSpeedy.{pid} mapping not found (inject first)"))

}



fn do_disable(pid: u32) -> Result<(), String> {

    if do_status(pid).is_none() {

        return Ok(());

    }

    for attempt in 0..30 {

        match write_speedpatch_enabled(pid, false) {

            Ok(()) => return Ok(()),

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

    read_speedpatch_enabled(pid).ok_or_else(|| format!("no DzsSpeedy.{pid} mapping"))

}



/// Check if speedpatch DLL is loaded in the target process, and if enabled.

/// Returns: Some(true) = enabled, Some(false) = injected+disabled, None = not injected.

fn do_status(pid: u32) -> Option<bool> {

    let dll_name = speedpatch_dll(is_process_64bit(pid));



    // Check if DLL is loaded in target process

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) }.ok()?;

    let mut me = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };

    let mut injected = false;



    unsafe {

        if Module32FirstW(snap, &mut me).is_ok() {

            loop {

                let mod_name = String::from_utf16_lossy(&me.szModule)

                    .trim_end_matches('\0').to_lowercase();

                if mod_name == dll_name.to_lowercase() {

                    injected = true;

                    break;

                }

                if Module32NextW(snap, &mut me).is_err() { break; }

            }

        }

    }

    unsafe { let _ = CloseHandle(snap); }



    if !injected {

        return None;

    }

    read_speedpatch_enabled(pid)

}



fn do_set_speed(factor: f64) {

    let _ = write_global_speed_factor(factor);

    let dll_wide = to_wide(OWN_SPEEDPATCH);

    unsafe {

        let Ok(h) = LoadLibraryW(PCWSTR::from_raw(dll_wide.as_ptr())) else { return };

        let set_speed: Option<unsafe extern "C" fn(f64)> =

            std::mem::transmute(GetProcAddress(h, s!("SP_SetSpeed")));

        if let Some(f) = set_speed { f(factor); }

    }

}



fn do_get_speed() -> f64 {

    if let Some(v) = read_global_speed_factor() {

        return v;

    }

    let dll_wide = to_wide(OWN_SPEEDPATCH);

    unsafe {

        let Ok(h) = LoadLibraryW(PCWSTR::from_raw(dll_wide.as_ptr())) else { return 1.0 };

        let get_speed: Option<unsafe extern "C" fn() -> f64> =

            std::mem::transmute(GetProcAddress(h, s!("SP_GetSpeed")));

        if let Some(f) = get_speed { f() } else { 1.0 }

    }

}



// ── Command dispatch ─────────────────────────────────────────────────────



fn handle_command(line: &str) -> String {

    let mut parts = line.trim().split_whitespace();
    let cmd = parts.next().unwrap_or("").to_uppercase();
    let raw = line.trim().to_string();
    dbg_log(&format!("cmd in: {raw}"));

    let resp = match cmd.as_str() {

        "INJECT" => {

            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_inject(pid) { Ok(()) => "OK".into(), Err(e) => format!("ERROR {e}") }

        }

        "EJECT" => {

            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_eject(pid) { Ok(()) => "OK".into(), Err(e) => format!("ERROR {e}") }

        }

        "ENABLE" => {

            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_enable(pid) { Ok(()) => "OK".into(), Err(e) => format!("ERROR {e}") }

        }

        "DISABLE" => {

            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_disable(pid) { Ok(()) => "OK".into(), Err(e) => format!("ERROR {e}") }

        }

        "ISENABLED" => {

            let pid: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

            match do_is_enabled(pid) {

                Ok(true) => "OK 1".into(), Ok(false) => "OK 0".into(),

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

                Some(true) => "OK ENABLED".into(),

                Some(false) => "OK DISABLED".into(),

                None => "OK NOT_INJECTED".into(),

            }

        }

        "PING" | "VERSION" => "OK bridge-filemap-v2".into(),

        "SHUTDOWN" => "OK shutting down".into(),

        _ => "ERROR unknown command".into(),

    };

    dbg_log(&format!("cmd out: {raw} -> {resp}"));

    resp

}



fn write_resp(h_pipe: HANDLE, msg: &str) {

    let mut written = 0u32;

    unsafe { let _ = WriteFile(h_pipe, Some(msg.as_bytes()), Some(&mut written), None); }

}



// ── Pipe server ───────────────────────────────────────────────────────────



#[cfg(target_arch = "x86_64")]

const PIPE_NAME: &str = r"\\.\pipe\DzsSpeedyBridge64";

#[cfg(target_arch = "x86")]

const PIPE_NAME: &str = r"\\.\pipe\DzsSpeedyBridge32";



fn pipe_server() {

    loop {

        let name_wide = to_wide(PIPE_NAME);

        let h_pipe = unsafe {

            CreateNamedPipeW(

                PCWSTR::from_raw(name_wide.as_ptr()),

                FILE_FLAGS_AND_ATTRIBUTES(3), // PIPE_ACCESS_DUPLEX

                NAMED_PIPE_MODE(PIPE_TYPE_MESSAGE.0 | PIPE_READMODE_MESSAGE.0 | PIPE_WAIT.0),

                255, 4096, 4096, 0, None,

            )

        };

        if h_pipe == INVALID_HANDLE_VALUE {

            std::thread::sleep(std::time::Duration::from_secs(1));

            continue;

        }



        let connected = unsafe { ConnectNamedPipe(h_pipe, None) };

        let connected = connected.is_ok() || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;

        if !connected {

            unsafe { let _ = CloseHandle(h_pipe); }

            continue;

        }



        let mut buf = [0u8; 4096];

        loop {

            let mut nread = 0u32;

            let ok = unsafe { ReadFile(h_pipe, Some(&mut buf), Some(&mut nread), None) };

            if ok.is_err() || nread == 0 { break; }



            let text = String::from_utf8_lossy(&buf[..nread as usize]);

            for line in text.lines() {

                let line = line.trim();

                if line.is_empty() { continue; }

                if line.eq_ignore_ascii_case("SHUTDOWN") {

                    if retained_hook_count() > 0 {

                        write_resp(h_pipe, "OK hooks active; bridge staying alive\n");

                        continue;

                    }

                    write_resp(h_pipe, "OK shutting down\n");

                    unsafe { let _ = CloseHandle(h_pipe); }

                    std::process::exit(0);

                }
                let resp = handle_command(line);

                write_resp(h_pipe, &format!("{resp}\n"));

            }

        }

        unsafe { let _ = CloseHandle(h_pipe); }

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

        let mut mode = NAMED_PIPE_MODE(PIPE_READMODE_MESSAGE.0);

        let _ = SetNamedPipeHandleState(h, Some(&mut mode), None, None);



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
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    // 简易 ISO-ish 时间戳（避免拉 chrono 依赖）
    format!("{}.{:03}", secs, ms)
}

fn main() {

    let args: Vec<String> = std::env::args().collect();

    dbg_log(&format!("main: bridge launched, exe={} args={:?}",
              std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
              args));

    if args.len() > 1 {

        let line = args[1..].join(" ");

        println!("{}", handle_command(&line));

        return;

    }

    if !acquire_bridge_singleton() {

        if existing_bridge_pipe_alive() {

            std::process::exit(0);

        }

        std::process::exit(2);

    }

    pipe_server();

}

