//! Windows のコマンド解決。ConPTY（`CreateProcess`）は拡張子なしの名前（`claude`、`npm`）を解決せず、
//! `.ps1` は実行できない（error 193）ので、`PATHEXT` で実行ファイルを探し、`.ps1` は `powershell.exe -File` で起動する。
//! ファイルシステムに触れるだけで OS の API は呼ばないので、Unix でも単体テストできる（実際に使うのは Windows だけ）。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `PATHEXT` が無いときの既定（cmd.exe と同じ）。
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";
const POWERSHELL: &str = "powershell.exe";

/// 解決に使う、子の環境の `PATH` / `PATHEXT` と作業ディレクトリ。
#[derive(Debug, Clone)]
pub struct WindowsEnv {
    path_dirs: Vec<PathBuf>,
    /// 先頭にドットを付けた拡張子（環境変数のままの大文字小文字）。
    pathext: Vec<String>,
    cwd: PathBuf,
}

/// `;` 区切りの値を分ける。空の要素は捨て、前後の引用符は外す。
fn split_list(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(';')
        .map(|part| part.trim().trim_matches('"'))
        .filter(|part| !part.is_empty())
}

fn find<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsString> {
    env.iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

impl WindowsEnv {
    /// 子に渡す環境（キーの大文字小文字は区別しない）から作る。
    pub fn from_pairs(env: &[(OsString, OsString)], cwd: &str) -> Self {
        let path = find(env, "PATH").map(|v| v.to_string_lossy().into_owned());
        let pathext = find(env, "PATHEXT").map(|v| v.to_string_lossy().into_owned());
        Self {
            path_dirs: path
                .iter()
                .flat_map(|p| split_list(p))
                .map(PathBuf::from)
                .collect(),
            pathext: split_list(pathext.as_deref().unwrap_or(DEFAULT_PATHEXT))
                .map(str::to_string)
                .collect(),
            cwd: PathBuf::from(cwd),
        }
    }
}

/// ディレクトリを含む指定（`C:\tool`、`.\tool`、`sub\tool`）か。含めば PATH は探さない。
fn has_directory(program: &str) -> bool {
    program.contains(['\\', '/']) || program.as_bytes().get(1) == Some(&b':')
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_ascii_lowercase()))
}

/// `base` で起動しうるファイルを、優先順に返す。`base` 自身は、実行できる拡張子（`PATHEXT` か `.ps1`）のときだけ。
/// そのあとに `PATHEXT` の各拡張子を付けたもの（`npm` -> `npm.cmd`）。
fn candidates(base: &Path, env: &WindowsEnv) -> Vec<PathBuf> {
    let runnable = |ext: &str| ext == ".ps1" || env.pathext.iter().any(|e| e.eq_ignore_ascii_case(ext));
    let mut out = Vec::new();
    if extension_of(base).is_some_and(|ext| runnable(&ext)) {
        out.push(base.to_path_buf());
    }
    for ext in &env.pathext {
        let mut name = base.as_os_str().to_os_string();
        name.push(ext);
        out.push(PathBuf::from(name));
    }
    out
}

fn locate(program: &str, env: &WindowsEnv) -> Option<PathBuf> {
    let bases: Vec<PathBuf> = if has_directory(program) {
        vec![env.cwd.join(program)]
    } else {
        env.path_dirs.iter().map(|dir| dir.join(program)).collect()
    };
    bases
        .iter()
        .flat_map(|base| candidates(base, env))
        .find(|candidate| candidate.is_file())
}

/// `.` の要素だけ取り除いて文字列にする（`cwd\.\tool.exe` を `cwd\tool.exe` に）。
fn display(path: PathBuf) -> String {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// 起動する argv に直す。`.ps1` は `powershell.exe -File` に包み、それ以外は見つけたファイルのフルパスに置き換える。
/// 見つからなければ、名前を含むエラー。
pub fn resolve_command(cmd: &[String], env: &WindowsEnv) -> Result<Vec<String>, String> {
    let (program, args) = cmd.split_first().ok_or("cmd is empty")?;
    let found = locate(program, env).ok_or_else(|| format!("command not found: {program}"))?;
    if extension_of(&found).as_deref() != Some(".ps1") {
        return Ok(std::iter::once(display(found))
            .chain(args.iter().cloned())
            .collect());
    }
    // 実行ポリシー（既定は Restricted）に止められないよう、この 1 プロセスだけ Bypass にする。
    let shell = locate(POWERSHELL, env)
        .ok_or_else(|| format!("command not found: {POWERSHELL} (needed to run {program})"))?;
    Ok([
        display(shell),
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        display(found),
    ]
    .into_iter()
    .chain(args.iter().cloned())
    .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn env(dirs: &[&Path], pathext: Option<&str>, cwd: &Path) -> WindowsEnv {
        let path = dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(";");
        // Unix のファイルシステムは大文字小文字を区別するので、テストの PATHEXT は小文字にする。
        let ext = pathext.unwrap_or(".com;.exe;.bat;.cmd");
        let pairs = vec![
            (OsString::from("Path"), OsString::from(path)),
            (OsString::from("PATHEXT"), OsString::from(ext)),
        ];
        WindowsEnv::from_pairs(&pairs, &cwd.to_string_lossy())
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, "").unwrap();
        path
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn a_name_without_an_extension_is_resolved_through_pathext_in_order() {
        let bin = TempDir::new().unwrap();
        touch(bin.path(), "npm"); // 拡張子なしのシェルスクリプトは起動できないので選ばない
        let cmd = touch(bin.path(), "npm.cmd");
        let e = env(&[bin.path()], Some(".exe;.cmd"), bin.path());
        assert_eq!(
            resolve_command(&argv(&["npm", "--version"]), &e).unwrap(),
            vec![display(cmd), "--version".to_string()]
        );
        // 先に並んだ拡張子が勝つ
        let exe = touch(bin.path(), "npm.exe");
        assert_eq!(resolve_command(&argv(&["npm"]), &e).unwrap(), vec![display(exe)]);
    }

    #[test]
    fn path_directories_are_searched_in_order_and_the_default_pathext_applies() {
        let (first, second) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        touch(second.path(), "tool.exe");
        let later = touch(first.path(), "tool.cmd");
        let e = env(&[first.path(), second.path()], None, first.path());
        assert_eq!(
            resolve_command(&argv(&["tool"]), &e).unwrap(),
            vec![display(later)]
        );
    }

    #[test]
    fn an_explicit_extension_and_a_directory_are_honoured() {
        let bin = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let exe = touch(bin.path(), "a.exe");
        touch(other.path(), "b.cmd");
        let e = env(&[other.path()], None, bin.path());
        // ディレクトリ付きの指定は PATH を探さない。相対なら cwd 基準
        assert_eq!(
            resolve_command(&argv(&[&exe.to_string_lossy()]), &e).unwrap(),
            vec![display(exe.clone())]
        );
        assert_eq!(
            resolve_command(&argv(&["./a.exe"]), &e).unwrap(),
            vec![display(exe)]
        );
        assert!(resolve_command(&argv(&["b"]), &env(&[bin.path()], None, bin.path())).is_err());
        // 拡張子が違えば名前に PATHEXT を足して探す（foo.bar -> foo.bar.exe）
        let bar = touch(bin.path(), "foo.bar.exe");
        assert_eq!(
            resolve_command(&argv(&["foo.bar"]), &env(&[bin.path()], None, bin.path())).unwrap(),
            vec![display(bar)]
        );
    }

    #[test]
    fn a_ps1_script_is_run_through_powershell_file() {
        let bin = TempDir::new().unwrap();
        let script = touch(bin.path(), "deploy.ps1");
        let shell = touch(bin.path(), "powershell.exe");
        let e = env(&[bin.path()], Some(".exe;.cmd"), bin.path());
        let resolved = resolve_command(&argv(&["deploy.ps1", "-Flag", "x y"]), &e).unwrap();
        assert_eq!(
            resolved,
            vec![
                display(shell),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                display(script),
                "-Flag".into(),
                "x y".into(),
            ]
        );
    }

    #[test]
    fn a_missing_command_is_reported_with_its_name() {
        let bin = TempDir::new().unwrap();
        let e = env(&[bin.path()], None, bin.path());
        assert_eq!(
            resolve_command(&argv(&["claude"]), &e).unwrap_err(),
            "command not found: claude"
        );
        touch(bin.path(), "run.ps1");
        let err = resolve_command(&argv(&["run.ps1"]), &e).unwrap_err();
        assert!(err.contains("powershell.exe") && err.contains("run.ps1"), "{err}");
        assert!(resolve_command(&[], &e).is_err());
    }

    #[test]
    fn list_values_drop_empty_entries_and_quotes() {
        assert_eq!(
            split_list("C:\\a;;\"C:\\b c\"; ").collect::<Vec<_>>(),
            vec!["C:\\a", "C:\\b c"]
        );
    }
}
