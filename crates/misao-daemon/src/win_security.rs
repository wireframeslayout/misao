//! Windows のセキュリティ記述子: 起動したユーザーだけにアクセスを許す DACL。
//! 名前付きパイプ（接続口）と停止イベントの作成に使う。`unsafe` は Win32 の呼び出しだけに限る。
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::os::windows::io::RawHandle;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// 閉じ忘れないための、Win32 のハンドルの持ち主。
pub struct Handle(pub HANDLE);

// SAFETY: カーネルオブジェクトのハンドルは、どのスレッドからでも使える。
unsafe impl Send for Handle {}
// SAFETY: 同上。ハンドルの値は変更しない。
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `self.0` は有効な、まだ閉じていないハンドル。
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// NUL 終端の UTF-16 文字列を読む。
///
/// # Safety
/// `ptr` は NUL で終わる有効な UTF-16 文字列を指していること。
unsafe fn read_wide(ptr: *const u16) -> String {
    let mut len = 0;
    // SAFETY: 呼び出し側の保証どおり、NUL までは読める。
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: 上で数えた `len` 要素は読める。
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// このプロセスを動かしているユーザーの SID を文字列（`S-1-5-21-...`）で返す。
pub fn current_user_sid() -> io::Result<String> {
    // SAFETY: 疑似ハンドル（現在のプロセス）は閉じなくてよく、常に有効。
    user_sid_of_process(unsafe { GetCurrentProcess() })
}

/// 別のプロセスを動かしているユーザーの SID。トークンを読めない（権限が足りない）プロセスは失敗する。
fn user_sid_of_process(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` は有効なハンドル（呼び出し側の保証）。`token` は書き込める。
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut needed = 0u32;
    // SAFETY: バッファ長 0 でサイズだけを問い合わせる（失敗が正常）。
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER はポインタを含むので、8 バイト境界に揃えた領域に読む。
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buffer` は `needed` バイト以上の書き込める領域。
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: 成功したので、先頭は有効な TOKEN_USER（境界も揃っている）。
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    // SAFETY: `user.User.Sid` は `buffer` の中の有効な SID で、`buffer` はこの関数の間生きている。
    unsafe { sid_to_string(user.User.Sid) }
}

/// SID を文字列（`S-1-5-32-544` など）にする。
///
/// # Safety
/// `sid` は有効な SID を指していること。
pub unsafe fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: 呼び出し側の保証どおり `sid` は有効。成功すると `text` に LocalAlloc の文字列が入る。
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: 成功したので `text` は NUL 終端の文字列。読み終えたら LocalFree で解放する。
    let out = unsafe { read_wide(text) };
    // SAFETY: `text` は ConvertSidToStringSidW が LocalAlloc で確保した領域。
    unsafe { LocalFree(text.cast()) };
    Ok(out)
}

/// 起動したユーザーだけに全アクセスを許し（継承しない `D:P`）、他は誰も（SYSTEM も Administrators も）許さない DACL。
/// 名前付きパイプの作成時に `SECURITY_ATTRIBUTES` として渡す。
pub struct UserOnlyAcl {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

// SAFETY: 記述子は作成後に変更しない読み取り専用のメモリで、解放は Drop だけが行う。
unsafe impl Send for UserOnlyAcl {}
// SAFETY: 同上。
unsafe impl Sync for UserOnlyAcl {}

impl UserOnlyAcl {
    pub fn new() -> io::Result<Self> {
        let sddl = wide(&format!("D:P(A;;GA;;;{})", current_user_sid()?));
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` は NUL 終端の UTF-16。成功すると `descriptor` に LocalAlloc の記述子が入る。
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor,
            attributes: SECURITY_ATTRIBUTES {
                nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
        })
    }

    /// `CreateNamedPipeW` などに渡す `SECURITY_ATTRIBUTES` へのポインタ（`self` が生きている間だけ有効）。
    pub fn attributes(&self) -> *mut c_void {
        std::ptr::from_ref(&self.attributes).cast_mut().cast()
    }
}

impl Drop for UserOnlyAcl {
    fn drop(&mut self) {
        // SAFETY: `descriptor` は ConvertStringSecurityDescriptorToSecurityDescriptorW が LocalAlloc で確保した領域。
        unsafe { LocalFree(self.descriptor) };
    }
}

/// 開いたパイプのサーバー側のプロセスが、`expected` の SID のユーザーのものか確かめる。
/// 別のユーザーが先に同じ名前のパイプを作って待ち受けている（先取り）のを、接続の直後に見つける。
pub fn check_pipe_server_user(pipe: RawHandle, expected: &str) -> io::Result<()> {
    let mut pid = 0u32;
    // SAFETY: `pipe` は呼び出し側が開いている、有効なパイプのハンドル。`pid` は書き込める。
    if unsafe { GetNamedPipeServerProcessId(pipe as HANDLE, &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: プロセスの情報を問い合わせる権限だけで開く。ハンドルは `Handle` が閉じる。
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        let cause = io::Error::last_os_error();
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("cannot inspect the pipe server (pid {pid}): {cause}"),
        ));
    }
    let process = Handle(process);
    let actual = user_sid_of_process(process.0).map_err(|cause| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("cannot read the user of the pipe server (pid {pid}): {cause}"),
        )
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the pipe server (pid {pid}) runs as {actual}, not as {expected}"),
        ))
    }
}

/// 開いたパイプのサーバーが、このプロセスと同じユーザーのものか確かめる。
pub fn check_pipe_server(pipe: RawHandle) -> io::Result<()> {
    check_pipe_server_user(pipe, &current_user_sid()?)
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;

    use super::*;

    #[tokio::test]
    async fn the_pipe_server_of_the_same_user_is_accepted_and_another_user_is_refused() {
        let name = format!("\\\\.\\pipe\\misao-sidtest-{}", std::process::id());
        let acl = UserOnlyAcl::new().unwrap();
        let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
        options.first_pipe_instance(true);
        // SAFETY: `acl` は有効な SECURITY_ATTRIBUTES を持ち、この呼び出しの間生きている。
        let _server =
            unsafe { options.create_with_security_attributes_raw(&name, acl.attributes()) }.unwrap();
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        check_pipe_server(client.as_raw_handle()).unwrap();
        let err = check_pipe_server_user(client.as_raw_handle(), "S-1-5-21-1-2-3-4242").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("not as S-1-5-21-1-2-3-4242"), "{err}");
    }
}
