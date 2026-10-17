#![allow(clippy::unwrap_used)]
//! session / loop / picker を偽デーモンに対して動かすテスト（Node 版の `session.test.ts` `loop.test.ts` `picker.test.ts` に対応）。

use std::sync::Arc;

use serde_json::json;

use super::fake_daemon::{FakeDaemon, pane_json};
use super::loop_::{AttachLoopOptions, run_attach_loop};
use super::picker::{PickerChoice, PickerOptions, attachable_panes, resumed_notice, run_picker};
use super::prefix_filter::PrefixAction;
use super::session::{Replay, SessionEnd, SessionOptions, SessionReason, run_session};
use super::tty::TTY_RESET;
use crate::client::Client;
use crate::io::CliSignal;
use crate::io::testing::{TestIo, TestIoOptions, wait_for};
use crate::keys::KeyBindings;
use crate::model::PaneInfo;

const A: &str = "p_01M3AAAAAAAAAAAAAAAAAAAA01";
const B: &str = "p_01M3AAAAAAAAAAAAAAAAAAAA02";
const PREFIX_D: &[u8] = &[0x1e, b'd'];

fn tty_io() -> TestIo {
    TestIo::new(TestIoOptions {
        is_tty: true,
        size: Some((100, 30)),
        shell: Some("/bin/sh".to_string()),
        ..TestIoOptions::default()
    })
}

fn named(id: &str, name: &str) -> serde_json::Value {
    pane_json(id, |v| v["labels"] = json!({"name": name}))
}

async fn session(
    daemon: &FakeDaemon,
    io: &Arc<TestIo>,
    pane: PaneInfo,
    readonly: bool,
) -> tokio::task::JoinHandle<Result<SessionEnd, crate::error::CliError>> {
    let client = Client::connect(&daemon.socket).await.unwrap();
    let io = Arc::clone(io);
    tokio::spawn(async move {
        let result = run_session(SessionOptions {
            client: &client,
            io: &io.io,
            pane: &pane,
            keys: KeyBindings::default(),
            is_readonly: readonly,
            replay: Replay::Snapshot,
        })
        .await;
        client.close();
        result
    })
}

fn assert_restored(io: &TestIo) {
    assert_eq!(io.raw_modes(), vec![true, false]);
    assert!(io.out.text().ends_with(TTY_RESET), "TTY_RESET is written last");
}

#[tokio::test]
async fn input_output_and_size_are_wired_and_prefix_d_detaches() {
    let daemon = FakeDaemon::start(vec![named(A, "セッション")]).await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), false).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    let attach = daemon.calls("pane.attach").remove(0);
    assert_eq!(attach["clientId"], "cli-4242");
    assert_eq!(
        (
            attach["cols"].as_u64(),
            attach["rows"].as_u64(),
            attach["replay"].as_str()
        ),
        (Some(100), Some(30), Some("snapshot"))
    );
    assert!(
        io.err.text().starts_with("[misao] (未登録) セッション · sh · "),
        "{}",
        io.err.text()
    );

    daemon.output(A, b"ready-marker\r\n");
    wait_for("output", || io.out.text().contains("ready-marker")).await;
    io.type_text(b"abc\r");
    wait_for("write", || !daemon.calls("pane.write").is_empty()).await;
    let write = daemon.calls("pane.write").remove(0);
    assert_eq!(
        (write["source"].as_str(), write["clientId"].as_str()),
        (Some("terminal"), Some("cli-4242"))
    );
    assert_eq!(write["dataB64"], "YWJjDQ==");

    io.set_size(120, 40);
    io.signals.emit(CliSignal::WindowChange);
    wait_for("resize", || !daemon.calls("pane.resize").is_empty()).await;
    let resize = daemon.calls("pane.resize").remove(0);
    assert_eq!(
        (resize["cols"].as_u64(), resize["rows"].as_u64()),
        (Some(120), Some(40))
    );

    io.type_text(PREFIX_D);
    let end = end.await.unwrap().unwrap();
    assert!(
        matches!(end.reason, SessionReason::Action(PrefixAction::Detach)),
        "{:?}",
        end.reason
    );
    assert_eq!(end.input_count, 1);
    assert_restored(&io);
    assert_eq!(daemon.calls("pane.detach").len(), 1);
}

#[tokio::test]
async fn a_terminal_reporting_size_zero_attaches_without_a_size() {
    let daemon = FakeDaemon::start(vec![named(A, "zero")]).await;
    let io = Arc::new(TestIo::new(TestIoOptions {
        is_tty: true,
        size: Some((0, 0)),
        ..TestIoOptions::default()
    }));
    let end = session(&daemon, &io, daemon.pane(A), false).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    let attach = daemon.calls("pane.attach").remove(0);
    assert!(attach.get("cols").is_none() && attach.get("rows").is_none());
    io.signals.emit(CliSignal::WindowChange);
    io.type_text(PREFIX_D);
    assert!(matches!(
        end.await.unwrap().unwrap().reason,
        SessionReason::Action(PrefixAction::Detach)
    ));
    assert!(daemon.calls("pane.resize").is_empty());
}

#[tokio::test]
async fn the_prefix_twice_sends_one_prefix_and_other_keys_are_dropped() {
    let daemon = FakeDaemon::start(vec![named(A, "cat")]).await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), false).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    io.type_text(&[0x1e, 0x1e]);
    wait_for("prefix byte", || daemon.calls("pane.write").len() == 1).await;
    assert_eq!(daemon.calls("pane.write")[0]["dataB64"], "Hg==");
    io.type_text(&[0x1e, b'x', 0x1e, b'n']);
    let end = end.await.unwrap().unwrap();
    assert!(matches!(end.reason, SessionReason::Action(PrefixAction::Next)));
    assert_eq!(end.input_count, 1, "the count is the number of pane.write calls");
    assert_restored(&io);
}

#[tokio::test]
async fn readonly_sends_no_input_and_no_resize_but_prefix_actions_work() {
    let daemon = FakeDaemon::start(vec![named(A, "ro")]).await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), true).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    assert!(io.err.text().contains("READONLY"));
    let attach = daemon.calls("pane.attach").remove(0);
    assert!(attach.get("cols").is_none(), "readonly does not announce a size");
    io.type_text(b"typed\r");
    io.set_size(150, 50);
    io.signals.emit(CliSignal::WindowChange);
    io.type_text(PREFIX_D);
    let end = end.await.unwrap().unwrap();
    assert_eq!(end.input_count, 0);
    assert!(daemon.calls("pane.write").is_empty() && daemon.calls("pane.resize").is_empty());
}

#[tokio::test]
async fn an_exit_event_ends_the_session_with_the_exit_code() {
    let daemon = FakeDaemon::start(vec![named(A, "exits")]).await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), false).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    daemon.event(1, "pane.exited", A, &json!({"exitCode": 3, "signal": null}));
    let end = end.await.unwrap().unwrap();
    assert!(
        matches!(
            end.reason,
            SessionReason::Exited {
                exit_code: Some(3),
                signal: None
            }
        ),
        "{:?}",
        end.reason
    );
    assert_restored(&io);
}

#[tokio::test]
async fn a_pane_that_already_exited_ends_right_after_attaching() {
    let daemon = FakeDaemon::start(vec![pane_json(A, |v| {
        v["processState"] = json!("exited");
        v["exitCode"] = json!(0);
    })])
    .await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), false)
        .await
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        end.reason,
        SessionReason::Exited {
            exit_code: Some(0),
            ..
        }
    ));
    assert_restored(&io);
}

#[tokio::test]
async fn a_lost_connection_ends_with_disconnected_and_restores_the_terminal() {
    let daemon = FakeDaemon::start(vec![named(A, "dc")]).await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), false).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    daemon.drop_connections();
    let end = end.await.unwrap().unwrap();
    assert!(
        matches!(end.reason, SessionReason::Disconnected),
        "{:?}",
        end.reason
    );
    assert_restored(&io);
}

#[tokio::test]
async fn a_termination_signal_ends_with_signal_and_restores_the_terminal() {
    let daemon = FakeDaemon::start(vec![named(A, "sig")]).await;
    let io = Arc::new(tty_io());
    let end = session(&daemon, &io, daemon.pane(A), false).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    io.signals.emit(CliSignal::Terminate);
    assert!(matches!(
        end.await.unwrap().unwrap().reason,
        SessionReason::Signal
    ));
    assert_restored(&io);
}

#[tokio::test]
async fn a_rejected_attach_is_an_error_and_the_terminal_is_restored() {
    let daemon = FakeDaemon::start(vec![named(A, "rejected")]).await;
    daemon.reject_attach();
    let io = Arc::new(tty_io());
    let error = session(&daemon, &io, daemon.pane(A), false)
        .await
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.message.contains("not found"), "{}", error.message);
    assert_restored(&io);
    assert!(daemon.calls("pane.detach").is_empty());
}

#[tokio::test]
async fn without_a_terminal_it_fails_before_raw_mode() {
    let daemon = FakeDaemon::start(vec![named(A, "pipe")]).await;
    let io = Arc::new(TestIo::new(TestIoOptions::default()));
    let error = session(&daemon, &io, daemon.pane(A), false)
        .await
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.message.contains("TTY"));
    assert_eq!(io.raw_modes(), Vec::<bool>::new());
}

fn loop_options(client: Client, pane: PaneInfo, readonly: bool) -> AttachLoopOptions {
    AttachLoopOptions {
        client,
        keys: KeyBindings::default(),
        initial: pane,
        is_readonly: readonly,
        replay: Replay::Snapshot,
    }
}

fn banners(io: &TestIo) -> usize {
    io.err
        .text()
        .lines()
        .filter(|l| l.contains("[misao] ") && l.contains("抜ける:"))
        .count()
}

fn prompts(io: &TestIo) -> usize {
    io.out.text().matches("番号で入る").count()
}

async fn start_loop(
    daemon: &FakeDaemon,
    io: &Arc<TestIo>,
    initial: &str,
    readonly: bool,
) -> tokio::task::JoinHandle<Result<i32, crate::error::CliError>> {
    let client = Client::connect(&daemon.socket).await.unwrap();
    let pane = daemon.pane(initial);
    let io = Arc::clone(io);
    tokio::spawn(async move {
        let result = run_attach_loop(&io.io, loop_options(client.clone(), pane, readonly)).await;
        client.close();
        result
    })
}

#[tokio::test]
async fn n_and_p_move_to_neighbours_d_returns_to_the_list_and_q_exits() {
    let daemon = FakeDaemon::start(vec![named(A, "loop-one"), named(B, "loop-two")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("first banner", || banners(&io) == 1).await;
    io.type_text(&[0x1e, b'n']);
    wait_for("second banner", || banners(&io) == 2).await;
    io.type_text(&[0x1e, b'p']);
    wait_for("third banner", || banners(&io) == 3).await;
    io.type_text(&[0x1e, b'd']);
    wait_for("list", || prompts(&io) == 1).await;
    assert!(
        io.out
            .text()
            .contains("から抜けました（入力 0 回を記録。内容は保存していません）")
    );
    io.type_text(b"q\n");
    assert_eq!(end.await.unwrap().unwrap(), 0);
    let modes = io.raw_modes();
    assert_eq!(
        modes.iter().filter(|m| **m).count(),
        modes.iter().filter(|m| !**m).count()
    );
}

#[tokio::test]
async fn l_goes_to_the_list_without_a_farewell_and_a_number_enters_another_pane() {
    let daemon = FakeDaemon::start(vec![named(A, "pick-a"), named(B, "pick-b")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("banner", || banners(&io) == 1).await;
    io.type_text(&[0x1e, b'l']);
    wait_for("list", || prompts(&io) == 1).await;
    assert!(!io.out.text().contains("から抜けました"));
    let out = io.out.text();
    let row = out.lines().find(|l| l.contains("pick-b")).unwrap();
    let number = row.split_whitespace().next().unwrap();
    io.type_text(format!("{number}\n").as_bytes());
    wait_for("second banner", || banners(&io) == 2).await;
    assert!(
        io.err
            .text()
            .lines()
            .rfind(|l| l.contains("抜ける:"))
            .unwrap()
            .contains("pick-b")
    );
    io.type_text(&[0x1e, b'd']);
    wait_for("list again", || prompts(&io) == 2).await;
    io.type_text(b"q\n");
    assert_eq!(end.await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn a_pane_closed_before_it_is_chosen_returns_to_the_list() {
    let daemon = FakeDaemon::start(vec![named(A, "stay"), named(B, "gone-closed")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("banner", || banners(&io) == 1).await;
    io.type_text(&[0x1e, b'l']);
    wait_for("list", || prompts(&io) == 1).await;
    let out = io.out.text();
    let number = out
        .lines()
        .find(|l| l.contains("gone-closed"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    daemon.remove_pane(B);
    io.type_text(format!("{number}\n").as_bytes());
    wait_for("notice", || {
        io.out
            .text()
            .contains("(未登録) gone-closed はすでに終了しています")
    })
    .await;
    wait_for("list again", || prompts(&io) == 2).await;
    io.type_text(b"q\n");
    assert_eq!(
        end.await.unwrap().unwrap(),
        0,
        "a gone choice does not fail the loop"
    );
    assert_eq!(banners(&io), 1);
}

#[tokio::test]
async fn n_in_the_list_creates_a_terminal_shell_and_enters_it() {
    let daemon = FakeDaemon::start(vec![named(A, "base")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("banner", || banners(&io) == 1).await;
    io.type_text(&[0x1e, b'l']);
    wait_for("list", || prompts(&io) == 1).await;
    io.type_text(b"n\n");
    wait_for("second banner", || banners(&io) == 2).await;
    let open = daemon.calls("pane.open").remove(0);
    assert_eq!(open["cmd"], json!(["/bin/sh", "-l"]));
    assert_eq!(open["labels"]["origin"], "terminal");
    io.type_text(&[0x1e, b'd']);
    wait_for("list again", || prompts(&io) == 2).await;
    io.type_text(b"q\n");
    assert_eq!(end.await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn the_only_pane_exiting_ends_with_zero_and_the_exit_code_is_reported() {
    let daemon = FakeDaemon::start(vec![pane_json(A, |v| v["labels"] = json!({"windowId": "77"}))]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("banner", || banners(&io) == 1).await;
    daemon.set_pane(A, |v| {
        v["processState"] = json!("exited");
        v["agentState"] = json!("exited");
    });
    daemon.event(1, "pane.exited", A, &json!({"exitCode": 2, "signal": null}));
    assert_eq!(end.await.unwrap().unwrap(), 0);
    assert!(io.out.text().contains("W-77 が終了しました（終了コード 2）"));
    assert_eq!(io.raw_modes(), vec![true, false]);
}

#[tokio::test]
async fn an_already_exited_initial_pane_is_an_error() {
    let daemon = FakeDaemon::start(vec![pane_json(A, |v| v["processState"] = json!("exited"))]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    let error = end.await.unwrap().unwrap_err();
    assert!(error.message.contains("すでに終了しています"));
    assert_eq!(io.raw_modes(), Vec::<bool>::new());
}

#[tokio::test]
async fn a_lost_connection_exits_with_one() {
    let daemon = FakeDaemon::start(vec![named(A, "dc")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("banner", || banners(&io) == 1).await;
    wait_for("attach", || !daemon.calls("pane.attach").is_empty()).await;
    daemon.drop_connections();
    assert_eq!(end.await.unwrap().unwrap(), 1);
    assert!(io.err.text().contains("接続が切れました"));
    assert_eq!(io.raw_modes(), vec![true, false]);
}

#[tokio::test]
async fn a_termination_signal_while_waiting_in_the_list_exits_with_one() {
    let daemon = FakeDaemon::start(vec![named(A, "sig")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, false).await;
    wait_for("banner", || banners(&io) == 1).await;
    io.type_text(&[0x1e, b'd']);
    wait_for("list", || prompts(&io) == 1).await;
    io.signals.emit(CliSignal::Terminate);
    assert_eq!(end.await.unwrap().unwrap(), 1);
    assert!(io.err.text().contains("終了シグナルを受けたため終了しました"));
}

#[tokio::test]
async fn the_readonly_list_has_no_new_pane_option() {
    let daemon = FakeDaemon::start(vec![named(A, "ro")]).await;
    let io = Arc::new(tty_io());
    let end = start_loop(&daemon, &io, A, true).await;
    wait_for("banner", || banners(&io) == 1).await;
    io.type_text(&[0x1e, b'd']);
    wait_for("list", || prompts(&io) == 1).await;
    assert!(!io.out.text().contains("n で新しいペイン"));
    io.type_text(b"q\n");
    assert_eq!(end.await.unwrap().unwrap(), 0);
}

fn pane_with(edit: impl FnOnce(&mut serde_json::Value)) -> PaneInfo {
    crate::model::testing::make_pane(edit)
}

#[tokio::test]
async fn the_picker_lists_attachable_panes_and_reads_a_choice() {
    let live = pane_with(|v| v["labels"] = json!({"name": "alive"}));
    let dead = pane_with(|v| {
        v["processState"] = json!("exited");
        v["agentState"] = json!("exited");
    });
    let stopped = pane_with(|v| v["processState"] = json!("stopped"));
    let all = [live.clone(), dead, stopped];
    assert_eq!(attachable_panes(&all).len(), 1);

    let io = TestIo::new(TestIoOptions::default());
    let header = vec!["header".to_string()];
    io.type_text(b"x\n0\n2\n1\n");
    let choice = run_picker(
        &io.io,
        &all,
        PickerOptions {
            header: &header,
            can_create: true,
        },
    )
    .await;
    assert_eq!(choice, PickerChoice::Pane(Box::new(live)));
    let out = io.out.text();
    assert!(out.starts_with("header\n  #  STATE"), "{out}");
    assert!(out.contains("番号で入る · n で新しいペイン · q で終了"));
    assert_eq!(
        io.err.text().matches("のいずれかを入力してください").count(),
        3,
        "x, 0, 2 are rejected"
    );
}

#[tokio::test]
async fn the_picker_handles_q_n_eof_and_the_readonly_guide() {
    let pane = pane_with(|_| {});
    let new = TestIo::new(TestIoOptions::default());
    new.type_text(b"N\n");
    assert_eq!(
        run_picker(
            &new.io,
            std::slice::from_ref(&pane),
            PickerOptions {
                header: &[],
                can_create: true
            }
        )
        .await,
        PickerChoice::New
    );
    let quit = TestIo::new(TestIoOptions::default());
    quit.type_text(b" Q \n");
    assert_eq!(
        run_picker(
            &quit.io,
            std::slice::from_ref(&pane),
            PickerOptions {
                header: &[],
                can_create: false
            }
        )
        .await,
        PickerChoice::Quit
    );
    assert!(!quit.out.text().contains("n で新しいペイン"));
    let eof = TestIo::new(TestIoOptions::default());
    drop(eof.stdin.clone());
    let empty = run_picker(
        &eof.io,
        &[],
        PickerOptions {
            header: &[],
            can_create: true,
        },
    );
    eof.type_text(b"n\n");
    assert_eq!(empty.await, PickerChoice::New);
    assert!(eof.out.text().contains("[misao] 入れるペインがありません"));
}

#[tokio::test]
async fn the_picker_returns_signal_when_terminated_while_waiting() {
    let io = Arc::new(TestIo::new(TestIoOptions::default()));
    let waiting = {
        let io = Arc::clone(&io);
        tokio::spawn(async move {
            run_picker(
                &io.io,
                &[],
                PickerOptions {
                    header: &[],
                    can_create: true,
                },
            )
            .await
        })
    };
    wait_for("prompt", || io.err.text().contains("> ")).await;
    io.signals.emit(CliSignal::Interrupt);
    assert_eq!(waiting.await.unwrap(), PickerChoice::Signal);
}

#[test]
fn a_blocked_pane_that_resumed_is_announced() {
    let blocked = pane_with(|v| {
        v["agentState"] = json!("blocked");
        v["labels"] = json!({"windowId": "806"});
    });
    let working = pane_with(|v| {
        v["agentState"] = json!("working");
        v["labels"] = json!({"windowId": "806"});
    });
    assert_eq!(
        resumed_notice(&blocked, &working, "/h").as_deref(),
        Some("W-806 はいま再開しました（blocked → working）")
    );
    assert_eq!(resumed_notice(&working, &working, "/h"), None);
    assert_eq!(resumed_notice(&blocked, &blocked, "/h"), None);
}
