use serde_json::{Map, Value, json};

use super::CommandContext;
use super::shared::{expect_positionals, write_json};
use crate::connect::connect_daemon;
use crate::error::{CliError, ErrorKind};

pub const USAGE: &str = "misao status [--json]";

fn format_uptime(total_sec: u64) -> String {
    let (h, m, s) = (total_sec / 3600, (total_sec % 3600) / 60, total_sec % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if h > 0 || m > 0 {
        parts.push(format!("{m}m"));
    }
    parts.push(format!("{s}s"));
    parts.join(" ")
}

fn text(info: &Value, key: &str) -> String {
    match info.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

pub async fn run(ctx: CommandContext<'_>) -> Result<i32, CliError> {
    let CommandContext {
        args,
        io,
        is_json,
        socket,
        ..
    } = ctx;
    expect_positionals(&args, 0, 0, USAGE)?;
    let socket_text = socket.to_string_lossy().into_owned();
    let client = match connect_daemon(&socket).await {
        Ok(client) => client,
        Err(e) if e.kind == ErrorKind::DaemonUnreachable => {
            if is_json {
                write_json(io, &json!({"running": false, "socket": socket_text}));
            } else {
                io.out_line(&format!("[misao] 停止中（{socket_text}）"));
            }
            return Ok(1);
        }
        Err(e) => return Err(e),
    };
    let info = client.request("server.info", json!({})).await;
    client.close();
    let info = info?;
    if is_json {
        let mut body = Map::new();
        body.insert("running".into(), json!(true));
        if let Some(fields) = info.as_object() {
            body.extend(fields.clone());
        }
        body.insert("socket".into(), json!(socket_text));
        write_json(io, &Value::Object(body));
        return Ok(0);
    }
    let uptime = info.get("uptimeSec").and_then(Value::as_u64).unwrap_or(0);
    io.out_line("[misao] 稼働中");
    io.out_line(&format!("  プロトコル  {}", text(&info, "protocolVersion")));
    io.out_line(&format!("  pid         {}", text(&info, "pid")));
    io.out_line(&format!("  epoch       {}", text(&info, "epoch")));
    io.out_line(&format!("  稼働時間    {}", format_uptime(uptime)));
    io.out_line(&format!("  ペイン数    {}", text(&info, "paneCount")));
    io.out_line(&format!("  ソケット    {socket_text}"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::format_uptime;

    #[test]
    fn uptime_drops_leading_zero_units() {
        assert_eq!(format_uptime(5), "5s");
        assert_eq!(format_uptime(65), "1m 5s");
        assert_eq!(format_uptime(3600), "1h 0m 0s");
        assert_eq!(format_uptime(3725), "1h 2m 5s");
    }
}
