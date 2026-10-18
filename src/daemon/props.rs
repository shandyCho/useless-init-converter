//! D-Bus property tables for the Manager, Unit, Service, Target, Scope and Job
//! objects, with systemd's names and signatures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use zbus::zvariant::{ObjectPath, Value};

use super::s6::Status;
use super::{job_path, unit_path, Daemon};
use crate::cgroup;
use crate::conditions;
use crate::execcmd;
use crate::install;
use crate::loader::{Loader, Unit};
use crate::state::{self, keys};
use crate::unit::parse_timespan_usec;
use crate::util;

pub type Props = BTreeMap<String, Value<'static>>;
/// interface name -> properties
pub type Ifaces = BTreeMap<String, Props>;

pub const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
pub const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";

fn s(v: impl Into<String>) -> Value<'static> {
    Value::from(v.into())
}

fn strv(v: Vec<String>) -> Value<'static> {
    Value::from(v)
}

fn op(p: String) -> ObjectPath<'static> {
    ObjectPath::try_from(p).unwrap_or_else(|_| ObjectPath::try_from("/").unwrap())
}

fn timespan(unit: &Unit, sec: &str, keys_: &[&str], default: u64) -> u64 {
    keys_
        .iter()
        .find_map(|k| unit.get(sec, k))
        .and_then(|v| parse_timespan_usec(&v, 1_000_000))
        .unwrap_or(default)
}

impl Daemon {
    pub(super) fn manager_props(&self, nfailed: u32, nnames: u32) -> Props {
        let mut p = Props::new();
        let njobs = self.jobs.lock().unwrap().len() as u32;
        let state = if njobs > 0 && !self.boot_done.load(std::sync::atomic::Ordering::Relaxed) {
            "starting"
        } else if nfailed > 0 {
            "degraded"
        } else {
            "running"
        };
        p.insert("Version".into(), s(format!("{} (sdcompat)", super::COMPAT_VERSION)));
        p.insert("Features".into(), s("-PAM -AUDIT +SELINUX -APPARMOR -SECCOMP sdcompat"));
        p.insert("Virtualization".into(), s(match conditions::detect_virtualization() {
            "none" => "",
            v => v,
        }));
        p.insert("ConfidentialVirtualization".into(), s(""));
        p.insert("Architecture".into(), s(crate::loader::systemd_arch()));
        p.insert("Tainted".into(), s(""));
        for k in [
            "FirmwareTimestamp", "FirmwareTimestampMonotonic", "LoaderTimestamp",
            "LoaderTimestampMonotonic", "KernelTimestamp", "KernelTimestampMonotonic",
            "InitRDTimestamp", "InitRDTimestampMonotonic", "SecurityStartTimestamp",
            "SecurityStartTimestampMonotonic", "SecurityFinishTimestamp",
            "SecurityFinishTimestampMonotonic", "GeneratorsStartTimestamp",
            "GeneratorsStartTimestampMonotonic", "GeneratorsFinishTimestamp",
            "GeneratorsFinishTimestampMonotonic", "UnitsLoadStartTimestamp",
            "UnitsLoadStartTimestampMonotonic", "UnitsLoadFinishTimestamp",
            "UnitsLoadFinishTimestampMonotonic", "InitRDSecurityStartTimestamp",
            "InitRDSecurityStartTimestampMonotonic", "InitRDSecurityFinishTimestamp",
            "InitRDSecurityFinishTimestampMonotonic", "InitRDGeneratorsStartTimestamp",
            "InitRDGeneratorsStartTimestampMonotonic", "InitRDGeneratorsFinishTimestamp",
            "InitRDGeneratorsFinishTimestampMonotonic", "InitRDUnitsLoadStartTimestamp",
            "InitRDUnitsLoadStartTimestampMonotonic", "InitRDUnitsLoadFinishTimestamp",
            "InitRDUnitsLoadFinishTimestampMonotonic",
        ] {
            p.insert(k.into(), Value::from(0u64));
        }
        p.insert("UserspaceTimestamp".into(), Value::from(self.started.0));
        p.insert("UserspaceTimestampMonotonic".into(), Value::from(self.started.1));
        let fin = self.boot_finished.lock().unwrap().unwrap_or((0, 0));
        p.insert("FinishTimestamp".into(), Value::from(fin.0));
        p.insert("FinishTimestampMonotonic".into(), Value::from(fin.1));
        p.insert("LogLevel".into(), s("info"));
        p.insert("LogTarget".into(), s("console"));
        p.insert("NNames".into(), Value::from(nnames));
        p.insert("NFailedUnits".into(), Value::from(nfailed));
        p.insert("NJobs".into(), Value::from(njobs));
        p.insert("NInstalledJobs".into(), Value::from(self.next_job.load(std::sync::atomic::Ordering::Relaxed)));
        p.insert("NFailedJobs".into(), Value::from(0u32));
        p.insert("Progress".into(), Value::from(1.0f64));
        p.insert("Environment".into(), strv(self.manager_env.lock().unwrap().clone()));
        p.insert("ConfirmSpawn".into(), Value::from(false));
        p.insert("ShowStatus".into(), Value::from(false));
        p.insert(
            "UnitPath".into(),
            strv(self.cfg.unit_path.iter().map(|p| p.display().to_string()).collect()),
        );
        p.insert("DefaultStandardOutput".into(), s("journal"));
        p.insert("DefaultStandardError".into(), s("inherit"));
        p.insert("RuntimeWatchdogUSec".into(), Value::from(0u64));
        p.insert("RebootWatchdogUSec".into(), Value::from(0u64));
        p.insert("KExecWatchdogUSec".into(), Value::from(0u64));
        p.insert("ServiceWatchdogs".into(), Value::from(true));
        p.insert("ControlGroup".into(), s(""));
        p.insert("SystemState".into(), s(state));
        p.insert("ExitCode".into(), Value::from(0u8));
        p.insert("DefaultTimeoutStartUSec".into(), Value::from(90_000_000u64));
        p.insert("DefaultTimeoutStopUSec".into(), Value::from(90_000_000u64));
        p.insert("DefaultTimeoutAbortUSec".into(), Value::from(90_000_000u64));
        p.insert("DefaultRestartUSec".into(), Value::from(100_000u64));
        p.insert("DefaultStartLimitIntervalUSec".into(), Value::from(10_000_000u64));
        p.insert("DefaultStartLimitBurst".into(), Value::from(5u32));
        p.insert("DefaultTasksMax".into(), Value::from(u64::MAX));
        p.insert("TimerSlackNSec".into(), Value::from(50_000u64));
        p.insert("DefaultOOMPolicy".into(), s("stop"));
        p.insert("CtrlAltDelBurstAction".into(), s("reboot-force"));
        p
    }

    /// All property interfaces of a unit object.
    pub(super) async fn unit_props(&self, id: &str, unit: &Unit, active: &BTreeSet<String>) -> Ifaces {
        let status = self.unit_status(id, active, unit).await;
        let mut out = Ifaces::new();
        out.insert(UNIT_IFACE.into(), self.unit_iface(id, unit, &status));
        match unit.utype() {
            "service" => {
                out.insert("org.freedesktop.systemd1.Service".into(), self.service_iface(id, unit, &status));
            }
            "target" => {
                out.insert("org.freedesktop.systemd1.Target".into(), Props::new());
            }
            "scope" => {
                out.insert("org.freedesktop.systemd1.Scope".into(), self.scope_iface(id));
            }
            _ => {}
        }
        out
    }

    fn unit_iface(&self, id: &str, unit: &Unit, status: &Status) -> Props {
        let st = state::for_unit(&self.cfg.run_dir, id);
        let mut p = Props::new();
        let words = |k: &str| strv(unit.get_words("Unit", k));
        p.insert("Id".into(), s(id));
        let mut names = unit.names.clone();
        if names.is_empty() {
            names.push(id.to_string());
        }
        p.insert("Names".into(), strv(names));
        p.insert("Following".into(), s(""));
        for k in [
            "Requires", "Requisite", "Wants", "BindsTo", "PartOf", "Upholds", "Conflicts",
            "Before", "After", "OnSuccess", "OnFailure", "PropagatesReloadTo", "ReloadPropagatedFrom",
            "PropagatesStopTo", "StopPropagatedFrom", "JoinsNamespaceOf", "RequiresMountsFor",
        ] {
            p.insert(k.into(), words(k));
        }
        for k in [
            "RequiredBy", "RequisiteOf", "WantedBy", "BoundBy", "UpheldBy", "ConsistsOf",
            "ConflictedBy", "OnSuccessOf", "OnFailureOf", "Triggers", "TriggeredBy",
        ] {
            p.insert(k.into(), strv(Vec::new()));
        }
        p.insert("Documentation".into(), words("Documentation"));
        p.insert("Description".into(), s(unit.description()));
        p.insert("AccessSELinuxContext".into(), s(""));
        p.insert("LoadState".into(), s(unit.load_state.as_str()));
        p.insert("ActiveState".into(), s(status.active));
        p.insert("FreezerState".into(), s("running"));
        p.insert("SubState".into(), s(status.sub));
        p.insert(
            "FragmentPath".into(),
            s(unit.fragment.as_ref().map(|f| f.display().to_string()).unwrap_or_default()),
        );
        p.insert("SourcePath".into(), s(unit.get("Unit", "SourcePath").unwrap_or_default()));
        p.insert(
            "DropInPaths".into(),
            strv(unit.dropins.iter().map(|d| d.display().to_string()).collect()),
        );
        let ufs = if self.transient.lock().unwrap().contains_key(id) {
            "transient".to_string()
        } else {
            install::unit_file_state(&self.cfg, id).unwrap_or_default()
        };
        p.insert("UnitFileState".into(), s(ufs));
        p.insert("UnitFilePreset".into(), s(install::preset_state(&self.cfg, id)));

        let ts = |k: &str| st.get_u64(k);
        let (ae, ax, ie, ix) = (
            ts(keys::ACTIVE_ENTER),
            ts(keys::ACTIVE_EXIT),
            ts(keys::INACTIVE_ENTER),
            ts(keys::INACTIVE_EXIT),
        );
        let mono = |k: &str| st.get_u64(&format!("{k}-mono"));
        let change = ae.max(ax).max(ie).max(ix);
        p.insert("StateChangeTimestamp".into(), Value::from(change));
        p.insert("StateChangeTimestampMonotonic".into(), Value::from(0u64));
        p.insert("InactiveExitTimestamp".into(), Value::from(ix));
        p.insert("InactiveExitTimestampMonotonic".into(), Value::from(mono(keys::INACTIVE_EXIT)));
        p.insert("ActiveEnterTimestamp".into(), Value::from(ae));
        p.insert("ActiveEnterTimestampMonotonic".into(), Value::from(mono(keys::ACTIVE_ENTER)));
        p.insert("ActiveExitTimestamp".into(), Value::from(ax));
        p.insert("ActiveExitTimestampMonotonic".into(), Value::from(mono(keys::ACTIVE_EXIT)));
        p.insert("InactiveEnterTimestamp".into(), Value::from(ie));
        p.insert("InactiveEnterTimestampMonotonic".into(), Value::from(mono(keys::INACTIVE_ENTER)));

        let refuse_start = unit.get_bool("Unit", "RefuseManualStart", false);
        let refuse_stop = unit.get_bool("Unit", "RefuseManualStop", false);
        p.insert("CanStart".into(), Value::from(!refuse_start && unit.is_loaded()));
        p.insert("CanStop".into(), Value::from(!refuse_stop && unit.is_loaded()));
        p.insert("CanReload".into(), Value::from(Self::can_reload(unit)));
        p.insert("CanIsolate".into(), Value::from(unit.get_bool("Unit", "AllowIsolate", false)));
        p.insert("CanClean".into(), strv(Vec::new()));
        p.insert("CanFreeze".into(), Value::from(false));
        p.insert("CanLiveMount".into(), Value::from(false));
        let job = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .find(|j| j.unit == id)
            .map(|j| (j.id, op(job_path(j.id))))
            .unwrap_or((0, op("/".into())));
        p.insert("Job".into(), Value::from(job));
        p.insert("StopWhenUnneeded".into(), Value::from(unit.get_bool("Unit", "StopWhenUnneeded", false)));
        p.insert("RefuseManualStart".into(), Value::from(refuse_start));
        p.insert("RefuseManualStop".into(), Value::from(refuse_stop));
        p.insert("AllowIsolate".into(), Value::from(unit.get_bool("Unit", "AllowIsolate", false)));
        p.insert(
            "DefaultDependencies".into(),
            Value::from(unit.get_bool("Unit", "DefaultDependencies", true)),
        );
        p.insert("SurviveFinalKillSignal".into(), Value::from(false));
        p.insert("OnSuccessJobMode".into(), s("fail"));
        p.insert("OnFailureJobMode".into(), s("replace"));
        p.insert("IgnoreOnIsolate".into(), Value::from(unit.get_bool("Unit", "IgnoreOnIsolate", false)));
        p.insert("NeedDaemonReload".into(), Value::from(false));
        p.insert("Markers".into(), strv(Vec::new()));
        p.insert("JobTimeoutUSec".into(), Value::from(u64::MAX));
        p.insert("JobRunningTimeoutUSec".into(), Value::from(u64::MAX));
        p.insert("JobTimeoutAction".into(), s("none"));
        p.insert("JobTimeoutRebootArgument".into(), s(""));
        p.insert("ConditionResult".into(), Value::from(status.condition_ok));
        p.insert("AssertResult".into(), Value::from(!st.has(keys::ASSERT_FAILED)));
        p.insert("ConditionTimestamp".into(), Value::from(ix));
        p.insert("ConditionTimestampMonotonic".into(), Value::from(mono(keys::INACTIVE_EXIT)));
        p.insert("AssertTimestamp".into(), Value::from(ix));
        p.insert("AssertTimestampMonotonic".into(), Value::from(mono(keys::INACTIVE_EXIT)));

        // Conditions are evaluated lazily for display only.
        let ev = conditions::evaluate(unit);
        let conv = |v: &[conditions::CheckResult]| -> Vec<(String, bool, bool, String, i32)> {
            v.iter()
                .map(|r| (r.key.clone(), r.trigger, r.negate, r.param.clone(), r.state))
                .collect()
        };
        p.insert("Conditions".into(), Value::from(conv(&ev.conditions)));
        p.insert("Asserts".into(), Value::from(conv(&ev.asserts)));
        let load_error = match &unit.load_error {
            Some(e) => ("org.freedesktop.systemd1.LoadFailed".to_string(), e.clone()),
            None if !unit.is_loaded() => (
                "org.freedesktop.systemd1.NoSuchUnit".to_string(),
                format!("Unit {id} not found."),
            ),
            None => (String::new(), String::new()),
        };
        p.insert("LoadError".into(), Value::from(load_error));
        p.insert("Transient".into(), Value::from(self.transient.lock().unwrap().contains_key(id)));
        p.insert("Perpetual".into(), Value::from(false));
        p.insert("StartLimitIntervalUSec".into(), Value::from(timespan(unit, "Unit", &["StartLimitIntervalSec"], 10_000_000)));
        p.insert(
            "StartLimitBurst".into(),
            Value::from(unit.get("Unit", "StartLimitBurst").and_then(|v| v.parse::<u32>().ok()).unwrap_or(5)),
        );
        p.insert("StartLimitAction".into(), s("none"));
        p.insert("FailureAction".into(), s("none"));
        p.insert("FailureActionExitStatus".into(), Value::from(-1i32));
        p.insert("SuccessAction".into(), s("none"));
        p.insert("SuccessActionExitStatus".into(), Value::from(-1i32));
        p.insert("RebootArgument".into(), s(""));
        let inv = st.get(keys::INVOCATION).unwrap_or_default();
        let inv_bytes: Vec<u8> = (0..inv.len() / 2)
            .filter_map(|i| u8::from_str_radix(&inv[i * 2..i * 2 + 2], 16).ok())
            .collect();
        p.insert("InvocationID".into(), Value::from(inv_bytes));
        p.insert("CollectMode".into(), s("inactive"));
        p.insert("Refs".into(), strv(Vec::new()));
        p.insert("ActivationDetails".into(), Value::from(Vec::<(String, String)>::new()));
        p
    }

    pub(super) fn cgroup_dir_of(&self, id: &str, unit: &Unit) -> PathBuf {
        if let Some(t) = self.transient.lock().unwrap().get(id) {
            return t.cgroup.clone();
        }
        state::for_unit(&self.cfg.run_dir, id)
            .get(keys::CGROUP)
            .map(PathBuf::from)
            .unwrap_or_else(|| cgroup::unit_cgroup_dir(id, &cgroup::slice_of(unit)))
    }

    fn cgroup_props(&self, p: &mut Props, id: &str, unit: &Unit) {
        let slice = self
            .transient
            .lock()
            .unwrap()
            .get(id)
            .map(|t| t.slice.clone())
            .unwrap_or_else(|| cgroup::slice_of(unit));
        let dir = self.cgroup_dir_of(id, unit);
        let exists = dir.is_dir();
        p.insert("Slice".into(), s(slice.clone()));
        let rel = dir
            .strip_prefix(cgroup::CGROUP_ROOT)
            .map(|r| format!("/{}", r.display()))
            .unwrap_or_default();
        p.insert("ControlGroup".into(), s(if exists { rel } else { String::new() }));
        p.insert("ControlGroupId".into(), Value::from(0u64));
        let rd = |f: &str| if exists { cgroup::read_u64(&dir, f).unwrap_or(u64::MAX) } else { u64::MAX };
        p.insert("MemoryCurrent".into(), Value::from(rd("memory.current")));
        p.insert("MemoryPeak".into(), Value::from(rd("memory.peak")));
        p.insert("MemorySwapCurrent".into(), Value::from(rd("memory.swap.current")));
        p.insert("MemorySwapPeak".into(), Value::from(u64::MAX));
        p.insert("MemoryZSwapCurrent".into(), Value::from(u64::MAX));
        p.insert("MemoryAvailable".into(), Value::from(u64::MAX));
        p.insert(
            "CPUUsageNSec".into(),
            Value::from(if exists { cgroup::cpu_usage_nsec(&dir).unwrap_or(u64::MAX) } else { u64::MAX }),
        );
        p.insert(
            "TasksCurrent".into(),
            Value::from(if exists { cgroup::read_u64(&dir, "pids.current").unwrap_or(util::cgroup_pids(&dir).len() as u64) } else { u64::MAX }),
        );
        let tasks_max = match unit.get("Service", "TasksMax").as_deref() {
            Some("infinity") | None => u64::MAX,
            Some(v) => v.parse().unwrap_or(u64::MAX),
        };
        p.insert("TasksMax".into(), Value::from(tasks_max));
        p.insert("IPIngressBytes".into(), Value::from(u64::MAX));
        p.insert("IPEgressBytes".into(), Value::from(u64::MAX));
        p.insert("IPIngressPackets".into(), Value::from(u64::MAX));
        p.insert("IPEgressPackets".into(), Value::from(u64::MAX));
        p.insert("IOReadBytes".into(), Value::from(u64::MAX));
        p.insert("IOWriteBytes".into(), Value::from(u64::MAX));
        p.insert("IOReadOperations".into(), Value::from(u64::MAX));
        p.insert("IOWriteOperations".into(), Value::from(u64::MAX));
        p.insert("Delegate".into(), Value::from(false));
        p.insert("ManagedOOMMemoryPressure".into(), s("auto"));
        p.insert("ManagedOOMSwap".into(), s("auto"));
    }

    fn exec_list(&self, id: &str, unit: &Unit, key: &str) -> Value<'static> {
        type Exec = (String, Vec<String>, bool, u64, u64, u64, u64, u32, i32, i32);
        let st = state::for_unit(&self.cfg.run_dir, id);
        let mut out: Vec<Exec> = Vec::new();
        for line in unit.get_all("Service", key) {
            if let Ok(cmds) = execcmd::parse(&line) {
                for c in cmds {
                    // Runtime details are only tracked for the main command.
                    let (t0, m0, t1, m1, pid, code, status) = if key == "ExecStart" {
                        let exited = st.get_u64(keys::EXEC_MAIN_EXIT) >= st.get_u64(keys::EXEC_MAIN_START)
                            && st.has(keys::EXEC_MAIN_EXIT);
                        (
                            st.get_u64(keys::EXEC_MAIN_START),
                            st.get_u64("exec-main-start-mono"),
                            if exited { st.get_u64(keys::EXEC_MAIN_EXIT) } else { 0 },
                            if exited { st.get_u64("exec-main-exit-mono") } else { 0 },
                            st.get_u64(keys::MAIN_PID) as u32,
                            if exited { st.get_i64(keys::EXEC_MAIN_CODE) as i32 } else { 0 },
                            if exited { st.get_i64(keys::EXEC_MAIN_STATUS) as i32 } else { 0 },
                        )
                    } else {
                        (0, 0, 0, 0, 0, 0, 0)
                    };
                    out.push((c.path, c.argv, c.ignore_failure, t0, m0, t1, m1, pid, code, status));
                }
            }
        }
        Value::from(out)
    }

    fn service_iface(&self, id: &str, unit: &Unit, status: &Status) -> Props {
        let st = state::for_unit(&self.cfg.run_dir, id);
        let mut p = Props::new();
        let get = |k: &str| unit.get("Service", k).unwrap_or_default();
        let ty = unit.get("Service", "Type").unwrap_or_else(|| {
            if unit.file.get("Service", "ExecStart").is_none() {
                "oneshot".into()
            } else if unit.file.get("Service", "BusName").is_some() {
                "dbus".into()
            } else {
                "simple".into()
            }
        });
        p.insert("Type".into(), s(ty));
        p.insert("ExitType".into(), s(unit.get("Service", "ExitType").unwrap_or_else(|| "main".into())));
        p.insert("Restart".into(), s(unit.get("Service", "Restart").unwrap_or_else(|| "no".into())));
        p.insert("RestartMode".into(), s("normal"));
        p.insert("PIDFile".into(), s(get("PIDFile")));
        p.insert("NotifyAccess".into(), s(unit.get("Service", "NotifyAccess").unwrap_or_else(|| "none".into())));
        p.insert("RestartUSec".into(), Value::from(timespan(unit, "Service", &["RestartSec"], 100_000)));
        p.insert("RestartSteps".into(), Value::from(0u32));
        p.insert("RestartMaxDelayUSec".into(), Value::from(u64::MAX));
        p.insert("RestartUSecNext".into(), Value::from(timespan(unit, "Service", &["RestartSec"], 100_000)));
        p.insert("TimeoutStartUSec".into(), Value::from(timespan(unit, "Service", &["TimeoutStartSec", "TimeoutSec"], 90_000_000)));
        p.insert("TimeoutStopUSec".into(), Value::from(timespan(unit, "Service", &["TimeoutStopSec", "TimeoutSec"], 90_000_000)));
        p.insert("TimeoutAbortUSec".into(), Value::from(timespan(unit, "Service", &["TimeoutAbortSec", "TimeoutStopSec"], 90_000_000)));
        p.insert("TimeoutStartFailureMode".into(), s("terminate"));
        p.insert("TimeoutStopFailureMode".into(), s("terminate"));
        p.insert("RuntimeMaxUSec".into(), Value::from(u64::MAX));
        p.insert("RuntimeRandomizedExtraUSec".into(), Value::from(0u64));
        p.insert("WatchdogUSec".into(), Value::from(0u64));
        p.insert("WatchdogTimestamp".into(), Value::from(0u64));
        p.insert("WatchdogTimestampMonotonic".into(), Value::from(0u64));
        p.insert("RootDirectoryStartOnly".into(), Value::from(false));
        p.insert("RemainAfterExit".into(), Value::from(unit.get_bool("Service", "RemainAfterExit", false)));
        p.insert("GuessMainPID".into(), Value::from(unit.get_bool("Service", "GuessMainPID", true)));
        p.insert("MainPID".into(), Value::from(status.main_pid));
        p.insert("ControlPID".into(), Value::from(0u32));
        p.insert("BusName".into(), s(get("BusName")));
        p.insert("FileDescriptorStoreMax".into(), Value::from(0u32));
        p.insert("NFileDescriptorStore".into(), Value::from(0u32));
        p.insert("FileDescriptorStorePreserve".into(), s("restart"));
        p.insert("StatusText".into(), s(st.get(keys::STATUS_TEXT).unwrap_or_default()));
        p.insert("StatusErrno".into(), Value::from(0i32));
        p.insert("StatusBusError".into(), s(""));
        p.insert("StatusVarlinkError".into(), s(""));
        let result = match status.result.as_str() {
            "running" => "success".to_string(),
            r => r.to_string(),
        };
        p.insert("Result".into(), s(result));
        p.insert("ReloadResult".into(), s("success"));
        p.insert("CleanResult".into(), s("success"));
        p.insert("LiveMountResult".into(), s("success"));
        p.insert("USBFunctionDescriptors".into(), s(""));
        p.insert("USBFunctionStrings".into(), s(""));
        p.insert("UID".into(), Value::from(u32::MAX));
        p.insert("GID".into(), Value::from(u32::MAX));
        p.insert("NRestarts".into(), Value::from(st.get_u64(keys::NRESTARTS) as u32));
        p.insert("OOMPolicy".into(), s("stop"));
        p.insert("OpenFile".into(), Value::from(Vec::<(String, String, u64)>::new()));
        p.insert("ExtraFileDescriptorNames".into(), strv(Vec::new()));
        p.insert("ReloadSignal".into(), Value::from(libc::SIGHUP));
        p.insert("ExecMainStartTimestamp".into(), Value::from(st.get_u64(keys::EXEC_MAIN_START)));
        p.insert("ExecMainStartTimestampMonotonic".into(), Value::from(st.get_u64("exec-main-start-mono")));
        p.insert("ExecMainExitTimestamp".into(), Value::from(st.get_u64(keys::EXEC_MAIN_EXIT)));
        p.insert("ExecMainExitTimestampMonotonic".into(), Value::from(st.get_u64("exec-main-exit-mono")));
        p.insert("ExecMainHandoffTimestamp".into(), Value::from(0u64));
        p.insert("ExecMainHandoffTimestampMonotonic".into(), Value::from(0u64));
        let exec_main_pid = if status.main_pid > 0 { status.main_pid } else { st.get_u64(keys::MAIN_PID) as u32 };
        p.insert("ExecMainPID".into(), Value::from(exec_main_pid));
        p.insert("ExecMainCode".into(), Value::from(st.get_i64(keys::EXEC_MAIN_CODE) as i32));
        p.insert("ExecMainStatus".into(), Value::from(st.get_i64(keys::EXEC_MAIN_STATUS) as i32));
        for k in ["ExecCondition", "ExecStartPre", "ExecStart", "ExecStartPost", "ExecReload", "ExecStop", "ExecStopPost"] {
            p.insert(k.into(), self.exec_list(id, unit, k));
        }
        p.insert("User".into(), s(get("User")));
        p.insert("Group".into(), s(get("Group")));
        p.insert("DynamicUser".into(), Value::from(unit.get_bool("Service", "DynamicUser", false)));
        p.insert("SupplementaryGroups".into(), strv(unit.get_words("Service", "SupplementaryGroups")));
        p.insert("Environment".into(), strv(unit.get_words("Service", "Environment")));
        p.insert(
            "EnvironmentFiles".into(),
            Value::from(
                unit.get_all("Service", "EnvironmentFile")
                    .into_iter()
                    .map(|f| match f.strip_prefix('-') {
                        Some(p) => (p.to_string(), true),
                        None => (f, false),
                    })
                    .collect::<Vec<(String, bool)>>(),
            ),
        );
        p.insert("WorkingDirectory".into(), s(get("WorkingDirectory")));
        p.insert("RootDirectory".into(), s(get("RootDirectory")));
        p.insert("KillMode".into(), s(unit.get("Service", "KillMode").unwrap_or_else(|| "control-group".into())));
        p.insert(
            "KillSignal".into(),
            Value::from(unit.get("Service", "KillSignal").and_then(|s| util::signal_number(&s)).unwrap_or(libc::SIGTERM)),
        );
        p.insert("FinalKillSignal".into(), Value::from(libc::SIGKILL));
        p.insert("SendSIGKILL".into(), Value::from(unit.get_bool("Service", "SendSIGKILL", true)));
        p.insert("SendSIGHUP".into(), Value::from(unit.get_bool("Service", "SendSIGHUP", false)));
        p.insert("WatchdogSignal".into(), Value::from(libc::SIGABRT));
        p.insert("StandardInput".into(), s(unit.get("Service", "StandardInput").unwrap_or_else(|| "null".into())));
        p.insert("StandardOutput".into(), s(unit.get("Service", "StandardOutput").unwrap_or_else(|| "journal".into())));
        p.insert("StandardError".into(), s(unit.get("Service", "StandardError").unwrap_or_else(|| "inherit".into())));
        p.insert("PrivateTmp".into(), Value::from(unit.get_bool("Service", "PrivateTmp", false)));
        p.insert("ProtectSystem".into(), s(unit.get("Service", "ProtectSystem").unwrap_or_else(|| "no".into())));
        p.insert("ProtectHome".into(), s(unit.get("Service", "ProtectHome").unwrap_or_else(|| "no".into())));
        p.insert("NoNewPrivileges".into(), Value::from(unit.get_bool("Service", "NoNewPrivileges", false)));
        self.cgroup_props(&mut p, id, unit);
        p
    }

    fn scope_iface(&self, id: &str) -> Props {
        let mut p = Props::new();
        let ld = Loader::new(&self.cfg);
        let unit = ld.load(id);
        p.insert("Controller".into(), s(""));
        p.insert("Result".into(), s("success"));
        p.insert("TimeoutStopUSec".into(), Value::from(90_000_000u64));
        p.insert("RuntimeMaxUSec".into(), Value::from(u64::MAX));
        p.insert("KillMode".into(), s("control-group"));
        p.insert("KillSignal".into(), Value::from(libc::SIGTERM));
        self.cgroup_props(&mut p, id, &unit);
        p
    }

    pub(super) fn job_props(&self, id: u32) -> Option<Props> {
        let jobs = self.jobs.lock().unwrap();
        let j = jobs.get(&id)?;
        let mut p = Props::new();
        p.insert("Id".into(), Value::from(j.id));
        p.insert("Unit".into(), Value::from((j.unit.clone(), op(unit_path(&j.unit)))));
        p.insert("JobType".into(), s(j.jtype.clone()));
        p.insert("State".into(), s("running"));
        p.insert("ActivationDetails".into(), Value::from(Vec::<(String, String)>::new()));
        Some(p)
    }

    /// Units shown by ListUnits.
    pub(super) fn known_units(&self) -> Vec<String> {
        let idx = self.index.read().unwrap();
        let mut v: Vec<String> = idx.units.keys().cloned().collect();
        v.extend(self.transient.lock().unwrap().keys().cloned());
        v
    }
}
