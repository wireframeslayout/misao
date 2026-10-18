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
    ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
    GetAclInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
};

use crate::DaemonError;
use crate::win_security::{current_user_sid, sid_to_string};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ERROR_SUCCESS: u32 = 0;
/// `ACE_HEADER::AceType`: 許可 ACE、許可の callback ACE（SID の位置は許可 ACE と同じ）、許可の object ACE、許可の callback object ACE。
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_ALLOWED_OBJECT_ACE_TYPE: u8 = 5;
const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 9;
const ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE: u8 = 11;
/// object ACE の `Flags`: ObjectType / InheritedObjectType の GUID（各 16 バイト）が SID の前にある。
const ACE_OBJECT_TYPE_PRESENT: u32 = 0x1;
const ACE_INHERITED_OBJECT_TYPE_PRESENT: u32 = 0x2;
/// `ACE_HEADER::AceFlags`: 子へ継承する（ファイル / ディレクトリ）か、継承専用（この ACL の対象自身には効かない）か。
const OBJECT_INHERIT_ACE: u8 = 0x1;
const CONTAINER_INHERIT_ACE: u8 = 0x2;
const INHERIT_ONLY_ACE: u8 = 0x8;
const SYSTEM: &str = "S-1-5-18";
/// 昇格した管理者が作ったファイルの既定の所有者。
const ADMINISTRATORS: &str = "S-1-5-32-544";
const FRIENDLY_NAMES: [(&str, &str); 5] = [
    ("S-1-1-0", "Everyone"),
    ("S-1-5-11", "Authenticated Users"),
    ("S-1-5-32-545", "Users"),
    ("S-1-5-7", "Anonymous"),
    ("S-1-3-0", "CREATOR OWNER"),
];
/// FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_WRITE | GENERIC_ALL
const WRITE_MASK: u32 = 0x2 | 0x4 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

/// 保存した状態・pid ファイルの置き場所を用意する。無ければ作る（Windows では既定の ACL を継承する）。
/// ディレクトリであること・リンク（再解析ポイント）でないこと・所有者が起動したユーザー（昇格した管理者なら Administrators）であること・
/// 書き込み系の許可 ACE が、起動したユーザー・SYSTEM・Administrators のものだけであること（それ以外の SID は、
/// 別のユーザーのアカウントでも広いグループでも拒否する）を検証する。
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
    let me = current_user_sid().map_err(|e| DaemonError::io("cannot determine the current user", e))?;
    let security = read_owner_and_dacl(dir, &me)
        .map_err(|e| DaemonError::io(format!("cannot read the access control of {}", dir.display()), e))?;
    if security.owner != me && security.owner != ADMINISTRATORS {
        return Err(DaemonError::DataDirNotOwned(dir.to_path_buf()));
    }
    if let Some(principal) = security.foreign_writer {
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
    foreign_writer: Option<String>,
}

fn read_owner_and_dacl(dir: &Path, me: &str) -> std::io::Result<DirSecurity> {
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
        let foreign_writer = unsafe { find_foreign_writer(dacl, me) }?;
        Ok(DirSecurity {
            owner,
            foreign_writer,
        })
    });
    // SAFETY: descriptor は GetNamedSecurityInfoW が LocalAlloc で確保した領域。owner と dacl はもう使わない。
    unsafe { LocalFree(descriptor.cast::<c_void>()) };
    result
}

/// DACL の許可 ACE から、書き込み系の権限を `me`・SYSTEM・Administrators 以外に与えているものを探す（あればその SID）。
/// DACL が NULL（誰にでも全権）なら Everyone。object ACE と callback ACE も読み、継承専用で子にも伝わらない ACE は無視する。
///
/// # Safety
/// `dacl` は有効な ACL か NULL を指していること。
unsafe fn find_foreign_writer(dacl: *mut ACL, me: &str) -> std::io::Result<Option<String>> {
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
        // SAFETY: GetAce が返した ACE は有効で、先頭に ACE_HEADER を持つ。
        let Some((mask, sid)) = (unsafe { allowed_mask_and_sid(ace) }) else {
            continue;
        };
        if mask & WRITE_MASK == 0 {
            continue;
        }
        // SAFETY: `sid` は ACE の中の有効な SID。
        let sid = unsafe { sid_to_string(sid) }?;
        if sid != me && sid != SYSTEM && sid != ADMINISTRATORS {
            let name = FRIENDLY_NAMES
                .iter()
                .find(|(id, _)| *id == sid)
                .map_or(sid.as_str(), |(_, n)| *n);
            return Ok(Some(name.to_string()));
        }
    }
    Ok(None)
}

/// 許可系の ACE（継承専用で子にも伝わらないものを除く）の権限マスクと SID を読む。それ以外の ACE は None。
///
/// # Safety
/// `ace` は GetAce が返した有効な ACE を指していること。
unsafe fn allowed_mask_and_sid(ace: *mut c_void) -> Option<(u32, PSID)> {
    let base = ace.cast::<u8>();
    // SAFETY: 先頭は ACE_HEADER（呼び出し側の保証）。
    let header = unsafe { std::ptr::read_unaligned(base.cast::<ACE_HEADER>()) };
    let propagates = header.AceFlags & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) != 0;
    if header.AceFlags & INHERIT_ONLY_ACE != 0 && !propagates {
        return None;
    }
    // どの型も、ヘッダの後ろが Mask (4 バイト) で、その後ろの並びが違う。
    // SAFETY: 許可系の ACE は ヘッダ 4 バイト + Mask 4 バイトより長い。
    let mask = unsafe { std::ptr::read_unaligned(base.add(4).cast::<u32>()) };
    let sid_offset = match header.AceType {
        ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => 8,
        ACCESS_ALLOWED_OBJECT_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE => {
            // SAFETY: object ACE は Mask の後ろに Flags (4 バイト) を持つ。
            let flags = unsafe { std::ptr::read_unaligned(base.add(8).cast::<u32>()) };
            12 + 16 * usize::from(flags & ACE_OBJECT_TYPE_PRESENT != 0)
                + 16 * usize::from(flags & ACE_INHERITED_OBJECT_TYPE_PRESENT != 0)
        }
        _ => return None,
    };
    // SAFETY: SID は上のオフセットから、ACE の中で始まる。
    Some((mask, unsafe { base.add(sid_offset) }.cast::<c_void>()))
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

    /// `icacls` で `sid` に `grant`（例 `(OI)(CI)(M)`）を付けた新しいディレクトリ。
    // 実在しない SID は icacls が受け付けないので、実在する別のグループ（Backup Operators = S-1-5-32-551）を「ユーザーでも SYSTEM でも Administrators でもない SID」に使う。
    fn dir_with_grant(tmp: &TempDir, sid: &str, grant: &str) -> std::path::PathBuf {
        let dir = tmp.path().join("granted");
        std::fs::create_dir(&dir).unwrap();
        let done = Command::new("icacls")
            .arg(&dir)
            .args(["/grant", &format!("*{sid}:{grant}")])
            .output()
            .unwrap();
        assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stdout));
        dir.join("persistence.json")
    }

    #[test]
    fn modify_for_a_non_broad_group_that_is_not_the_user_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let state = dir_with_grant(&tmp, "S-1-5-32-551", "(OI)(CI)(M)");
        let err = ensure_data_dir(&state).unwrap_err();
        assert!(
            matches!(&err, DaemonError::DataDirWritableByPrincipal { principal, .. } if principal == "S-1-5-32-551"),
            "{err}"
        );
    }

    #[test]
    fn an_inherit_only_write_entry_that_reaches_children_is_rejected() {
        let tmp = TempDir::new().unwrap();
        // Power Users: 自身には効かないが、中に作るファイルとサブディレクトリには伝わる
        let state = dir_with_grant(&tmp, "S-1-5-32-547", "(OI)(CI)(IO)(M)");
        assert!(matches!(
            ensure_data_dir(&state).unwrap_err(),
            DaemonError::DataDirWritableByPrincipal { .. }
        ));
    }

    #[test]
    fn read_only_entries_for_other_accounts_are_fine() {
        let tmp = TempDir::new().unwrap();
        let state = dir_with_grant(&tmp, "S-1-5-32-551", "(OI)(CI)(RX)");
        ensure_data_dir(&state).unwrap();
    }
}
