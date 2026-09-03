//! Install / update / uninstall of the service and its files.
//!
//! Guardrails (see the module docs): the install directory is `%ProgramFiles%`,
//! resolved through the API and never hard-coded (G1); a pre-existing directory
//! is only trusted if owned by Administrators or SYSTEM (G1); the service's
//! `ImagePath` is quoted with fixed arguments (G2); paths are compared by
//! `FILE_ID_INFO`, never as strings (§8); every step is journalled (G7).

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceConfigW, QUERY_SERVICE_CONFIGW, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SERVICE_CONTROL_STOP, SERVICE_DEMAND_START,
    SERVICE_ERROR_NORMAL, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS,
    SERVICE_STATUS, SERVICE_STOP, SERVICE_WIN32_OWN_PROCESS,
};

/// The standard `DELETE` access right (`0x0001_0000`); windows-sys exposes it
/// under other namespaces, so name it here for the service handle.
const DELETE: u32 = 0x0001_0000;

use super::{CLI_EXE, EVENT_SOURCE, GUI_EXE, INSTALL_DIR, SERVICE_DISPLAY, SERVICE_NAME};
use crate::switcher::sys::quiet_command;
use crate::switcher::{is_elevated, run_self_elevated, Error, Result};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

// ---- Paths -------------------------------------------------------------------

/// `%ProgramFiles%`, from the API — never hard-coded (wrong on non-English
/// Windows or a non-`C:` install).
fn program_files() -> Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};

    let mut raw: *mut u16 = std::ptr::null_mut();
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        return Err(Error::Elevation("could not locate %ProgramFiles%".into()));
    }
    let mut len = 0usize;
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(raw, len) };
    let path = String::from_utf16_lossy(slice);
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(path))
}

fn install_dir() -> Result<PathBuf> {
    Ok(program_files()?.join(INSTALL_DIR))
}

fn installed_cli() -> Result<PathBuf> {
    Ok(install_dir()?.join(CLI_EXE))
}

/// The directory the running binary sits in — the source of an install/update.
fn source_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Elevation(format!("cannot locate the running executable: {e}")))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::Elevation("running executable has no parent directory".into()))
}

// ---- FILE_ID_INFO identity ---------------------------------------------------

/// A file's `(volume, file id)` identity — the filesystem's own notion of "the
/// same file", immune to case, 8.3 names, junctions and drive substitution.
fn file_identity(path: &Path) -> Option<(u64, [u8; 16])> {
    let wide_path = wide(&path.to_string_lossy());
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS, // also opens directories
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            windows_sys::Win32::Storage::FileSystem::FileIdInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some((info.VolumeSerialNumber, info.FileId.Identifier))
}

/// Whether two paths resolve to the same file on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    match (file_identity(a), file_identity(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

// ---- Ownership (G1) ----------------------------------------------------------

/// Whether `path` is owned by Administrators or SYSTEM — the check before
/// trusting a pre-existing install directory (TOCTOU parade).
fn owned_by_admin_or_system(path: &Path) -> bool {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid, OWNER_SECURITY_INFORMATION,
    };

    let wide_path = wide(&path.to_string_lossy());
    let mut owner: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if status != 0 || owner.is_null() {
        if !sd.is_null() {
            unsafe { LocalFree(sd) };
        }
        return false;
    }
    let admin = unsafe { IsWellKnownSid(owner, WinBuiltinAdministratorsSid) } != 0;
    let system = unsafe { IsWellKnownSid(owner, WinLocalSystemSid) } != 0;
    if !sd.is_null() {
        unsafe { LocalFree(sd) };
    }
    admin || system
}

// ---- SCM ---------------------------------------------------------------------

/// An SCM handle, closed on drop.
struct ScHandle(SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

fn open_scm(access: u32) -> Result<ScHandle> {
    let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    if h.is_null() {
        return Err(Error::Elevation(format!(
            "could not open the service manager (error {})",
            last_error()
        )));
    }
    Ok(ScHandle(h))
}

fn open_service(scm: &ScHandle, access: u32) -> Option<ScHandle> {
    let name = wide(SERVICE_NAME);
    let h = unsafe { OpenServiceW(scm.0, name.as_ptr(), access) };
    if h.is_null() {
        None
    } else {
        Some(ScHandle(h))
    }
}

/// The binary the registered service is set to launch, if the service exists.
fn registered_target() -> Option<PathBuf> {
    let scm = open_scm(SC_MANAGER_CONNECT).ok()?;
    let service = open_service(&scm, SERVICE_QUERY_CONFIG)?;

    let mut needed = 0u32;
    unsafe { QueryServiceConfigW(service.0, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let ok =
        unsafe { QueryServiceConfigW(service.0, buf.as_mut_ptr().cast(), needed, &mut needed) };
    if ok == 0 {
        return None;
    }
    let config = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    if config.lpBinaryPathName.is_null() {
        return None;
    }
    // Read the wide command line, then strip the quoted exe out of it.
    let mut len = 0usize;
    while unsafe { *config.lpBinaryPathName.add(len) } != 0 {
        len += 1;
    }
    let cmdline = from_wide(unsafe { std::slice::from_raw_parts(config.lpBinaryPathName, len) });
    Some(PathBuf::from(exe_from_command_line(&cmdline)))
}

/// Pulls the executable out of `"C:\...\os-switcher.exe" run-service`.
fn exe_from_command_line(cmdline: &str) -> String {
    let trimmed = cmdline.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    // Unquoted: take up to the first space.
    trimmed
        .split_whitespace()
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

/// Whether the service exists *and* points at the installed CLI binary that
/// really exists on disk.
pub fn is_current() -> bool {
    let (Some(registered), Ok(installed)) = (registered_target(), installed_cli()) else {
        return false;
    };
    installed.exists() && same_file(&registered, &installed)
}

// ---- Install / update --------------------------------------------------------

/// Public entry: install (self-elevating first if needed).
pub fn run_install() -> Result<()> {
    if !is_elevated() {
        return run_self_elevated(&["install"]);
    }
    do_install()?;
    log_event("service broker installed", false);
    Ok(())
}

fn do_install() -> Result<()> {
    let dir = install_dir()?;
    let src = source_dir()?;
    let src_cli = src.join(CLI_EXE);
    let src_gui = src.join(GUI_EXE);
    if !src_cli.exists() {
        return Err(Error::Elevation(format!(
            "{CLI_EXE} was not found next to this binary; keep both executables side by side"
        )));
    }

    // Directory: create it (inheriting the parent's safe ACL — never write one),
    // or, if it already exists, only trust it when Administrators/SYSTEM owns it.
    if dir.exists() {
        if !owned_by_admin_or_system(&dir) {
            return Err(Error::Elevation(format!(
                "{} exists but is not owned by Administrators/SYSTEM — refusing to use it",
                dir.display()
            )));
        }
    } else {
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::Elevation(format!("could not create {}: {e}", dir.display())))?;
    }

    // The target file is locked while the service runs.
    let _ = stop_service();

    copy_if_different(&src_cli, &dir.join(CLI_EXE))?;
    if src_gui.exists() {
        copy_if_different(&src_gui, &dir.join(GUI_EXE))?;
    }

    register_or_update_service(&dir.join(CLI_EXE))?;
    remove_legacy_task();
    write_uninstall_key(&dir)?;
    let _ = create_shortcut(&dir.join(GUI_EXE));
    // No explicit start: the pipe trigger starts the service on first use.
    Ok(())
}

fn copy_if_different(from: &Path, to: &Path) -> Result<()> {
    if to.exists() && same_file(from, to) {
        return Ok(()); // installing onto itself
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|e| {
        Error::Elevation(format!(
            "could not copy {} -> {}: {e}",
            from.display(),
            to.display()
        ))
    })
}

/// Creates the service, or re-points an existing one — `ChangeServiceConfig`
/// rather than delete/recreate, to dodge `ERROR_SERVICE_MARKED_FOR_DELETE`.
///
/// The service is **demand-start with a named-pipe trigger**: the SCM starts it
/// when a client opens the pipe and it is otherwise not running, so there is no
/// resident process and no boot cost, and the (unprivileged) client needs no
/// start permission.
fn register_or_update_service(cli: &Path) -> Result<()> {
    use windows_sys::Win32::System::Services::{SERVICE_ALL_ACCESS, SERVICE_CHANGE_CONFIG};

    // Quoted path, fixed argument (G2): the SCM takes the whole command line.
    let image = format!("\"{}\" run-service", cli.display());
    let image_w = wide(&image);
    let scm = open_scm(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;

    let service = if let Some(service) = open_service(&scm, SERVICE_CHANGE_CONFIG) {
        let ok = unsafe {
            ChangeServiceConfigW(
                service.0,
                SERVICE_NO_CHANGE,
                SERVICE_DEMAND_START,
                SERVICE_NO_CHANGE,
                image_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(Error::Elevation(format!(
                "could not update the service (error {})",
                last_error()
            )));
        }
        service
    } else {
        let name = wide(SERVICE_NAME);
        let display = wide(SERVICE_DISPLAY);
        let handle = unsafe {
            CreateServiceW(
                scm.0,
                name.as_ptr(),
                display.as_ptr(),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_DEMAND_START,
                SERVICE_ERROR_NORMAL,
                image_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(), // LocalSystem
                std::ptr::null(),
            )
        };
        if handle.is_null() {
            return Err(Error::Elevation(format!(
                "could not create the service (error {})",
                last_error()
            )));
        }
        ScHandle(handle)
    };

    configure_pipe_trigger(&service)
}

/// Registers the named-pipe start trigger, so opening the broker pipe starts the
/// service on demand.
fn configure_pipe_trigger(service: &ScHandle) -> Result<()> {
    use windows_sys::Win32::System::Services::NAMED_PIPE_EVENT_GUID;
    use windows_sys::Win32::System::Services::{
        ChangeServiceConfig2W, SERVICE_CONFIG_TRIGGER_INFO, SERVICE_TRIGGER,
        SERVICE_TRIGGER_ACTION_SERVICE_START, SERVICE_TRIGGER_DATA_TYPE_STRING,
        SERVICE_TRIGGER_INFO, SERVICE_TRIGGER_SPECIFIC_DATA_ITEM,
        SERVICE_TRIGGER_TYPE_NETWORK_ENDPOINT,
    };

    // The trigger data is the pipe name without the `\\.\pipe\` prefix.
    let pipe_name: Vec<u16> = "os-switcher-broker"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut data = SERVICE_TRIGGER_SPECIFIC_DATA_ITEM {
        dwDataType: SERVICE_TRIGGER_DATA_TYPE_STRING,
        cbData: (pipe_name.len() * 2) as u32,
        pData: pipe_name.as_ptr() as *mut u8,
    };
    let mut subtype = NAMED_PIPE_EVENT_GUID;
    let mut trigger = SERVICE_TRIGGER {
        dwTriggerType: SERVICE_TRIGGER_TYPE_NETWORK_ENDPOINT,
        dwAction: SERVICE_TRIGGER_ACTION_SERVICE_START,
        pTriggerSubtype: &mut subtype,
        cDataItems: 1,
        pDataItems: &mut data,
    };
    let info = SERVICE_TRIGGER_INFO {
        cTriggers: 1,
        pTriggers: &mut trigger,
        pReserved: std::ptr::null_mut(),
    };
    let ok = unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_TRIGGER_INFO,
            (&info as *const SERVICE_TRIGGER_INFO).cast(),
        )
    };
    if ok == 0 {
        return Err(Error::Elevation(format!(
            "could not set the pipe start trigger (error {})",
            last_error()
        )));
    }
    Ok(())
}

fn stop_service() -> Result<()> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let Some(service) = open_service(&scm, SERVICE_STOP | SERVICE_QUERY_STATUS) else {
        return Ok(()); // not registered
    };
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) };
    Ok(())
}

// ---- Uninstall ---------------------------------------------------------------

pub fn run_uninstall(purge: bool) -> Result<()> {
    if !is_elevated() {
        let mut args = vec!["uninstall"];
        if purge {
            args.push("--purge");
        }
        return run_self_elevated(&args);
    }
    do_uninstall(purge)?;
    log_event("service broker removed", false);
    Ok(())
}

fn do_uninstall(purge: bool) -> Result<()> {
    let _ = stop_service();
    delete_service()?;
    remove_legacy_task();
    let _ = remove_shortcut();
    let _ = remove_uninstall_key();

    if let Ok(dir) = install_dir() {
        // Best-effort: files in use (this very binary) are scheduled for removal
        // at the next reboot instead.
        if std::fs::remove_dir_all(&dir).is_err() {
            schedule_delete_on_reboot(&dir);
        }
    }
    if purge {
        if let Ok(state) = program_data_dir() {
            let _ = std::fs::remove_dir_all(state);
        }
    }
    Ok(())
}

fn delete_service() -> Result<()> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let Some(service) = open_service(&scm, DELETE) else {
        return Ok(());
    };
    if unsafe { DeleteService(service.0) } == 0 {
        return Err(Error::Elevation(format!(
            "could not delete the service (error {})",
            last_error()
        )));
    }
    Ok(())
}

fn schedule_delete_on_reboot(dir: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
    // Schedule the two executables (what keeps the directory busy).
    for exe in [CLI_EXE, GUI_EXE] {
        let p = wide(&dir.join(exe).to_string_lossy());
        unsafe { MoveFileExW(p.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    }
}

// ---- Repair ------------------------------------------------------------------

pub fn run_repair() -> Result<()> {
    if !is_elevated() {
        return run_self_elevated(&["repair-service"]);
    }
    let cli = installed_cli()?;
    if !cli.exists() {
        return Err(Error::Elevation(format!(
            "{} is missing — run `install` instead",
            cli.display()
        )));
    }
    register_or_update_service(&cli)?;
    log_event("service broker re-pointed at the installed binary", false);
    Ok(())
}

// ---- Legacy scheduled-task migration -----------------------------------------

/// Removes the old scheduled-task registration if present (ignore if absent):
/// two elevation paths active at once would be a bug.
fn remove_legacy_task() {
    // The task the pre-broker builds registered under this name.
    let _ = quiet_command("schtasks")
        .args(["/delete", "/f", "/tn", "OS Switcher"])
        .output();
}

// ---- Uninstall registry entry ------------------------------------------------

fn program_data_dir() -> Result<PathBuf> {
    let base = std::env::var_os("ProgramData")
        .ok_or_else(|| Error::Elevation("ProgramData is not set".into()))?;
    Ok(PathBuf::from(base).join(INSTALL_DIR))
}

fn write_uninstall_key(dir: &Path) -> Result<()> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let path = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\os-switcher");
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(Error::Elevation(format!(
            "could not write the uninstall entry (error {status})"
        )));
    }

    let uninstall_cmd = format!("\"{}\" uninstall", dir.join(CLI_EXE).display());
    let set = |name: &str, value: &str| {
        let n = wide(name);
        let v = wide(value);
        unsafe {
            RegSetValueExW(
                key,
                n.as_ptr(),
                0,
                REG_SZ,
                v.as_ptr().cast(),
                (v.len() * 2) as u32,
            )
        };
    };
    set("DisplayName", "OS Switcher");
    set("DisplayVersion", env!("CARGO_PKG_VERSION"));
    set("Publisher", env!("CARGO_PKG_AUTHORS"));
    set("InstallLocation", &dir.to_string_lossy());
    set("UninstallString", &uninstall_cmd);
    set("DisplayIcon", &dir.join(GUI_EXE).to_string_lossy());
    unsafe { RegCloseKey(key) };
    Ok(())
}

fn remove_uninstall_key() -> Result<()> {
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_LOCAL_MACHINE};
    let path = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\os-switcher");
    unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, path.as_ptr()) };
    Ok(())
}

// ---- Shortcuts (best-effort, via WScript.Shell) ------------------------------

fn common_programs() -> Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_CommonPrograms, SHGetKnownFolderPath};
    let mut raw: *mut u16 = std::ptr::null_mut();
    let hr = unsafe {
        SHGetKnownFolderPath(&FOLDERID_CommonPrograms, 0, std::ptr::null_mut(), &mut raw)
    };
    if hr < 0 || raw.is_null() {
        return Err(Error::Elevation(
            "could not locate the Start Menu folder".into(),
        ));
    }
    let mut len = 0usize;
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(path))
}

fn shortcut_path() -> Result<PathBuf> {
    Ok(common_programs()?.join("OS Switcher.lnk"))
}

/// Creates the Start-menu shortcut pointing at the installed GUI. Scripted
/// through `WScript.Shell` (a COM API, so not localization-sensitive).
fn create_shortcut(gui: &Path) -> Result<()> {
    let lnk = shortcut_path()?;
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Save()",
        lnk.display(),
        gui.display()
    );
    let ok = quiet_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(Error::Elevation(
            "could not create the Start-menu shortcut".into(),
        ))
    }
}

fn remove_shortcut() -> Result<()> {
    if let Ok(lnk) = shortcut_path() {
        let _ = std::fs::remove_file(lnk);
    }
    Ok(())
}

// ---- Event Log (G7) ----------------------------------------------------------

/// Writes one line to the Application event log under our source. Best-effort:
/// diagnostics must never take the operation down.
pub fn log_event(message: &str, is_error: bool) {
    use windows_sys::Win32::System::EventLog::{
        DeregisterEventSource, RegisterEventSourceW, ReportEventW, EVENTLOG_ERROR_TYPE,
        EVENTLOG_INFORMATION_TYPE,
    };
    let source = wide(EVENT_SOURCE);
    let handle = unsafe { RegisterEventSourceW(std::ptr::null(), source.as_ptr()) };
    if handle.is_null() {
        return;
    }
    let text = wide(message);
    let mut strings = [text.as_ptr()];
    let kind = if is_error {
        EVENTLOG_ERROR_TYPE
    } else {
        EVENTLOG_INFORMATION_TYPE
    };
    unsafe {
        ReportEventW(
            handle,
            kind,
            0,
            1000,
            std::ptr::null_mut(),
            1,
            0,
            strings.as_mut_ptr(),
            std::ptr::null(),
        );
        DeregisterEventSource(handle);
    }
}
