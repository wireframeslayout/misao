//! Windows の保存先ディレクトリ（`--data`）の検証。Unix の「所有者が自分・group/other が書けない」に当たる確認を、
//! 所有者の SID と DACL で行う。
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION,
    GetAce, GetAclInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
};

use crate::DaemonError;
use crate::win_security::{current_user_sid, sid_to_string};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ERROR_SUCCESS: u32 = 0;
/// `ACE_HEADER::AceType` が許可 ACE のとき（ACCESS_ALLOWED_ACE_TYPE）。
const ACCESS_ALLOWED_ACE_TYPE: u32 = 0;
/// 昇格した管理者が作ったファイルの既定の所有者。
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// 起動したユーザー以外のこれらに書き込みを許すと、他のユーザーがファイルを差し替えられる。
const BROAD_PRINCIPALS: [(&str, &str); 4] = [
    ("S-1-1-0", "Everyone"),
    ("S-1-5-11", "Authenticated Users"),
    ("S-1-5-32-545", "Users"),
    ("S-1-5-7", "Anonymous"),
];
/// FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_WRITE | GENERIC_ALL
const WRITE_MASK: u32 = 0x2 | 0x4 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

/// 保存した状態・pid ファイルの置き場所を用意する。無ければ作る（Windows では既定の ACL を継承する）。
/// ディレクトリであること・リンク（再解析ポイント）でないこと・所有者が起動したユーザー（昇格した管理者なら Administrators）であること・
/// 他のユーザー（Everyone / Users / Authenticated Users）に書き込みを許していないことを検証する。
pub fn ensure_data_dir(file_in_dir: &Path) -> Result<(), DaemonError> {
    let dir = file_in_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)
        .map_err(|e| DaemonError::io(format!("cannot create data directory {}", dir.display()), e))?;
    let meta = fs::symlink_metadata(dir)
        .map_err(|e| DaemonError::io(format!("cannot stat data directory {}", dir.display()), e))?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(DaemonError::DataDirIsSymlink(dir.to_path_buf()));
    }
    if !meta.is_dir() {
        return Err(DaemonError::DataDirNotDirectory(dir.to_path_buf()));
    }
    let security = read_owner_and_dacl(dir)
        .map_err(|e| DaemonError::io(format!("cannot read the access control of {}", dir.display()), e))?;
    let me = current_user_sid().map_err(|e| DaemonError::io("cannot determine the current user", e))?;
    if security.owner != me && security.owner != ADMINISTRATORS {
        return Err(DaemonError::DataDirNotOwned(dir.to_path_buf()));
    }
    if let Some(principal) = security.broad_writer {
        return Err(DaemonError::DataDirWritableByPrincipal {
            dir: dir.to_path_buf(),
            principal,
        });
    }
    Ok(())
}

struct DirSecurity {
    owner: String,
    /// 書き込みを許している広いグループ（あれば最初の 1 つ）。
    broad_writer: Option<String>,
}

fn read_owner_and_dacl(dir: &Path) -> std::io::Result<DirSecurity> {
    let path: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `path` は NUL 終端の UTF-16。出力は書き込める変数で、成功すると owner と dacl は descriptor の中を指す。
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ));
    }
    // SAFETY: 成功したので、owner は descriptor の中の有効な SID（descriptor を解放するまで）。
    let result = unsafe { sid_to_string(owner) }.and_then(|owner| {
        // SAFETY: dacl は descriptor の中の ACL か NULL（NULL の DACL は誰にでも全権を与える）。
        let broad_writer = unsafe { find_broad_writer(dacl) }?;
        Ok(DirSecurity { owner, broad_writer })
    });
    // SAFETY: descriptor は GetNamedSecurityInfoW が LocalAlloc で確保した領域。owner と dacl はもう使わない。
    unsafe { LocalFree(descriptor.cast::<c_void>()) };
    result
}

/// DACL の許可 ACE から、広いグループへの書き込み許可を探す。DACL が NULL（誰にでも全権）なら Everyone。
///
/// # Safety
/// `dacl` は有効な ACL か NULL を指していること。
unsafe fn find_broad_writer(dacl: *mut ACL) -> std::io::Result<Option<String>> {
    if dacl.is_null() {
        return Ok(Some("Everyone".to_string()));
    }
    // SAFETY: 全ビットがゼロの構造体は有効で、成功したときだけ読む。
    let mut size: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `dacl` は有効な ACL（呼び出し側の保証）。`size` はサイズを渡した書き込める構造体。
    let ok = unsafe {
        GetAclInformation(
            dacl,
            std::ptr::from_mut(&mut size).cast(),
            u32::try_from(std::mem::size_of_val(&size)).unwrap_or(u32::MAX),
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    for index in 0..size.AceCount {
        let mut ace: *mut c_void = std::ptr::null_mut();
        // SAFETY: index は ACE の数の範囲内。
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: GetAce が返した ACE は、先頭に ACE_HEADER を持つ。
        if u32::from(unsafe { (*ace.cast::<ACE_HEADER>()).AceType }) != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: 許可 ACE なので ACCESS_ALLOWED_ACE の形で、SidStart から SID が始まる。
        let (mask, sid) = unsafe {
            (
                (*allowed).Mask,
                std::ptr::addr_of_mut!((*allowed).SidStart).cast::<c_void>(),
            )
        };
        if mask & WRITE_MASK == 0 {
            continue;
        }
        // SAFETY: `sid` は ACE の中の有効な SID。
        let sid = unsafe { sid_to_string(sid) }?;
        if let Some((_, name)) = BROAD_PRINCIPALS.iter().find(|(id, _)| *id == sid) {
            return Ok(Some((*name).to_string()));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_fresh_directory_owned_by_the_user_is_accepted_and_created_when_missing() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("new/data/persistence.json");
        ensure_data_dir(&state).unwrap();
        assert!(tmp.path().join("new/data").is_dir());
        ensure_data_dir(&state).unwrap();
    }

    #[test]
    fn a_file_in_place_of_the_directory_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("data");
        std::fs::write(&file, "").unwrap();
        let err = ensure_data_dir(&file.join("persistence.json")).unwrap_err();
        assert!(
            matches!(err, DaemonError::DataDirNotDirectory(_) | DaemonError::Io { .. }),
            "{err}"
        );
    }

    #[test]
    fn a_directory_that_everyone_can_write_to_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("shared");
        std::fs::create_dir(&dir).unwrap();
        let granted = Command::new("icacls")
            .arg(&dir)
            .args(["/grant", "*S-1-1-0:(OI)(CI)(M)"])
            .output()
            .unwrap();
        assert!(
            granted.status.success(),
            "{}",
            String::from_utf8_lossy(&granted.stdout)
        );
        let err = ensure_data_dir(&dir.join("persistence.json")).unwrap_err();
        assert!(
            matches!(&err, DaemonError::DataDirWritableByPrincipal { principal, .. } if principal == "Everyone"),
            "{err}"
        );
    }
}
