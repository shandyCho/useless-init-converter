//! The s6 backend of the generator: the plan as an s6-rc source tree
//! (longruns, oneshots, bundles, s6-log consumers, ExecStop= atoms), and the
//! s6-rc repository steps that make it the boot and live database
//! (repo-sync, set-fix/-change/-commit/-install, s6-rc-update).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use uinitconverter_base::automask;

/// The files of a longrun's directory that s6-rc's source format and a
/// plain s6 service directory share: (name, contents, executable).
/// `on_demand`: the runner binds the sockets and waits for activity before
/// the first start (see Planned::on_demand).
pub fn longrun_files(cfg: &Config, id: &str, t: &Timings, on_demand: bool) -> Vec<(&'static str, String, bool)> {
    let args = cfg.runner_args();
    let arg = unit_arg(id);
    if id.ends_with(".timer") || id.ends_with(".path") {
        let sub = if id.ends_with(".timer") { "timer-run" } else { "path-run" };
        let timer_runner = cfg.timer_runner.display();
        return vec![
            ("run", format!("#!/bin/sh\nexec {timer_runner} {sub}{args} {arg}\n"), true),
            ("notification-fd", "3\n".into(), false),
        ];
    }
    let runner = cfg.runner.display();
    let lazy = if on_demand { " --on-demand" } else { "" };
    let mut out = vec![
        ("run", format!("#!/bin/sh\nexec {runner} run{args}{lazy} {arg}\n"), true),
        ("finish", format!("#!/bin/sh\nexec {runner} finish{args} {arg} \"$@\"\n"), true),
        ("notification-fd", "3\n".into(), false),
        ("timeout-up", format!("{}\n", t.up_ms), false),
        ("timeout-down", format!("{}\n", t.down_ms), false),
        ("timeout-kill", format!("{}\n", t.down_ms), false),
        ("timeout-finish", format!("{}\n", t.finish_ms), false),
    ];
    if let Some(sig) = &t.down_signal {
        out.push(("down-signal", format!("{sig}\n"), false));
    }
    out
}

fn write_file(path: &Path, content: &str, exec: bool) -> std::io::Result<()> {
    fs::write(path, content)?;
    if exec {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

pub fn write_source(cfg: &Config, plan: &Plan, dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let runner = cfg.runner.display().to_string();
    let args = cfg.runner_args();

    for p in plan.units.values() {
        let sdir = dir.join(&p.id);
        fs::create_dir_all(&sdir)?;
        write_file(&sdir.join("type"), &format!("{}\n", p.kind.as_str()), false)?;
        let list_dir = if p.kind == Kind::Bundle { "contents.d" } else { "dependencies.d" };
        fs::create_dir_all(sdir.join(list_dir))?;
        for d in &p.deps {
            fs::write(sdir.join(list_dir).join(d), "")?;
            // A bundle brings up the `.stop` oneshots of its longruns too:
            // `s6-rc -p change` would otherwise take one down alone and run
            // the ExecStop= of a service that stays up.
            if p.kind == Kind::Bundle {
                if let Some(stop) = plan.units.get(d).and_then(|u| u.stop_atom.as_ref()) {
                    fs::write(sdir.join(list_dir).join(stop), "")?;
                }
            }
        }

        if p.kind != Kind::Bundle && plan.boot.contains(&p.id) {
            fs::write(sdir.join("flag-recommended"), "")?;
        }

        match p.kind {
            Kind::Bundle => {}
            Kind::Oneshot => {
                let arg = unit_arg(&p.id);
                write_file(&sdir.join("up"), &format!("{runner} up{args} {arg}\n"), false)?;
                write_file(&sdir.join("down"), &format!("{runner} down{args} {arg}\n"), false)?;
                write_file(&sdir.join("timeout-up"), &format!("{}\n", p.timings.up_ms), false)?;
                write_file(&sdir.join("timeout-down"), &format!("{}\n", p.timings.down_ms), false)?;
            }
            Kind::Longrun => {
                for (name, text, exec) in longrun_files(cfg, &p.id, &p.timings, p.on_demand) {
                    write_file(&sdir.join(name), &text, exec)?;
                }
                if let Some(stop) = &p.stop_atom {
                    write_stop_atom(cfg, plan, p, &dir.join(stop))?;
                }
                if let (Some(log), Some(log_dir)) = (&p.log_atom, &cfg.log_dir) {
                    write_file(&sdir.join("producer-for"), &format!("{log}\n"), false)?;
                    let ldir = dir.join(log);
                    fs::create_dir_all(&ldir)?;
                    write_file(&ldir.join("type"), "longrun\n", false)?;
                    write_file(&ldir.join("consumer-for"), &format!("{}\n", p.id), false)?;
                    write_file(&ldir.join("notification-fd"), "3\n", false)?;
                    if plan.boot.contains(log) {
                        fs::write(ldir.join("flag-recommended"), "")?;
                    }
                    let logdir = util::sh_quote(&log_dir.join(&p.id).display().to_string());
                    let s6log = util::sh_quote(&tools::path("s6-log"));
                    write_file(
                        &ldir.join("run"),
                        &format!(
                            "#!/bin/sh\nmkdir -p {logdir} || exit 1\nexec {s6log} -d3 n10 s1000000 T {logdir}\n"
                        ),
                        true,
                    )?;
                }
            }
        }
    }

    // Booted when /system-update exists (offline updates, dnf system-upgrade).
    let b = dir.join("uinitconverter-system-update");
    fs::create_dir_all(b.join("contents.d"))?;
    write_file(&b.join("type"), "bundle\n", false)?;
    // logind too: update tools may ask it to reboot when they are done.
    for u in ["system-update.target", "systemd-logind.service"] {
        if let Some(p) = plan.units.get(u) {
            fs::write(b.join("contents.d").join(u), "")?;
            if let Some(stop) = &p.stop_atom {
                fs::write(b.join("contents.d").join(stop), "")?;
            }
        }
    }

    // Index read by the daemon and the runner.
    let mut idx = String::new();
    for p in plan.units.values() {
        let _ = writeln!(
            idx,
            "unit\t{}\t{}\t{}",
            p.id,
            p.kind.as_str(),
            p.log_atom.as_deref().unwrap_or("-")
        );
    }
    for (a, id) in &plan.aliases {
        let _ = writeln!(idx, "alias\t{a}\t{id}");
    }
    for (t, u) in &plan.triggers {
        let _ = writeln!(idx, "trigger\t{t}\t{u}");
    }
    if let Some(def) = &plan.default_target {
        let _ = writeln!(idx, "default\t{def}");
    }
    fs::write(dir.join("..").join("index"), idx)?;
    fs::write(dir.join("..").join("report.txt"), report(plan))?;
    Ok(())
}

/// The `.stop` oneshot of a longrun (see index::stop_atom). Nothing depends
/// on it: s6-rc-update restarts a service that gains a dependency on a new
/// service, and an upgrade would have restarted every dependent of such a
/// longrun (gdm, with the session). So when s6-rc stops the longrun with
/// units that depend on it, its ExecStop= runs beside their stops rather
/// than after them; the longrun itself still goes down last.
fn write_stop_atom(cfg: &Config, plan: &Plan, p: &Planned, sdir: &Path) -> std::io::Result<()> {
    let runner = cfg.runner.display().to_string();
    let args = cfg.runner_args();
    fs::create_dir_all(sdir.join("dependencies.d"))?;
    write_file(&sdir.join("type"), "oneshot\n", false)?;
    fs::write(sdir.join("dependencies.d").join(&p.id), "")?;
    // Empty: nothing to do when it comes up.
    write_file(&sdir.join("up"), "", false)?;
    write_file(&sdir.join("down"), &format!("{runner} stop-atom{args} {}\n", unit_arg(&p.id)), false)?;
    // The runner ends ExecStop= at TimeoutStopSec= and always succeeds; a
    // failed down would keep s6-rc from stopping the service.
    let down = if p.timings.down_ms == 0 { 0 } else { p.timings.down_ms.saturating_add(10_000).min(u32::MAX as u64) };
    write_file(&sdir.join("timeout-down"), &format!("{down}\n"), false)?;
    if plan.boot.contains(&p.id) {
        fs::write(sdir.join("flag-recommended"), "")?;
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    } else {
        Err(format!(
            "{:?} failed: {}{}",
            cmd,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ))
    }
}

/// Replace the symlink `link` with one to `target`, atomically.
fn swap_symlink(target: &Path, link: &Path) -> Result<(), String> {
    let name = link.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = link.with_file_name(format!(".{name}.tmp"));
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, link).map_err(|e| format!("{}: {e}", link.display()))
}

/// The stores the repository links, in s6-rc's order (a later store's
/// service overrides an earlier one's): the generated one, then the
/// hand-written ones that exist.
pub fn stores(cfg: &Config) -> Vec<PathBuf> {
    let mut v = vec![cfg.store_dir()];
    v.extend(cfg.extra_source.iter().filter(|d| d.is_dir()).cloned());
    v
}

/// The stores an existing repository links (REPO/stores/NNNN), or None
/// when there is no repository.
fn repo_stores(cfg: &Config) -> Option<Vec<PathBuf>> {
    let dir = cfg.repo_dir.join("stores");
    let mut names: Vec<String> = fs::read_dir(&dir).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Some(names.iter().filter_map(|n| fs::read_link(dir.join(n)).ok()).collect())
}

/// Create the repository, change its stores, or sync it with them.
fn repo_sync(cfg: &Config) -> Result<(), String> {
    let want = stores(cfg);
    // s6-rc-set-commit makes this bundle itself and refuses a store that
    // defines one (uinitconverter shipped /etc/uinitconverter/s6/default before).
    if let Some(d) = want.iter().map(|s| s.join(&cfg.default_bundle)).find(|d| d.exists()) {
        return Err(format!(
            "{}: s6-rc-set-commit makes the {} bundle from the set; remove this one",
            d.display(),
            cfg.default_bundle
        ));
    }
    let repo = cfg.repo_dir.as_os_str();
    match repo_stores(cfg) {
        None => {
            if let Some(parent) = cfg.repo_dir.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            run(Command::new(tools::path("s6-rc-repo-init")).arg("-r").arg(repo).args(&want))?;
        }
        Some(have) if have != want => {
            run(Command::new(tools::path("s6-rc-repo-init")).arg("-r").arg(repo).arg("-U").args(&want))?;
        }
        Some(_) => {
            run(Command::new(tools::path("s6-rc-repo-sync")).arg("-r").arg(repo))?;
        }
    }
    if !cfg.repo_dir.join("sources").join(&cfg.set_name).is_dir() {
        run(Command::new(tools::path("s6-rc-set-new")).arg("-r").arg(repo).arg(&cfg.set_name))?;
    }
    Ok(())
}


/// The early services are `always`: those with flag-essential, and the
/// members of SYSINIT_BUNDLE (uinitconverter init adds or removes syslog,
/// journal-relay and resolv there, so they carry no flag of their own).
/// A repository sync reads flags only for services it hasn't seen before,
/// so this keeps the set in step when they change; a service that is no
/// longer either goes back to `active`.
fn sync_essentials(cfg: &Config, changes: &mut Vec<String>) -> Result<(), String> {
    let sources = cfg.repo_dir.join("sources");
    let atomics = sources.join(".atomics");
    let Ok(rd) = fs::read_dir(&atomics) else { return Ok(()) };
    let members: BTreeSet<String> = fs::read_dir(sources.join(".bundles").join(&cfg.sysinit_bundle).join("contents.d"))
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let mut up = Vec::new();
    let mut down = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let flagged = atomics.join(&name).join("flag-essential").exists() || members.contains(&name);
        match (flagged, install::set_rx(cfg, &name)) {
            (true, Some(rx)) if rx != "always" => up.push(name),
            (false, Some("always")) => down.push(name),
            _ => {}
        }
    }
    for (rx, names) in [("always", up), ("active", down)] {
        if names.is_empty() {
            continue;
        }
        run(Command::new(tools::path("s6-rc-set-change"))
            .arg("-r")
            .arg(&cfg.repo_dir)
            .arg("-e")
            .arg(&cfg.set_name)
            .arg(rx)
            .args(&names))?;
        changes.push(format!("{} -> {rx} (early boot service)", names.join(" ")));
    }
    Ok(())
}

/// A logger is `active` with its service (both were in the boot closure),
/// but `s6 set disable -p X` moves only X and what depends on it: the
/// logger is X's dependency, so it stays and boots with nothing to log.
/// Move a last consumer that is `active` while none of its producers is
/// active or always to `usable`. Starting the service still starts it (a
/// dependency), and set-fix -u makes it active again with the service.
fn sync_loggers(cfg: &Config, changes: &mut Vec<String>) -> Result<(), String> {
    let atomics = cfg.repo_dir.join("sources").join(".atomics");
    let Ok(rd) = fs::read_dir(&atomics) else { return Ok(()) };
    let mut idle = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let dir = atomics.join(&name);
        if name.starts_with('.') || dir.join("producer-for").exists() {
            continue;
        }
        let Ok(producers) = fs::read_to_string(dir.join("consumer-for")) else { continue };
        if install::set_rx(cfg, &name) == Some("active")
            && !producers
                .lines()
                .filter(|l| !l.is_empty())
                .any(|p| matches!(install::set_rx(cfg, p), Some("active" | "always")))
        {
            idle.push(name);
        }
    }
    if idle.is_empty() {
        return Ok(());
    }
    idle.sort();
    run(Command::new(tools::path("s6-rc-set-change"))
        .arg("-r")
        .arg(&cfg.repo_dir)
        .arg(&cfg.set_name)
        .arg("usable")
        .args(&idle))?;
    changes.push(format!("{} -> usable (logger of a service that is not active)", idle.join(" ")));
    Ok(())
}

/// The services AUTO_MASK= masks: translated services, timers and paths
/// that are `usable` in the set (or masked by an earlier run of this), not
/// pulled in by the boot, without .socket units (their runner binds them
/// and the fd store holds them), not taken back by the admin, and on which only
/// such services depend (s6-rc refuses a set where a service that stays
/// depends on a masked one; a bundle that contains one keeps it).
fn auto_mask_candidates(cfg: &Config, plan: &Plan, ours: &BTreeSet<String>, kept: &BTreeSet<String>) -> BTreeSet<String> {
    let rx = |n: &str| install::set_rx(cfg, n);
    let mut e: BTreeSet<String> = plan
        .units
        .values()
        .filter(|p| matches!(p.kind, Kind::Longrun | Kind::Oneshot))
        .filter(|p| [".service", ".timer", ".path"].iter().any(|s| p.id.ends_with(s)))
        .filter(|p| !p.has_sockets && !plan.boot.contains(&p.id) && !kept.contains(&p.id) && !cfg.is_excluded(&p.id))
        .filter(|p| match rx(&p.id) {
            Some("usable") => true,
            Some("masked") => ours.contains(&p.id),
            _ => false,
        })
        .map(|p| p.id.clone())
        .collect();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in plan.units.values() {
        for d in &p.deps {
            dependents.entry(d.as_str()).or_default().push(p.id.as_str());
        }
    }
    loop {
        let blocked: Vec<String> = e
            .iter()
            .filter(|x| {
                dependents
                    .get(x.as_str())
                    .into_iter()
                    .flatten()
                    .any(|u| !e.contains(*u) && rx(u) != Some("masked"))
            })
            .cloned()
            .collect();
        if blocked.is_empty() {
            return e;
        }
        for b in blocked {
            e.remove(&b);
        }
    }
}

/// A service with its logger and ExecStop= oneshot, the ones the set has:
/// s6-rc masks a pipeline as a whole, and the oneshot depends on it.
fn with_companions(cfg: &Config, plan: &Plan, id: &str) -> Vec<String> {
    let mut v = vec![id.to_string()];
    if let Some(p) = plan.units.get(id) {
        v.extend(p.log_atom.iter().chain(p.stop_atom.iter()).filter(|n| install::set_rx(cfg, n).is_some()).cloned());
    }
    v
}

/// s6-rc-set-change RX for groups of services: all at once, and if the
/// set refuses that (masking one a service from a hand-written store
/// depends on), group by group, leaving out the groups it refuses. Returns the ids of
/// the groups that moved.
fn set_change_groups(cfg: &Config, rx: &str, groups: &[(String, Vec<String>)]) -> Vec<String> {
    // Masking must not leave a remaining service depending on a masked one
    // (-i: refuse). Unmasking to usable may leave an active service depending
    // on a usable one: set-fix -u before the commit makes it active.
    let fail = if rx == "masked" { "-i" } else { "-I" };
    let change = |names: &[String]| {
        run(Command::new(tools::path("s6-rc-set-change"))
            .arg("-r")
            .arg(&cfg.repo_dir)
            .arg(fail)
            .arg(&cfg.set_name)
            .arg(rx)
            .args(names))
    };
    if groups.is_empty() {
        return Vec::new();
    }
    let all: Vec<String> = groups.iter().flat_map(|(_, g)| g.iter().cloned()).collect();
    if change(&all).is_ok() {
        return groups.iter().map(|(id, _)| id.clone()).collect();
    }
    let mut moved = Vec::new();
    for (id, g) in groups {
        match change(g) {
            Ok(_) => moved.push(id.clone()),
            Err(e) => eprintln!("uinitconverter-gen: {id} stays {}: {}", install::set_rx(cfg, id).unwrap_or("?"), e.trim()),
        }
    }
    moved
}

/// AUTO_MASK=: mask the candidates (auto_mask_candidates) and unmask, back
/// to `usable`, what this masked and is no longer one (something now
/// depends on it, or AUTO_MASK=no). A service the admin moved out of
/// `masked` is kept as the admin left it from then on. One running as a
/// dynamic instance stays masked until it stops: in the database it would
/// get a second supervisor under the same name.
fn sync_auto_mask(cfg: &Config, plan: &Plan, changes: &mut Vec<String>) -> Result<(), String> {
    let mut ours = automask::masked(cfg);
    let mut kept = automask::kept(cfg);
    let taken_back: Vec<String> =
        ours.iter().filter(|n| install::set_rx(cfg, n).is_some_and(|rx| rx != "masked")).cloned().collect();
    for n in &taken_back {
        ours.remove(n);
        kept.insert(n.clone());
    }
    if !taken_back.is_empty() {
        changes.push(format!("{} moved out of masked by the admin: no longer masked automatically", taken_back.join(" ")));
    }
    // Gone from the set (unit removed): forget it.
    ours.retain(|n| install::set_rx(cfg, n).is_some());

    let want = if cfg.auto_mask { auto_mask_candidates(cfg, plan, &ours, &kept) } else { BTreeSet::new() };
    let running = |n: &str| cfg.run_dir.join("dyn").join(n).exists();
    let unmask: Vec<(String, Vec<String>)> = ours
        .iter()
        .filter(|n| !want.contains(*n) && !running(n))
        .map(|n| (n.clone(), with_companions(cfg, plan, n)))
        .collect();
    let unmasked = set_change_groups(cfg, "usable", &unmask);
    for n in &unmasked {
        ours.remove(n);
    }
    if !unmasked.is_empty() {
        changes.push(format!("{} -> usable (no longer masked automatically)", unmasked.join(" ")));
    }
    let mask: Vec<(String, Vec<String>)> = want
        .iter()
        .filter(|n| install::set_rx(cfg, n) != Some("masked"))
        .map(|n| (n.clone(), with_companions(cfg, plan, n)))
        .collect();
    let masked = set_change_groups(cfg, "masked", &mask);
    ours.extend(masked.iter().cloned());
    if !masked.is_empty() {
        changes.push(format!(
            "{} -> masked (nothing boots or needs them: started on demand, AUTO_MASK=)",
            masked.join(" ")
        ));
    }
    automask::save(cfg, &ours, &kept)
}

/// A longrun's `.stop` oneshot (see write_stop_atom) takes its service's
/// prescription (active for always). `s6 set disable X` without -p leaves
/// it active, and set-fix -u would then enable X again, as the oneshot
/// depends on it; `s6 set enable X` leaves it usable, out of the default
/// bundle, so `s6 live reset` would take it down alone and run X's
/// ExecStop= while X stays up.
fn sync_stop_atoms(cfg: &Config, changes: &mut Vec<String>) -> Result<(), String> {
    let atomics = cfg.repo_dir.join("sources").join(".atomics");
    let Ok(rd) = fs::read_dir(&atomics) else { return Ok(()) };
    let mut moves: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(base) = name.strip_suffix(".stop") else { continue };
        if stop_atom(base) != name || !atomics.join(base).exists() {
            continue;
        }
        let want = match install::set_rx(cfg, base) {
            Some("always" | "active") => "active",
            Some(rx) => rx,
            None => continue,
        };
        if install::set_rx(cfg, &name) != Some(want) {
            moves.entry(want).or_default().push(name);
        }
    }
    for (rx, mut names) in moves {
        names.sort();
        run(Command::new(tools::path("s6-rc-set-change"))
            .arg("-r")
            .arg(&cfg.repo_dir)
            .arg(&cfg.set_name)
            .arg(rx)
            .args(&names))?;
        changes.push(format!("{} -> {rx} (with its service)", names.join(" ")));
    }
    Ok(())
}

/// Commit the set and install it: as the boot database, and (with a live
/// state) as the live one. s6-rc-set-fix -u first enables what active
/// services depend on: a new package's service may need one the set has
/// as usable, and the commit refuses an inconsistent set.
fn commit_and_install(cfg: &Config, update_live: bool, changes: &mut Vec<String>) -> Result<bool, String> {
    let repo = cfg.repo_dir.as_os_str();
    let out = run(Command::new(tools::path("s6-rc-set-fix")).arg("-r").arg(repo).arg("-v2").arg("-u").arg(&cfg.set_name))?;
    changes.extend(
        out.lines()
            .filter_map(|l| l.split_once("moving service ").map(|(_, r)| r.to_string())),
    );
    // Again for the services set-fix just enabled.
    sync_stop_atoms(cfg, changes)?;
    // --no-snapshot for both: s6-rc 0.8.0.2's s6rc_repo_setuptodate()
    // terminates the snapshot's path one byte too late (an uninitialized
    // byte is left in it), so with snapshots set-install randomly finds a
    // just-committed set "not up-to-date".
    run(Command::new(tools::path("s6-rc-set-commit"))
        .arg("-r")
        .arg(repo)
        .arg("--no-snapshot")
        .arg("-f")
        .arg("-D")
        .arg(&cfg.default_bundle)
        .arg(&cfg.set_name))?;

    let live = update_live && fs::symlink_metadata(cfg.live_dir.join("compiled")).is_ok();
    let install = |extra: &[&str]| -> Result<String, String> {
        run(Command::new(tools::path("s6-rc-set-install"))
            .arg("-r")
            .arg(repo)
            .arg("-c")
            .arg(cfg.boot_db())
            .arg("-l")
            .arg(&cfg.live_dir)
            // -K: the replaced database stays, as compiled.previous.
            .arg("-K")
            .arg("--no-snapshot")
            .args(extra)
            .arg(&cfg.set_name))
    };
    if !live {
        return install(&["--no-update"]).map(|_| false);
    }
    // s6-rc-update can fail after its switch ("unable to manage new service
    // directories": relink_supervised) or race s6-svscan's rescan; repair
    // or rescan, and retry.
    // Report the first error: later attempts may only fail because of
    // what the first one left behind.
    //
    // When s6-rc-update fails, set-install has copied the database beside
    // the boot link but not switched the link. With the copy there, running
    // set-install again says "already installed" and exits 0 without
    // updating anything. So: if the live state already runs on the copy
    // (s6-rc-update failed after its switch, starting the new
    // supervisors), run s6-rc-update on it again and switch the boot link
    // here; otherwise remove the copy and run set-install again.
    let copy = fs::read_link(cfg.repo_dir.join("compiled").join(&cfg.set_name))
        .ok()
        .zip(cfg.boot_db().parent().map(Path::to_path_buf))
        .map(|(name, dir)| (name.clone(), dir.join(name)));
    let mut attempt = 0;
    let mut first: Option<String> = None;
    let mut res = install(&[]).map(|_| ());
    loop {
        let e = match res {
            Ok(()) => return Ok(true),
            Err(e) => e,
        };
        // A failed rollback leaves a running service's directory in the
        // new live directory: every retry fails on the rename.
        if attempt >= 3 || e.contains("unable to rollback") {
            return Err(match first {
                Some(f) if f != e => format!("{}\n(last attempt: {})", f.trim(), e.trim()),
                _ => e,
            });
        }
        attempt += 1;
        eprintln!("uinitconverter: installing the set failed, retrying: {}", e.trim());
        let linking = e.contains("manage new service directories");
        first.get_or_insert(e);
        if linking {
            // No rescan here: it would kill the supervisors whose links the
            // failed update removed (see relink_supervised).
            relink_supervised(&cfg.live_dir);
        } else if let Ok(scan) = fs::canonicalize(cfg.live_dir.join("scandir")) {
            let _ = Command::new(tools::path("s6-svscanctl")).arg("-an").arg(scan).status();
        }
        std::thread::sleep(std::time::Duration::from_millis(500 * attempt));
        let live_db = fs::canonicalize(cfg.live_dir.join("compiled")).ok();
        res = match &copy {
            Some((name, path)) if live_db.is_some() && live_db == fs::canonicalize(path).ok() => {
                run(Command::new(tools::path("s6-rc-update")).arg("-e").arg("-l").arg(&cfg.live_dir).arg(path))
                    .and_then(|_| swap_symlink(name, &cfg.boot_db()))
            }
            Some((_, path)) => {
                if fs::canonicalize(cfg.boot_db()).ok() != fs::canonicalize(path).ok() {
                    let _ = fs::remove_dir_all(path);
                }
                install(&[]).map(|_| ())
            }
            None => install(&[]).map(|_| ()),
        };
    }
}

/// Put back the scan directory links of live service directories that
/// still have a supervisor. "unable to manage new service directories"
/// comes after s6-rc-update's switch to the new live directory: its
/// s6-ftrigrd died (s6 2.15.1.1 crashes when a subscription follows one
/// that already fired), and s6_supervise_link_names() then unlinks every
/// name it had linked so far, those of running services too. The next scan
/// would kill their supervisors, and the services stay down while s6-rc
/// has them up. Directories with no supervisor stay unlinked: the retry's
/// s6-rc-update links them and refuses a link that exists.
fn relink_supervised(live: &Path) {
    let prefix = fs::read_to_string(live.join("prefix")).unwrap_or_default();
    let Ok(dir) = fs::read_dir(live.join("servicedirs")) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let link = live.join("scandir").join(format!("{prefix}{name}"));
        if fs::symlink_metadata(&link).is_ok()
            || !uinitconverter_base::util::lock_held(&e.path().join("supervise/lock"))
        {
            continue;
        }
        // The target s6-rc gives it: LIVE/servicedirs/NAME as -l names LIVE.
        match std::os::unix::fs::symlink(live.join("servicedirs").join(&name), &link) {
            Ok(()) => eprintln!("uinitconverter: relinked {} (its supervisor still runs)", link.display()),
            Err(err) => eprintln!("uinitconverter: relinking {}: {err}", link.display()),
        }
    }
}

/// Plan and write the generated store, sync the repository with it, then
/// commit the set and install it (as the live database too when
/// `update_live` and a live s6-rc state exists).
pub fn regenerate(cfg: &Config, plan: Plan, update_live: bool) -> Result<GenResult, String> {
    let stamp = format!("{}-{}", util::now_realtime_usec(), std::process::id());
    let db_root = cfg.state_dir.join("db");
    let db_dir = db_root.join(&stamp);
    let source = db_dir.join("source");
    write_source(cfg, &plan, &source).map_err(|e| format!("{}: {e}", source.display()))?;
    let source_abs = fs::canonicalize(&source).map_err(|e| e.to_string())?;

    // The repository links the store by STORE_DIR, a symlink to the newest
    // source. If anything below fails, it goes back to the old one.
    let store = cfg.store_dir();
    let old_store = fs::read_link(&store).ok();
    let old_boot = fs::canonicalize(cfg.boot_db()).ok();
    swap_symlink(&source_abs, &store)?;
    let mut set_changes = Vec::new();
    let res = repo_sync(cfg)
        .and_then(|_| sync_essentials(cfg, &mut set_changes))
        .and_then(|_| sync_loggers(cfg, &mut set_changes))
        .and_then(|_| sync_auto_mask(cfg, &plan, &mut set_changes))
        .and_then(|_| sync_stop_atoms(cfg, &mut set_changes))
        .and_then(|_| commit_and_install(cfg, update_live, &mut set_changes));
    let updated_live = match res {
        Ok(r) => r,
        Err(e) => {
            match &old_store {
                Some(old) => {
                    let _ = swap_symlink(old, &store);
                    let _ = repo_sync(cfg);
                }
                None => {
                    let _ = fs::remove_file(&store);
                }
            }
            let _ = fs::remove_dir_all(&db_dir);
            return Err(e);
        }
    };

    // The database installed before is the one to roll back to
    // (uinitconverter.db=previous on the kernel command line); the one before
    // that goes. After a rollback boot the live database is the previous
    // one: it stays "previous" rather than the database the user fled from.
    let previous = cfg.state_dir.join("compiled.previous");
    let old_previous = fs::canonicalize(&previous).ok();
    let booted_previous = old_previous.is_some()
        && fs::canonicalize(cfg.live_dir.join("compiled")).ok() == old_previous;
    let state_abs = fs::canonicalize(&cfg.state_dir).unwrap_or_else(|_| cfg.state_dir.clone());
    let replaced = old_boot.filter(|o| fs::canonicalize(cfg.boot_db()).ok().as_ref() != Some(o));
    match replaced {
        Some(old) if booted_previous => {
            if old.starts_with(&state_abs) && Some(&old) != old_previous.as_ref() {
                let _ = fs::remove_dir_all(old);
            }
        }
        Some(old) => {
            swap_symlink(&old, &previous)?;
            if let Some(p) = old_previous.filter(|p| *p != old && p.starts_with(&state_abs)) {
                let _ = fs::remove_dir_all(p);
            }
        }
        None => {}
    }

    // "current" (index and report for the daemon) and "previous" (the
    // index of the database to roll back to).
    if let Ok(cur) = fs::read_link(cfg.state_dir.join("current")) {
        if !booted_previous {
            swap_symlink(&cur, &cfg.state_dir.join("previous"))?;
        }
    }
    swap_symlink(&PathBuf::from("db").join(&stamp), &cfg.state_dir.join("current"))?;

    // Garbage-collect generated sources that are neither current nor
    // previous, keeping any that still hold a database in use (databases
    // compiled before the repository existed lived in db/<stamp>/compiled).
    let keep: Vec<PathBuf> = ["current", "previous", "compiled", "compiled.previous"]
        .iter()
        .filter_map(|l| fs::canonicalize(cfg.state_dir.join(l)).ok())
        .chain(fs::canonicalize(cfg.live_dir.join("compiled")).ok())
        .collect();
    if let Ok(rd) = fs::read_dir(&db_root) {
        for e in rd.flatten() {
            let p = fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
            if !keep.iter().any(|k| k.starts_with(&p)) {
                let _ = fs::remove_dir_all(&p);
            }
        }
    }
    // ... and the copies s6-rc-set-install made (STATE_DIR/.SET:...) that
    // are no longer the boot, previous or live database.
    let copy_prefix = format!(".{}:", cfg.set_name);
    if let Ok(rd) = fs::read_dir(&state_abs) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&copy_prefix) && !keep.contains(&e.path()) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }

    Ok(GenResult { db_dir, plan, updated_live, set_changes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// After s6-rc-update's failed linking, only directories that still
    /// have a supervisor get their link back.
    #[test]
    fn relink_supervised_only_locked() {
        use std::io::{BufRead, Write};
        let tmp = std::env::temp_dir().join(format!("uinitconverter-gen-relink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let live = tmp.join("live");
        for n in ["run.service", "new.service", "kept.service"] {
            fs::create_dir_all(live.join("servicedirs").join(n).join("supervise")).unwrap();
            fs::write(live.join("servicedirs").join(n).join("supervise/lock"), "").unwrap();
        }
        fs::create_dir_all(live.join("scandir")).unwrap();
        fs::write(live.join("prefix"), "").unwrap();
        std::os::unix::fs::symlink("elsewhere", live.join("scandir/kept.service")).unwrap();
        // A supervisor's fcntl lock, held by another process.
        let mut child = Command::new("python3")
            .args(["-c", "import fcntl,sys; f=open(sys.argv[1],'w'); fcntl.lockf(f, fcntl.LOCK_EX); print('locked', flush=True); sys.stdin.read()"])
            .arg(live.join("servicedirs/run.service/supervise/lock"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "locked");

        relink_supervised(&live);
        assert_eq!(
            fs::read_link(live.join("scandir/run.service")).unwrap(),
            live.join("servicedirs/run.service")
        );
        assert!(fs::symlink_metadata(live.join("scandir/new.service")).is_err());
        assert_eq!(fs::read_link(live.join("scandir/kept.service")).unwrap(), Path::new("elsewhere"));
        let _ = child.stdin.take().unwrap().write_all(b"");
        let _ = child.wait();
        let _ = fs::remove_dir_all(&tmp);
    }
}
