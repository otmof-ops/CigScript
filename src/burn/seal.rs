// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Sealed: tin objects and snapshots encrypted at rest, so a tin that is on
//! for a whole project never keeps an old `.env`, a private key or a
//! kubeconfig in the clear. No crypto of our own: XChaCha20-Poly1305 in the
//! STREAM construction (`aead::stream`), one megabyte a segment, every
//! segment authenticated and the last one marked, so a byte flipped or a
//! tail cut off is refused on read. The key lives in the OS keyring (Secret
//! Service, Keychain, Credential Manager) and, where there is none, in a
//! file beside the tin that only its owner can read. A sealed object is
//! named by a random id, not its hash, and never shared between files, so
//! two equal secrets do not reveal that they are equal.

use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{KeyInit, Payload};
use chacha20poly1305::XChaCha20Poly1305;
use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The first bytes of every sealed file.
pub const MAGIC: &[u8; 8] = b"CIGSEAL1";
/// Plaintext bytes per segment.
const SEGMENT: usize = 1 << 20;
/// Poly1305's tag.
const TAG: usize = 16;
/// XChaCha20's 24-byte nonce less the STREAM counter and last-block flag.
const NONCE: usize = 19;
/// The suffix a sealed snapshot in a run directory (or a tree) carries.
pub const SUFFIX: &str = ".cig-sealed";

/// Names that are secrets until proven otherwise. Matched against the file
/// name alone, in any directory.
pub const SECRET_NAMES: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "id_dsa*",
    "*.p12",
    "*.pfx",
    "credentials*",
    "*secret*",
    ".npmrc",
    ".pypirc",
    "kubeconfig",
    "*.kdbx",
    ".netrc",
    ".git-credentials",
    "*.ppk",
    "*.jks",
    "*.keystore",
];

/// What the `seal` setting asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Secret-shaped names, `.cigseal` and `touches_sealed` (the default).
    Secrets,
    /// Everything the tin keeps.
    All,
    /// Nothing.
    Never,
}

pub fn policy() -> Policy {
    static P: OnceLock<Policy> = OnceLock::new();
    *P.get_or_init(
        || match crate::config::Config::load().get("seal").as_str() {
            "all" => Policy::All,
            "never" => Policy::Never,
            _ => Policy::Secrets,
        },
    )
}

fn secret_set() -> &'static GlobSet {
    static SET: OnceLock<GlobSet> = OnceLock::new();
    SET.get_or_init(|| {
        let mut b = GlobSetBuilder::new();
        for p in SECRET_NAMES {
            if let Ok(g) = Glob::new(p) {
                b.add(g);
            }
        }
        b.build().expect("the built-in secret names are globs")
    })
}

/// Does the file's name alone make it a secret?
pub fn secret_name(path: &Path) -> bool {
    path.file_name()
        .map(|n| secret_set().is_match(Path::new(n)))
        .unwrap_or(false)
}

/// The `.cigseal` of a pack root, as a glob set: one glob per line, `#`
/// comments, relative to the root; `None` when there is none.
pub fn seal_set(root: &Path) -> Option<GlobSet> {
    let text = fs::read_to_string(root.join(".cigseal")).ok()?;
    let mut b = GlobSetBuilder::new();
    let mut any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let pat = line.trim_start_matches("./").trim_end_matches('/');
        for p in [pat.to_string(), format!("{pat}/**")] {
            if let Ok(g) = Glob::new(&p) {
                b.add(g);
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    b.build().ok()
}

/// Should this file be kept sealed? By the setting, by its name, by the
/// pack root's `.cigseal`, or by the hop's `touches_sealed` globs (both
/// matched on the path relative to the root).
pub fn should_seal(path: &Path, root: Option<&Path>, sets: &[&GlobSet]) -> bool {
    match policy() {
        Policy::Never => false,
        Policy::All => true,
        Policy::Secrets => {
            if secret_name(path) {
                return true;
            }
            let Some(root) = root else {
                return false;
            };
            let Ok(rel) = path.strip_prefix(root) else {
                return false;
            };
            sets.iter().any(|s| s.is_match(rel))
        }
    }
}

/// Should this file be kept sealed, judged from the file alone: the
/// setting, its name, and any `.cigseal` in a directory above it (a
/// project's list applies to the whole tree below it). What a rollback
/// displaces, a native op's snapshot and a tree snapshot ask this.
pub fn should_seal_here(path: &Path) -> bool {
    match policy() {
        Policy::Never => false,
        Policy::All => true,
        Policy::Secrets => {
            if secret_name(path) {
                return true;
            }
            let mut dir = path.parent();
            let mut depth = 0;
            while let Some(d) = dir {
                if depth > 64 {
                    break;
                }
                if let Some(set) = cached_seal_set(d) {
                    if path.strip_prefix(d).is_ok_and(|rel| set.is_match(rel)) {
                        return true;
                    }
                }
                dir = d.parent();
                depth += 1;
            }
            false
        }
    }
}

/// A directory's `.cigseal`, compiled once and reused while the file is
/// unchanged (its size and modification time). A tree snapshot asks for
/// every file it copies, up every directory above it; reading and compiling
/// the same list for each one made a large tree slow for nothing.
fn cached_seal_set(dir: &Path) -> Option<std::rc::Rc<GlobSet>> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    type Stamp = (u64, Option<std::time::SystemTime>);
    type Cache = HashMap<PathBuf, (Stamp, Option<Rc<GlobSet>>)>;
    thread_local! {
        static CACHE: RefCell<Cache> = RefCell::new(HashMap::new());
    }
    let meta = fs::metadata(dir.join(".cigseal"))
        .ok()
        .filter(|m| m.is_file())?;
    let stamp: Stamp = (meta.len(), meta.modified().ok());
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((seen, set)) = c.get(dir) {
            if *seen == stamp {
                return set.clone();
            }
        }
        let set = seal_set(dir).map(Rc::new);
        c.insert(dir.to_path_buf(), (stamp, set.clone()));
        set
    })
}

/// Is this snapshot path a sealed object or snapshot?
pub fn is_sealed_path(path: &Path) -> bool {
    path.starts_with(super::tin::sealed_dir())
        || path
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(SUFFIX))
            .unwrap_or(false)
}

// ----- the key -------------------------------------------------------------------

pub type Key = [u8; 32];

/// Where the key is kept when the keyring has none: beside the tin, read
/// only by its owner.
pub fn key_path() -> PathBuf {
    super::tin::dir().join("seal.key")
}

/// Where the key came from this time, for the plan and the notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Keeper {
    Keyring,
    File,
}

/// The key, found once per process: the OS keyring when the setting asks
/// for it and it answers, else the key file (made on first use). `Err` is
/// the reason none could be had, and nothing is sealed or unsealed then.
pub fn key() -> Result<(Key, Keeper), String> {
    static K: OnceLock<Result<(Key, Keeper), String>> = OnceLock::new();
    K.get_or_init(key_uncached).clone()
}

fn key_uncached() -> Result<(Key, Keeper), String> {
    let cfg = crate::config::Config::load();
    if cfg.get("seal_key") != "file" {
        match keyring_key() {
            Ok(k) => return Ok((k, Keeper::Keyring)),
            Err(e) => {
                let first = fs::metadata(key_path()).is_err();
                if first {
                    eprintln!(
                        "note: the OS keyring is not available ({e}); the tin's key is kept in {} instead, readable by you alone (cig config seal_key file makes that the rule)",
                        crate::crash::tilde(&key_path())
                    );
                }
            }
        }
    }
    file_key().map(|k| (k, Keeper::File))
}

const SERVICE: &str = "cigscript";
const USER: &str = "tin-seal";

/// The key from the OS keyring, made and stored on first use. Asked on a
/// thread with a deadline: a D-Bus activation that hangs must not hang
/// the run.
fn keyring_key() -> Result<Key, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = (|| -> Result<Key, String> {
            let entry = keyring::Entry::new(SERVICE, USER).map_err(|e| e.to_string())?;
            match entry.get_password() {
                Ok(hex_key) => decode_key(hex_key.trim()),
                Err(keyring::Error::NoEntry) => {
                    if sealed_in_use() {
                        return Err(format!(
                            "the keyring has no {SERVICE}/{USER} entry, and this machine holds sealed objects only the key that sealed them opens; a new key would lock them out for good"
                        ));
                    }
                    let k = random_key()?;
                    entry
                        .set_password(&hex::encode(k))
                        .map_err(|e| e.to_string())?;
                    Ok(k)
                }
                Err(e) => Err(e.to_string()),
            }
        })();
        let _ = tx.send(r);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(r) => r,
        Err(_) => Err("the keyring did not answer within 5 seconds".to_string()),
    }
}

/// Does this machine hold anything only the current key opens: an object
/// under the tin's sealed directory, or a sealed snapshot in a run's record?
/// Asked before a new key is made where none was found: a key that went
/// missing is put back, never replaced behind the user's back.
fn sealed_in_use() -> bool {
    let any_object = fs::read_dir(super::tin::sealed_dir())
        .map(|buckets| {
            buckets.flatten().any(|b| {
                fs::read_dir(b.path())
                    .map(|mut objects| objects.next().is_some())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if any_object {
        return true;
    }
    fs::read_dir(super::runs::runs_dir())
        .map(|runs| {
            runs.flatten().any(|r| {
                fs::read_dir(r.path().join("snapshots"))
                    .map(|snaps| {
                        snaps
                            .flatten()
                            .any(|f| f.file_name().to_string_lossy().ends_with(SUFFIX))
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn file_key() -> Result<Key, String> {
    let path = key_path();
    match fs::read_to_string(&path) {
        Ok(text) => decode_key(text.trim()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if sealed_in_use() {
                return Err(format!(
                    "the tin's key is gone ({} is not there), and this machine holds sealed objects only that key opens; a new key would lock them out for good. Put the key back (the keyring entry {SERVICE}/{USER}, or that file), or move {} and the sealed snapshots under {} away to start over",
                    crate::crash::tilde(&path),
                    crate::crash::tilde(&super::tin::sealed_dir()),
                    crate::crash::tilde(&super::runs::runs_dir())
                ));
            }
            let k = random_key()?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts
                .open(&path)
                .map_err(|e| format!("cannot create the key file {}: {e}", path.display()))?;
            f.write_all(hex::encode(k).as_bytes())
                .and_then(|()| f.write_all(b"\n"))
                .and_then(|()| f.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(k)
        }
        Err(e) => Err(format!("cannot read the key file {}: {e}", path.display())),
    }
}

fn decode_key(text: &str) -> Result<Key, String> {
    let bytes = hex::decode(text).map_err(|_| "the stored key is not hex".to_string())?;
    bytes
        .try_into()
        .map_err(|_| "the stored key is not 32 bytes".to_string())
}

fn random_key() -> Result<Key, String> {
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).map_err(|e| format!("no randomness: {e}"))?;
    Ok(k)
}

/// A short, public fingerprint of a key, for messages and manifests.
pub fn fingerprint(key: &Key) -> String {
    hex::encode(Sha256::digest(key))[..16].to_string()
}

/// A fresh random id for a sealed object: 16 bytes, hex.
pub fn random_id() -> io::Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(hex::encode(b))
}

// ----- sealing and unsealing -----------------------------------------------------

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn key_or_refuse() -> io::Result<Key> {
    key().map(|(k, _)| k).map_err(|e| {
        io::Error::other(format!(
            "the key that seals the tin is not available ({e}); nothing was written"
        ))
    })
}

/// Seal `src` into `dest`: the magic, a random nonce, then the segments.
/// `aad` binds the file to its name. Returns the plaintext's SHA-256 and
/// size, which the journal records so the bytes are checked on the way
/// back whatever the ciphertext says.
pub fn seal_file(src: &Path, dest: &Path, aad: &[u8]) -> io::Result<(String, u64)> {
    let result = seal_inner(src, dest, aad);
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

fn seal_inner(src: &Path, dest: &Path, aad: &[u8]) -> io::Result<(String, u64)> {
    let key = key_or_refuse()?;
    let mut nonce = [0u8; NONCE];
    getrandom::getrandom(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut enc = EncryptorBE32::from_aead(cipher, (&nonce).into());
    let mut input = fs::File::open(src)?;
    let mut out = io::BufWriter::new(fs::File::create(dest)?);
    out.write_all(MAGIC)?;
    out.write_all(&nonce)?;
    let mut buf = vec![0u8; SEGMENT];
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        let n = read_full(&mut input, &mut buf)?;
        hasher.update(&buf[..n]);
        total += n as u64;
        let payload = Payload {
            msg: &buf[..n],
            aad,
        };
        if n == SEGMENT {
            let ct = enc
                .encrypt_next(payload)
                .map_err(|_| io::Error::other("sealing failed"))?;
            out.write_all(&ct)?;
        } else {
            let ct = enc
                .encrypt_last(payload)
                .map_err(|_| io::Error::other("sealing failed"))?;
            out.write_all(&ct)?;
            break;
        }
    }
    out.flush()?;
    out.get_ref().sync_all()?;
    Ok((hex::encode(hasher.finalize()), total))
}

/// Unseal `src` into `dst`, refusing a byte flipped or a tail cut off (a
/// segment that does not authenticate), and, when `expect_sha256` is
/// given, bytes other than the ones the journal recorded.
pub fn unseal_to(
    src: &Path,
    aad: &[u8],
    expect_sha256: &str,
    dst: &mut dyn Write,
) -> io::Result<u64> {
    let key = key().map(|(k, _)| k).map_err(|e| {
        io::Error::other(format!(
            "the key that sealed it is not available ({e}); nothing was written"
        ))
    })?;
    let mut input = io::BufReader::new(fs::File::open(src)?);
    let mut head = [0u8; MAGIC.len() + NONCE];
    if read_full(&mut input, &mut head)? != head.len() || &head[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a sealed object", src.display()),
        ));
    }
    let nonce: [u8; NONCE] = head[MAGIC.len()..].try_into().expect("nonce length");
    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut dec = DecryptorBE32::from_aead(cipher, (&nonce).into());
    let mut buf = vec![0u8; SEGMENT + TAG];
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let bad = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} does not open with the tin's key: a byte was changed, a tail cut off, or it was sealed with another key; refusing to restore from it",
                src.display()
            ),
        )
    };
    loop {
        let n = read_full(&mut input, &mut buf)?;
        let payload = Payload {
            msg: &buf[..n],
            aad,
        };
        if n == SEGMENT + TAG {
            let pt = dec.decrypt_next(payload).map_err(|_| bad())?;
            hasher.update(&pt);
            dst.write_all(&pt)?;
            total += pt.len() as u64;
        } else {
            if n < TAG {
                return Err(bad());
            }
            let pt = dec.decrypt_last(payload).map_err(|_| bad())?;
            hasher.update(&pt);
            dst.write_all(&pt)?;
            total += pt.len() as u64;
            break;
        }
    }
    let actual = hex::encode(hasher.finalize());
    if !expect_sha256.is_empty() && actual != expect_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} opens, but holds other bytes than the journal recorded (sha256 {} against {}); refusing to restore from it",
                src.display(),
                &actual[..12],
                &expect_sha256[..expect_sha256.len().min(12)]
            ),
        ));
    }
    Ok(total)
}

/// Unseal `src` into the file `dest`; nothing is left there on failure.
pub fn unseal_file(src: &Path, aad: &[u8], expect_sha256: &str, dest: &Path) -> io::Result<()> {
    let result = (|| -> io::Result<()> {
        let mut out = io::BufWriter::new(fs::File::create(dest)?);
        unseal_to(src, aad, expect_sha256, &mut out)?;
        out.flush()?;
        out.get_ref().sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// Does the sealed file open with the tin's key, whole? Nothing is written.
pub fn opens(src: &Path, aad: &[u8]) -> io::Result<u64> {
    unseal_to(src, aad, "", &mut io::sink())
}

/// The name a sealed file is bound to: its own file name.
pub fn aad_of(path: &Path) -> Vec<u8> {
    path.file_name()
        .map(|n| n.to_string_lossy().as_bytes().to_vec())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewritten_cigseal_is_read_again() {
        let _g = super::super::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".cigseal"), "a.txt\n").unwrap();
        assert!(should_seal_here(&dir.join("a.txt")));
        assert!(!should_seal_here(&dir.join("bb.txt")));
        fs::write(dir.join(".cigseal"), "bb.txt\n").unwrap();
        assert!(should_seal_here(&dir.join("bb.txt")));
        assert!(!should_seal_here(&dir.join("a.txt")));
        fs::remove_file(dir.join(".cigseal")).unwrap();
        assert!(!should_seal_here(&dir.join("bb.txt")));
    }

    fn with_key_file<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _g = super::super::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let old = std::env::var_os("CIGSCRIPT_HOME");
        std::env::set_var("CIGSCRIPT_HOME", tmp.path().join("home"));
        std::env::set_var("CIG_SEAL_KEY", "file");
        let out = f(tmp.path());
        match old {
            Some(v) => std::env::set_var("CIGSCRIPT_HOME", v),
            None => std::env::remove_var("CIGSCRIPT_HOME"),
        }
        out
    }

    #[test]
    fn a_sealed_file_round_trips_and_a_flipped_byte_is_refused() {
        with_key_file(|root| {
            let plain = root.join("id_rsa");
            let data: Vec<u8> = (0..(3 * SEGMENT + 777)).map(|i| (i % 251) as u8).collect();
            fs::write(&plain, &data).unwrap();
            let sealed = root.join("obj");
            let (sha, n) = seal_file(&plain, &sealed, b"obj").unwrap();
            assert_eq!(n as usize, data.len());
            let bytes = fs::read(&sealed).unwrap();
            assert!(bytes.starts_with(MAGIC));
            // Not a byte of the plaintext in the clear.
            assert!(!bytes
                .windows(64)
                .any(|w| data.windows(64).next() == Some(w)));
            let back = root.join("back");
            unseal_file(&sealed, b"obj", &sha, &back).unwrap();
            assert_eq!(fs::read(&back).unwrap(), data);
            // A flipped byte, a cut tail, the wrong name: each refused, nothing left.
            let mut bad = bytes.clone();
            bad[MAGIC.len() + NONCE + 100] ^= 1;
            fs::write(&sealed, &bad).unwrap();
            assert!(unseal_file(&sealed, b"obj", &sha, &back).is_err());
            assert!(!back.exists());
            fs::write(&sealed, &bytes[..bytes.len() - 20]).unwrap();
            assert!(unseal_file(&sealed, b"obj", &sha, &back).is_err());
            fs::write(&sealed, &bytes).unwrap();
            assert!(unseal_file(&sealed, b"other", &sha, &back).is_err());
            assert!(opens(&sealed, b"obj").is_ok());
            // An empty file seals and opens too.
            fs::write(&plain, b"").unwrap();
            let (sha, n) = seal_file(&plain, &sealed, b"obj").unwrap();
            assert_eq!(n, 0);
            unseal_file(&sealed, b"obj", &sha, &back).unwrap();
            assert_eq!(fs::read(&back).unwrap(), b"");
            assert!(
                secret_name(Path::new("/x/.env.local")) && secret_name(Path::new("id_rsa.pub"))
            );
            assert!(secret_name(Path::new("deploy.pem")) && !secret_name(Path::new("main.rs")));
        });
    }
}
