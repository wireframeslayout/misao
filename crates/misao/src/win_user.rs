//! Windows のユーザー名。SDK の `os.userInfo().username` と同じ値でパイプ名が決まるので、環境変数ではなく API から読む。
#![allow(unsafe_code)]

use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;

/// `UNLEN`（ユーザー名の最大文字数）+ 終端。
const NAME_BUFFER: usize = 257;

pub fn current_user_name() -> std::io::Result<String> {
    let mut buffer = [0u16; NAME_BUFFER];
    let mut size = u32::try_from(NAME_BUFFER).unwrap_or(u32::MAX);
    // SAFETY: `buffer` は `size` 文字ぶんの書き込める領域。成功すると `size` に終端を含む長さが入る。
    if unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut size) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let length = usize::try_from(size).unwrap_or(0).saturating_sub(1);
    Ok(String::from_utf16_lossy(&buffer[..length.min(NAME_BUFFER)]))
}
