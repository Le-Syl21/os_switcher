//! The SCM side: the service dispatcher and its control handler.
//!
//! `run` is the process's `run-service` entry point; the SCM starts it. It hands
//! control to the dispatcher, which calls [`service_main`] on the service
//! thread. There we enable the firmware privilege, register a Stop handler, and
//! run the pipe server until asked to stop.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use super::{install, pipe, SERVICE_NAME};
use crate::switcher::{Error, Result};

define_windows_service!(ffi_service_main, service_main);

/// `run-service` entry point: block in the SCM dispatcher until the service ends.
pub fn run() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| Error::Elevation(format!("service dispatcher failed to start: {e}")))
}

/// Runs on the service thread once the dispatcher connects us to the SCM.
fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = serve() {
        install::log_event(&format!("broker service error: {e}"), true);
    }
}

fn serve() -> Result<()> {
    // Belt and braces: LocalSystem holds SeSystemEnvironmentPrivilege but it may
    // not be enabled on the token. Enabling it is harmless if already on, and
    // the efivar back-end needs it to read/write the firmware.
    let _ = enable_system_environment_privilege();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_handler = stop.clone();

    let handler = move |control| match control {
        ServiceControl::Stop => {
            stop_for_handler.store(true, Ordering::SeqCst);
            pipe::wake_accept();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, handler)
        .map_err(|e| Error::Elevation(format!("could not register the control handler: {e}")))?;

    set_state(&status_handle, ServiceState::Running, true)?;
    let result = pipe::serve(&stop);
    // Report Stopped whatever happened, so the SCM does not think we hung.
    let _ = set_state(&status_handle, ServiceState::Stopped, false);
    result
}

fn set_state(
    handle: &service_control_handler::ServiceStatusHandle,
    state: ServiceState,
    accept_stop: bool,
) -> Result<()> {
    let controls_accepted = if accept_stop {
        ServiceControlAccept::STOP
    } else {
        ServiceControlAccept::empty()
    };
    handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .map_err(|e| Error::Elevation(format!("could not report service status: {e}")))
}

/// Enables `SeSystemEnvironmentPrivilege` on the current process token.
fn enable_system_environment_privilege() -> Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let name: Vec<u16> = "SeSystemEnvironmentPrivilege"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // Safe: every handle is closed, and the LUID/privilege buffers are stack
    // locals sized from their own types.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return Err(Error::Elevation("could not open the process token".into()));
        }
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
            CloseHandle(token);
            return Err(Error::Elevation(
                "could not look up SeSystemEnvironmentPrivilege".into(),
            ));
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let ok = AdjustTokenPrivileges(
            token,
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        CloseHandle(token);
        if ok == 0 {
            return Err(Error::Elevation(
                "could not enable SeSystemEnvironmentPrivilege".into(),
            ));
        }
    }
    Ok(())
}
