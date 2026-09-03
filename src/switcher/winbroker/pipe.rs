//! The named pipe: server (in the SYSTEM service) and client (in the app).
//!
//! Security posture:
//!   - the server creates the first instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`
//!     so a name already taken is *detected* (someone squatting it) rather than
//!     silently shared (G5);
//!   - the pipe's ACL lets authenticated users connect — they can only invoke
//!     the whitelisted verbs, validated server-side (G3) — while creation stays
//!     SYSTEM-only;
//!   - the client verifies the server process runs as LocalSystem before sending
//!     anything, so it never hands a request to an impostor (G5).

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    SetNamedPipeHandleState, WaitNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::wire;
use super::{BrokerEntry, PIPE_NAME};
use crate::switcher::{Error, Result, Scope};

const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
/// Largest response we read back (an entry list — a few kB in practice).
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// SDDL for the pipe: SYSTEM full control, authenticated users read+write.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GRGW;;;AU)";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // Safe: GetLastError only reads thread-local state.
    unsafe { GetLastError() }
}

// ---- Server ------------------------------------------------------------------

/// Serves pipe requests until `stop` is set. Each connection is one request and
/// one response, message-framed. Runs on the service's worker thread.
pub fn serve(stop: &AtomicBool) -> Result<()> {
    let mut security = PipeSecurity::new()?;
    let mut first = true;

    while !stop.load(Ordering::SeqCst) {
        let pipe = create_instance(first, &mut security)?;
        first = false;

        // Wait for a client (or for `wake_accept` to unblock us on stop).
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
            || last_error() == ERROR_PIPE_CONNECTED;

        if stop.load(Ordering::SeqCst) {
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
            break;
        }

        if connected {
            serve_one(pipe);
        }
        unsafe {
            DisconnectNamedPipe(pipe);
            CloseHandle(pipe);
        }
    }
    Ok(())
}

/// Unblocks a pending `ConnectNamedPipe` so [`serve`] can notice `stop`. Called
/// from the SCM control handler; a self-connect is the standard wake trick.
pub fn wake_accept() {
    let name = wide(PIPE_NAME);
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h != INVALID_HANDLE_VALUE {
        unsafe { CloseHandle(h) };
    }
}

fn create_instance(first: bool, security: &mut PipeSecurity) -> Result<HANDLE> {
    let name = wide(PIPE_NAME);
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            MAX_RESPONSE_BYTES as u32,
            wire::MAX_REQUEST_BYTES as u32,
            0,
            security.attributes(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::Elevation(format!(
            "could not create the broker pipe (error {})",
            last_error()
        )));
    }
    Ok(handle)
}

fn serve_one(pipe: HANDLE) {
    let mut buf = vec![0u8; wire::MAX_REQUEST_BYTES];
    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            pipe,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || read == 0 {
        return;
    }
    let request = String::from_utf8_lossy(&buf[..read as usize]).into_owned();
    let response = wire::handle(&request);
    let bytes = response.as_bytes();
    let mut written = 0u32;
    unsafe {
        WriteFile(
            pipe,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        );
    }
}

/// Owns the security descriptor backing the pipe's `SECURITY_ATTRIBUTES`.
struct PipeSecurity {
    descriptor: *mut core::ffi::c_void,
    attributes: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    fn new() -> Result<Self> {
        let sddl = wide(PIPE_SDDL);
        let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::Elevation(format!(
                "could not build the pipe security descriptor (error {})",
                last_error()
            )));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        Ok(PipeSecurity {
            descriptor,
            attributes,
        })
    }

    fn attributes(&mut self) -> *const SECURITY_ATTRIBUTES {
        &self.attributes
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe { LocalFree(self.descriptor) };
        }
    }
}

// ---- Client ------------------------------------------------------------------

/// Fetches the boot state from the service.
pub fn client_get_state() -> Result<Vec<BrokerEntry>> {
    let reply = round_trip(&wire::request_get_state())?;
    wire::parse_state(&reply).map_err(Error::Elevation)
}

/// Asks the service to select `key` for `scope`.
pub fn client_set(key: &str, scope: Scope) -> Result<()> {
    let reply = round_trip(&wire::request_set(key, scope))?;
    wire::parse_ok(&reply).map_err(Error::Elevation)
}

/// Asks the service to clear any one-shot selection.
pub fn client_clear_next() -> Result<()> {
    let reply = round_trip(&wire::request_clear_next())?;
    wire::parse_ok(&reply).map_err(Error::Elevation)
}

/// Opens the pipe, verifies the server is SYSTEM, sends one request and returns
/// the reply.
fn round_trip(request: &str) -> Result<String> {
    let pipe = connect()?;
    let result = (|| {
        verify_server_is_system(pipe)?;
        write_all(pipe, request.as_bytes())?;
        read_message(pipe)
    })();
    unsafe { CloseHandle(pipe) };
    result
}

fn connect() -> Result<HANDLE> {
    let name = wide(PIPE_NAME);
    for _ in 0..10 {
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            // Message mode, to read each reply as one framed message.
            let mode = PIPE_READMODE_MESSAGE;
            unsafe {
                SetNamedPipeHandleState(h, &mode, std::ptr::null(), std::ptr::null());
            }
            return Ok(h);
        }
        // Busy: wait for a free instance and retry.
        unsafe { WaitNamedPipeW(name.as_ptr(), 2000) };
    }
    Err(Error::Elevation(
        "the broker service is not answering (is it running?)".into(),
    ))
}

/// Refuses to talk to a pipe whose server is not LocalSystem.
fn verify_server_is_system(pipe: HANDLE) -> Result<()> {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
        return Err(Error::Elevation(
            "could not identify the pipe server".into(),
        ));
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(Error::Elevation(
            "could not open the pipe server process".into(),
        ));
    }
    let is_system = process_is_system(process);
    unsafe { CloseHandle(process) };
    if is_system {
        Ok(())
    } else {
        Err(Error::Elevation(
            "the pipe is not owned by the system service — refusing to use it".into(),
        ))
    }
}

fn process_is_system(process: HANDLE) -> bool {
    // Compare the process token's user SID to the well-known LocalSystem SID.
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut size = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
    if size == 0 {
        unsafe { CloseHandle(token) };
        return false;
    }
    let mut buf = vec![0u8; size as usize];
    let ok =
        unsafe { GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size, &mut size) };
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return false;
    }
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    system_sid()
        .map(|mut sid| unsafe { EqualSid(user.User.Sid, sid.as_mut_ptr().cast()) } != 0)
        .unwrap_or(false)
}

/// Builds the LocalSystem SID (`S-1-5-18`) as a raw byte buffer.
fn system_sid() -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    let text = wide("S-1-5-18");
    let mut psid: *mut core::ffi::c_void = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut psid) } == 0 || psid.is_null() {
        return None;
    }
    // Copy into an owned buffer, then free the API allocation.
    let len = unsafe { windows_sys::Win32::Security::GetLengthSid(psid) } as usize;
    let mut buf = vec![0u8; len];
    unsafe {
        std::ptr::copy_nonoverlapping(psid.cast::<u8>(), buf.as_mut_ptr(), len);
        LocalFree(psid);
    }
    Some(buf)
}

fn write_all(pipe: HANDLE, bytes: &[u8]) -> Result<()> {
    let mut written = 0u32;
    let ok = unsafe {
        WriteFile(
            pipe,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(Error::Elevation(format!(
            "could not send the request (error {})",
            last_error()
        )));
    }
    Ok(())
}

fn read_message(pipe: HANDLE) -> Result<String> {
    let mut buf = vec![0u8; MAX_RESPONSE_BYTES];
    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            pipe,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(Error::Elevation(format!(
            "could not read the reply (error {})",
            last_error()
        )));
    }
    Ok(String::from_utf8_lossy(&buf[..read as usize]).into_owned())
}
