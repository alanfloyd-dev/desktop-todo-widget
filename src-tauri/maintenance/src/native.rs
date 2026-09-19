use crate::{
    lifecycle::{self, DataMode},
    lock::Gate,
    paths::{self, Paths},
    receipt, Error, ErrorKind, Result, HELPER_EXE,
};
use std::{os::windows::process::CommandExt, process::Command};
use windows::{
    core::{GUID, PCWSTR},
    Win32::{
        System::Com::*,
        UI::{Controls::*, Shell::*, WindowsAndMessaging::*},
    },
};

pub fn message(text: &str, error: bool) {
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(paths::wide(text).as_ptr()),
            windows::core::w!("desktop-todo-widget Maintenance"),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        );
    }
}
pub fn confirm(paths: &Paths) -> Result<Option<DataMode>> {
    let keep = paths::wide("Keep local data and settings (Recommended)");
    let remove = paths::wide("Delete local data and settings");
    let uninstall = paths::wide("Uninstall");
    let content = paths::wide(format!(
        "Remove desktop-todo-widget from this Windows account.\nLocal data: {}",
        paths.data().display()
    ));
    let buttons = [TASKDIALOG_BUTTON {
        nButtonID: 100,
        pszButtonText: PCWSTR(uninstall.as_ptr()),
    }];
    let radios = [
        TASKDIALOG_BUTTON {
            nButtonID: 200,
            pszButtonText: PCWSTR(keep.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: 201,
            pszButtonText: PCWSTR(remove.as_ptr()),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: windows::core::w!("desktop-todo-widget Maintenance"),
        pszMainInstruction: windows::core::w!("Uninstall desktop-todo-widget"),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: IDCANCEL.0,
        cRadioButtons: if lifecycle::can_remove_data(paths) {
            2
        } else {
            1
        },
        pRadioButtons: radios.as_ptr(),
        nDefaultRadioButton: 200,
        ..Default::default()
    };
    let mut button = 0;
    let mut radio = 200;
    unsafe {
        TaskDialogIndirect(&config, Some(&mut button), Some(&mut radio), None)?;
    }
    if button != 100 {
        return Ok(None);
    }
    if radio == 201 {
        let warning = paths::wide(format!("Delete local data and settings?\n\nThis removes tasks, settings, review history, quick links and app-owned images from:\n{}\n\nThis cannot be undone. Unknown files will be preserved.", paths.data().display()));
        if unsafe {
            MessageBoxW(
                None,
                PCWSTR(warning.as_ptr()),
                windows::core::w!("Confirm permanent data deletion"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
            )
        } != IDYES
        {
            return Ok(None);
        }
        return Ok(Some(DataMode::RemoveUserData));
    }
    Ok(Some(DataMode::KeepUserData))
}
struct Progress(IProgressDialog);
impl Progress {
    fn start() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let result = (|| {
                let dialog: IProgressDialog = CoCreateInstance(
                    &GUID::from_u128(0xf8383852_fcd3_11d1_a6b9_006097df5bd4),
                    None,
                    CLSCTX_INPROC_SERVER,
                )?;
                dialog.SetTitle(windows::core::w!("desktop-todo-widget Maintenance"))?;
                dialog.SetLine(1, windows::core::w!("Removing application..."), false, None)?;
                dialog.StartProgressDialog(
                    None,
                    None,
                    PROGDLG_MARQUEEPROGRESS | PROGDLG_NOCANCEL | PROGDLG_NOTIME,
                    None,
                )?;
                Ok(Self(dialog))
            })();
            if result.is_err() {
                CoUninitialize();
            }
            result
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.StopProgressDialog();
            CoUninitialize();
        }
    }
}

/// Runner is launched before asking for consent. No consent/lock is transferred
/// via mutable files or command-line arguments; the runner owns both itself.
/// QA interception uses the same handoff so no flow ever deletes a running
/// image.
pub fn runner_handoff(paths: &Paths) -> Result<bool> {
    let current = std::env::current_exe()?;
    let parent = current
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::UnsafePath, "Helper has no parent"))?;
    let runners = paths.state().join("runners");
    if parent.parent().is_some_and(|p| paths::equal(p, &runners)) {
        let id = parent
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::new(ErrorKind::UnsafePath, "Invalid runner identity"))?;
        receipt::validate_uuid(id)?;
        let expected = std::fs::read_to_string(parent.join("runner.sha256"))?;
        if receipt::fingerprint(&current)?.1 != expected {
            return Err(Error::new(ErrorKind::InvalidInstallation, "Runner changed"));
        }
        return Ok(false);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let destination = runners.join(id);
    paths::create_dir(&destination)?;
    let target = destination.join(HELPER_EXE);
    let (size, hash) = receipt::fingerprint(&current)?;
    let _parents = paths::pin_parents(&target)?;
    let mut input = paths::open_regular(&current)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    if receipt::fingerprint(&target)? != (size, hash.clone()) {
        return Err(Error::new(
            ErrorKind::InvalidInstallation,
            "Runner copy mismatch",
        ));
    }
    paths::atomic_write(&destination.join("runner.sha256"), hash.as_bytes())?;
    Command::new(&target)
        .arg("--uninstall")
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(true)
}
pub fn uninstall(paths: &Paths) -> Result<()> {
    if runner_handoff(paths)? {
        return Ok(());
    }
    let _gate = Gate::acquire(paths)?;
    let Some(mode) = confirm(paths)? else {
        return Ok(());
    };
    loop {
        let check = crate::lock::exclusive_app(paths)
            .and_then(|_lease| lifecycle::require_no_legacy_processes(paths));
        match check {
            Ok(()) => break,
            Err(e) if e.kind == ErrorKind::MainProcessStillRunning => {
                if unsafe {
                    MessageBoxW(
                        None,
                        PCWSTR(paths::wide(e.to_string()).as_ptr()),
                        windows::core::w!("Quit the widget to continue"),
                        MB_RETRYCANCEL | MB_ICONINFORMATION,
                    )
                } != IDRETRY
                {
                    return Ok(());
                }
            }
            Err(e) => return Err(e),
        }
    }
    let progress = Progress::start()?;
    let outcome = lifecycle::uninstall_locked(paths, mode);
    drop(progress);
    let outcome = outcome?;
    message(
        &format!(
            "Application removed.\n{}\n{}",
            if mode == DataMode::KeepUserData {
                "Local data and settings were kept."
            } else {
                "Known app-owned local data was removed."
            },
            outcome.preserved.join("\n")
        ),
        false,
    );
    Ok(())
}
