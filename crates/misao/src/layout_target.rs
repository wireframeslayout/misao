//! `new --workspace W --window NAME` の置き先 windowId を決める。

use serde_json::{Value, json};

use crate::client::Client;
use crate::error::CliError;

/// デーモンの既定の workspace / window 名（pane.open で windowId を省略したときの置き先）。
const DEFAULT_NAME: &str = "default";

fn invalid(method: &str) -> CliError {
    CliError::runtime(&format!("invalid result for {method}"))
}

/// どちらか片方だけなら、もう片方は既定名。無い workspace / window は作る。
/// どちらも指定されなければ None（デーモンの既定に任せる）。
pub async fn resolve_window_for_open(
    client: &Client,
    workspace: Option<&str>,
    window: Option<&str>,
) -> Result<Option<String>, CliError> {
    if workspace.is_none() && window.is_none() {
        return Ok(None);
    }
    let workspace_name = workspace.unwrap_or(DEFAULT_NAME);
    let window_name = window.unwrap_or(DEFAULT_NAME);
    let list = client.request("workspace.list", json!({})).await?;
    let existing = list
        .as_array()
        .ok_or_else(|| invalid("workspace.list"))?
        .iter()
        .find(|w| w.get("name").and_then(Value::as_str) == Some(workspace_name))
        .cloned();
    let workspace_value = match existing {
        Some(w) => w,
        None => {
            client
                .request("workspace.create", json!({"name": workspace_name}))
                .await?
        }
    };
    let found = workspace_value
        .get("windows")
        .and_then(Value::as_array)
        .and_then(|windows| {
            windows
                .iter()
                .find(|w| w.get("name").and_then(Value::as_str) == Some(window_name))
        })
        .cloned();
    let window_value = match found {
        Some(w) => w,
        None => {
            client
                .request(
                    "window.create",
                    json!({"workspace": workspace_name, "name": window_name}),
                )
                .await?
        }
    };
    window_value
        .get("windowId")
        .and_then(Value::as_str)
        .map(|id| Some(id.to_string()))
        .ok_or_else(|| invalid("window.create"))
}
