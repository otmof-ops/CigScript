// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig internal-forward <sock> <port>`: the forwarder inside a boxed hop's
//! network namespace. It listens on `127.0.0.1:<port>` and relays each
//! connection to the parent's proxy on the Unix socket; it has no opinion
//! of its own and prints nothing. It dies with the namespace.

#[cfg(unix)]
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;

pub fn forward(sock: &Path, port: u16) -> i32 {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) => return 1,
    };
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let sock = sock.to_path_buf();
        std::thread::spawn(move || relay(conn, &sock));
    }
    0
}

#[cfg(unix)]
fn relay(conn: TcpStream, sock: &Path) {
    use std::os::unix::net::UnixStream;
    let Ok(up) = UnixStream::connect(sock) else {
        let _ = conn.shutdown(Shutdown::Both);
        return;
    };
    let (Ok(mut a_r), Ok(mut b_r)) = (conn.try_clone(), up.try_clone()) else {
        return;
    };
    let mut a_w = conn;
    let mut b_w = up;
    let t = std::thread::spawn(move || {
        let _ = copy(&mut a_r, &mut b_w);
        let _ = b_w.shutdown(Shutdown::Write);
    });
    let _ = copy(&mut b_r, &mut a_w);
    let _ = a_w.shutdown(Shutdown::Write);
    let _ = t.join();
}

#[cfg(not(unix))]
fn relay(conn: TcpStream, _sock: &Path) {
    let _ = conn.shutdown(Shutdown::Both);
}

#[cfg(unix)]
fn copy(r: &mut dyn Read, w: &mut dyn Write) -> io::Result<u64> {
    io::copy(r, w)
}
