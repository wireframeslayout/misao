//! misao のワイヤプロトコル（NDJSON JSON-RPC 2.0）の Rust 型。
//!
//! 正本は TypeScript の `@misao/protocol`（Zod スキーマ）。ここの型が食い違わないことは
//! `tests/schema_conformance.rs` が `schema/protocol.json`（Zod から書き出した JSON Schema）で確かめる。

mod error;
mod framing;
mod jsonrpc;
mod names;
mod server;

pub use error::ErrorCode;
pub use framing::{DEFAULT_MAX_LINE_BYTES, LineError, LineSplitter, encode_message};
pub use jsonrpc::{ErrorBody, Incoming, Request, RequestId, Response, parse_incoming};
pub use names::{METHODS, NOTIFICATIONS};
pub use server::ServerInfo;

/// プロトコルバージョン（semver）。`packages/protocol/src/version.ts` と一致させる。
pub const PROTOCOL_VERSION: &str = "0.3.0";

/// `server.schema` の結果（プロトコル全体の JSON Schema）。`@misao/protocol` から書き出したもの。
pub const PROTOCOL_SCHEMA: &str = include_str!("../schema/protocol.json");
