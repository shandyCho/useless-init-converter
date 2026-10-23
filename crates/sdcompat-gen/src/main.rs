use std::path::PathBuf;
use std::process::exit;

use sdcompat_base::config::Config;
use sdcompat_gen as gen;
use sdcompat_install as install;

const USAGE: &str = "\
usage: sdcompat-gen [--config PATH] COMMAND [ARGS]
       sdcompat-gen --version

Database:
  reload [--no-update]     translate units, compile the s6-rc database and
                           switch the live state to it (s6-rc-update)
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
                match install::unit_file_state(&cfg, n) {
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
