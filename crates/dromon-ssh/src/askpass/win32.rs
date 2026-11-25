//! The Win32 calls behind the askpass named pipe, and the crate's only `unsafe`: the current
//! user's SID, the user of a pipe's client process, and creating a pipe instance whose DACL lets
//! in that user alone. Everything here is synchronous and returns owned, safe values.

#![allow(unsafe_code)]

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

/// Memory from `LocalAlloc` (as the SID and security-descriptor conversions return), freed on drop.
struct LocalMem(HLOCAL);

impl Drop for LocalMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` came from a Win32 call documented to allocate with `LocalAlloc`, is
            // owned by this guard alone, and is freed exactly once here.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// The user SID of this process, as a string (`S-1-5-21-…`).
pub(crate) fn current_user_sid() -> io::Result<String> {
    // SAFETY: no arguments; returns a pseudo-handle that is always valid and needs no closing.
    let process = unsafe { GetCurrentProcess() };
    process_user_sid(process)
}

/// The user SID (as a string) of the process at the other end of a connected pipe instance.
pub(crate) fn pipe_client_user_sid(pipe: &NamedPipeServer) -> io::Result<String> {
    let mut pid = 0u32;
    // SAFETY: the handle is the live server end of a named pipe, borrowed from `pipe` for the
    // duration of the call; `pid` is a valid, writable `u32`.
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: plain call with a constant access mask, no inheritance and a pid; the returned
    // handle is checked for null before use.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a fresh, non-null process handle that nothing else owns; `OwnedHandle`
    // closes it once on drop.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    process_user_sid(process.as_raw_handle())
}

/// The user SID (as a string) in the primary token of `process`.
fn process_user_sid(process: HANDLE) -> io::Result<String> {
    let mut raw: HANDLE = ptr::null_mut();
    // SAFETY: `process` is a valid process handle (or the current-process pseudo-handle) for the
    // duration of the call; `raw` is a valid, writable handle slot.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `raw` is a fresh token handle that nothing else owns; `OwnedHandle`
    // closes it once on drop.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };

    let mut len = 0u32;
    // SAFETY: a size query: null buffer with length 0, `len` a valid, writable `u32`. It fails
    // with ERROR_INSUFFICIENT_BUFFER and sets `len`, which is checked below.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, ptr::null_mut(), 0, &mut len) };
    if (len as usize) < size_of::<TOKEN_USER>() {
        return Err(io::Error::last_os_error());
    }
    // `u64` elements, so the buffer is aligned for `TOKEN_USER` (which holds a pointer).
    let mut buf = vec![0u64; (len as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` is writable for at least `len` bytes and suitably aligned; `len` is a valid,
    // writable `u32`.
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `buf` starts with an initialised, aligned `TOKEN_USER` (checked size
    // above); its `Sid` points into `buf`, which outlives every use of it below.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };

    let mut wide: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` is a valid SID inside `buf`; `wide` is a valid, writable pointer slot.
    if unsafe { ConvertSidToStringSidW(sid, &mut wide) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _free = LocalMem(wide.cast());
    let mut n = 0usize;
    // SAFETY: on success `wide` is a NUL-terminated UTF-16 string allocated by the call and kept
    // alive by `_free`; reading stops at the terminator.
    while unsafe { *wide.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `wide[..n]` was just read element by element above, so it is initialised, aligned
    // and alive (owned by `_free`) for the lifetime of the slice.
    let units = unsafe { std::slice::from_raw_parts(wide, n) };
    String::from_utf16(units).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "user SID is not valid UTF-16"))
}

/// Creates a pipe instance with `options` whose DACL grants access to the user `sid` only
/// (`D:P(A;;GA;;;<sid>)`, protected, so nothing is inherited). Must be called inside a tokio
/// runtime.
pub(crate) fn create_pipe_for_user(options: &ServerOptions, name: &str, sid: &str) -> io::Result<NamedPipeServer> {
    // The SID goes into SDDL text: accept only the `S-1-…` form so it can't add ACEs.
    if !sid.starts_with("S-1-") || !sid.bytes().all(|b| b == b'S' || b == b'-' || b.is_ascii_digit()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a string SID"));
    }
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})").encode_utf16().chain(Some(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `sddl` is a NUL-terminated UTF-16 string alive for the call; `descriptor` is a valid,
    // writable pointer slot; the size out-parameter is optional and passed as null.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut())
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _free = LocalMem(descriptor);
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: `attributes` is a valid, initialised SECURITY_ATTRIBUTES that lives on this stack
    // frame for the whole call, and its descriptor stays allocated until `_free` drops after it;
    // CreateNamedPipeW copies the descriptor into the pipe object and keeps no pointer to either.
    unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast()) }
}
