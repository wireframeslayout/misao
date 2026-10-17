/// JSON-RPC のエラーコード（`packages/protocol/src/errors.ts` の `ErrorCode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode;

impl ErrorCode {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const PANE_NOT_FOUND: i64 = 1001;
    pub const PANE_EXITED: i64 = 1002;
    pub const AMBIGUOUS: i64 = 1003;
    pub const UNSUPPORTED: i64 = 1004;
    pub const NOT_IMPLEMENTED: i64 = 1005;
    pub const WORKSPACE_NOT_FOUND: i64 = 1006;
    pub const WINDOW_NOT_FOUND: i64 = 1007;
    pub const ALREADY_EXISTS: i64 = 1008;

    /// スキーマの `errors`（名前 → コード）と突き合わせるための一覧。名前は TypeScript 側の識別子。
    pub const ALL: [(&'static str, i64); 13] = [
        ("Parse", Self::PARSE),
        ("InvalidRequest", Self::INVALID_REQUEST),
        ("MethodNotFound", Self::METHOD_NOT_FOUND),
        ("InvalidParams", Self::INVALID_PARAMS),
        ("Internal", Self::INTERNAL),
        ("PaneNotFound", Self::PANE_NOT_FOUND),
        ("PaneExited", Self::PANE_EXITED),
        ("Ambiguous", Self::AMBIGUOUS),
        ("Unsupported", Self::UNSUPPORTED),
        ("NotImplemented", Self::NOT_IMPLEMENTED),
        ("WorkspaceNotFound", Self::WORKSPACE_NOT_FOUND),
        ("WindowNotFound", Self::WINDOW_NOT_FOUND),
        ("AlreadyExists", Self::ALREADY_EXISTS),
    ];
}
