// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The way out of a boxed hop's network namespace: a CONNECT proxy the
//! parent runs on a Unix socket, which the forwarder inside the namespace
//! relays to. It allows the hosts the hop was given (`allow_hosts`: a
//! name, `*.suffix`, or `host:port`), resolves them itself, splices the
//! bytes and counts them; everything else (another host, a raw address not
//! listed, a method other than CONNECT) is refused and recorded as an
//! event. It reads one request line and the headers, nothing more.

#[cfg(unix)]
mod imp {
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::net::{Shutdown, TcpStream, ToSocketAddrs};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A running proxy; `stop` ends it and removes its socket.
    pub struct Proxy {
        sock: PathBuf,
        fd: i32,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    #[derive(Clone)]
    struct Shared {
        allow: Arc<Vec<String>>,
        events: Arc<Mutex<std::fs::File>>,
    }

    /// Start the proxy on `sock`, recording to `events` (one JSON line per
    /// connection). Each entry of `allow` is a host name, `*.suffix`, or
    /// `host:port`; the match is case-insensitive.
    pub fn start(sock: &Path, allow: Vec<String>, events: &Path) -> io::Result<Proxy> {
        let _ = std::fs::remove_file(sock);
        if let Some(parent) = events.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(events)?;
        let listener = UnixListener::bind(sock)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600));
        }
        let fd = {
            use std::os::unix::io::AsRawFd;
            listener.as_raw_fd()
        };
        let shared = Shared {
            allow: Arc::new(allow.into_iter().map(|h| h.to_ascii_lowercase()).collect()),
            events: Arc::new(Mutex::new(file)),
        };
        let thread = std::thread::Builder::new()
            .name("cig-box-proxy".to_string())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { break };
                    let shared = shared.clone();
                    let _ = std::thread::Builder::new()
                        .name("cig-box-proxy-conn".to_string())
                        .spawn(move || serve(conn, &shared));
                }
            })?;
        Ok(Proxy {
            sock: sock.to_path_buf(),
            fd,
            thread: Some(thread),
        })
    }

    impl Proxy {
        /// End the proxy: the listener is shut down (its accept loop ends),
        /// the thread joined, the socket removed. Connections still open end
        /// with the hop's processes, which are gone by now.
        pub fn stop(mut self) {
            // SAFETY: shutting down a descriptor this process owns; the
            // listener's accept then fails and the thread returns.
            unsafe {
                libc::shutdown(self.fd, libc::SHUT_RDWR);
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            let _ = std::fs::remove_file(&self.sock);
        }
    }

    /// Does `host:port` pass the allow list?
    pub fn allowed(allow: &[String], host: &str, port: u16) -> bool {
        let host = host.to_ascii_lowercase();
        let with_port = format!("{host}:{port}");
        allow.iter().any(|a| {
            if a.contains(':') && !a.starts_with('[') {
                *a == with_port
            } else if let Some(suffix) = a.strip_prefix("*.") {
                host == suffix || host.ends_with(&format!(".{suffix}"))
            } else {
                *a == host
            }
        })
    }

    fn event(shared: &Shared, line: serde_json::Value) {
        if let Ok(mut f) = shared.events.lock() {
            let _ = writeln!(f, "{line}");
        }
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn serve(conn: UnixStream, shared: &Shared) {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
        let mut reader = BufReader::new(match conn.try_clone() {
            Ok(c) => c,
            Err(_) => return,
        });
        let mut writer = conn;
        let mut request = String::new();
        if reader.read_line(&mut request).is_err() {
            return;
        }
        // The headers, read and dropped (capped: this is a tunnel, not a web server).
        let mut total = request.len();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if line == "\r\n" || line == "\n" || total > 8192 {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        let mut parts = request.split_whitespace();
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if method != "CONNECT" {
            let _ =
                writer.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n");
            event(
                shared,
                serde_json::json!({"t": now(), "call": "proxy", "method": method, "target": target, "result": "refused", "why": "not CONNECT"}),
            );
            return;
        }
        let (host, port) = match split_host_port(target) {
            Some(hp) => hp,
            None => {
                let _ = writer.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
                event(
                    shared,
                    serde_json::json!({"t": now(), "call": "connect", "target": target, "result": "refused", "why": "malformed"}),
                );
                return;
            }
        };
        if !allowed(&shared.allow, &host, port) {
            let _ = writer.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
            event(
                shared,
                serde_json::json!({"t": now(), "call": "connect", "host": host, "port": port, "result": "refused", "why": "not in allow_hosts"}),
            );
            return;
        }
        let upstream = (host.as_str(), port)
            .to_socket_addrs()
            .ok()
            .and_then(|addrs| {
                addrs
                    .into_iter()
                    .find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(10)).ok())
            });
        let Some(upstream) = upstream else {
            let _ = writer.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n");
            event(
                shared,
                serde_json::json!({"t": now(), "call": "connect", "host": host, "port": port, "result": "unreachable"}),
            );
            return;
        };
        if writer
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .is_err()
        {
            return;
        }
        // Whatever the client sent after its headers, buffered already.
        let mut up_w = match upstream.try_clone() {
            Ok(u) => u,
            Err(_) => return,
        };
        let pending = reader.buffer().to_vec();
        if !pending.is_empty() && up_w.write_all(&pending).is_err() {
            return;
        }
        let mut down_r = upstream;
        let up_thread = std::thread::spawn(move || {
            let mut n = pending.len() as u64;
            n += io::copy(&mut reader, &mut up_w).unwrap_or(0);
            let _ = up_w.shutdown(Shutdown::Write);
            n
        });
        let down = copy_to(&mut down_r, &mut writer);
        let _ = writer.shutdown(Shutdown::Write);
        let up = up_thread.join().unwrap_or(0);
        event(
            shared,
            serde_json::json!({"t": now(), "call": "connect", "host": host, "port": port, "result": "allowed", "bytes_up": up, "bytes_down": down}),
        );
    }

    fn copy_to(r: &mut dyn Read, w: &mut dyn Write) -> u64 {
        io::copy(r, w).unwrap_or(0)
    }

    /// `host:port`, with a bracketed IPv6 host allowed.
    fn split_host_port(target: &str) -> Option<(String, u16)> {
        let (host, port) = if let Some(rest) = target.strip_prefix('[') {
            let (h, p) = rest.split_once(']')?;
            (h.to_string(), p.strip_prefix(':')?)
        } else {
            let (h, p) = target.rsplit_once(':')?;
            (h.to_string(), p)
        };
        if host.is_empty() {
            return None;
        }
        Some((host, port.parse().ok()?))
    }

    /// The refused connections a hop's events record, as `host:port` lines.
    pub fn refused(events: &Path) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(events) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v["result"] == "refused" {
                let what = match (v["host"].as_str(), v["port"].as_u64()) {
                    (Some(h), Some(p)) => format!("{h}:{p}"),
                    _ => v["target"].as_str().unwrap_or("?").to_string(),
                };
                if !out.contains(&what) {
                    out.push(what);
                }
            }
        }
        out
    }
}

#[cfg(unix)]
pub use imp::{allowed, refused, start, Proxy};

/// Off Unix there is no box and so no proxy: the calls exist so the hop's
/// code reads the same everywhere, and `start` says why it cannot.
#[cfg(not(unix))]
mod imp {
    use std::io;
    use std::path::Path;

    pub struct Proxy;

    impl Proxy {
        pub fn stop(self) {}
    }

    pub fn start(_sock: &Path, _allow: Vec<String>, _events: &Path) -> io::Result<Proxy> {
        Err(io::Error::other(
            "the box's proxy runs on a Unix socket, which this system does not have",
        ))
    }

    pub fn allowed(_allow: &[String], _host: &str, _port: u16) -> bool {
        false
    }

    pub fn refused(_events: &Path) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(not(unix))]
pub use imp::{allowed, refused, start, Proxy};
