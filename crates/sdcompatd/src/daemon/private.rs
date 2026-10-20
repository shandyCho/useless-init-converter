//! /run/systemd/private: peer-to-peer D-Bus without a bus daemon.
//!
//! systemctl run as root talks only to this socket (systemd >= 25x no longer
//! falls back to the system bus). We do the SASL EXTERNAL handshake
//! ourselves and authorize by SO_PEERCRED, like systemd does, then hand the
//! authenticated socket to zbus.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use zbus::message::Type as MsgType;
use zbus::MessageStream;

use super::{Daemon, Peer};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub async fn listen(d: Arc<Daemon>, path: PathBuf) {
    let _ = std::fs::remove_file(&path);
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("sdcompatd: cannot listen on {}: {e}", path.display());
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(serve(d.clone(), stream));
            }
            Err(e) => {
                eprintln!("sdcompatd: accept on {}: {e}", path.display());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

async fn read_line(s: &mut UnixStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    loop {
        let b = s.read_u8().await?;
        buf.push(b);
        if buf.ends_with(b"\r\n") {
            buf.truncate(buf.len() - 2);
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        if buf.len() > 16384 {
            return Err(std::io::Error::other("SASL line too long"));
        }
    }
}

async fn handshake(s: &mut UnixStream, guid: &str) -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    if s.read_u8().await.map_err(e)? != 0 {
        return Err("missing leading NUL byte".into());
    }
    let ok = format!("OK {guid}\r\n");
    let mut authed = false;
    loop {
        let line = read_line(s).await.map_err(e)?;
        let mut words = line.split_whitespace();
        let reply: &[u8] = match (words.next(), words.next(), words.next()) {
            (Some("AUTH"), Some("EXTERNAL"), Some(_)) => {
                authed = true;
                ok.as_bytes()
            }
            (Some("AUTH"), Some("EXTERNAL"), None) => b"DATA\r\n",
            (Some("AUTH"), _, _) => b"REJECTED EXTERNAL\r\n",
            (Some("DATA"), _, _) => {
                authed = true;
                ok.as_bytes()
            }
            (Some("NEGOTIATE_UNIX_FD"), _, _) if authed => b"AGREE_UNIX_FD\r\n",
            (Some("BEGIN"), _, _) if authed => return Ok(()),
            (Some("CANCEL"), _, _) | (Some("ERROR"), _, _) => b"REJECTED EXTERNAL\r\n",
            _ => b"ERROR\r\n",
        };
        s.write_all(reply).await.map_err(e)?;
    }
}

async fn serve(d: Arc<Daemon>, mut stream: UnixStream) {
    let Ok(cred) = stream.peer_cred() else { return };
    let uid = cred.uid();
    if uid != 0 && !d.cfg.allow_uids.contains(&uid) {
        return;
    }
    let pid = cred.pid().map(|p| p as u32);
    let guid = zbus::Guid::generate();
    if let Err(e) = handshake(&mut stream, guid.as_str()).await {
        eprintln!("sdcompatd: private socket handshake: {e}");
        return;
    }
    let conn = match zbus::connection::Builder::authenticated_socket(stream, guid)
        .map(|b| b.p2p())
    {
        Ok(b) => match b.build().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("sdcompatd: private connection: {e}");
                return;
            }
        },
        Err(e) => {
            eprintln!("sdcompatd: private connection: {e}");
            return;
        }
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    d.private.lock().unwrap().push((id, conn.clone()));

    let mut msgs = MessageStream::from(&conn);
    while let Some(m) = msgs.next().await {
        let Ok(msg) = m else { break };
        if msg.message_type() == MsgType::MethodCall {
            let peer = Peer { conn: conn.clone(), uid: Some(uid), pid };
            tokio::spawn(d.clone().handle_call(peer, msg));
        }
    }
    d.private.lock().unwrap().retain(|(i, _)| *i != id);
}
