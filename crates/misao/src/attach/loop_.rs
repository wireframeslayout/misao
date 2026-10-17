//! session と picker をつなぎ、ペイン間の移動（n / p）、一覧（l / d）、新しいシェル（一覧の n）を扱う（Node 版の `attach/loop.ts`）。

use misao_protocol::ErrorCode;

use super::picker::{PickerChoice, PickerOptions, attachable_panes, resumed_notice, run_picker};
use super::prefix_filter::PrefixAction;
use super::session::{Replay, SessionEnd, SessionOptions, SessionReason, run_session};
use crate::client::{Client, ClientError};
use crate::error::{CliError, ErrorKind};
use crate::io::CliIo;
use crate::keys::KeyBindings;
use crate::model::PaneInfo;
use crate::open_pane::{OpenPaneRequest, list_panes, login_shell, open_terminal_pane, pane_info};
use crate::view::pane_view::short_display_name;

pub struct AttachLoopOptions {
    pub client: Client,
    pub keys: KeyBindings,
    /// 最初に入るペイン。
    pub initial: PaneInfo,
    pub is_readonly: bool,
    pub replay: Replay,
}

/// 次にすること: 別のペインへ入る / 一覧を出す / 終了コードで終わる。
/// `is_initial` は利用者がコマンドで直接指したペイン。終わっていればエラーにする（一覧や n / p で選んだ先なら一覧へ戻る）。
enum Next {
    Attach { pane: Box<PaneInfo>, is_initial: bool },
    List { header: Vec<String> },
    Exit(i32),
}

pub async fn run_attach_loop(io: &CliIo, opts: AttachLoopOptions) -> Result<i32, CliError> {
    let mut next = Next::Attach {
        pane: Box::new(opts.initial.clone()),
        is_initial: true,
    };
    loop {
        match next {
            Next::Exit(code) => return Ok(code),
            Next::List { header } => {
                let panes = list_panes(&opts.client).await?;
                let options = PickerOptions {
                    header: &header,
                    can_create: !opts.is_readonly,
                };
                match run_picker(io, &panes, options).await {
                    PickerChoice::Quit => return Ok(0),
                    PickerChoice::Signal => {
                        io.err_line("\n[misao] 終了シグナルを受けたため終了しました");
                        return Ok(1);
                    }
                    PickerChoice::New => {
                        let pane = create_shell_pane(io, &opts.client).await?;
                        next = Next::Attach {
                            pane: Box::new(pane),
                            is_initial: false,
                        };
                    }
                    PickerChoice::Pane(pane) => {
                        next = Next::Attach {
                            pane,
                            is_initial: false,
                        }
                    }
                }
            }
            Next::Attach {
                pane: listed,
                is_initial,
            } => {
                let listed = *listed;
                let refreshed = refresh_for_attach(&opts.client, &listed).await?;
                let Some(pane) = refreshed.clone().filter(|p| p.process_state == "running") else {
                    let name = short_display_name(refreshed.as_ref().unwrap_or(&listed), &io.home_dir);
                    if is_initial {
                        return Err(CliError::new(
                            ErrorKind::Runtime,
                            format!("{name} はすでに終了しています"),
                        ));
                    }
                    next = back_to_list(io, &opts.client, format!("[misao] {name} はすでに終了しています"))
                        .await?;
                    continue;
                };
                let end = attach_once(io, &opts, &listed, &pane).await?;
                next = decide_next(io, &opts, &listed, end).await?;
            }
        }
    }
}

async fn create_shell_pane(io: &CliIo, client: &Client) -> Result<PaneInfo, CliError> {
    let request = OpenPaneRequest {
        cmd: login_shell(io.shell.as_deref(), cfg!(windows))?,
        cwd: crate::socket_path::normalize_lexical(&io.cwd),
        labels: Vec::new(),
        env: Vec::new(),
        window_id: None,
    };
    let pane_id = open_terminal_pane(client, &request).await?;
    pane_info(client, &pane_id).await
}

/// 入る直前の状態を取り直す。一覧を出してから選ぶまでの間に閉じられていれば None。
async fn refresh_for_attach(client: &Client, listed: &PaneInfo) -> Result<Option<PaneInfo>, CliError> {
    match pane_info_raw(client, &listed.pane_id).await {
        Ok(pane) => Ok(Some(pane)),
        Err(ClientError::Rpc { code, .. }) if code == ErrorCode::PANE_NOT_FOUND => Ok(None),
        Err(e) => Err(e.into()),
    }
}

async fn pane_info_raw(client: &Client, pane_id: &str) -> Result<PaneInfo, ClientError> {
    let value = client
        .request("pane.info", serde_json::json!({"paneId": pane_id}))
        .await?;
    PaneInfo::from_value(value).map_err(ClientError::Other)
}

/// 入れるペインが残っていれば一覧へ戻り、無ければ知らせて 0 で終わる。
async fn back_to_list(io: &CliIo, client: &Client, message: String) -> Result<Next, CliError> {
    if !attachable_panes(&list_panes(client).await?).is_empty() {
        return Ok(Next::List {
            header: vec![message],
        });
    }
    io.out_line(&message);
    Ok(Next::Exit(0))
}

/// 入る（直前の状態と一覧に出した状態を比べて blocked → working を知らせる）。
async fn attach_once(
    io: &CliIo,
    opts: &AttachLoopOptions,
    listed: &PaneInfo,
    pane: &PaneInfo,
) -> Result<SessionEnd, CliError> {
    if let Some(resumed) = resumed_notice(listed, pane, &io.home_dir) {
        io.err_line(&format!("[misao] {resumed}"));
    }
    run_session(SessionOptions {
        client: &opts.client,
        io,
        pane,
        keys: opts.keys,
        is_readonly: opts.is_readonly,
        replay: opts.replay,
    })
    .await
}

async fn decide_next(
    io: &CliIo,
    opts: &AttachLoopOptions,
    current: &PaneInfo,
    end: SessionEnd,
) -> Result<Next, CliError> {
    let name = short_display_name(current, &io.home_dir);
    match end.reason {
        SessionReason::Failed(error) => Err(error),
        SessionReason::Disconnected => {
            io.err_line("[misao] 接続が切れました");
            Ok(Next::Exit(1))
        }
        SessionReason::Signal => {
            io.err_line("[misao] 終了シグナルを受けたため抜けました");
            Ok(Next::Exit(1))
        }
        SessionReason::Exited { exit_code, signal } => {
            let how = match exit_code {
                Some(code) => format!("終了コード {code}"),
                None => format!("シグナル {}", signal.map_or("?".to_string(), |s| s.to_string())),
            };
            back_to_list(
                io,
                &opts.client,
                format!("[misao] {name} が終了しました（{how}）"),
            )
            .await
        }
        SessionReason::Action(PrefixAction::List) => Ok(Next::List { header: Vec::new() }),
        SessionReason::Action(PrefixAction::Detach) => Ok(Next::List {
            header: vec![format!(
                "[misao] {name} から抜けました（入力 {} 回を記録。内容は保存していません）",
                end.input_count
            )],
        }),
        SessionReason::Action(action) => {
            let panes = attachable_panes(&list_panes(&opts.client).await?);
            Ok(neighbor(
                &panes,
                current,
                if action == PrefixAction::Next { 1 } else { -1 },
            ))
        }
    }
}

/// 一覧（ls と同じ並び）で隣のペインへ。循環する。移動先が自分だけなら一覧へ戻る。
fn neighbor(panes: &[PaneInfo], current: &PaneInfo, step: i64) -> Next {
    let len = i64::try_from(panes.len()).unwrap_or(i64::MAX);
    let target = match panes.iter().position(|p| p.pane_id == current.pane_id) {
        None => panes.first(),
        Some(index) => {
            let at = (i64::try_from(index).unwrap_or(0) + step + len).rem_euclid(len);
            usize::try_from(at).ok().and_then(|i| panes.get(i))
        }
    };
    match target {
        Some(pane) if pane.pane_id != current.pane_id => Next::Attach {
            pane: Box::new(pane.clone()),
            is_initial: false,
        },
        _ => Next::List {
            header: vec!["[misao] 移動できる別のペインがありません".to_string()],
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::testing::make_pane;

    fn ids(next: &Next) -> String {
        match next {
            Next::Attach { pane, .. } => pane.pane_id.clone(),
            Next::List { header } => format!("list:{}", header.join("|")),
            Next::Exit(code) => format!("exit:{code}"),
        }
    }

    #[test]
    fn neighbors_wrap_around_and_a_single_pane_goes_back_to_the_list() {
        let panes: Vec<PaneInfo> = (0..3)
            .map(|i| make_pane(|v| v["paneId"] = json!(format!("p_0000000000000000000000000{i}"))))
            .collect();
        assert_eq!(ids(&neighbor(&panes, &panes[0], 1)), panes[1].pane_id);
        assert_eq!(ids(&neighbor(&panes, &panes[2], 1)), panes[0].pane_id);
        assert_eq!(ids(&neighbor(&panes, &panes[0], -1)), panes[2].pane_id);
        assert_eq!(
            ids(&neighbor(&panes[..1], &panes[0], 1)),
            "list:[misao] 移動できる別のペインがありません"
        );
        let stranger = make_pane(|_| {});
        assert_eq!(ids(&neighbor(&panes, &stranger, 1)), panes[0].pane_id);
        assert!(ids(&neighbor(&[], &stranger, 1)).starts_with("list:"));
    }
}
