//! Named-pipe client for communicating with bridge64.exe / bridge32.exe.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use windows::Win32::System::Pipes::{
    SetNamedPipeHandleState, NAMED_PIPE_MODE, PIPE_READMODE_MESSAGE,
};
use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};

const PIPE_64: &str = r"\\.\pipe\DzsSpeedyBridge64";
const PIPE_32: &str = r"\\.\pipe\DzsSpeedyBridge32";
const SHUTDOWN_EVENT_64: &str = r"Global\DzsSpeedyBridge64Shutdown";
const SHUTDOWN_EVENT_32: &str = r"Global\DzsSpeedyBridge32Shutdown";
const OUTCOME_UNKNOWN_MARKER: &str = "BRIDGE_OUTCOME_UNKNOWN:";

static PIPE_64_LOCK: Mutex<()> = Mutex::new(());
static PIPE_32_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum BridgeStatus {
    Enabled,
    Disabled,
    Initializing,
    Failed(String),
    NotInjected,
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn open_pipe(name: &str) -> Result<HANDLE, String> {
    let pipe_name = name;
    let name = to_wide(pipe_name);
    let mut last_error = String::from("unknown");

    for _ in 0..40 {
        let h = unsafe {
            CreateFileW(
                PCWSTR::from_raw(name.as_ptr()),
                0xC0000000 | 0x40000000, // GENERIC_READ | GENERIC_WRITE
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            )
        };
        match h {
            Ok(h) if h != INVALID_HANDLE_VALUE => {
                let mut mode = NAMED_PIPE_MODE(PIPE_READMODE_MESSAGE.0);
                let _ = unsafe { SetNamedPipeHandleState(h, Some(&mut mode), None, None) };
                return Ok(h);
            }
            Ok(_) => {
                last_error = "CreateFileW returned INVALID_HANDLE_VALUE".into();
            }
            Err(e) => {
                last_error = format!("{e:?}");
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    Err(format!(
        "open named pipe {pipe_name} failed after 40 attempts: {last_error}"
    ))
}

fn pipe_lock(pipe: &str) -> &'static Mutex<()> {
    if pipe == PIPE_64 {
        &PIPE_64_LOCK
    } else {
        &PIPE_32_LOCK
    }
}

fn outcome_unknown(cmd: &str, detail: &str) -> String {
    format!("{OUTCOME_UNKNOWN_MARKER} {cmd}: {detail}")
}

fn pipe_command(pipe: &str, cmd: &str) -> Result<String, String> {
    let _guard = pipe_lock(pipe)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let h = match open_pipe(pipe) {
        Ok(h) => h,
        Err(error) => {
            let line = format!("[bridge] {cmd} -> {error}");
            eprintln!("{line}");
            frontend_log(&line);
            return Err(error);
        }
    };

    let msg = format!("{cmd}\n");
    let mut written = 0u32;
    let write_result = unsafe { WriteFile(h, Some(msg.as_bytes()), Some(&mut written), None) };
    if write_result.is_err() || written != msg.len() as u32 {
        let detail = format!(
            "pipe write failed for {cmd} (err={:?}, nwritten={written})",
            write_result.err()
        );
        unsafe {
            let _ = CloseHandle(h);
        }
        let error = outcome_unknown(cmd, &detail);
        let line = format!("[bridge] {cmd} -> {error}");
        eprintln!("{line}");
        frontend_log(&line);
        return Err(error);
    }

    let mut buf = [0u8; 4096];
    let mut nread = 0u32;
    let read_result = unsafe { ReadFile(h, Some(&mut buf), Some(&mut nread), None) };
    unsafe {
        let _ = CloseHandle(h);
    }

    if let Err(error) = read_result {
        let detail = format!("pipe read failed for {cmd} (err={error:?}, nread={nread})");
        let error = outcome_unknown(cmd, &detail);
        let line = format!("[bridge] {cmd} -> {error}");
        eprintln!("{line}");
        frontend_log(&line);
        return Err(error);
    }
    if nread == 0 {
        let detail = format!("pipe read returned no data for {cmd}");
        let error = outcome_unknown(cmd, &detail);
        let line = format!("[bridge] {cmd} -> {error}");
        eprintln!("{line}");
        frontend_log(&line);
        return Err(error);
    }

    let resp = String::from_utf8_lossy(&buf[..nread as usize])
        .trim()
        .to_string();
    let line = format!("[bridge] {cmd} -> {resp}");
    eprintln!("{line}");
    frontend_log(&line);
    Ok(resp)
}

fn bridge_response_result(cmd: &str, response: &str) -> Result<(), String> {
    if response == "OK" {
        return Ok(());
    }

    if let Some(detail) = response.strip_prefix("ERROR ") {
        return Err(format!("{cmd}: {detail}"));
    }

    Err(format!("{cmd}: unexpected bridge response: {response}"))
}

fn parse_status_response(cmd: &str, response: &str) -> Result<BridgeStatus, String> {
    match response {
        "OK ENABLED" => Ok(BridgeStatus::Enabled),
        "OK DISABLED" => Ok(BridgeStatus::Disabled),
        "OK INITIALIZING" => Ok(BridgeStatus::Initializing),
        _ if response.starts_with("OK FAILED ") => {
            Ok(BridgeStatus::Failed(response[10..].trim().to_string()))
        }
        "OK NOT_INJECTED" => Ok(BridgeStatus::NotInjected),
        _ if response.starts_with("ERROR ") => Err(format!("{cmd}: {}", &response[6..])),
        _ => Err(format!("{cmd}: unexpected bridge response: {response}")),
    }
}

fn expect_ok(pipe: &str, cmd: &str) -> Result<(), String> {
    let response = pipe_command(pipe, cmd)?;
    bridge_response_result(cmd, &response)
}

#[cfg(test)]
mod tests {
    use super::{bridge_response_result, outcome_unknown, parse_status_response, BridgeStatus};

    #[test]
    fn accepts_ok_response() {
        assert_eq!(bridge_response_result("INJECT 42", "OK"), Ok(()));
    }

    #[test]
    fn preserves_bridge_error_response() {
        assert_eq!(
            bridge_response_result("INJECT 42", "ERROR remote LoadLibrary failed"),
            Err("INJECT 42: remote LoadLibrary failed".to_string())
        );
    }

    #[test]
    fn reports_unexpected_response() {
        assert_eq!(
            bridge_response_result("ENABLE 42", "BROKEN"),
            Err("ENABLE 42: unexpected bridge response: BROKEN".to_string())
        );
    }

    #[test]
    fn parses_each_status_without_conflating_not_injected_and_errors() {
        assert_eq!(
            parse_status_response("STATUS 42", "OK ENABLED"),
            Ok(BridgeStatus::Enabled)
        );
        assert_eq!(
            parse_status_response("STATUS 42", "OK DISABLED"),
            Ok(BridgeStatus::Disabled)
        );
        assert_eq!(
            parse_status_response("STATUS 42", "OK INITIALIZING"),
            Ok(BridgeStatus::Initializing)
        );
        assert_eq!(
            parse_status_response("STATUS 42", "OK FAILED MH_EnableHook failed"),
            Ok(BridgeStatus::Failed("MH_EnableHook failed".to_string()))
        );
        assert_eq!(
            parse_status_response("STATUS 42", "OK NOT_INJECTED"),
            Ok(BridgeStatus::NotInjected)
        );
        assert_eq!(
            parse_status_response("STATUS 42", "ERROR access denied"),
            Err("STATUS 42: access denied".to_string())
        );
    }

    #[test]
    fn serializes_status_as_a_stable_tagged_frontend_contract() {
        assert_eq!(
            serde_json::to_value(BridgeStatus::Initializing).unwrap(),
            serde_json::json!({ "state": "initializing" })
        );
        assert_eq!(
            serde_json::to_value(BridgeStatus::Failed("MH_EnableHook failed".into())).unwrap(),
            serde_json::json!({
                "state": "failed",
                "detail": "MH_EnableHook failed"
            })
        );
    }

    #[test]
    fn marks_post_send_transport_failures_as_outcome_unknown() {
        assert_eq!(
            outcome_unknown("INJECT 42", "pipe ended"),
            "BRIDGE_OUTCOME_UNKNOWN: INJECT 42: pipe ended"
        );
    }
}

/// 追加写诊断日志到 %TEMP%\dzsspeedy-frontend.log
/// 让 release 模式也能取证。
fn frontend_log(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("dzsspeedy-frontend.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let _ = writeln!(
            f,
            "[{}.{:03}] [pid={}] {}",
            now.as_secs(),
            now.subsec_millis(),
            std::process::id(),
            msg
        );
        let _ = f.flush();
    }
}

/// Check if bridge64 is running and responsive.
pub fn bridge64_health() -> bool {
    pipe_command(PIPE_64, "GETSPEED")
        .map(|r| r.starts_with("OK"))
        .unwrap_or(false)
}

/// Check if bridge32 is running and responsive.
pub fn bridge32_health() -> bool {
    pipe_command(PIPE_32, "GETSPEED")
        .map(|r| r.starts_with("OK"))
        .unwrap_or(false)
}

/// Set speed factor via bridge64.
pub fn bridge64_set_speed(factor: f64) -> bool {
    pipe_command(PIPE_64, &format!("SETSPEED {factor}"))
        .map(|r| r.starts_with("OK"))
        .unwrap_or(false)
}

/// Set speed factor via bridge32.
pub fn bridge32_set_speed(factor: f64) -> bool {
    pipe_command(PIPE_32, &format!("SETSPEED {factor}"))
        .map(|r| r.starts_with("OK"))
        .unwrap_or(false)
}

/// Get speed factor from bridge64.
pub fn bridge64_get_speed() -> Option<f64> {
    pipe_command(PIPE_64, "GETSPEED")
        .ok()
        .and_then(|r| r.strip_prefix("OK ").and_then(|s| s.parse().ok()))
}

fn signal_shutdown_event(name: &str) -> Result<(), String> {
    let name_wide = to_wide(name);
    let event = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            PCWSTR::from_raw(name_wide.as_ptr()),
        )
    }
    .map_err(|error| format!("OpenEventW({name}) failed: {error:?}"))?;

    let result =
        unsafe { SetEvent(event) }.map_err(|error| format!("SetEvent({name}) failed: {error:?}"));
    unsafe {
        let _ = CloseHandle(event);
    }
    result
}

/// Signal bridge64's dedicated shutdown watcher.
pub fn bridge64_shutdown() {
    if let Err(error) = signal_shutdown_event(SHUTDOWN_EVENT_64) {
        frontend_log(&format!("[bridge] shutdown64 -> {error}"));
    }
}

/// Signal bridge32's dedicated shutdown watcher.
pub fn bridge32_shutdown() {
    if let Err(error) = signal_shutdown_event(SHUTDOWN_EVENT_32) {
        frontend_log(&format!("[bridge] shutdown32 -> {error}"));
    }
}

// ── Per-arch inject / eject / enable / disable ──

pub fn bridge64_inject(pid: u32) -> Result<(), String> {
    expect_ok(PIPE_64, &format!("INJECT {pid}"))
}
pub fn bridge32_inject(pid: u32) -> Result<(), String> {
    expect_ok(PIPE_32, &format!("INJECT {pid}"))
}

#[allow(dead_code)]
pub fn bridge64_eject(pid: u32) -> bool {
    pipe_command(PIPE_64, &format!("EJECT {pid}"))
        .map(|r| r == "OK")
        .unwrap_or(false)
}
#[allow(dead_code)]
pub fn bridge32_eject(pid: u32) -> bool {
    pipe_command(PIPE_32, &format!("EJECT {pid}"))
        .map(|r| r == "OK")
        .unwrap_or(false)
}

pub fn bridge64_enable(pid: u32) -> Result<(), String> {
    expect_ok(PIPE_64, &format!("ENABLE {pid}"))
}
pub fn bridge32_enable(pid: u32) -> Result<(), String> {
    expect_ok(PIPE_32, &format!("ENABLE {pid}"))
}

pub fn bridge64_disable(pid: u32) -> Result<(), String> {
    expect_ok(PIPE_64, &format!("DISABLE {pid}"))
}
pub fn bridge32_disable(pid: u32) -> Result<(), String> {
    expect_ok(PIPE_32, &format!("DISABLE {pid}"))
}

/// Query per-PID status from bridge without conflating absence and transport errors.
pub fn bridge64_get_status(pid: u32) -> Result<BridgeStatus, String> {
    let cmd = format!("STATUS {pid}");
    let response = pipe_command(PIPE_64, &cmd)?;
    parse_status_response(&cmd, &response)
}
pub fn bridge32_get_status(pid: u32) -> Result<BridgeStatus, String> {
    let cmd = format!("STATUS {pid}");
    let response = pipe_command(PIPE_32, &cmd)?;
    parse_status_response(&cmd, &response)
}
