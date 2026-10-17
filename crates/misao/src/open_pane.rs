//! 端末から作るペイン（`new` と attach の一覧の n）。

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::client::Client;
use crate::error::CliError;
use crate::model::PaneInfo;

pub struct OpenPaneRequest {
    pub cmd: Vec<String>,
    pub cwd: PathBuf,
    pub labels: Vec<(String, String)>,
    pub env: Vec<(String, String)>,
    /// 置き先の窓。省略時はデーモンの既定の窓。
    pub window_id: Option<String>,
}

/// ログインシェルの起動コマンド。シェルが取れない環境では usage エラー。Windows の cmd.exe は -l を受け付けないので付けない。
pub fn login_shell(shell: Option<&str>, is_windows: bool) -> Result<Vec<String>, CliError> {
    let shell = shell.ok_or_else(|| {
        CliError::usage("ログインシェルを特定できません。-- の後ろにコマンドを指定してください")
    })?;
    Ok(if is_windows {
        vec![shell.to_string()]
    } else {
        vec![shell.to_string(), "-l".to_string()]
    })
}

fn to_object(pairs: &[(String, String)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect()
}

/// 端末から作るペインを開く。origin=terminal ラベルを必ず付ける（未登録のペインを見分けるため）。
pub async fn open_terminal_pane(client: &Client, request: &OpenPaneRequest) -> Result<String, CliError> {
    let mut labels = to_object(&request.labels);
    labels.insert("origin".to_string(), json!("terminal"));
    let mut params = json!({
        "cmd": request.cmd,
        "cwd": request.cwd.to_string_lossy(),
        "labels": labels,
    });
    if !request.env.is_empty() {
        params["env"] = Value::Object(to_object(&request.env));
    }
    if let Some(window_id) = &request.window_id {
        params["windowId"] = json!(window_id);
    }
    let result = client.request("pane.open", params).await?;
    result
        .get("paneId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CliError::runtime(&"invalid result for pane.open"))
}

/// `pane.list` の結果を型にして返す。
pub async fn list_panes(client: &Client) -> Result<Vec<PaneInfo>, CliError> {
    let value = client.request("pane.list", json!({})).await?;
    PaneInfo::list_from_value(value).map_err(|e| CliError::runtime(&e))
}

pub async fn pane_info(client: &Client, pane_id: &str) -> Result<PaneInfo, CliError> {
    let value = client.request("pane.info", json!({"paneId": pane_id})).await?;
    PaneInfo::from_value(value).map_err(|e| CliError::runtime(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_shells_get_l_and_cmd_exe_does_not() {
        assert_eq!(
            login_shell(Some("/bin/bash"), false).unwrap(),
            vec!["/bin/bash", "-l"]
        );
        assert_eq!(
            login_shell(Some("C:\\Windows\\System32\\cmd.exe"), true).unwrap(),
            vec!["C:\\Windows\\System32\\cmd.exe"]
        );
        let err = login_shell(None, true).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.message.contains("ログインシェルを特定できません"));
    }
}
