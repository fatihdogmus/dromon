//! The helper side: ssh runs our executable with the prompt as `argv[1]`; we ask the app and
//! print the answer. Plain `std`, no tokio runtime, so it starts in a few milliseconds.

use std::ffi::OsString;
use std::io::{self, Write};

use zeroize::Zeroizing;

use super::protocol::{Reply, Request};
use super::{ENDPOINT_ENV, HINT_ENV, TOKEN_ENV};

/// Whether this process was started by ssh as our askpass helper. Check it in `main()` before
/// any GUI setup.
pub fn is_helper_invocation() -> bool {
    std::env::var_os(ENDPOINT_ENV).is_some()
}

/// The helper's whole job, reading its inputs from the environment and `argv`, writing the
/// answer to stdout. Returns the process exit code: 0 with an answer, 1 on cancel or any error.
pub fn run_helper() -> i32 {
    let (Some(endpoint), Some(token)) = (env_string(ENDPOINT_ENV), env_string(TOKEN_ENV)) else {
        eprintln!("dromon askpass: {ENDPOINT_ENV} and {TOKEN_ENV} must both be set");
        return 1;
    };
    let prompt = std::env::args_os().nth(1).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let hint = env_string(HINT_ENV);
    let mut stdout = io::stdout().lock();
    run_helper_with(&endpoint, &token, &prompt, hint.as_deref(), &mut stdout)
}

fn env_string(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v: OsString| v.to_string_lossy().into_owned())
}

/// [`run_helper`] with explicit inputs: asks the app, then writes `answer\n` to `out` and returns
/// 0, or returns 1 on cancel or any error (reported on stderr without secrets).
pub fn run_helper_with(endpoint: &str, token: &str, prompt: &str, hint: Option<&str>, out: &mut dyn Write) -> i32 {
    match ask(endpoint, token, prompt, hint) {
        Ok(Some(answer)) => {
            let mut line = Zeroizing::new(Vec::with_capacity(answer.len() + 1));
            line.extend_from_slice(answer.as_bytes());
            line.push(b'\n');
            match out.write_all(&line).and_then(|()| out.flush()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("dromon askpass: couldn't write the answer: {e}");
                    1
                }
            }
        }
        Ok(None) => 1,
        Err(e) => {
            eprintln!("dromon askpass: {e}");
            1
        }
    }
}

/// Sends one prompt to the app and blocks for its reply: `Some(answer)`, or `None` when the user
/// cancelled.
pub fn ask(endpoint: &str, token: &str, prompt: &str, hint: Option<&str>) -> io::Result<Option<Zeroizing<String>>> {
    let request = Request { token: token.to_owned(), prompt: prompt.to_owned(), hint: hint.map(str::to_owned) };
    match transport::exchange(endpoint, &request)? {
        Reply::Answer(answer) => Ok(Some(answer)),
        Reply::Cancel => Ok(None),
    }
}

#[cfg(unix)]
mod transport {
    use std::io::{self, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::super::protocol::{Reply, Request, decode_reply, encode_request, read_message};

    pub(super) fn exchange(endpoint: &str, request: &Request) -> io::Result<Reply> {
        if endpoint.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no askpass endpoint"));
        }
        let mut stream = UnixStream::connect(endpoint)?;
        // Sending is quick; the reply waits for the user, so it has no timeout.
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        stream.write_all(&encode_request(request)?)?;
        stream.flush()?;
        let line = read_message(&mut BufReader::new(&stream))?;
        decode_reply(&line)
    }
}

#[cfg(windows)]
mod transport {
    use std::fs::{File, OpenOptions};
    use std::io::{self, BufReader, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    use windows_sys::Win32::Storage::FileSystem::SECURITY_ANONYMOUS;

    use super::super::protocol::{Reply, Request, decode_reply, encode_request, read_message};

    /// How long to keep retrying while every pipe instance is busy.
    const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
    const BUSY_RETRY: Duration = Duration::from_millis(10);
    const PIPE_PREFIX: &str = r"\\.\pipe\";

    pub(super) fn exchange(endpoint: &str, request: &Request) -> io::Result<Reply> {
        if endpoint.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no askpass endpoint"));
        }
        // Never write the token anywhere but a local pipe.
        if !endpoint.starts_with(PIPE_PREFIX) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "askpass endpoint is not a local named pipe"));
        }
        let mut pipe = open_pipe(endpoint)?;
        pipe.write_all(&encode_request(request)?)?;
        pipe.flush()?;
        // The reply waits for the user, so it has no timeout.
        let line = read_message(&mut BufReader::new(&pipe))?;
        decode_reply(&line)
    }

    /// Opens the client end, retrying for [`BUSY_TIMEOUT`] while all instances are taken (the
    /// server creates the next one right after each connection). A missing pipe fails at once.
    fn open_pipe(endpoint: &str) -> io::Result<File> {
        let deadline = Instant::now() + BUSY_TIMEOUT;
        loop {
            // Anonymous: the app needs no right to impersonate us.
            match OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_ANONYMOUS).open(endpoint) {
                Ok(pipe) => return Ok(pipe),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && Instant::now() < deadline => {
                    std::thread::sleep(BUSY_RETRY);
                }
                Err(e) => return Err(e),
            }
        }
    }
}
