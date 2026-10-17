/// プロトコルの全メソッド名（`methods` の宣言順）。デーモンは、ここに無い名前を MethodNotFound、
/// あっても未実装なら NotImplemented にする。
pub const METHODS: [&str; 24] = [
    "server.info",
    "server.schema",
    "workspace.list",
    "workspace.create",
    "workspace.close",
    "workspace.rename",
    "window.create",
    "window.close",
    "window.rename",
    "window.focus",
    "pane.open",
    "pane.info",
    "pane.list",
    "pane.write",
    "pane.send_keys",
    "pane.resize",
    "pane.screen",
    "pane.set_label",
    "pane.attach",
    "pane.detach",
    "pane.subscribe_lines",
    "pane.close",
    "pane.respawn",
    "events.subscribe",
];

/// サーバーから届く通知の名前。
pub const NOTIFICATIONS: [&str; 3] = ["pane.output", "pane.line", "event"];
