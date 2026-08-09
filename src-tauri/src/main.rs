// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, CloseHandle};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
use windows::core::PCWSTR;

fn is_elevated() -> bool {
    unsafe {
        let mut token = Default::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned_length = 0u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut std::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        );

        let _ = CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    }
}

fn show_elevation_required_message() {
    let text: Vec<u16> = "DzsSpeedy 必须以管理员身份运行。请在 UAC 提示中允许权限后重试。\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "需要管理员权限\0".encode_utf16().collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR::from_raw(text.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn main() {
    // The embedded manifest requests elevation. Keep this runtime gate as a fail-closed
    // check for unusual launchers or binaries built without the expected manifest.
    if !is_elevated() {
        show_elevation_required_message();
        return;
    }

    // Single instance guard
    let name: Vec<u16> = "DzsSpeedy_SingleInstance\0".encode_utf16().collect();
    unsafe {
        if let Ok(h) = CreateMutexW(None, true, PCWSTR::from_raw(name.as_ptr())) {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(h);
                return;
            }
        }
    }

    dzsspeedy_lib::run()
}
