//! Windows の停止の要求。Windows にはシグナルが無く、プロトコルに停止のメソッドを足さない（0.3.0 のまま）ので、
//! 起動したユーザーだけが開ける名前付きイベントで、`misao stop` からデーモンへ伝える。
//! 名前は接続口の名前から決まるので、`--socket` を変えたデーモンにも届く。
#![allow(unsafe_code)]

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, OpenProcess, SetEvent, WaitForSingleObject,
};

use crate::win_security::{Handle, UserOnlyAcl};

/// 停止の要求を待つ間隔（これより細かくは終了を確かめない）。
const POLL: Duration = Duration::from_millis(200);

/// 接続口の名前から決まるイベント名。別のセッション（SSH など）からも見えるよう `Global\` に置く。
fn event_name(endpoint: &str) -> Vec<u16> {
    let digest = Sha256::digest(endpoint.to_lowercase().as_bytes());
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("Global\\misao-stop-{hash}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// デーモンが持つ停止イベント。`misao stop` が立てると `stopped` が完了する。
pub struct StopEvent {
    notify: Arc<Notify>,
    closed: Arc<AtomicBool>,
}

impl StopEvent {
    /// 起動したユーザーだけが立てられるイベントを作り、待つスレッドを始める。
    pub fn create(endpoint: &str) -> io::Result<Self> {
        let acl = UserOnlyAcl::new()?;
        let name = event_name(endpoint);
        // SAFETY: `acl.attributes()` は有効な SECURITY_ATTRIBUTES、`name` は NUL 終端の UTF-16。手動リセットの、立っていないイベント。
        let handle = unsafe { CreateEventW(acl.attributes().cast(), 1, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let event = Handle(handle);
        // SAFETY: 直前の Win32 呼び出しのエラーを読むだけ。同じ名前が既にあれば、別のデーモンのイベントなので使わない。
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Err(io::Error::other("the stop event already exists"));
        }
        let notify = Arc::new(Notify::new());
        let closed = Arc::new(AtomicBool::new(false));
        let (thread_notify, thread_closed) = (Arc::clone(&notify), Arc::clone(&closed));
        std::thread::spawn(move || {
            let event = event; // フィールドだけでなく Handle ごと（Send）スレッドへ移す
            while !thread_closed.load(Ordering::Acquire) {
                // SAFETY: `event` は有効なハンドルで、このスレッドが生きている間は閉じない。
                let status =
                    unsafe { WaitForSingleObject(event.0, u32::try_from(POLL.as_millis()).unwrap_or(200)) };
                if status == WAIT_OBJECT_0 {
                    thread_notify.notify_one();
                    return;
                }
                if status != WAIT_TIMEOUT {
                    return;
                }
            }
        });
        Ok(Self { notify, closed })
    }

    /// 停止が要求されるまで待つ。
    pub async fn stopped(&self) {
        self.notify.notified().await;
    }
}

impl Drop for StopEvent {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

/// 接続口 `endpoint` のデーモンに停止を要求する。デーモンのイベントが無ければ false。
pub fn request_stop(endpoint: &str) -> io::Result<bool> {
    let name = event_name(endpoint);
    // SAFETY: `name` は NUL 終端の UTF-16。イベントを立てる権限だけで開く。
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if handle.is_null() {
        // SAFETY: 直前の Win32 呼び出しのエラーを読むだけ。
        return match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => Ok(false),
            _ => Err(io::Error::last_os_error()),
        };
    }
    let event = Handle(handle);
    // SAFETY: `event` は有効なハンドル。
    if unsafe { SetEvent(event.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(true)
}

/// プロセス `pid` が終わるのを `timeout` まで待つ。終わった（または既に無い）なら true。
pub fn wait_process_exit(pid: u32, timeout: Duration) -> io::Result<bool> {
    // SAFETY: 終了を待つ権限だけで開く。
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        // SAFETY: 直前の Win32 呼び出しのエラーを読むだけ。プロセスが既に無ければ ERROR_INVALID_PARAMETER。
        return match unsafe { GetLastError() } {
            87 => Ok(true),
            _ => Err(io::Error::last_os_error()),
        };
    }
    let process = Handle(handle);
    // SAFETY: `process` は有効なハンドル。
    let status =
        unsafe { WaitForSingleObject(process.0, u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX)) };
    Ok(status == WAIT_OBJECT_0)
}
