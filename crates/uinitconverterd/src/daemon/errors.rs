//! D-Bus error names and the daemon's error type.

pub(super) const E_NO_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
pub(super) const E_NO_JOB: &str = "org.freedesktop.systemd1.NoSuchJob";
pub(super) const E_MASKED: &str = "org.freedesktop.systemd1.UnitMasked";
pub(super) const E_LOAD_FAILED: &str = "org.freedesktop.systemd1.LoadFailed";
pub(super) const E_NOT_APPLICABLE: &str = "org.freedesktop.systemd1.JobTypeNotApplicable";
pub(super) const E_INACTIVE: &str = "org.freedesktop.systemd1.UnitInactive";
pub(super) const E_ONLY_BY_DEP: &str = "org.freedesktop.systemd1.OnlyByDependency";
pub(super) const E_NO_UNIT_FOR_PID: &str = "org.freedesktop.systemd1.NoUnitForPID";
pub(super) const E_DESTRUCTIVE: &str = "org.freedesktop.systemd1.TransactionIsDestructive";
pub(super) const E_ACCESS: &str = "org.freedesktop.DBus.Error.AccessDenied";
pub(super) const E_UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
pub(super) const E_UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";
pub(super) const E_UNKNOWN_PROPERTY: &str = "org.freedesktop.DBus.Error.UnknownProperty";
pub(super) const E_INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
pub(super) const E_READ_ONLY: &str = "org.freedesktop.DBus.Error.PropertyReadOnly";
pub(super) const E_NOT_SUPPORTED: &str = "org.freedesktop.DBus.Error.NotSupported";
pub(super) const E_FAILED: &str = "org.freedesktop.DBus.Error.Failed";

pub(super) struct DErr {
    pub(super) name: &'static str,
    pub(super) msg: String,
}

pub(super) type R<T> = Result<T, DErr>;

pub(super) fn err(name: &'static str, msg: impl Into<String>) -> DErr {
    DErr {
        name,
        msg: msg.into(),
    }
}

pub(super) fn zerr(e: zbus::Error) -> DErr {
    err(E_FAILED, e.to_string())
}
