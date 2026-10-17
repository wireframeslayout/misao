#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use super::*;
use crate::connection::ConnHandle;

impl Pane {
    /// 生出力リングの `(oldest, head)`。
    fn raw_bounds(&self) -> (u64, u64) {
        let inner = lock(&self.inner);
        (inner.raw.oldest(), inner.raw.head())
    }

    fn raw_bytes(&self) -> usize {
        let inner = lock(&self.inner);
        (inner.raw.oldest()..=inner.raw.head())
            .filter_map(|seq| inner.raw.entry(seq).map(|e| e.item.len()))
            .sum()
    }
}

#[test]
fn the_raw_ring_keeps_only_the_newest_bytes_up_to_its_capacity() {
    let socket = PathBuf::from("/tmp/unused.sock");
    let pane = Pane::spawn(PaneOpenOptions {
        cmd: vec![
            "sh".into(),
            "-c".into(),
            "head -c 20000 /dev/zero | tr '\\0' x".into(),
        ],
        cwd: None,
        env: BTreeMap::new(),
        ephemeral_env: BTreeMap::new(),
        cols: None,
        rows: None,
        socket_path: &socket,
        raw_ring_bytes: 4096,
        lines_ring_bytes: 1024,
        scrollback: 100,
        profiles: Vec::new(),
        events: Arc::new(EventLog::new(10)),
    })
    .unwrap();
    pane.start();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pane.state() != RunState::Exited {
        assert!(Instant::now() < deadline, "the pane did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    let (oldest, head) = pane.raw_bounds();
    assert!(
        oldest > 1 && head >= oldest,
        "old chunks were evicted: {oldest}..{head}"
    );
    // 直近のチャンクは、容量を超えていても 1 つは残る。それ以外は容量に収まる。
    assert!(pane.raw_bytes() <= 4096 + READ_CHUNK);
}

/// リーダーが自然に終了し、SIGHUP を無視する子が残った pane。
fn pane_with_a_survivor(dir: &Path) -> (Arc<Pane>, PathBuf) {
    let (pid_file, ready) = (dir.join("survivor.pid"), dir.join("ready"));
    let script = format!(
        "(trap '' HUP; touch {ready}; exec sleep 300) & pid=$!; while [ ! -e {ready} ]; do sleep 0.05; done; echo $pid > {pid}",
        ready = ready.display(),
        pid = pid_file.display()
    );
    let socket = PathBuf::from("/tmp/unused.sock");
    let pane = Pane::spawn(PaneOpenOptions {
        cmd: vec!["sh".into(), "-c".into(), script],
        cwd: None,
        env: BTreeMap::new(),
        ephemeral_env: BTreeMap::new(),
        cols: None,
        rows: None,
        socket_path: &socket,
        raw_ring_bytes: 1024,
        lines_ring_bytes: 1024,
        scrollback: 100,
        profiles: Vec::new(),
        events: Arc::new(EventLog::new(10)),
    })
    .unwrap();
    pane.start();
    (pane, pid_file)
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_input_channel_is_closed_when_the_child_exits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (pane, pid_file) = pane_with_a_survivor(tmp.path());
    wait_for("the pane to exit", || pane.state() == RunState::Exited);
    assert!(std::fs::read_to_string(pid_file).is_ok());
    wait_for("the input channel to be released", || lock(&pane.input).is_none());
    pane.tree.kill();
}

#[test]
fn the_input_queue_refuses_input_beyond_its_limit_but_accepts_one_into_an_empty_queue() {
    let socket = PathBuf::from("/tmp/unused.sock");
    let pane = Pane::spawn(PaneOpenOptions {
        cmd: vec!["sleep".into(), "30".into()],
        cwd: None,
        env: BTreeMap::new(),
        ephemeral_env: BTreeMap::new(),
        cols: None,
        rows: None,
        socket_path: &socket,
        raw_ring_bytes: 1024,
        lines_ring_bytes: 1024,
        scrollback: 100,
        profiles: Vec::new(),
        events: Arc::new(EventLog::new(10)),
    })
    .unwrap();
    // 待ち行列が空なら、上限より大きい入力も受ける。
    assert!(pane.check_input_room(INPUT_QUEUE_BYTES * 2).is_ok());
    // 書き込みのスレッドが詰まっている状況（start せず、送り口だけがある）で溜める。
    let (tx, _rx) = mpsc::channel();
    *lock(&pane.input) = Some(tx);
    assert!(pane.write(vec![0; INPUT_QUEUE_BYTES - 10]).is_ok());
    assert!(pane.write(vec![0; 10]).is_ok(), "exactly at the limit");
    assert_eq!(pane.write(vec![0; 1]), Err(INPUT_QUEUE_BYTES));
    assert_eq!(pane.check_input_room(1), Err(INPUT_QUEUE_BYTES));
    pane.tree.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_after_a_natural_exit_releases_the_threads_and_the_master() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (pane, _) = pane_with_a_survivor(tmp.path());
    wait_for("the pane to exit", || pane.state() == RunState::Exited);
    // 孫が pty を握っているので、reader はまだ EOF になっていない（Pane への参照を持ち続ける）。
    let weak = Arc::downgrade(&pane);
    pane.close().await;
    assert_eq!(pane.tree.group_state(), GroupState::Empty);
    drop(pane);
    // reader / 待機のスレッドが終われば、最後の参照（= master と writer）が解放される。
    wait_for("the pane to be freed", || weak.upgrade().is_none());
}

/// プロセスの状態文字（`Z` はゾンビ）。プロセスが無ければ None。Linux は `/proc/<pid>/stat`、macOS は `ps -o stat=`。
#[cfg(target_os = "linux")]
fn proc_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.trim().chars().next()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn proc_state(pid: u32) -> Option<char> {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().chars().next()
}

/// 残りの子が pty を握ったまま、close も shutdown もせずに Pane を捨てても、reader が Pane を握り続けないので、
/// `Drop` が走って終了済みのリーダーが回収される。
#[test]
fn dropping_a_pane_without_close_reaps_the_exited_leader_even_while_a_child_holds_the_pty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (pane, pid_file) = pane_with_a_survivor(tmp.path());
    wait_for("the pane to exit", || pane.state() == RunState::Exited);
    let leader = pane.pid;
    let survivor: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(proc_state(leader), Some('Z'), "held as a zombie until reaped");
    drop(pane);
    wait_for("the leader to be reaped", || proc_state(leader).is_none());
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(survivor).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_and_repeated_closes_clean_up_once_and_the_later_ones_just_wait() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (pane, _) = pane_with_a_survivor(tmp.path());
    wait_for("the pane to exit", || pane.state() == RunState::Exited);
    let leader = pane.pid;
    tokio::join!(pane.close(), pane.close());
    assert_eq!(proc_state(leader), None, "reaped by the single clean-up");
    assert!(pane.reaped.load(Ordering::Acquire));
    // 回収後の close は、グループに触れずにすぐ戻る（pid が再利用されていても害がない）。
    let started = Instant::now();
    pane.close().await;
    assert!(started.elapsed() < Duration::from_millis(500));
}

/// close の途中で呼び出し側が abort されても（デーモンの終了で接続のタスクが abort される）、片付けは続く。
/// 後から close（shutdown）しても、SIGHUP を送り直さず、猶予も数え直さない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_close_does_not_restart_the_clean_up_for_the_next_close() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (hups, ready) = (tmp.path().join("hups"), tmp.path().join("ready"));
    // SIGHUP を数えて無視する。SIGKILL まで生き続ける。
    let script = format!(
        "trap 'echo hup >> {hups}' HUP; touch {ready}; while true; do sleep 0.05; done",
        hups = hups.display(),
        ready = ready.display()
    );
    let socket = PathBuf::from("/tmp/unused.sock");
    let pane = Pane::spawn(PaneOpenOptions {
        cmd: vec!["sh".into(), "-c".into(), script],
        cwd: None,
        env: BTreeMap::new(),
        ephemeral_env: BTreeMap::new(),
        cols: None,
        rows: None,
        socket_path: &socket,
        raw_ring_bytes: 1024,
        lines_ring_bytes: 1024,
        scrollback: 100,
        profiles: Vec::new(),
        events: Arc::new(EventLog::new(10)),
    })
    .unwrap();
    pane.start();
    wait_for("the trap", || ready.exists());
    let started = Instant::now();
    let first = tokio::spawn({
        let pane = Arc::clone(&pane);
        async move { pane.close().await }
    });
    wait_for("the first SIGHUP", || {
        std::fs::read_to_string(&hups).is_ok_and(|s| s.lines().count() == 1)
    });
    first.abort();
    let _ = first.await;
    pane.close().await; // shutdown 相当
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(2),
        "the grace period is honoured: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(5500),
        "the grace period is not counted again: {elapsed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&hups).unwrap().lines().count(),
        1,
        "SIGHUP exactly once"
    );
    assert_eq!(pane.state(), RunState::Exited);
    assert_eq!(pane.tree.group_state(), GroupState::Empty);
}

fn quiet_pane() -> Arc<Pane> {
    let socket = PathBuf::from("/tmp/unused.sock");
    Pane::spawn(PaneOpenOptions {
        cmd: vec!["sleep".into(), "30".into()],
        cwd: None,
        env: BTreeMap::new(),
        ephemeral_env: BTreeMap::new(),
        cols: Some(10),
        rows: Some(3),
        socket_path: &socket,
        raw_ring_bytes: 1024,
        lines_ring_bytes: 1024,
        scrollback: 10,
        profiles: Vec::new(),
        events: Arc::new(EventLog::new(10)),
    })
    .unwrap()
}

/// 出力の取り込みと resize は、受け取った順に画面へ反映される（取り込み中に resize が割り込まない）。
#[test]
fn a_resize_waits_for_the_output_being_ingested_and_the_output_waits_for_a_resize() {
    let pane = quiet_pane();
    let done = Arc::new(AtomicBool::new(false));
    let order = lock(&pane.order);
    let resizing = {
        let (pane, done) = (Arc::clone(&pane), Arc::clone(&done));
        std::thread::spawn(move || {
            pane.resize(20, 3, None).unwrap();
            done.store(true, Ordering::Release);
        })
    };
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !done.load(Ordering::Acquire),
        "resize must not overtake the output being ingested"
    );
    drop(order);
    resizing.join().unwrap();
    assert_eq!(pane.size(), (20, 3));

    // 逆に、resize の途中では出力の取り込みも待つ
    let order = lock(&pane.order);
    let ingesting = {
        let pane = Arc::clone(&pane);
        std::thread::spawn(move || pane.on_output(b"x"))
    };
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        pane.screen().text.trim(),
        "",
        "the output must wait for the ordering lock"
    );
    drop(order);
    ingesting.join().unwrap();
    assert_eq!(pane.screen().text.trim(), "x");
    pane.tree.kill();
}

/// 出力と resize を別のスレッドから並行させても、画面は壊れない（順序そのものは上のテストが保証する）。
#[test]
fn concurrent_output_and_resize_leave_a_consistent_screen() {
    for _ in 0..30 {
        let pane = quiet_pane();
        let (a, b) = (Arc::clone(&pane), Arc::clone(&pane));
        let writer = std::thread::spawn(move || {
            for _ in 0..20 {
                a.on_output(b"0123456789abcdef\r\n");
            }
        });
        let resizer = std::thread::spawn(move || {
            for cols in [4_u64, 30, 6, 12] {
                b.resize(cols, 3, None).unwrap();
            }
        });
        writer.join().unwrap();
        resizer.join().unwrap();
        let text = pane.screen().text;
        assert_eq!(text.split('\n').count(), 3);
        pane.tree.kill();
    }
}

/// pane 側に登録した後・接続側に登録する前で attach が止まっている間に close が始まっても、
/// 両側の解除は線形化される: close は attach の完了を待ち、接続側にも pane 側にも何も残らない。
#[test]
fn a_close_waits_for_an_attach_paused_between_the_pane_side_and_the_connection_side() {
    let pane = quiet_pane();
    let (conn, _rx) = ConnHandle::new();
    let arrived = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    pane.pause_next_attach(Arc::clone(&arrived), Arc::clone(&release));
    let attacher = {
        let (pane, conn) = (Arc::clone(&pane), Arc::clone(&conn));
        std::thread::spawn(move || pane.attach(&conn, "A", crate::params::Replay::None, None).is_ok())
    };
    arrived.wait(); // attach は pane 側に登録済みで、order を持ったまま止まっている
    let closer = {
        let pane = Arc::clone(&pane);
        std::thread::spawn(move || pane.close_attachments().len())
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        conn.attachment(&pane.id).is_none(),
        "the connection side is not registered yet"
    );
    release.wait(); // attach を再開する
    assert!(
        attacher.join().unwrap(),
        "the attach that started before the close completes"
    );
    assert_eq!(
        closer.join().unwrap(),
        1,
        "the close released it on the pane side"
    );
    assert!(conn.attachment(&pane.id).is_none(), "and on the connection side");
    assert!(!pane.has_attachment_for("A"));
    // close の後の attach は断る
    assert!(
        pane.attach(&conn, "B", crate::params::Replay::None, None)
            .is_err()
    );
    assert!(conn.attachment(&pane.id).is_none());
}

/// 断られた attach は、同じ接続・同じ clientId の古い attach を両側に残す。
#[test]
fn a_refused_attach_leaves_the_old_attachment_on_both_sides() {
    let pane = quiet_pane();
    let (conn, _rx) = ConnHandle::new();
    let first = pane
        .attach(&conn, "A", crate::params::Replay::None, None)
        .unwrap();
    pane.refuse_attach();
    assert!(
        pane.attach(&conn, "A", crate::params::Replay::None, None)
            .is_err()
    );
    assert_eq!(conn.attachment(&pane.id).map(|a| a.id), Some(first.attachment.id));
    assert!(pane.has_attachment_for("A"));
}

/// 同じ接続で別の clientId が attach すると、古い方は両側から外れ、クライアントとサイズの調停からも外れる。
#[test]
fn attaching_with_another_client_id_on_the_same_connection_displaces_the_old_client() {
    let pane = quiet_pane();
    let (conn, _rx) = ConnHandle::new();
    pane.attach(&conn, "A", crate::params::Replay::None, None)
        .unwrap();
    let second = pane
        .attach(&conn, "B", crate::params::Replay::None, None)
        .unwrap();
    assert_eq!(second.displaced.as_ref().map(|d| d.client_id.as_str()), Some("A"));
    assert!(!second.replaced);
    assert!(!pane.has_attachment_for("A") && pane.has_attachment_for("B"));
    assert_eq!(
        conn.attachment(&pane.id).map(|a| a.id),
        Some(second.attachment.id)
    );
}

/// close が始まった後の attach はサイズも変えない（サイズ・所有者・画面とも元のまま）。
#[test]
fn an_attach_refused_because_of_a_close_does_not_resize_the_pane() {
    let pane = quiet_pane();
    let (conn, _rx) = ConnHandle::new();
    pane.refuse_attach();
    let refused = pane.attach(&conn, "A", crate::params::Replay::Snapshot, Some((50, 20)));
    assert!(matches!(refused, Err(crate::pane::AttachRefused::Closing)));
    assert_eq!(pane.size(), (10, 3));
    assert!(!pane.has_attachment_for("A"));
    assert_eq!(
        pane.screen().text.split('\n').count(),
        3,
        "the screen was not resized"
    );
    // 成功したときだけサイズが変わる
    let open = quiet_pane();
    let attached = open
        .attach(&conn, "A", crate::params::Replay::None, Some((50, 20)))
        .unwrap();
    assert!(attached.resized);
    assert_eq!(open.size(), (50, 20));
}
