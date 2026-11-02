//! Who is calling: message arguments and replies, credentials, authorization.

use super::*;

impl Daemon {
    pub(super) fn args<T>(msg: &Message) -> R<T>
    where
        T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        msg.body()
            .deserialize::<T>()
            .map_err(|e| err(E_INVALID_ARGS, e.to_string()))
    }

    pub(super) async fn reply<B>(&self, c: &Peer, msg: &Message, body: &B) -> R<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        c.conn.reply(&msg.header(), body).await.map_err(zerr)
    }

    pub(super) async fn caller_uid(&self, c: &Peer, msg: &Message) -> Option<u32> {
        if c.uid.is_some() {
            return c.uid;
        }
        let hdr = msg.header();
        let sender = hdr.sender()?.to_string();
        if let Some(u) = self.uid_cache.lock().unwrap().get(&sender) {
            return Some(*u);
        }
        let proxy = zbus::fdo::DBusProxy::new(&c.conn).await.ok()?;
        let name = zbus::names::BusName::try_from(sender.clone()).ok()?;
        let uid = proxy.get_connection_unix_user(name).await.ok()?;
        let mut cache = self.uid_cache.lock().unwrap();
        // Unique names are never reused, but they pile up: keep it bounded.
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(sender, uid);
        Some(uid)
    }

    pub(super) async fn caller_pid(&self, c: &Peer, msg: &Message) -> Option<u32> {
        if c.pid.is_some() {
            return c.pid;
        }
        let hdr = msg.header();
        let sender = hdr.sender()?.to_string();
        let proxy = zbus::fdo::DBusProxy::new(&c.conn).await.ok()?;
        let name = zbus::names::BusName::try_from(sender).ok()?;
        proxy.get_connection_unix_process_id(name).await.ok()
    }

    pub(super) async fn require_privileged(&self, c: &Peer, msg: &Message) -> R<()> {
        self.authorize_with(
            c,
            msg,
            "org.freedesktop.systemd1.manage-units",
            HashMap::new(),
        )
        .await
    }

    /// manage-units for one unit, with the details systemd passes to polkit:
    /// rules such as `action.lookup("unit") == "foo.service"` match on them.
    pub(super) async fn require_unit(
        &self,
        c: &Peer,
        msg: &Message,
        unit: &str,
        verb: &str,
    ) -> R<()> {
        let details = HashMap::from([
            ("unit".to_string(), unit.to_string()),
            ("verb".to_string(), verb.to_string()),
            (
                "polkit.message".to_string(),
                format!("Authentication is required to {verb} '{unit}'."),
            ),
            ("polkit.gettext_domain".to_string(), "systemd".to_string()),
        ]);
        self.authorize_with(c, msg, "org.freedesktop.systemd1.manage-units", details)
            .await
    }

    pub(super) async fn authorize(&self, c: &Peer, msg: &Message, action: &str) -> R<()> {
        self.authorize_with(c, msg, action, HashMap::new()).await
    }

    /// Root and ALLOW_UIDS pass; other callers on the bus are checked with
    /// polkit using systemd's action IDs (the policy ships with systemd).
    pub(super) async fn authorize_with(
        &self,
        c: &Peer,
        msg: &Message,
        action: &str,
        details: HashMap<String, String>,
    ) -> R<()> {
        match self.caller_uid(c, msg).await {
            Some(0) => return Ok(()),
            Some(u) if self.cfg.allow_uids.contains(&u) => return Ok(()),
            None => return Err(err(E_ACCESS, "Access denied")),
            Some(_) => {}
        }
        // Peer-to-peer callers are root-only; polkit needs a bus name.
        let hdr = msg.header();
        let Some(sender) = hdr
            .sender()
            .map(|s| s.to_string())
            .filter(|_| c.uid.is_none())
        else {
            return Err(err(E_ACCESS, "Access denied"));
        };
        let interactive = hdr
            .primary()
            .flags()
            .contains(zbus::message::Flags::AllowInteractiveAuth);
        drop(hdr);
        let mut subject_details: HashMap<String, Value<'static>> = HashMap::new();
        subject_details.insert("name".into(), Value::from(sender));
        let subject = ("system-bus-name".to_string(), subject_details);
        let reply = c
            .conn
            .call_method(
                Some("org.freedesktop.PolicyKit1"),
                "/org/freedesktop/PolicyKit1/Authority",
                Some("org.freedesktop.PolicyKit1.Authority"),
                "CheckAuthorization",
                &(subject, action, details, u32::from(interactive), ""),
            )
            .await;
        let Ok(reply) = reply else {
            // No polkit: fail closed.
            return Err(err(E_ACCESS, "Access denied"));
        };
        match reply
            .body()
            .deserialize::<((bool, bool, HashMap<String, String>),)>()
        {
            Ok(((true, _, _),)) => Ok(()),
            Ok(((false, true, _),)) => Err(err(
                "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
                "Interactive authentication required.",
            )),
            _ => Err(err(E_ACCESS, "Access denied")),
        }
    }
}
