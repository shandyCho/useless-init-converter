use std::fs;
use std::path::PathBuf;
use std::process::exit;

use sdcompat_base::config::Config;
use sdcompat_gen as gen;
use sdcompat_install as install;

const USAGE: &str = "\
usage: sdcompat-gen [--config PATH] COMMAND [ARGS]
       sdcompat-gen --version

Database:
  reload [--no-update] [--direct]
                           translate units into the generated store, sync the
                           s6-rc repository, commit its set and install it as
                           the boot and live database (s6-rc-set-install);
                           --no-update: the boot database only. On a running
                           s6 boot sdcompatd does it (daemon-reload); --direct
                           does it here
  plan                     print the translation report without writing anything

Offline unit file management:
  enable|disable|mask|unmask|preset [--runtime] [--force] UNIT...
  is-enabled UNIT...
";

fn die(msg: &str) -> ! {
    eprintln!("sdcompat-gen: {msg}");
    exit(1)
}

fn print_changes(changes: &[install::Change]) {
    for c in changes {
        if c.kind == "symlink" {
            println!("Created symlink {} → {}.", c.path, c.source);
        } else {
            println!("Removed \"{}\".", c.path);
        }
    }
}

/// `reload` on a running s6 boot: Manager.Reload on sdcompatd. Refuses a
/// manager that isn't sdcompatd (systemd's own private socket).
fn reload_via_daemon(cfg: &Config) -> i32 {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let res: Result<(), String> = rt.block_on(async {
        let conn = sdcompat_client::connect(cfg).await?;
        let version: String = zbus::fdo::PropertiesProxy::builder(&conn)
            .destination("org.freedesktop.systemd1")
            .and_then(|b| b.path("/org/freedesktop/systemd1"))
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string())?
            .get(zbus::names::InterfaceName::from_static_str_unchecked("org.freedesktop.systemd1.Manager"), "Version")
            .await
            .map_err(|e| e.to_string())
            .and_then(|v| String::try_from(v).map_err(|e| e.to_string()))?;
        if !version.contains("sdcompat") {
            return Err(format!("the service manager is not sdcompatd ({version})"));
        }
        sdcompat_client::manager_call(&conn, "Reload", &()).await.map(|_| ())
    });
    match res {
        Ok(()) => {
            let db = fs::read_link(cfg.state_dir.join("current")).unwrap_or_default();
            println!("reloaded by sdcompatd");
            println!("database: {} (live state updated)", cfg.state_dir.join(&db).display());
            println!("report:   {}", cfg.state_dir.join(&db).join("report.txt").display());
            0
        }
        Err(e) => {
            eprintln!("sdcompat-gen: reload through sdcompatd failed: {e}");
            eprintln!("sdcompat-gen: `reload --direct` updates the live state from this process instead");
            1
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // --config PATH before or after the command (generated scripts and the
    // sdcompat front end put it after), up to a "--".
    let mut config_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        if args[i] == "--config" {
            args.remove(i);
            if i >= args.len() {
                die("--config needs a path");
            }
            config_path = Some(PathBuf::from(args.remove(i)));
        } else {
            i += 1;
        }
    }
    let Some(cmd) = args.first().cloned() else {
        eprint!("{USAGE}");
        exit(1);
    };
    if sdcompat_version::wanted(&cmd) {
        sdcompat_version::print("sdcompat-gen");
        exit(0);
    }
    let rest = &args[1..];
    let cfg = Config::load(config_path.as_deref()).unwrap_or_else(|e| die(&e));

    let code = match cmd.as_str() {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            0
        }
        "plan" => {
            print!("{}", gen::report(&gen::plan(&cfg)));
            0
        }
        "reload" => {
            let update = !rest.iter().any(|a| a == "--no-update");
            let direct = rest.iter().any(|a| a == "--direct");
            // With a live state, sdcompatd does the update (as daemon-reload
            // does): run from a login shell, s6-rc-update's links and service
            // directories get the shell's SELinux labels (user_tmp_t in the
            // tmpfs scan directory), which init_t can't read; s6-svscan then
            // never supervises them and s6-rc-update waits forever.
            if update && !direct && fs::symlink_metadata(cfg.live_dir.join("compiled")).is_ok() {
                exit(reload_via_daemon(&cfg));
            }
            match gen::regenerate(&cfg, update) {
                Ok(r) => {
                    let first = gen::report(&r.plan);
                    println!("{}", first.lines().next().unwrap_or(""));
                    println!(
                        "database: {}{}",
                        r.db_dir.display(),
                        if r.updated_live { " (live state updated)" } else { "" }
                    );
                    println!("report:   {}", r.db_dir.join("report.txt").display());
                    for c in &r.set_changes {
                        println!("set {}: {c}", cfg.set_name);
                    }
                    0
                }
                Err(e) => die(&e),
            }
        }
        "enable" | "disable" | "mask" | "unmask" | "preset" => {
            let names: Vec<String> = rest.iter().filter(|a| !a.starts_with("--")).cloned().collect();
            let runtime = rest.iter().any(|a| a == "--runtime");
            let force = rest.iter().any(|a| a == "--force");
            let res = match cmd.as_str() {
                "enable" => install::enable(&cfg, &names, runtime, force).map(|(_, c)| c),
                "disable" => install::disable(&cfg, &names, runtime),
                "mask" => install::mask(&cfg, &names, runtime, force),
                "unmask" => install::unmask(&cfg, &names, runtime),
                _ => install::preset(&cfg, &names, "full", runtime, force).map(|(_, c)| c),
            };
            match res {
                Ok(c) => {
                    print_changes(&c);
                    0
                }
                Err(e) => die(&e),
            }
        }
        "is-enabled" => {
            let mut code = 1;
            for n in rest {
                match install::unit_file_state(&cfg, n).map(|s| install::set_file_state(&cfg, n, s)) {
                    Ok(s) => {
                        if s.starts_with("enabled") || s == "static" || s == "alias" {
                            code = 0;
                        }
                        println!("{s}");
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
            code
        }
        other => die(&format!("unknown command {other} (see sdcompat-gen help)")),
    };
    exit(code);
}
