//! toxiproxy in front of the sshd of [`Variant::Toxic`](crate::Variant::Toxic): a container of
//! `tests/docker/toxiproxy/` next to the sshd container, its proxy `sshd` forwarding to that
//! container's address on the Docker network. [`Fixture::port`](crate::Fixture::port) is the
//! proxy's published port, so every ssh connection of the tests goes through it.
//!
//! At start the testkit adds the [`BASE_TOXICS`] (a slow, choppy link); tests add more and
//! remove them again through toxiproxy's HTTP API ([`Toxiproxy::add`], [`Toxiproxy::remove`],
//! [`Toxiproxy::reset_peers`]). The API is spoken here with a minimal HTTP/1.1 client over a
//! plain TCP connection (one request per connection), so the testkit needs no HTTP crate.
//!
//! Toxics apply to every connection through the proxy, the live ones included: a test that
//! changes them must not share the server with others, see
//! [`matrix::run_exclusive`](crate::matrix::run_exclusive).

use std::fmt;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// The proxy's name in toxiproxy.
pub const PROXY: &str = "sshd";
/// The proxy's port inside its container.
pub(crate) const LISTEN_PORT: u16 = 2222;
/// toxiproxy's HTTP API port inside its container.
pub(crate) const API_PORT: u16 = 8474;

/// Which direction a toxic acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// Client to server (uploads, requests).
    Upstream,
    /// Server to client (downloads, listings, replies).
    Downstream,
}

impl Stream {
    fn as_str(self) -> &'static str {
        match self {
            Stream::Upstream => "upstream",
            Stream::Downstream => "downstream",
        }
    }
}

/// The toxics of the toxic server for its whole life: `(name, type, stream, attributes)`.
/// About 20 ms (±10) more round trip, 4 MB/s each way, and replies cut into ~1.4 KB slices
/// 50 µs apart.
pub const BASE_TOXICS: [(&str, &str, Stream, &str); 4] = [
    ("base_latency", "latency", Stream::Downstream, r#"{"latency":20,"jitter":10}"#),
    ("base_bandwidth_down", "bandwidth", Stream::Downstream, r#"{"rate":4096}"#),
    ("base_bandwidth_up", "bandwidth", Stream::Upstream, r#"{"rate":4096}"#),
    ("base_slicer", "slicer", Stream::Downstream, r#"{"average_size":1400,"size_variation":600,"delay":50}"#),
];

/// The names [`Toxiproxy::reset_peers`] adds.
const RESET_TOXICS: [(&str, Stream); 2] = [("reset_down", Stream::Downstream), ("reset_up", Stream::Upstream)];

/// The toxiproxy container and its API.
pub struct Toxiproxy {
    api: SocketAddr,
    pub(crate) container_id: String,
    /// Closed by the kernel when the process exits, which ends the container.
    _lifeline: Option<TcpStream>,
}

impl fmt::Debug for Toxiproxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Toxiproxy").field("api", &self.api).field("container_id", &self.container_id).finish()
    }
}

impl Toxiproxy {
    pub(crate) fn new(api: SocketAddr, container_id: String, lifeline: Option<TcpStream>) -> Toxiproxy {
        Toxiproxy { api, container_id, _lifeline: lifeline }
    }

    /// The API's address on this machine (`127.0.0.1:<published 8474>`).
    pub fn api(&self) -> SocketAddr {
        self.api
    }

    /// Adds the toxic `name` of `kind` (`latency`, `bandwidth`, `slicer`, `reset_peer`,
    /// `timeout`, `limit_data`, …) to `stream` of [`PROXY`]; `attributes` is its JSON object,
    /// e.g. `{"rate":512}`. Blocking (a request to 127.0.0.1).
    pub fn add(&self, name: &str, kind: &str, stream: Stream, attributes: &str) -> Result<(), String> {
        let body = format!(
            r#"{{"name":"{name}","type":"{kind}","stream":"{}","toxicity":1.0,"attributes":{attributes}}}"#,
            stream.as_str()
        );
        self.request("POST", &format!("/proxies/{PROXY}/toxics"), &body, &[200]).map(drop)
    }

    /// Removes the toxic `name`.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        self.request("DELETE", &format!("/proxies/{PROXY}/toxics/{name}"), "", &[200, 204]).map(drop)
    }

    /// The proxy's toxics as toxiproxy lists them (JSON), for messages.
    pub fn toxics(&self) -> Result<String, String> {
        self.request("GET", &format!("/proxies/{PROXY}/toxics"), "", &[200])
    }

    /// Resets every connection through the proxy the moment data next flows on it: adds
    /// `reset_peer` (timeout 0, a TCP RST) on both streams. New connections are reset as well
    /// until [`Toxiproxy::stop_resets`].
    pub fn reset_peers(&self) -> Result<(), String> {
        for (name, stream) in RESET_TOXICS {
            self.add(name, "reset_peer", stream, r#"{"timeout":0}"#)?;
        }
        Ok(())
    }

    /// Removes what [`Toxiproxy::reset_peers`] added.
    pub fn stop_resets(&self) -> Result<(), String> {
        for (name, _) in RESET_TOXICS {
            self.remove(name)?;
        }
        Ok(())
    }

    /// Adds [`BASE_TOXICS`].
    pub(crate) fn add_base(&self) -> Result<(), String> {
        for (name, kind, stream, attributes) in BASE_TOXICS {
            self.add(name, kind, stream, attributes)?;
        }
        Ok(())
    }

    /// One request; the body of a reply whose status is one of `ok`.
    fn request(&self, method: &str, path: &str, body: &str, ok: &[u16]) -> Result<String, String> {
        let what = format!("toxiproxy {method} {path}");
        let mut stream =
            TcpStream::connect_timeout(&self.api, Duration::from_secs(5)).map_err(|e| format!("{what}: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| format!("{what}: {e}"))?;
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            self.api,
            body.len()
        );
        stream.write_all(request.as_bytes()).map_err(|e| format!("{what}: {e}"))?;
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).map_err(|e| format!("{what}: {e}"))?;
        let reply = String::from_utf8_lossy(&reply);
        let status =
            reply.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).ok_or_else(|| {
                format!("{what}: not an HTTP reply: {:?}", reply.chars().take(200).collect::<String>())
            })?;
        let content = reply.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_owned();
        if !ok.contains(&status) {
            return Err(format!("{what}: HTTP {status}: {}", content.trim()));
        }
        Ok(content)
    }
}
