//! Minimal client for the sdcompat daemon (used by timers and the CLI).
//! Prefers the private socket, like systemctl does as root.

use zbus::Connection;

use crate::config::Config;

pub async fn connect(cfg: &Config) -> Result<Connection, String> {
    if let Some(sock) = &cfg.private_socket {
        if sock.exists() {
            match tokio::net::UnixStream::connect(sock).await {
                Ok(stream) => {
                    match zbus::connection::Builder::unix_stream(stream).p2p().build().await {
                        Ok(c) => return Ok(c),
                        Err(e) => eprintln!("sdcompat: {}: {e}", sock.display()),
                    }
                }
                Err(e) => eprintln!("sdcompat: {}: {e}", sock.display()),
            }
        }
    }
    let b = match &cfg.bus_address {
        Some(a) => zbus::connection::Builder::address(a.as_str()),
        None => zbus::connection::Builder::system(),
    }
    .map_err(|e| e.to_string())?;
    b.build().await.map_err(|e| format!("connecting to D-Bus: {e}"))
}

pub async fn manager_call<B>(conn: &Connection, method: &str, body: &B) -> Result<zbus::Message, String>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        Some("org.freedesktop.systemd1.Manager"),
        method,
        body,
    )
    .await
    .map_err(|e| e.to_string())
}
