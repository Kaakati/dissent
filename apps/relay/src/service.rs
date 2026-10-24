//! Installs this program as a background service and runs it as one.
//!
//! Each helper app (`dissent-seeder`, `dissent-relay`, `dissent-push-relay`) carries its own copy
//! of this file, with its own [`Spec`] and default level. The copies are kept identical except for
//! that spec.
//!
//! Platform mapping:
//! - Windows, user level: a logon-triggered Task Scheduler task, hidden, restarted on failure.
//! - Windows, system level: a real Windows service, started by the Service Control Manager.
//! - Linux: a systemd unit, `--user` or system.
//! - macOS: a launchd agent (user) or daemon (system).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Names and defaults for one helper.
pub struct Spec {
    /// systemd unit name, Windows service name, and file stem for launchd.
    pub name: &'static str,
    /// Windows scheduled-task name, or service display name.
    pub title: &'static str,
    /// launchd label.
    pub label: &'static str,
    /// One line for systemd and the service manager.
    pub description: &'static str,
    /// Options whose values must be absolute paths. A service has no shell and a fixed working
    /// directory, so a relative path would resolve somewhere unexpected.
    pub path_flags: &'static [&'static str],
    /// Install at user level unless `--system` is given.
    pub user_by_default: bool,
}

/// Where a service is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Runs for the logged-in user, with that user's credential store.
    User,
    /// Runs for the whole machine.
    System,
}

/// What `install` writes: the program, its recorded arguments, and where it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub exe: PathBuf,
    /// The run arguments, exactly as the user gave them after `--`.
    pub args: Vec<String>,
    /// Working directory of the service. Absolute.
    pub work_dir: PathBuf,
    /// Directories the service may write to. Used by the hardened systemd unit.
    pub writable: Vec<PathBuf>,
    /// The service binds a port below 1024, so on Linux it needs `CAP_NET_BIND_SERVICE`.
    pub low_port: bool,
    /// Linux system unit runs as this user. Root when `None`.
    pub run_as: Option<String>,
}

/// What `status` reports.
#[derive(Debug, Default)]
pub struct Status {
    pub installed: bool,
    /// `None` when the platform cannot tell.
    pub running: Option<bool>,
    pub last_start: Option<String>,
    /// Where the service is, for example "Windows scheduled task Dissent Seeder".
    pub detail: String,
}

/// Quotes one word for a systemd `ExecStart=` line.
pub fn systemd_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-:=,@+".contains(c));
    if plain {
        return arg.to_string();
    }
    let escaped = arg
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "$$")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

/// Quotes one argument the way the Microsoft C runtime parses a command line.
pub fn windows_word(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// Escapes text for an XML element or attribute.
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The systemd unit text. User units skip the system hardening, which needs root.
pub fn systemd_unit(spec: &Spec, level: Level, plan: &Plan) -> String {
    let exec: Vec<String> = std::iter::once(plan.exe.to_string_lossy().into_owned())
        .chain(plan.args.iter().cloned())
        .map(|w| systemd_word(&w))
        .collect();
    let mut unit = format!(
        "[Unit]\nDescription={}\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nExecStart={}\nWorkingDirectory={}\nRestart=on-failure\nRestartSec=5\n\
         KillSignal=SIGTERM\nTimeoutStopSec=10\n",
        spec.description,
        exec.join(" "),
        plan.work_dir.display()
    );
    if level == Level::System {
        if let Some(user) = &plan.run_as {
            unit.push_str(&format!("User={user}\n"));
        }
        unit.push_str("NoNewPrivileges=true\nProtectSystem=strict\nProtectHome=true\nPrivateTmp=true\n");
        if !plan.writable.is_empty() {
            let paths: Vec<String> = plan
                .writable
                .iter()
                .map(|p| systemd_word(&p.to_string_lossy()))
                .collect();
            unit.push_str(&format!("ReadWritePaths={}\n", paths.join(" ")));
        }
        if plan.low_port {
            unit.push_str(
                "AmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\n",
            );
        }
    }
    let target = match level {
        Level::User => "default.target",
        Level::System => "multi-user.target",
    };
    unit.push_str(&format!("\n[Install]\nWantedBy={target}\n"));
    unit
}

/// The launchd property list. KeepAlive restarts the job after a failure, not after a clean exit.
pub fn launchd_plist(spec: &Spec, plan: &Plan, log: &Path) -> String {
    let mut args = String::new();
    let program = std::iter::once(plan.exe.to_string_lossy().into_owned())
        .chain(plan.args.iter().cloned());
    for arg in program {
        args.push_str(&format!("    <string>{}</string>\n", xml_escape(&arg)));
    }
    let log = xml_escape(&log.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20   <key>Label</key>\n    <string>{label}</string>\n\
         \x20   <key>ProgramArguments</key>\n    <array>\n{args}    </array>\n\
         \x20   <key>WorkingDirectory</key>\n    <string>{wd}</string>\n\
         \x20   <key>RunAtLoad</key>\n    <true/>\n\
         \x20   <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n\
         \x20   <key>ThrottleInterval</key>\n    <integer>5</integer>\n\
         \x20   <key>ExitTimeOut</key>\n    <integer>10</integer>\n\
         \x20   <key>StandardOutPath</key>\n    <string>{log}</string>\n\
         \x20   <key>StandardErrorPath</key>\n    <string>{log}</string>\n\
         </dict>\n</plist>\n",
        label = xml_escape(spec.label),
        wd = xml_escape(&plan.work_dir.to_string_lossy()),
    )
}

/// The Task Scheduler definition for a user-level Windows task. It starts at logon, runs hidden,
/// restarts after a failure, and has no time limit.
pub fn task_xml(spec: &Spec, plan: &Plan, user: &str) -> String {
    let args: Vec<String> = plan.args.iter().map(|a| windows_word(a)).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
         \x20 <RegistrationInfo>\n    <Description>{desc}</Description>\n  </RegistrationInfo>\n\
         \x20 <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>{user}</UserId>\n    </LogonTrigger>\n  </Triggers>\n\
         \x20 <Principals>\n    <Principal id=\"Author\">\n      <UserId>{user}</UserId>\n      <LogonType>InteractiveToken</LogonType>\n      <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n  </Principals>\n\
         \x20 <Settings>\n    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
         \x20   <AllowHardTerminate>true</AllowHardTerminate>\n\
         \x20   <StartWhenAvailable>true</StartWhenAvailable>\n\
         \x20   <Hidden>true</Hidden>\n\
         \x20   <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
         \x20   <RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>999</Count>\n    </RestartOnFailure>\n\
         \x20   <Enabled>true</Enabled>\n    <AllowStartOnDemand>true</AllowStartOnDemand>\n\
         \x20   <IdleSettings>\n      <StopOnIdleEnd>false</StopOnIdleEnd>\n      <RestartOnIdle>false</RestartOnIdle>\n    </IdleSettings>\n\
         \x20 </Settings>\n\
         \x20 <Actions Context=\"Author\">\n    <Exec>\n      <Command>{exe}</Command>\n      <Arguments>{args}</Arguments>\n      <WorkingDirectory>{wd}</WorkingDirectory>\n    </Exec>\n  </Actions>\n\
         </Task>\n",
        desc = xml_escape(spec.description),
        user = xml_escape(user),
        exe = xml_escape(&plan.exe.to_string_lossy()),
        args = xml_escape(&args.join(" ")),
        wd = xml_escape(&plan.work_dir.to_string_lossy()),
    )
}

/// UTF-16 little endian with a byte order mark, the encoding Task Scheduler XML files use.
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Checks that every path-valued option in `args` is absolute. Accepts `--flag value` and
/// `--flag=value`.
pub fn check_absolute_paths(args: &[String], path_flags: &[&str]) -> Result<()> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (flag, value) = match arg.split_once('=') {
            Some((f, v)) if arg.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        if path_flags.contains(&flag) {
            let value = match value {
                Some(v) => v,
                None => match args.get(i + 1) {
                    Some(v) => {
                        i += 1;
                        v.clone()
                    }
                    None => bail!("{flag} needs a value"),
                },
            };
            if !Path::new(&value).is_absolute() {
                bail!("{flag} must be an absolute path when installed as a service, got {value}");
            }
        }
        i += 1;
    }
    Ok(())
}

/// The command line the service runs, quoted for Windows. Shown to the user after install.
pub fn command_line(plan: &Plan) -> String {
    std::iter::once(plan.exe.to_string_lossy().into_owned())
        .chain(plan.args.iter().map(|a| windows_word(a)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns `args` with `--log-dir <dir>` appended, unless the user already gave one. The log
/// directory is the data directory's `logs` folder.
pub fn with_log_dir(args: &[String], data_dir: &Path) -> Vec<String> {
    let has = args
        .iter()
        .any(|a| a == "--log-dir" || a.starts_with("--log-dir="));
    let mut out = args.to_vec();
    if !has {
        out.push("--log-dir".into());
        out.push(data_dir.join("logs").to_string_lossy().into_owned());
    }
    out
}

/// The level to use: `--system` wins, then `--user`, then the spec's default.
pub fn level_for(spec: &Spec, user: bool, system: bool) -> Result<Level> {
    if user && system {
        bail!("pass either --user or --system, not both");
    }
    Ok(match (user, system) {
        (true, _) => Level::User,
        (_, true) => Level::System,
        _ if spec.user_by_default => Level::User,
        _ => Level::System,
    })
}

fn run_tool(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Installs the service. The output names the file or task written and the recorded command.
pub fn install(spec: &Spec, level: Level, plan: &Plan) -> Result<String> {
    check_absolute_paths(&plan.args, spec.path_flags)?;
    platform::install(spec, level, plan)
}

pub fn uninstall(spec: &Spec, level: Level, work_dir: Option<&Path>) -> Result<String> {
    platform::uninstall(spec, level, work_dir)
}

pub fn start(spec: &Spec, level: Level, work_dir: Option<&Path>) -> Result<String> {
    platform::start(spec, level, work_dir)
}

pub fn stop(spec: &Spec, level: Level, work_dir: Option<&Path>) -> Result<String> {
    platform::stop(spec, level, work_dir)
}

pub fn status(spec: &Spec, level: Level) -> Result<Status> {
    platform::status(spec, level)
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    fn unit_path(spec: &Spec, level: Level) -> Result<PathBuf> {
        Ok(match level {
            Level::User => {
                let home = std::env::var_os("HOME").context("HOME is not set")?;
                PathBuf::from(home).join(".config/systemd/user")
            }
            Level::System => PathBuf::from("/etc/systemd/system"),
        })
        .map(|dir| dir.join(format!("{}.service", spec.name)))
    }

    fn systemctl(level: Level, args: &[&str]) -> Result<String> {
        let mut all: Vec<&str> = Vec::new();
        if level == Level::User {
            all.push("--user");
        }
        all.extend_from_slice(args);
        run_tool("systemctl", &all)
    }

    pub fn install(spec: &Spec, level: Level, plan: &Plan) -> Result<String> {
        let path = unit_path(spec, level)?;
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
            .with_context(|| format!("create {}", path.display()))?;
        std::fs::write(&path, systemd_unit(spec, level, plan))
            .with_context(|| format!("write {}", path.display()))?;
        systemctl(level, &["daemon-reload"])?;
        systemctl(level, &["enable", "--now", &format!("{}.service", spec.name)])?;
        let mut msg = format!("installed {}\n", path.display());
        if level == Level::User {
            msg.push_str(
                "This user service stops at logout. To keep it running while you are logged out, run:\n  loginctl enable-linger $USER\n",
            );
        }
        Ok(msg)
    }

    pub fn uninstall(spec: &Spec, level: Level, _work_dir: Option<&Path>) -> Result<String> {
        let unit = format!("{}.service", spec.name);
        let _ = systemctl(level, &["disable", "--now", &unit]);
        let path = unit_path(spec, level)?;
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
        systemctl(level, &["daemon-reload"])?;
        Ok(format!("removed {}\n", path.display()))
    }

    pub fn start(spec: &Spec, level: Level, _work_dir: Option<&Path>) -> Result<String> {
        systemctl(level, &["start", &format!("{}.service", spec.name)])?;
        Ok("started\n".into())
    }

    pub fn stop(spec: &Spec, level: Level, _work_dir: Option<&Path>) -> Result<String> {
        systemctl(level, &["stop", &format!("{}.service", spec.name)])?;
        Ok("stopped\n".into())
    }

    pub fn status(spec: &Spec, level: Level) -> Result<Status> {
        let unit = format!("{}.service", spec.name);
        let installed = unit_path(spec, level)?.exists();
        let active = Command::new("systemctl")
            .args(level_args(level))
            .args(["is-active", &unit])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
            .unwrap_or(false);
        let last_start = Command::new("systemctl")
            .args(level_args(level))
            .args(["show", "-p", "ActiveEnterTimestamp", "--value", &unit])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(Status {
            installed,
            running: Some(active),
            last_start,
            detail: format!("systemd unit {unit} ({})", level_name(level)),
        })
    }

    fn level_args(level: Level) -> Vec<&'static str> {
        if level == Level::User { vec!["--user"] } else { vec![] }
    }

    fn level_name(level: Level) -> &'static str {
        if level == Level::User { "user" } else { "system" }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    fn plist_path(spec: &Spec, level: Level) -> Result<PathBuf> {
        let dir = match level {
            Level::User => {
                let home = std::env::var_os("HOME").context("HOME is not set")?;
                PathBuf::from(home).join("Library/LaunchAgents")
            }
            Level::System => PathBuf::from("/Library/LaunchDaemons"),
        };
        Ok(dir.join(format!("{}.plist", spec.label)))
    }

    fn domain(level: Level) -> Result<String> {
        Ok(match level {
            Level::User => format!("gui/{}", run_tool("id", &["-u"])?.trim()),
            Level::System => "system".to_string(),
        })
    }

    pub fn install(spec: &Spec, level: Level, plan: &Plan) -> Result<String> {
        let path = plist_path(spec, level)?;
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
            .with_context(|| format!("create {}", path.display()))?;
        let log = plan.work_dir.join("logs").join("launchd.log");
        std::fs::create_dir_all(plan.work_dir.join("logs")).ok();
        std::fs::write(&path, launchd_plist(spec, plan, &log))
            .with_context(|| format!("write {}", path.display()))?;
        let dom = domain(level)?;
        let _ = run_tool("launchctl", &["bootout", &dom, &path.to_string_lossy()]);
        run_tool("launchctl", &["bootstrap", &dom, &path.to_string_lossy()])?;
        Ok(format!("installed {}\n", path.display()))
    }

    pub fn uninstall(spec: &Spec, level: Level, _work_dir: Option<&Path>) -> Result<String> {
        let path = plist_path(spec, level)?;
        let dom = domain(level)?;
        let _ = run_tool("launchctl", &["bootout", &dom, &path.to_string_lossy()]);
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(format!("removed {}\n", path.display()))
    }

    pub fn start(spec: &Spec, level: Level, _work_dir: Option<&Path>) -> Result<String> {
        let dom = domain(level)?;
        run_tool("launchctl", &["kickstart", &format!("{dom}/{}", spec.label)])?;
        Ok("started\n".into())
    }

    pub fn stop(spec: &Spec, level: Level, _work_dir: Option<&Path>) -> Result<String> {
        let dom = domain(level)?;
        run_tool("launchctl", &["kill", "SIGTERM", &format!("{dom}/{}", spec.label)])?;
        Ok("stop signal sent\n".into())
    }

    pub fn status(spec: &Spec, level: Level) -> Result<Status> {
        let installed = plist_path(spec, level)?.exists();
        let listing = Command::new("launchctl")
            .args(["list"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let line = listing
            .lines()
            .find(|l| l.ends_with(spec.label));
        let running = line.map(|l| !l.trim_start().starts_with('-'));
        Ok(Status {
            installed,
            running,
            last_start: None,
            detail: format!("launchd {} ({})", spec.label, if level == Level::User { "agent" } else { "daemon" }),
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    /// The current user as Task Scheduler names them, for example `HOST\me`.
    fn current_user() -> String {
        let domain = std::env::var("USERDOMAIN").unwrap_or_default();
        let name = std::env::var("USERNAME").unwrap_or_default();
        if domain.is_empty() {
            name
        } else {
            format!("{domain}\\{name}")
        }
    }

    fn task_status(spec: &Spec) -> Result<Option<String>> {
        let out = Command::new("schtasks")
            .args(["/Query", "/TN", spec.title, "/V", "/FO", "LIST"])
            .output()
            .context("run schtasks")?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    pub fn install(spec: &Spec, level: Level, plan: &Plan) -> Result<String> {
        match level {
            Level::User => {
                let xml = task_xml(spec, plan, &current_user());
                let file = std::env::temp_dir().join(format!("{}-task-{}.xml", spec.name, std::process::id()));
                std::fs::write(&file, utf16_with_bom(&xml))
                    .with_context(|| format!("write {}", file.display()))?;
                let result = run_tool(
                    "schtasks",
                    &["/Create", "/TN", spec.title, "/XML", &file.to_string_lossy(), "/F"],
                );
                let _ = std::fs::remove_file(&file);
                result?;
                Ok(format!("created scheduled task \"{}\" for {}\n", spec.title, current_user()))
            }
            Level::System => windows_svc::install(spec, plan),
        }
    }

    pub fn uninstall(spec: &Spec, level: Level, work_dir: Option<&Path>) -> Result<String> {
        match level {
            Level::User => {
                if let Some(dir) = work_dir {
                    let _ = std::fs::remove_file(dir.join(super::STOP_FILE));
                }
                let _ = run_tool("schtasks", &["/End", "/TN", spec.title]);
                run_tool("schtasks", &["/Delete", "/TN", spec.title, "/F"])?;
                Ok(format!("deleted scheduled task \"{}\"\n", spec.title))
            }
            Level::System => windows_svc::uninstall(spec),
        }
    }

    pub fn start(spec: &Spec, level: Level, work_dir: Option<&Path>) -> Result<String> {
        match level {
            Level::User => {
                if let Some(dir) = work_dir {
                    let _ = std::fs::remove_file(dir.join(super::STOP_FILE));
                }
                run_tool("schtasks", &["/Run", "/TN", spec.title])?;
                Ok("started\n".into())
            }
            Level::System => windows_svc::start(spec),
        }
    }

    pub fn stop(spec: &Spec, level: Level, work_dir: Option<&Path>) -> Result<String> {
        match level {
            Level::User => {
                // A hidden task has no console to signal. The program polls for this file and
                // shuts down cleanly. Hard-end the task only if it does not stop in time.
                let dir = work_dir.context("stop needs --data-dir for a user-level task")?;
                std::fs::write(dir.join(super::STOP_FILE), b"stop\n")
                    .context("write the stop request")?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let running = task_status(spec)?
                        .and_then(|l| field(&l, "Status"))
                        .is_some_and(|s| s == "Running");
                    if !running {
                        return Ok("stopped\n".into());
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                run_tool("schtasks", &["/End", "/TN", spec.title])?;
                Ok("did not stop in 5 s; ended the task\n".into())
            }
            Level::System => windows_svc::stop(spec),
        }
    }

    pub fn status(spec: &Spec, level: Level) -> Result<Status> {
        match level {
            Level::User => match task_status(spec)? {
                None => Ok(Status {
                    installed: false,
                    detail: format!("scheduled task \"{}\" not found", spec.title),
                    ..Status::default()
                }),
                Some(listing) => Ok(Status {
                    installed: true,
                    running: field(&listing, "Status").map(|s| s == "Running"),
                    last_start: field(&listing, "Last Run Time"),
                    detail: format!("scheduled task \"{}\" (user {})", spec.title, current_user()),
                }),
            },
            Level::System => windows_svc::status(spec),
        }
    }

    /// Windows system service, through the Service Control Manager.
    pub mod windows_svc {
        use super::*;
        use std::ffi::OsString;
        use windows_service::service::{
            ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl,
            ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType,
            ServiceState, ServiceType,
        };
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
            ServiceManager::local_computer(None::<&str>, access)
                .context("open the Service Control Manager (run as administrator)")
        }

        pub fn install(spec: &Spec, plan: &Plan) -> Result<String> {
            let manager = manager(ServiceManagerAccess::CREATE_SERVICE)?;
            let info = ServiceInfo {
                name: OsString::from(spec.name),
                display_name: OsString::from(spec.title),
                service_type: ServiceType::OWN_PROCESS,
                start_type: ServiceStartType::AutoStart,
                error_control: ServiceErrorControl::Normal,
                executable_path: plan.exe.clone(),
                launch_arguments: plan.args.iter().map(OsString::from).collect(),
                dependencies: vec![],
                account_name: None,
                account_password: None,
            };
            let service = manager
                .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
                .context("create the service")?;
            let _ = service.set_description(spec.description);
            service
                .update_failure_actions(ServiceFailureActions {
                    reset_period: ServiceFailureResetPeriod::After(std::time::Duration::from_secs(86_400)),
                    reboot_msg: None,
                    command: None,
                    actions: Some(vec![
                        ServiceAction {
                            action_type: ServiceActionType::Restart,
                            delay: std::time::Duration::from_secs(5),
                        };
                        3
                    ]),
                })
                .context("set restart-on-failure")?;
            Ok(format!("created Windows service \"{}\"\n", spec.name))
        }

        pub fn uninstall(spec: &Spec) -> Result<String> {
            let manager = manager(ServiceManagerAccess::CONNECT)?;
            let service = manager
                .open_service(spec.name, ServiceAccess::DELETE | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)
                .context("open the service")?;
            if service.query_status().map(|s| s.current_state != ServiceState::Stopped).unwrap_or(false) {
                let _ = service.stop();
            }
            service.delete().context("delete the service")?;
            Ok(format!("deleted Windows service \"{}\"\n", spec.name))
        }

        pub fn start(spec: &Spec) -> Result<String> {
            let manager = manager(ServiceManagerAccess::CONNECT)?;
            let service = manager
                .open_service(spec.name, ServiceAccess::START)
                .context("open the service")?;
            service.start::<&str>(&[]).context("start the service")?;
            Ok("started\n".into())
        }

        pub fn stop(spec: &Spec) -> Result<String> {
            let manager = manager(ServiceManagerAccess::CONNECT)?;
            let service = manager
                .open_service(spec.name, ServiceAccess::STOP)
                .context("open the service")?;
            service.stop().context("stop the service")?;
            Ok("stop requested\n".into())
        }

        pub fn status(spec: &Spec) -> Result<Status> {
            let manager = match ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) {
                Ok(m) => m,
                Err(_) => {
                    return Ok(Status {
                        detail: "cannot open the Service Control Manager".into(),
                        ..Status::default()
                    });
                }
            };
            match manager.open_service(spec.name, ServiceAccess::QUERY_STATUS) {
                Err(_) => Ok(Status {
                    detail: format!("Windows service \"{}\" not found", spec.name),
                    ..Status::default()
                }),
                Ok(service) => {
                    let state = service.query_status().context("query the service")?;
                    Ok(Status {
                        installed: true,
                        running: Some(state.current_state == ServiceState::Running),
                        last_start: None,
                        detail: format!("Windows service \"{}\" (system)", spec.name),
                    })
                }
            }
        }
    }
}

/// The file a user-level task polls for. Writing it asks the program to shut down.
pub const STOP_FILE: &str = "stop.request";

/// The value of one `Name: value` line of `schtasks /Query /V /FO LIST` output.
#[cfg(any(windows, test))]
fn field(listing: &str, name: &str) -> Option<String> {
    listing
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|rest| rest.strip_prefix(':')))
        .map(|v| v.trim().to_string())
}

/// Windows only. Runs the program as a Windows service when the Service Control Manager started
/// it. Returns `None` when a console started it, so the caller runs normally.
#[cfg(windows)]
pub mod scm {
    use super::Spec;
    use anyhow::{Context, Result};
    use std::ffi::OsString;
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;
    use tokio::sync::Notify;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    /// Runs the program until `stop` is notified. Gets the run arguments from the process.
    pub type Runner = fn(Vec<String>, Arc<Notify>) -> Result<()>;

    static NAME: OnceLock<&'static str> = OnceLock::new();
    static RUNNER: OnceLock<Runner> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            tracing::error!(error = %e, "service failed");
        }
    }

    fn set_state(
        status: &service_control_handler::ServiceStatusHandle,
        state: ServiceState,
    ) -> Result<()> {
        let accepted = if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        };
        status
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accepted,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })
            .context("report service state")
    }

    fn run_service() -> Result<()> {
        let name = NAME.get().copied().context("service name not set")?;
        let runner = RUNNER.get().copied().context("runner not set")?;
        let stop = Arc::new(Notify::new());
        let handler_stop = stop.clone();
        let status = service_control_handler::register(name, move |event| match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                handler_stop.notify_one();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })
        .context("register the service control handler")?;
        set_state(&status, ServiceState::Running)?;
        let args: Vec<String> = std::env::args().skip(1).collect();
        let result = runner(args, stop);
        set_state(&status, ServiceState::Stopped)?;
        result
    }

    /// `None` when a console started the process.
    pub fn run_if_service(spec: &Spec, runner: Runner) -> Option<Result<()>> {
        let _ = NAME.set(spec.name);
        let _ = RUNNER.set(runner);
        match service_dispatcher::start(spec.name, ffi_service_main) {
            Ok(()) => Some(Ok(())),
            Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(1063) => None,
            Err(e) => Some(Err(e.into())),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;

    pub fn install(_: &Spec, _: Level, _: &Plan) -> Result<String> {
        bail!("services are not supported on this platform")
    }
    pub fn uninstall(_: &Spec, _: Level, _: Option<&Path>) -> Result<String> {
        bail!("services are not supported on this platform")
    }
    pub fn start(_: &Spec, _: Level, _: Option<&Path>) -> Result<String> {
        bail!("services are not supported on this platform")
    }
    pub fn stop(_: &Spec, _: Level, _: Option<&Path>) -> Result<String> {
        bail!("services are not supported on this platform")
    }
    pub fn status(_: &Spec, _: Level) -> Result<Status> {
        bail!("services are not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: Spec = Spec {
        name: "dissent-test",
        title: "Dissent Test",
        label: "org.dissent.test",
        description: "Test service",
        path_flags: &["--data-dir", "--cert"],
        user_by_default: true,
    };

    fn plan(exe: &str, args: &[&str], wd: &str) -> Plan {
        Plan {
            exe: PathBuf::from(exe),
            args: args.iter().map(|s| s.to_string()).collect(),
            work_dir: PathBuf::from(wd),
            writable: vec![PathBuf::from(wd)],
            low_port: false,
            run_as: None,
        }
    }

    #[test]
    fn systemd_quotes_paths_with_spaces_and_specials() {
        assert_eq!(systemd_word("plain-arg"), "plain-arg");
        assert_eq!(systemd_word("H:/Rust Projects/x"), "\"H:/Rust Projects/x\"");
        assert_eq!(systemd_word("a\"b"), "\"a\\\"b\"");
        assert_eq!(systemd_word("$HOME%x"), "\"$$HOME%%x\"");
        assert_eq!(systemd_word(""), "\"\"");
    }

    #[test]
    fn windows_words_follow_the_c_runtime_rules() {
        assert_eq!(windows_word("plain"), "plain");
        assert_eq!(windows_word("H:/Rust Projects/x"), "\"H:/Rust Projects/x\"");
        assert_eq!(windows_word("say \"hi\""), "\"say \\\"hi\\\"\"");
        // Backslashes before a quote are doubled, and so is a trailing run before the closing quote.
        assert_eq!(windows_word("a b\\"), "\"a b\\\\\"");
        assert_eq!(windows_word("C:\\dir\\"), "C:\\dir\\");
        assert_eq!(windows_word(""), "\"\"");
    }

    #[test]
    fn xml_escape_covers_markup_characters() {
        assert_eq!(xml_escape("a&b<c>\"d'"), "a&amp;b&lt;c&gt;&quot;d&apos;");
    }

    #[test]
    fn systemd_user_unit_has_arguments_and_paths_quoted() {
        let p = plan(
            "H:/Rust Projects/Dissent/target/debug/dissent-seeder.exe",
            &["--data-dir", "H:/Rust Projects/seed data", "--loopback"],
            "H:/Rust Projects/seed data",
        );
        let unit = systemd_unit(&SPEC, Level::User, &p);
        assert!(unit.contains(
            "ExecStart=\"H:/Rust Projects/Dissent/target/debug/dissent-seeder.exe\" --data-dir \"H:/Rust Projects/seed data\" --loopback\n"
        ), "{unit}");
        assert!(unit.contains("WorkingDirectory=H:/Rust Projects/seed data\n"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("ProtectSystem"), "user units skip root-only hardening");
    }

    #[test]
    fn systemd_system_unit_is_hardened_and_grants_low_ports_only_when_needed() {
        let mut p = plan("/usr/local/bin/dissent-relay", &["--bind", "0.0.0.0:443"], "/var/lib/dissent-relay");
        p.low_port = true;
        let unit = systemd_unit(&SPEC, Level::System, &p);
        for line in [
            "NoNewPrivileges=true",
            "ProtectSystem=strict",
            "ProtectHome=true",
            "PrivateTmp=true",
            "ReadWritePaths=/var/lib/dissent-relay",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE",
            "WantedBy=multi-user.target",
        ] {
            assert!(unit.contains(line), "missing {line}:\n{unit}");
        }
        p.low_port = false;
        assert!(!systemd_unit(&SPEC, Level::System, &p).contains("AmbientCapabilities"));
    }

    #[test]
    fn launchd_plist_lists_each_argument_and_escapes_xml() {
        let p = plan("/opt/dissent/bin/seeder", &["--name", "Tom & Jerry <3"], "/var/lib/seed");
        let plist = launchd_plist(&SPEC, &p, Path::new("/var/lib/seed/logs/launchd.log"));
        assert!(plist.contains("<string>/opt/dissent/bin/seeder</string>"));
        assert!(plist.contains("<string>Tom &amp; Jerry &lt;3</string>"));
        assert!(plist.contains("<key>Label</key>\n    <string>org.dissent.test</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
        assert!(plist.contains("<string>/var/lib/seed/logs/launchd.log</string>"));
    }

    #[test]
    fn task_xml_is_hidden_restarting_and_quotes_arguments() {
        let p = plan(
            "H:/Rust Projects/Dissent/target/debug/dissent-seeder.exe",
            &["--data-dir", "H:/Rust Projects/seed data", "--name", "a \"b\""],
            "H:/Rust Projects/seed data",
        );
        let xml = task_xml(&SPEC, &p, "HOST\\me");
        assert!(xml.contains("<Hidden>true</Hidden>"));
        assert!(xml.contains("<RestartOnFailure>"));
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<UserId>HOST\\me</UserId>"));
        assert!(xml.contains("<Command>H:/Rust Projects/Dissent/target/debug/dissent-seeder.exe</Command>"));
        assert!(
            xml.contains(
                "<Arguments>--data-dir &quot;H:/Rust Projects/seed data&quot; --name &quot;a \\&quot;b\\&quot;&quot;</Arguments>"
            ),
            "{xml}"
        );
        assert!(xml.contains("<WorkingDirectory>H:/Rust Projects/seed data</WorkingDirectory>"));
    }

    #[test]
    fn task_file_is_utf16_with_bom() {
        let bytes = utf16_with_bom("<a/>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(bytes.len(), 2 + 4 * 2);
        assert_eq!(&bytes[2..4], &[b'<', 0]);
    }

    #[test]
    fn relative_paths_are_refused() {
        let args: Vec<String> = ["--data-dir", "relative/dir"].iter().map(|s| s.to_string()).collect();
        assert!(check_absolute_paths(&args, SPEC.path_flags).is_err());
        let args: Vec<String> = ["--cert=cert.pem"].iter().map(|s| s.to_string()).collect();
        assert!(check_absolute_paths(&args, SPEC.path_flags).is_err());
        let absolute = std::env::temp_dir().to_string_lossy().into_owned();
        let args: Vec<String> = vec!["--data-dir".into(), absolute, "--name".into(), "n".into()];
        assert!(check_absolute_paths(&args, SPEC.path_flags).is_ok());
    }

    #[test]
    fn log_dir_is_added_unless_given() {
        let args: Vec<String> = vec!["--data-dir".into(), "/d".into()];
        let with = with_log_dir(&args, Path::new("/d"));
        assert_eq!(with.last().unwrap(), &Path::new("/d").join("logs").to_string_lossy().into_owned());
        let given: Vec<String> = vec!["--log-dir=/x".into()];
        assert_eq!(with_log_dir(&given, Path::new("/d")), given);
    }

    #[test]
    fn level_defaults_and_conflicts() {
        assert_eq!(level_for(&SPEC, false, false).unwrap(), Level::User);
        assert_eq!(level_for(&SPEC, false, true).unwrap(), Level::System);
        assert!(level_for(&SPEC, true, true).is_err());
    }

    #[test]
    fn schtasks_field_parser_reads_status_line() {
        let listing = "Folder: \\\nTaskName: Dissent Seeder\nStatus: Running\nLast Run Time: 10/24/2026 9:00:00 AM\n";
        assert_eq!(field(listing, "Status").as_deref(), Some("Running"));
        assert_eq!(field(listing, "Last Run Time").as_deref(), Some("10/24/2026 9:00:00 AM"));
    }
}
