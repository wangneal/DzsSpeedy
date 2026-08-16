mod bridge_client;
mod process_enumerator;
mod system_stats;

use process_enumerator::ModuleInfo;
use process_enumerator::ProcessInfo;
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// (bridge exe name, child handle) — the name lets the repair path respawn
/// exactly the arch whose bridge died instead of assuming "any child alive
/// means all bridges are alive".
static BRIDGE_CHILDREN: Mutex<Vec<(String, Child)>> = Mutex::new(Vec::new());

const BRIDGE_NAMES: [&str; 2] = ["bridge64.exe", "bridge32.exe"];

fn bridge_name_for_arch(arch: &str) -> &'static str {
    if arch == "x86" {
        "bridge32.exe"
    } else {
        "bridge64.exe"
    }
}

fn child_is_live(child: &mut Child) -> bool {
    child.try_wait().ok().flatten().is_none()
}

fn prune_exited_children(children: &mut Vec<(String, Child)>) {
    let mut i = 0;
    while i < children.len() {
        if !child_is_live(&mut children[i].1) {
            children.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

fn live_child_for(children: &mut [(String, Child)], name: &str) -> bool {
    children
        .iter_mut()
        .any(|(child_name, child)| child_name == name && child_is_live(child))
}

fn spawn_bridge(name: &str, children: &mut Vec<(String, Child)>) {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    let path = exe_dir.join(name);
    if path.exists() {
        match std::process::Command::new(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => children.push((name.to_string(), child)),
            Err(_) => {}
        }
    }
}

fn ensure_bridges() {
    let mut children = BRIDGE_CHILDREN.lock().unwrap();
    prune_exited_children(&mut children);
    for name in BRIDGE_NAMES {
        if !live_child_for(&mut children, name) {
            spawn_bridge(name, &mut children);
        }
    }
}

/// Repair one bridge arch after a shutdown/pipe failure:
/// 1. wait (bounded) while a stale dying bridge still owns the pipe,
/// 2. respawn the arch's bridge when no live child covers it,
/// 3. wait briefly for the fresh (or takeover) child to serve the pipe so a
///    retried operation lands on a live bridge.
/// The bridge itself now exits promptly on shutdown and a fresh bridge child
/// waits for the stale owner to leave, so this converges in seconds.
fn repair_bridge(arch: &str) {
    let name = bridge_name_for_arch(arch);
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if bridge_client::bridge_health(arch) {
            return; // someone healthy serves this pipe again
        }
        if !bridge_client::bridge_pipe_present_arch(arch) {
            break; // stale owner gone
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    {
        let mut children = BRIDGE_CHILDREN.lock().unwrap();
        prune_exited_children(&mut children);
        if !live_child_for(&mut children, name) {
            spawn_bridge(name, &mut children);
        }
    }
    // A newly spawned child takes a moment to acquire the singleton; a child
    // that was already waiting takes over as soon as the stale owner leaves.
    // Give either one a short settle window before the retry.
    let settle_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < settle_deadline {
        if bridge_client::bridge_health(arch) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Run a bridge operation; on a shutdown/transport failure, repair the bridge
/// (waiting out any stale dying owner) and retry the operation once.
fn bridge_operation(arch: &str, run: impl Fn() -> Result<(), String>) -> Result<(), String> {
    match run() {
        Ok(()) => Ok(()),
        Err(error) if needs_bridge_repair(&error) => {
            bridge_client::frontend_log(&format!("[bridge] {arch} repair triggered: {error}"));
            repair_bridge(arch);
            run()
        }
        Err(error) => Err(error),
    }
}

fn needs_bridge_repair(error: &str) -> bool {
    bridge_client::bridge_shutdown_detected(error) || error.contains("open named pipe")
}

fn shutdown_bridges() {
    // Wake the bridge shutdown threads even if their pipe servers are blocked.
    bridge_client::bridge64_shutdown();
    bridge_client::bridge32_shutdown();

    if let Ok(mut children) = BRIDGE_CHILDREN.lock() {
        // Bridges exit within a bounded grace period (pending-injection
        // monitors give up ~2s after the shutdown event; the drain cap is 8s).
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let mut all_exited = true;
            for (_, child) in children.iter_mut() {
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

        // Do not force-kill a bridge while a Windows hook callback is
        // initializing speedpatch. The bridge has its own bounded shutdown
        // grace period and disables the target when initialization ends.
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
        "x86" => bridge_operation("x86", || bridge_client::bridge32_inject(pid)),
        "x64" => bridge_operation("x64", || bridge_client::bridge64_inject(pid)),
        _ => Err(format!("unsupported target architecture: {arch}")),
    }
}

#[tauri::command(async)]
async fn bridge_enable(pid: u32, arch: String) -> Result<(), String> {
    match arch.as_str() {
        "x86" => bridge_operation("x86", || bridge_client::bridge32_enable(pid)),
        "x64" => bridge_operation("x64", || bridge_client::bridge64_enable(pid)),
        _ => Err(format!("unsupported target architecture: {arch}")),
    }
}

#[tauri::command(async)]
async fn bridge_disable(pid: u32, arch: String) -> Result<(), String> {
    match arch.as_str() {
        "x86" => bridge_operation("x86", || bridge_client::bridge32_disable(pid)),
        "x64" => bridge_operation("x64", || bridge_client::bridge64_disable(pid)),
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
