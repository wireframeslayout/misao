//! Windows のセキュリティ記述子: 起動したユーザーだけにアクセスを許す DACL。
//! 名前付きパイプ（接続口）と停止イベントの作成に使う。`unsafe` は Win32 の呼び出しだけに限る。
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: 疑似ハンドル（現在のプロセス）の問い合わせ権限でトークンを開く。`token` は書き込める。
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
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
