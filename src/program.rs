// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Programs: a script appended to the `cig` that runs it.
//!
//! CigCompile's first back end (otmof-ops/cigcompile#2, stage A) makes a
//! program by appending a checked script to the `cig` binary of the same
//! version, built for the machine it is meant for. At start-up `cig` looks
//! at its own end: when a program is there, it runs that program through
//! the same run as `cig run` (the same kernel, journal, plan and rollback)
//! and takes no command.
//!
//! The end of a program's executable:
//!
//! ```text
//! ... cig ...                                  the runtime, as released
//! payload                                      `len` bytes of JSON: the program's record, its source included
//! "CIGPROG1"  len (u64, LE)  sha256(payload)   the trailer, 48 bytes
//! ```
//!
//! A trailer whose payload does not hash to the sha256 it carries is a
//! damaged program, and it refuses to run (E1050). The payload is written
//! with its fields in one fixed order and nothing that changes between
//! builds, so the same script, compiler and runtime give the same bytes.
//!
//! The sha256s are integrity, not authenticity: they catch a damaged or
//! truncated program, not an edited one (anyone can recompute them), and
//! the runtime half is not covered. Programs are unsigned until
//! cigcompile#10.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};

/// The trailer's first eight bytes.
pub const MAGIC: [u8; 8] = *b"CIGPROG1";
/// The trailer's length: magic, payload length, payload sha256.
pub const TRAILER_LEN: u64 = 48;
/// The payload's format.
pub const FORMAT: u32 = 1;
/// The most a payload may hold. A script is text; this is generous.
pub const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;
/// Set on cig's calls to itself (the ashtray's background send, the box's
/// network forwarder). A binary with a program appended is `cig` for
/// exactly those calls ([`own`]) and runs its program for anything else.
pub const SELF_ENV: &str = "CIG_INTERNAL_SELF";

/// Every cig says its version in its own bytes, where cigcompile reads it
/// without running the binary (a cig built for another machine cannot run
/// here): `\0cig-runtime-version=1.15.0\0`. [`crate::cli::entry`] reads it
/// once, so no linker drops it.
pub static RUNTIME_MARK: &str = concat!("\0cig-runtime-version=", env!("CARGO_PKG_VERSION"), "\0");

/// The version a cig binary's bytes say it is ([`RUNTIME_MARK`]); `None`
/// for a binary without the mark (a cig before 1.15.0, or not a cig).
pub fn runtime_version(bytes: &[u8]) -> Option<String> {
    let needle = b"\0cig-runtime-version=";
    let mut from = 0;
    while let Some(at) = find(&bytes[from..], needle) {
        let start = from + at + needle.len();
        let rest = &bytes[start..bytes.len().min(start + 32)];
        if let Some(end) = rest.iter().position(|b| *b == 0) {
            let v = &rest[..end];
            let parts: Vec<&[u8]> = v.split(|b| *b == b'.').collect();
            if parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.iter().all(u8::is_ascii_digit))
            {
                return Some(String::from_utf8_lossy(v).to_string());
            }
        }
        from = start;
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Note that this process is a compiled program (the front end calls it
/// first). What only `cig` itself should do checks [`running`].
pub fn set_running() {
    RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Is this process a compiled program rather than `cig`?
pub fn running() -> bool {
    RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}

/// A compiled program: what it is called, the script it was compiled
/// from, and who compiled it for which cig.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    /// The payload's format ([`FORMAT`]).
    pub format: u32,
    /// The program's name: its file's stem, or the name it was built as.
    pub name: String,
    /// The script's path as it was compiled. A runtime error points here,
    /// with the same line and column `cig run` gives.
    pub file: String,
    /// The script.
    pub source: String,
    /// sha256 of `source`, hex.
    pub sha256: String,
    /// The cig version the program was compiled for; it runs only on that one.
    pub cig: String,
    /// What compiled it, e.g. `cigcompile 0.1.0`.
    pub compiler: String,
}

impl Program {
    /// A program for this cig, from a script's text.
    pub fn new(name: &str, file: &str, source: &str, compiler: &str) -> Program {
        Program {
            format: FORMAT,
            name: name.to_string(),
            file: file.to_string(),
            source: source.to_string(),
            sha256: hex::encode(Sha256::digest(source.as_bytes())),
            cig: crate::VERSION.to_string(),
            compiler: compiler.to_string(),
        }
    }

    /// The payload: the record as JSON, its fields in declaration order.
    pub fn payload(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a program serialises")
    }

    /// The record without the script, for `--cig-program` and `cig runs`.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "format": self.format,
            "name": self.name,
            "file": self.file,
            "sha256": self.sha256,
            "cig": self.cig,
            "compiler": self.compiler,
        })
    }
}

/// The trailer for a payload.
pub fn trailer(payload: &[u8]) -> [u8; TRAILER_LEN as usize] {
    let mut t = [0u8; TRAILER_LEN as usize];
    t[..8].copy_from_slice(&MAGIC);
    t[8..16].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    t[16..].copy_from_slice(&Sha256::digest(payload));
    t
}

/// A program's executable: `runtime` with `program` appended.
pub fn append(runtime: &[u8], program: &Program) -> Vec<u8> {
    let payload = program.payload();
    let mut out = Vec::with_capacity(runtime.len() + payload.len() + TRAILER_LEN as usize);
    out.extend_from_slice(runtime);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&trailer(&payload));
    out
}

/// Does `bytes` end in a trailer (a program, damaged or not)?
pub fn carries_program(bytes: &[u8]) -> bool {
    bytes.len() >= TRAILER_LEN as usize && bytes[bytes.len() - TRAILER_LEN as usize..][..8] == MAGIC
}

/// The program at the end of `f`, if one is there. `Ok(None)`: none is;
/// `Err`: a trailer is there and what it points at is damaged.
pub fn read<F: Read + Seek>(f: &mut F) -> Result<Option<Program>, String> {
    let end = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if end < TRAILER_LEN {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(end - TRAILER_LEN))
        .map_err(|e| e.to_string())?;
    let mut t = [0u8; TRAILER_LEN as usize];
    f.read_exact(&mut t).map_err(|e| e.to_string())?;
    if t[..8] != MAGIC {
        return Ok(None);
    }
    let len = u64::from_le_bytes(t[8..16].try_into().expect("eight bytes"));
    if len > MAX_PAYLOAD || len > end - TRAILER_LEN {
        return Err(format!(
            "the trailer says the program is {len} bytes, which this file cannot hold"
        ));
    }
    f.seek(SeekFrom::Start(end - TRAILER_LEN - len))
        .map_err(|e| e.to_string())?;
    let mut payload = Vec::new();
    f.take(len)
        .read_to_end(&mut payload)
        .map_err(|e| e.to_string())?;
    if payload.len() as u64 != len {
        return Err("the program ends before its trailer says it does".to_string());
    }
    if Sha256::digest(&payload)[..] != t[16..] {
        return Err("the program's bytes do not match the sha256 in its trailer".to_string());
    }
    let p: Program = serde_json::from_slice(&payload)
        .map_err(|e| format!("the program's record does not read: {e}"))?;
    if p.format != FORMAT {
        return Err(format!(
            "the program is in format {}; this cig reads format {FORMAT}",
            p.format
        ));
    }
    if hex::encode(Sha256::digest(p.source.as_bytes())) != p.sha256 {
        return Err("the script does not match the sha256 the program records".to_string());
    }
    check_words(&p)?;
    Ok(Some(p))
}

/// What a program says about itself is printed and recorded: its name is
/// a plain word, and nothing it carries holds a control character (no
/// escape sequence reaches a terminal from a payload).
fn check_words(p: &Program) -> Result<(), String> {
    if !good_name(&p.name) {
        return Err(
            "the program's name is not a plain word (letters, digits, `.`, `_`, `-`)".to_string(),
        );
    }
    for (what, v, max) in [
        ("file", &p.file, 4096),
        ("compiler", &p.compiler, 128),
        ("cig", &p.cig, 32),
    ] {
        if v.is_empty() || v.chars().count() > max || v.chars().any(char::is_control) {
            return Err(format!("the program's `{what}` is not printable text"));
        }
    }
    Ok(())
}

/// A program's name: a letter or digit, then letters, digits, `.`, `_` or
/// `-`, 64 at most.
pub fn good_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// Why a binary that carries a program will not run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unrunnable {
    /// A trailer is there and what it points at is damaged (E1050).
    Damaged(String),
    /// The program was compiled for another cig (E1051).
    OtherCig {
        /// The program's name.
        name: String,
        /// The cig it was compiled for.
        cig: String,
    },
}

/// The program appended to this executable. `None`: no program, act as
/// `cig`; and, for a binary that carries one, cig's own internal calls
/// ([`SELF_ENV`] with exactly their command line, never in a process the
/// system raised), so a program's binary is never a general `cig`. The
/// marker is taken out of the environment either way, so nothing this
/// process starts inherits it. A program compiled for another cig refuses
/// to run on this one.
pub fn own() -> Option<Result<Program, Unrunnable>> {
    let marked = std::env::var_os(SELF_ENV).is_some();
    if marked {
        // Before any thread starts: nothing else reads the environment yet.
        std::env::remove_var(SELF_ENV);
    }
    let mut f = open_self()?;
    let p = match read(&mut f) {
        Ok(None) => return None,
        Ok(Some(p)) => p,
        Err(e) => return Some(Err(Unrunnable::Damaged(e))),
    };
    if marked && !privileged() {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if internal_call(&args) {
            return None;
        }
    }
    if p.cig != crate::VERSION {
        return Some(Err(Unrunnable::OtherCig {
            name: p.name,
            cig: p.cig,
        }));
    }
    Some(Ok(p))
}

/// The internal calls a program's binary answers as `cig`: the ashtray's
/// background send and the box's network forwarder, exactly as cig makes
/// them.
fn internal_call(args: &[std::ffi::OsString]) -> bool {
    let words: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
    if words.len() != args.len() {
        return false;
    }
    match words.as_slice() {
        ["crash", "flush", "--quiet"] => true,
        // The box, and so its forwarder, is Linux's.
        #[cfg(target_os = "linux")]
        ["internal-forward", sock, port] => {
            std::path::Path::new(sock).is_absolute() && port.parse::<u16>().is_ok()
        }
        _ => false,
    }
}

/// A process the system started with more than its caller's rights
/// (setuid, setgid, file capabilities): the marker counts for nothing there.
#[cfg(target_os = "linux")]
fn privileged() -> bool {
    // SAFETY: getauxval reads the process's auxiliary vector; no pointers.
    unsafe { libc::getauxval(libc::AT_SECURE) != 0 }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn privileged() -> bool {
    // SAFETY: plain id queries.
    unsafe { libc::getuid() != libc::geteuid() || libc::getgid() != libc::getegid() }
}

#[cfg(not(unix))]
fn privileged() -> bool {
    false
}

/// This executable, as it was started: on Linux the running image itself
/// (`/proc/self/exe` opens it even after its path was replaced or removed).
fn open_self() -> Option<std::fs::File> {
    #[cfg(target_os = "linux")]
    if let Ok(f) = std::fs::File::open("/proc/self/exe") {
        return Some(f);
    }
    std::fs::File::open(std::env::current_exe().ok()?).ok()
}

/// A command for cig to call itself with: this executable (on Linux the
/// running image, whatever its path holds now), marked ([`SELF_ENV`]) so a
/// program's binary answers it as `cig`.
pub fn self_command() -> std::io::Result<std::process::Command> {
    #[cfg(target_os = "linux")]
    let me = if std::path::Path::new("/proc/self/exe").exists() {
        std::path::PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe()?
    };
    #[cfg(not(target_os = "linux"))]
    let me = std::env::current_exe()?;
    let mut c = std::process::Command::new(me);
    c.env(SELF_ENV, "1");
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample() -> Program {
        Program::new("hello", "examples/hello.cig", "exhale \"hi\"\n", "test 0")
    }

    #[test]
    fn a_program_reads_back_from_the_end_of_its_runtime() {
        let bytes = append(b"\x7fELF runtime bytes", &sample());
        assert!(carries_program(&bytes));
        let back = read(&mut Cursor::new(&bytes)).unwrap().unwrap();
        assert_eq!(back, sample());
        assert_eq!(back.cig, crate::VERSION);
    }

    #[test]
    fn a_runtime_says_its_version_in_its_bytes() {
        let mut bytes = b"junk \0cig-runtime-version=\x01 more".to_vec();
        bytes.extend_from_slice(RUNTIME_MARK.as_bytes());
        bytes.extend_from_slice(b"tail");
        assert_eq!(runtime_version(&bytes).as_deref(), Some(crate::VERSION));
        assert_eq!(runtime_version(b"no mark here"), None);
        assert_eq!(runtime_version(b"\0cig-runtime-version=1.2\0"), None);
    }

    #[test]
    fn a_runtime_without_a_program_is_none() {
        assert!(read(&mut Cursor::new(b"just cig".to_vec()))
            .unwrap()
            .is_none());
        assert!(read(&mut Cursor::new(Vec::new())).unwrap().is_none());
        assert!(!carries_program(b"just cig"));
    }

    #[test]
    fn the_same_inputs_give_the_same_bytes() {
        assert_eq!(append(b"rt", &sample()), append(b"rt", &sample()));
    }

    #[test]
    fn a_damaged_payload_is_refused() {
        let mut bytes = append(b"runtime", &sample());
        let at = b"runtime".len() + 5;
        bytes[at] ^= 1;
        let e = read(&mut Cursor::new(&bytes)).unwrap_err();
        assert!(e.contains("sha256"), "{e}");
    }

    #[test]
    fn a_length_past_the_start_is_refused() {
        let mut bytes = append(b"rt", &sample());
        let n = bytes.len();
        bytes[n - 40..n - 32].copy_from_slice(&(u64::MAX).to_le_bytes());
        assert!(read(&mut Cursor::new(&bytes))
            .unwrap_err()
            .contains("cannot hold"));
    }

    #[test]
    fn an_edited_script_with_a_recomputed_trailer_still_fails_its_own_sha256() {
        let mut p = sample();
        p.source = "exhale \"changed\"\n".to_string();
        let payload = serde_json::to_vec(&p).unwrap();
        let mut bytes = b"rt".to_vec();
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&trailer(&payload));
        let e = read(&mut Cursor::new(&bytes)).unwrap_err();
        assert!(
            e.contains("does not match the sha256 the program records"),
            "{e}"
        );
    }

    #[test]
    fn a_hostile_trailer_is_refused_without_reading_past_the_file() {
        let good = append(b"rt", &sample());
        for len in [MAX_PAYLOAD + 1, good.len() as u64, u64::MAX] {
            let mut b = good.clone();
            let n = b.len();
            b[n - 40..n - 32].copy_from_slice(&len.to_le_bytes());
            assert!(read(&mut Cursor::new(&b)).is_err(), "len {len}");
        }
        // The magic alone, in a file too short to hold a trailer, and a zero length.
        assert!(read(&mut Cursor::new(MAGIC.to_vec())).unwrap().is_none());
        let mut zero = b"rt".to_vec();
        zero.extend_from_slice(&trailer(b""));
        assert!(read(&mut Cursor::new(&zero)).is_err());
        // A payload truncated before its trailer.
        let mut cut = good.clone();
        cut.drain(2..10);
        assert!(read(&mut Cursor::new(&cut)).is_err());
    }

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        let payload = "[".repeat(1_000_000).into_bytes();
        let mut b = b"rt".to_vec();
        b.extend_from_slice(&payload);
        b.extend_from_slice(&trailer(&payload));
        assert!(read(&mut Cursor::new(&b)).is_err());
    }

    #[test]
    fn a_name_or_file_with_control_characters_is_refused() {
        for bad in [
            "he\x1b[2Jllo",
            "",
            ".hidden",
            "a/b",
            "x".repeat(65).as_str(),
        ] {
            let mut p = sample();
            p.name = bad.to_string();
            let e = read(&mut Cursor::new(&append(b"rt", &p))).unwrap_err();
            assert!(e.contains("plain word"), "{bad:?}: {e}");
        }
        let mut p = sample();
        p.file = "a\nb.cig".to_string();
        assert!(read(&mut Cursor::new(&append(b"rt", &p)))
            .unwrap_err()
            .contains("`file`"));
    }

    #[test]
    fn only_cigs_own_internal_calls_count() {
        let w = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert!(internal_call(&w(&["crash", "flush", "--quiet"])));
        assert_eq!(
            internal_call(&w(&["internal-forward", "/run/x.sock", "3128"])),
            cfg!(target_os = "linux"),
            "the forwarder is the box's, and the box is Linux's"
        );
        assert!(!internal_call(&w(&["internal-forward", "x.sock", "3128"])));
        assert!(!internal_call(&w(&["internal-forward", "/x", "port"])));
        assert!(!internal_call(&w(&["run", "evil.cig"])));
        assert!(!internal_call(&w(&["update"])));
        assert!(!internal_call(&w(&["crash", "flush"])));
        assert!(!internal_call(&w(&[])));
    }

    #[test]
    fn another_format_is_refused() {
        let mut p = sample();
        p.format = 9;
        let bytes = append(b"rt", &p);
        assert!(read(&mut Cursor::new(&bytes))
            .unwrap_err()
            .contains("format 9"));
    }
}
