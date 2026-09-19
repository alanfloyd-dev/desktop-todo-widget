#![windows_subsystem = "windows"]
use desktop_todo_maintenance::{lifecycle, native, paths::Paths, Error, ErrorKind, Result};
fn run() -> Result<()> {
    let paths = Paths::resolve()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode] if mode == "--uninstall" => {
            #[cfg(feature = "qa")]
            if std::env::var("DTW_MAINTENANCE_QA_ID").is_ok() {
                // QA interception of the production --uninstall entry point,
                // keeping the same runner handoff: this image must exit before
                // anything deletes the installed helper.
                if native::runner_handoff(&paths)? {
                    return Ok(());
                }
                // Running as the session runner now. With a data choice set,
                // the real uninstall transaction runs unattended (consent
                // replaced by the explicit env choice); without one, the
                // handoff probe stops before any deletion.
                return match std::env::var("DTW_MAINTENANCE_QA_UNINSTALL_DATA")
                    .as_deref()
                {
                    Ok("keep") => {
                        qa_unattended_uninstall(&paths, lifecycle::DataMode::KeepUserData)
                    }
                    Ok("remove") => {
                        qa_unattended_uninstall(&paths, lifecycle::DataMode::RemoveUserData)
                    }
                    _ => qa_handoff_probe(&paths),
                };
            }
            native::uninstall(&paths)
        }
        [mode] if mode == "--install" => install(&paths, true),
        [mode, flag] if mode == "--install" && flag == "--no-start-menu-shortcut" => install(&paths, false),
        #[cfg(feature = "qa")]
        [mode, choice] if mode == "--qa-uninstall" && std::env::var("DTW_MAINTENANCE_QA_ID").is_ok() => {
            let data = match choice.as_str() { "keep" => lifecycle::DataMode::KeepUserData, "remove" => lifecycle::DataMode::RemoveUserData, _ => return Err(Error::new(ErrorKind::InvalidInstallation, "QA data choice must be keep/remove")) };
            let _gate = desktop_todo_maintenance::lock::Gate::acquire(&paths)?;
            lifecycle::uninstall_locked(&paths, data)?; Ok(())
        }
        _ => Err(Error::new(ErrorKind::InvalidInstallation, "Supported modes: --install [--no-start-menu-shortcut], --uninstall. Updates are not implemented.")),
    }
}

/// QA-only uninstall handoff seam: proves the chain "helper received --uninstall,
/// gate acquired, the running app's shared lease is released" and stops before
/// any deletion.
#[cfg(feature = "qa")]
fn qa_handoff_probe(paths: &Paths) -> Result<()> {
    let _gate = desktop_todo_maintenance::lock::Gate::acquire(paths)?;
    // The quitting app may still hold the shared lease for a moment; the real
    // uninstall flow retries the same way.
    let mut lease = None;
    for _ in 0..40 {
        match desktop_todo_maintenance::lock::exclusive_app(paths) {
            Ok(acquired) => {
                lease = Some(acquired);
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    let Some(_lease) = lease else {
        return Err(Error::new(
            ErrorKind::MainProcessStillRunning,
            "qa handoff: the app lease never released",
        ));
    };
    desktop_todo_maintenance::paths::atomic_write(
        &paths.state().join("qa-handoff-marker.txt"),
        b"gate + exclusive lease acquired; no deletion performed\n",
    )?;
    Ok(())
}

/// QA-only unattended uninstall: the real `uninstall_locked` transaction with
/// the native consent UI replaced by the explicit env data choice. Waits for
/// the quitting app to release its lease exactly like the native retry loop.
#[cfg(feature = "qa")]
fn qa_unattended_uninstall(paths: &Paths, data: lifecycle::DataMode) -> Result<()> {
    let _gate = desktop_todo_maintenance::lock::Gate::acquire(paths)?;
    let mut ready = Err(Error::new(
        ErrorKind::MainProcessStillRunning,
        "qa uninstall: the app lease never released",
    ));
    for _ in 0..120 {
        ready = desktop_todo_maintenance::lock::exclusive_app(paths)
            .and_then(|_lease| lifecycle::require_no_legacy_processes(paths));
        if ready.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    ready?;
    // The transaction is idempotent and resumable. Without the native consent
    // dialog's natural delay, a real-time protection service can hold the
    // freshly exited helper image for a moment; wait for it and resume rather
    // than leaving the transaction half-done in the sandbox.
    let mut outcome = lifecycle::uninstall_locked(paths, data);
    for _ in 0..10 {
        if outcome.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        outcome = lifecycle::uninstall_locked(paths, data);
    }
    outcome?;
    desktop_todo_maintenance::paths::atomic_write(
        &paths.state().join("qa-uninstall-complete.txt"),
        format!("unattended uninstall completed: {data:?}\n").as_bytes(),
    )?;
    Ok(())
}
fn install(paths: &Paths, shortcut: bool) -> Result<()> {
    let executable = std::env::current_exe()?;
    let source = executable
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::UnsafePath, "No package directory"))?;
    lifecycle::install(paths, source, shortcut)?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        #[cfg(feature = "qa")]
        if std::env::var("DTW_MAINTENANCE_QA_ID").is_ok() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        native::message(&format!("{error}\n\nIf cleanup was interrupted, run the maintenance helper from the original trusted package again. Local recovery logs are under the app's maintenance directory."), true);
        std::process::exit(1);
    }
}
