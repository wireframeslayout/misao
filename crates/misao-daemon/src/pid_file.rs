#![cfg_attr(windows, allow(unsafe_code))]

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::DaemonError;

/// pid のプロセスが生きているか。EPERM は「存在するが権限がない」なので生存扱い。
#[cfg(unix)]
fn is_alive(pid: i32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    match kill(Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(e) => e == Errno::EPERM,
    }
}

/// pid のプロセスが生きているか。開けない（権限が足りない）プロセスは「存在するが権限がない」なので生存扱い。
#[cfg(windows)]
fn is_alive(pid: i32) -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use crate::win_security::Handle;

    const STILL_ACTIVE: u32 = 259;
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

/// 秘密のファイルとして作る（Unix は 600。Windows は置き場所のディレクトリの ACL による）。
fn create_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// 先頭の 10 進数を pid として読む（Node の `parseInt` と同じ。中身が pid でなければ None）。
fn parse_pid(text: &str) -> Option<i32> {
    let digits: String = text
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<i32>().ok().filter(|&n| n > 0)
}

/// pid ファイルを O_EXCL で作り、起動を排他にする。
/// 既存のファイルの pid が生きていれば失敗。死んでいる（または中身が pid でない）なら置き換える。
pub fn acquire_pid_file(pid_path: &Path, pid: u32) -> Result<(), DaemonError> {
    let ctx = |e| DaemonError::io(format!("cannot write {}", pid_path.display()), e);
    for _ in 0..2 {
        match create_options().open(pid_path) {
            Ok(mut f) => return f.write_all(pid.to_string().as_bytes()).map_err(ctx),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(ctx(e)),
        }
        let text = fs::read_to_string(pid_path)
            .map_err(|e| DaemonError::io(format!("cannot read {}", pid_path.display()), e))?;
        if let Some(owner) = parse_pid(&text).filter(|&p| is_alive(p)) {
            return Err(DaemonError::PidFileHeld {
                pid: owner,
                path: pid_path.to_path_buf(),
            });
        }
        match fs::remove_file(pid_path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(ctx(e)),
            _ => {}
        }
    }
    Err(DaemonError::PidFileContended(pid_path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn creates_the_file_with_the_pid() {
        let root = TempDir::new().unwrap();
        let p = root.path().join("daemon.pid");
        acquire_pid_file(&p, 4321).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "4321");
    }

    #[test]
    fn refuses_when_the_owner_is_alive() {
        let root = TempDir::new().unwrap();
        let p = root.path().join("daemon.pid");
        fs::write(&p, std::process::id().to_string()).unwrap();
        let err = acquire_pid_file(&p, 1).unwrap_err();
        assert!(
            matches!(err, DaemonError::PidFileHeld { pid, .. } if pid == std::process::id() as i32),
            "{err}"
        );
    }

    #[test]
    fn replaces_garbage_and_dead_owners() {
        let root = TempDir::new().unwrap();
        for (i, content) in ["", "abc", "0", "-5", "2147483646"].into_iter().enumerate() {
            let p = root.path().join(format!("{i}.pid"));
            fs::write(&p, content).unwrap();
            acquire_pid_file(&p, 99).unwrap();
            assert_eq!(fs::read_to_string(&p).unwrap(), "99", "content {content:?}");
        }
    }
}
