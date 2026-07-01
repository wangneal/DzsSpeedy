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

use std::fs::OpenOptions;
use std::io::Write;

use windows::core::{PCSTR, PCWSTR, PWSTR, s};

use windows::Win32::Foundation::{

    CloseHandle, GetLastError, HANDLE, BOOL, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED,

    INVALID_HANDLE_VALUE,

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

    PAGE_READWRITE,

};

use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;

use windows::Win32::System::Diagnostics::ToolHelp::{

    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W,

    TH32CS_SNAPMODULE,

};

use windows::Win32::System::LibraryLoader::{

    GetModuleHandleW, GetProcAddress, LoadLibraryW,

};

use windows::Win32::System::Pipes::{

    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, PIPE_READMODE_MESSAGE,

    PIPE_TYPE_MESSAGE, PIPE_WAIT, NAMED_PIPE_MODE,

};



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



    let mut detail = String::new();

    if let Err(e) = try_inject_impl(&h_proc, &dll_str, "LoadLibraryW", true) {

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

    if let Err(e) = try_inject_impl(&h_proc, &dll_str, "LoadLibraryA", false) {

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



/// Attempt injection with a specific LoadLibrary variant.

/// `wide` = true → use wide-char path (LoadLibraryW), false → ANSI (LoadLibraryA).

fn try_inject_impl(h_proc: &HANDLE, dll_path: &str, fn_name: &str, wide: bool) -> Result<(), String> {

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



    let kernel32 = unsafe {

        GetModuleHandleW(PCWSTR::from_raw(to_wide("kernel32.dll").as_ptr()))

    }.map_err(|e| format!("GetModuleHandleW {e:?}"))?;

    let fn_cstr = std::ffi::CString::new(fn_name).unwrap();

    let load_lib = unsafe {

        GetProcAddress(kernel32, PCSTR::from_raw(fn_cstr.as_ptr() as *const u8))

    }.ok_or_else(|| format!("GetProcAddress {fn_name}"))?;



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



    let kernel32 = unsafe {

        GetModuleHandleW(PCWSTR::from_raw(to_wide("kernel32.dll").as_ptr()))

    }.map_err(|_| "GetModuleHandleW failed")?;

    let free_lib = unsafe { GetProcAddress(kernel32, s!("FreeLibrary")) }

        .ok_or("GetProcAddress FreeLibrary failed")?;



    let h_thread = unsafe {

        CreateRemoteThread(h_proc, None, 0,

            Some(std::mem::transmute(free_lib)), Some(h_mod_ptr), 0, None)

    }.map_err(|e| format!("CreateRemoteThread: {e:?}"))?;



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

