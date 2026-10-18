//! Windows で、pid のプロセスが生きているか。
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::win_security::Handle;

/// `GetExitCodeProcess` が、まだ終わっていないプロセスに返す値。
const STILL_ACTIVE: u32 = 259;

/// 開けない（権限が足りない）プロセスは「存在するが権限がない」なので生存扱い。
pub fn is_alive(pid: i32) -> bool {
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    // SAFETY: プロセスの状態を問い合わせる権限だけで開く。ハンドルは `Handle` が閉じる。
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        // SAFETY: 直前の Win32 呼び出しのエラーを読むだけ。
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let process = Handle(process);
    let mut code = 0u32;
    // SAFETY: `process` は有効なハンドルで、`code` は書き込める。
    let ok = unsafe { GetExitCodeProcess(process.0, &mut code) };
    ok == 0 || code == STILL_ACTIVE
}
