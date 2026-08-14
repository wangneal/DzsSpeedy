mod bridge_client;
mod process_enumerator;
mod system_stats;

use process_enumerator::ModuleInfo;
use process_enumerator::ProcessInfo;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static BRIDGE_CHILDREN: Mutex<Vec<Child>> = Mutex::new(Vec::new());

fn ensure_bridges() {
    let mut children = BRIDGE_CHILDREN.lock().unwrap();
    if !children.is_empty() {
        return;
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();

    for name in &["bridge64.exe", "bridge32.exe"] {
        let path = exe_dir.join(name);
        if path.exists() {
            match std::process::Command::new(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(child) => children.push(child),
                Err(_) => {}
            }
        }
    }
}

fn shutdown_bridges() {
    // Wake the bridge shutdown threads even if their pipe servers are blocked.
    bridge_client::bridge64_shutdown();
    bridge_client::bridge32_shutdown();

    if let Ok(mut children) = BRIDGE_CHILDREN.lock() {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut all_exited = true;
            for child in children.iter_mut() {
                match child.try_wait() {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => all_exited = false,
                }
            }
            if all_exited || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        // Do not force-kill a bridge that is draining a remote LoadLibraryW or
        // SP_Initialize thread. The bridge has its own bounded shutdown grace
        // period and will disable a target again when a pending injection ends.
        children.clear();
    }
}
#[tauri::command(async)]
async fn get_process_list_fast() -> Vec<ProcessInfo> {
    process_enumerator::enumerate_processes_fast()
}

#[tauri::command(async)]
async fn get_process_list() -> Vec<ProcessInfo> {
    process_enumerator::enumerate_processes_full()
}

#[tauri::command(async)]
async fn get_process_icon(pid: u32) -> Option<String> {
    process_enumerator::get_process_icon(pid)
}

#[tauri::command(async)]
async fn get_process_modules(pid: u32) -> Vec<ModuleInfo> {
    process_enumerator::enumerate_modules(pid)
}

#[tauri::command(async)]
async fn bridge64_health() -> bool {
    bridge_client::bridge64_health()
}

#[tauri::command(async)]
async fn bridge32_health() -> bool {
    bridge_client::bridge32_health()
}

#[tauri::command(async)]
async fn bridge_set_speed(factor: f64) -> bool {
    let a = bridge_client::bridge64_set_speed(factor);
    let b = bridge_client::bridge32_set_speed(factor);
    a || b
}

#[tauri::command(async)]
async fn bridge_get_speed() -> Option<f64> {
    bridge_client::bridge64_get_speed()
}

#[tauri::command(async)]
async fn get_system_stats() -> system_stats::SystemStats {
    system_stats::get_system_stats()
}

#[tauri::command(async)]
async fn bridge_inject(pid: u32, arch: String) -> Result<(), String> {
    match arch.as_str() {
        "x86" => bridge_client::bridge32_inject(pid),
        "x64" => bridge_client::bridge64_inject(pid),
        _ => Err(format!("unsupported target architecture: {arch}")),
    }
}

#[tauri::command(async)]
async fn bridge_enable(pid: u32, arch: String) -> Result<(), String> {
    match arch.as_str() {
        "x86" => bridge_client::bridge32_enable(pid),
        "x64" => bridge_client::bridge64_enable(pid),
        _ => Err(format!("unsupported target architecture: {arch}")),
    }
}

#[tauri::command(async)]
async fn bridge_disable(pid: u32, arch: String) -> Result<(), String> {
    match arch.as_str() {
        "x86" => bridge_client::bridge32_disable(pid),
        "x64" => bridge_client::bridge64_disable(pid),
        _ => Err(format!("unsupported target architecture: {arch}")),
    }
}

#[tauri::command(async)]
async fn bridge_get_status(pid: u32, arch: String) -> Result<bridge_client::BridgeStatus, String> {
    match arch.as_str() {
        "x86" => bridge_client::bridge32_get_status(pid),
        "x64" => bridge_client::bridge64_get_status(pid),
        _ => Err(format!("unsupported target architecture: {arch}")),
    }
}

#[tauri::command(async)]
async fn set_always_on_top(window: tauri::Window, on_top: bool) {
    let _ = window.set_always_on_top(on_top);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_autostart::Builder::default().build())
        .plugin(tauri_plugin_shell::init())
        .setup(|_app| {
            ensure_bridges();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_process_list,
            get_process_list_fast,
            get_process_icon,
            get_process_modules,
            bridge64_health,
            bridge32_health,
            bridge_set_speed,
            bridge_get_speed,
            get_system_stats,
            bridge_inject,
            bridge_enable,
            bridge_disable,
            bridge_get_status,
            set_always_on_top,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                shutdown_bridges();
            }
        });
}
