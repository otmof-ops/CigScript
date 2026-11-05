// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The box: a hop run over a copy-on-write layer on the pack, so a failing
//! hop's changes never land. On Linux the hop runs in its own user and
//! mount namespaces with each pack root mounted as overlayfs over itself
//! (the real root the read-only lower layer, an upper directory in the run
//! directory taking every write); a hop that succeeds has its upper layer
//! applied to the real pack through the journal, as native burns, so the
//! run is unburnable; a hop that fails has it deleted, and nothing was ever
//! written to the pack. Where the box cannot be made (no user namespaces,
//! a kernel before 5.11, another operating system), the hop runs on the tin
//! as usual and the plan says so.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where a boxed hop's layers live: one directory per hop under the run.
pub fn layers_dir(run_dir: &Path, seq: u64) -> PathBuf {
    run_dir.join("box").join(seq.to_string())
}

/// The layers of one hop: per root, an upper and a work directory.
#[derive(Debug)]
pub struct Layers {
    pub dir: PathBuf,
    /// (root, upper, work), outer roots first.
    pub roots: Vec<(PathBuf, PathBuf, PathBuf)>,
}

/// Make the directories a boxed hop needs. Outer roots come first so an
/// overlay over a parent is mounted before one over a child inside it.
pub fn prepare(run_dir: &Path, seq: u64, roots: &[PathBuf]) -> io::Result<Layers> {
    let dir = layers_dir(run_dir, seq);
    let mut ordered: Vec<PathBuf> = roots.to_vec();
    ordered.sort_by_key(|r| r.components().count());
    let mut out = Vec::new();
    for (i, root) in ordered.iter().enumerate() {
        let upper = dir.join(i.to_string()).join("upper");
        let work = dir.join(i.to_string()).join("work");
        fs::create_dir_all(&upper)?;
        fs::create_dir_all(&work)?;
        out.push((root.clone(), upper, work));
    }
    Ok(Layers { dir, roots: out })
}

/// Why the box could not be made, as the child wrote it before giving up.
pub fn why(layers: &Layers) -> Option<String> {
    fs::read_to_string(layers.dir.join("why"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Throw the layers away: nothing of the hop lands.
pub fn discard(layers: &Layers) {
    let _ = fs::remove_file(kept_record(&layers.dir));
    let _ = remove_tree_force(&layers.dir);
    // The `box` directory itself, once no hop's layers are left in it.
    if let Some(parent) = layers.dir.parent() {
        let _ = fs::remove_dir(parent);
    }
}

/// Remove a tree the box left, whatever its modes. overlayfs makes the
/// inner `work` directory of a work layer unreadable (mode 0), where a
/// plain `remove_dir_all` stops; the modes are opened first and the tree
/// goes whole, so a run's record can be pruned after a boxed hop.
pub fn remove_tree_force(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            #[cfg(unix)]
            {
                let _ = &e;
                open_modes(path);
                fs::remove_dir_all(path)
            }
            #[cfg(not(unix))]
            {
                Err(e)
            }
        }
    }
}

#[cfg(unix)]
fn open_modes(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                open_modes(&e.path());
            }
        }
    }
}

/// Can this platform make a box at all? Linux, with a kernel that lets an
/// unprivileged user namespace mount overlayfs (5.11 and later). The real
/// answer comes when the mount is tried; this only rules out what cannot
/// work, so the plan can say so before the run.
pub fn possible() -> Result<(), String> {
    // Test hook: the fallback, on a machine where the box would work.
    if std::env::var_os("CIG_INTERNAL_NO_BOX").is_some() {
        return Err("CIG_INTERNAL_NO_BOX is set".to_string());
    }
    #[cfg(target_os = "linux")]
    {
        if !(fs::read_to_string("/proc/filesystems")
            .map(|t| t.lines().any(|l| l.trim_end().ends_with("overlay")))
            .unwrap_or(false)
            || Path::new("/sys/module/overlay").exists())
        {
            return Err("this kernel has no overlayfs".to_string());
        }
        // The layers live under the state directory, and overlayfs refuses
        // an upper directory that sits on an overlay: a container's layer,
        // or another box. The box does not nest, and says so before the run.
        let home = super::runs::home_dir();
        if let Some(at) = overlay_mount_over(&home) {
            return Err(format!(
                "the layers would sit on an overlay filesystem ({} is on one, mounted at {}: a container's layer, or another box), which overlayfs refuses as an upper directory; a CIGSCRIPT_HOME on a plain filesystem lets the box be made",
                home.display(),
                at.display()
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("the box is made with overlayfs in a user namespace, which only Linux has".to_string())
    }
}

/// The mount point of the overlay filesystem `path` sits on, if any: the
/// longest mount point above it in `/proc/self/mountinfo` whose type is
/// `overlay`.
#[cfg(target_os = "linux")]
fn overlay_mount_over(path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<(PathBuf, String)> = None;
    for line in text.lines() {
        // id parent major:minor root mount_point options ... - fstype source superopts
        let (left, right) = line.split_once(" - ")?;
        let point = left.split(' ').nth(4)?;
        let fstype = right.split(' ').next()?;
        let point = PathBuf::from(
            point
                .replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\134", "\\"),
        );
        if !path.starts_with(&point) {
            continue;
        }
        let deeper = best
            .as_ref()
            .map(|(b, _)| point.components().count() > b.components().count())
            .unwrap_or(true);
        if deeper {
            best = Some((point, fstype.to_string()));
        }
    }
    best.filter(|(_, t)| t == "overlay").map(|(p, _)| p)
}

/// What the child does before it becomes the hop's program: new user and
/// mount namespaces, the user mapped to itself, every root mounted as an
/// overlay over itself, then into the working directory again (the
/// directory it stood in is the real root; the mount hid it). Only async-
/// signal-safe calls, since this runs between fork and exec. On any failure
/// the reason is written to `why` and the exec is refused, so the parent
/// runs the hop again without the box.
/// What a hop's `box` asks for beyond the overlay: its own PID namespace
/// (on by default), and the network it gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxSpec {
    pub pidns: bool,
    pub network: Network,
    pub allow_hosts: Vec<String>,
    pub apply: Apply,
}

impl Default for BoxSpec {
    fn default() -> Self {
        BoxSpec {
            pidns: true,
            network: Network::On,
            allow_hosts: Vec::new(),
            apply: Apply::OnSuccess,
        }
    }
}

/// The network a boxed hop gets: the machine's (`on`), none (`off`: a
/// namespace with only `lo`), or the registry alone (`registry`: the same
/// namespace, with one way out through cig's CONNECT proxy, which allows
/// `allow_hosts` and refuses and records the rest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    On,
    Off,
    Registry,
}

/// What becomes of a boxed hop's layer: applied when the hop succeeds (the
/// default), applied after a yes at the terminal, or kept and not applied
/// (`cig box apply` does it later).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    OnSuccess,
    Ask,
    Never,
}

/// The port inside a boxed hop's network namespace where the forwarder
/// listens (the namespace is the hop's own, so nothing else can hold it).
pub const PROXY_PORT: u16 = 38080;

/// Where the proxy's Unix socket for a hop lives: under the run's `proxy`,
/// a folder no hop may write (the guard leaves only `hops` and the inbox
/// writable, and the box binds this one read-only besides), so a hop cannot
/// put a link in the socket's place for the forwarder to follow; or, when
/// that path is too long for a socket address, under a private folder in
/// the temporary directory, bound read-only the same way. A socket on a
/// read-only mount still takes connections.
pub fn proxy_sock_path(run_dir: &Path, seq: u64) -> PathBuf {
    let dir = run_dir.join("proxy");
    let under = dir.join(format!("{seq}.sock"));
    if under.as_os_str().len() < 100 {
        let _ = fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
        return under;
    }
    let dir =
        std::env::temp_dir().join(format!("cig-{}-{}", dir_name(run_dir), std::process::id()));
    let _ = fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    dir.join(format!("{seq}.sock"))
}

/// Where a hop's events go (what it connected to, what was refused):
/// under the run, outside the folders the guard leaves writable.
pub fn events_path(run_dir: &Path, seq: u64) -> PathBuf {
    run_dir.join("events").join(format!("{seq}.jsonl"))
}

/// The hop's sequence number from its layers directory (`box/<seq>`).
pub fn seq_of(layers: &Layers) -> u64 {
    dir_name(&layers.dir).parse().unwrap_or(0)
}

/// A layer kept rather than applied (`apply: never`, or `ask` declined or
/// unattended): where it is and what it holds.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    pub run: String,
    pub seq: u64,
    pub label: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    pub roots: Vec<PathBuf>,
    #[serde(default)]
    pub touches_sealed: Vec<String>,
    pub written: usize,
    pub removed: usize,
    pub at: String,
    #[serde(skip)]
    pub dir: PathBuf,
}

/// Where a kept layer's record lives: `kept/<seq>.json` under the run, not
/// beside the layer. The layers directory is the hop's own while it runs, so
/// a record there could be planted (a "kept" layer naming roots of the hop's
/// choosing) or turned into a link that cig's write would follow; under the
/// guard `kept/` is read-only to every hop.
pub fn kept_record(layers_dir: &Path) -> PathBuf {
    let run_dir = layers_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| layers_dir.to_path_buf());
    run_dir
        .join("kept")
        .join(format!("{}.json", dir_name(layers_dir)))
}

/// Count what a layer would change: paths written (made or replaced) and
/// removed.
pub fn summarize(layers: &Layers) -> io::Result<(usize, usize)> {
    let (mut written, mut removed) = (0, 0);
    for (root, upper, _) in &layers.roots {
        for c in changes(root, upper)? {
            match c {
                Change::Removed { .. } => removed += 1,
                Change::Dir { .. } => {}
                _ => written += 1,
            }
        }
    }
    Ok((written, removed))
}

/// Keep a layer: write its record beside it, so `cig box apply` can apply
/// it later and `cig box list` can name it.
pub fn keep(
    layers: &Layers,
    label: &str,
    cwd: Option<&Path>,
    touches_sealed: &[String],
) -> io::Result<Kept> {
    let (written, removed) = summarize(layers)?;
    let kept = Kept {
        run: dir_name(&Plan::Boxed(layers).run_dir()),
        seq: seq_of(layers),
        label: label.to_string(),
        cwd: cwd.map(Path::to_path_buf),
        roots: layers.roots.iter().map(|(r, _, _)| r.clone()).collect(),
        touches_sealed: touches_sealed.to_vec(),
        written,
        removed,
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        dir: layers.dir.clone(),
    };
    let text = serde_json::to_string_pretty(&kept).map_err(io::Error::other)?;
    let record = kept_record(&layers.dir);
    let dir = record.parent().map(Path::to_path_buf).unwrap_or_default();
    fs::create_dir_all(&dir)?;
    // Written beside its final name and renamed in: a link planted at the
    // temporary name is replaced by the rename, never written through.
    let tmp = dir.join(format!(".{}.tmp", dir_name(&layers.dir)));
    let _ = fs::remove_file(&tmp);
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    fs::rename(&tmp, &record)?;
    Ok(kept)
}

/// Is this layers directory a kept layer?
pub fn is_kept(dir: &Path) -> bool {
    kept_record(dir).is_file()
}

/// Every kept layer under every run, oldest first.
pub fn kept_layers() -> Vec<Kept> {
    let mut out = Vec::new();
    let Ok(runs) = fs::read_dir(super::runs::runs_dir()) else {
        return out;
    };
    let mut run_dirs: Vec<PathBuf> = runs.flatten().map(|e| e.path()).collect();
    run_dirs.sort();
    for run in run_dirs {
        let Ok(seqs) = fs::read_dir(run.join("box")) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = seqs.flatten().map(|e| e.path()).collect();
        dirs.sort_by_key(|d| dir_name(d).parse::<u64>().unwrap_or(0));
        for dir in dirs {
            if let Ok(text) = fs::read_to_string(kept_record(&dir)) {
                if let Ok(mut k) = serde_json::from_str::<Kept>(&text) {
                    // The record names its own layer; one that names another
                    // run's or another hop's is not this layer's record.
                    if k.run == dir_name(&run) && k.seq.to_string() == dir_name(&dir) {
                        k.dir = dir.clone();
                        out.push(k);
                    }
                }
            }
        }
    }
    out
}

/// The layers of a kept layer, as `box_apply` wants them.
pub fn layers_of(kept: &Kept) -> Layers {
    Layers {
        dir: kept.dir.clone(),
        roots: kept
            .roots
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r.clone(),
                    kept.dir.join(i.to_string()).join("upper"),
                    kept.dir.join(i.to_string()).join("work"),
                )
            })
            .collect(),
    }
}

/// Remove the layers an interrupted run never got to apply, keeping the
/// ones kept on purpose. Returns how many went.
pub fn drop_unapplied(run_dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(run_dir.join("box")) {
        for e in rd.flatten() {
            let dir = e.path();
            if dir.is_dir() && !is_kept(&dir) {
                let _ = remove_tree_force(&dir);
                n += 1;
            }
        }
    }
    let _ = fs::remove_dir(run_dir.join("box"));
    // A record whose layer is gone keeps nothing.
    if let Ok(rd) = fs::read_dir(run_dir.join("kept")) {
        for e in rd.flatten() {
            let seq = e
                .path()
                .file_stem()
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or_default();
            if !run_dir.join("box").join(&seq).is_dir() {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let _ = fs::remove_dir(run_dir.join("kept"));
    n
}

#[cfg(target_os = "linux")]
pub struct Setup {
    pub uid_map: Vec<u8>,
    pub gid_map: Vec<u8>,
    /// (mount point, overlay options), each NUL-terminated.
    pub mounts: Vec<(std::ffi::CString, std::ffi::CString)>,
    /// The build and dependency trees hop watching never looks into (`.git`,
    /// `node_modules`, `target`, ...) that are there under a root: bound
    /// through the overlay from the real root, so what a hop writes there
    /// lands as it always did and is never journaled. A tree that is not
    /// there before the hop is made through the box like the rest.
    pub binds: Vec<std::ffi::CString>,
    pub cwd: std::ffi::CString,
    pub why: std::ffi::CString,
    /// The tin guard, when the setting asks for it (`tin_guard` auto or on).
    pub guard: Option<Guard>,
    /// The hop in a PID namespace of its own: a relay in the outer one, an
    /// init of cig's as pid 1 inside, the hop under it.
    pub pidns: bool,
    /// A network namespace of its own, with `lo` alone.
    pub netns: bool,
    /// `/dev/null` over `/etc/resolv.conf` inside: no resolver.
    pub resolv_null: bool,
    /// The folder of the proxy's socket, bound read-only inside, with the
    /// flags its mount must keep on a remount.
    pub sock_dir: Option<(std::ffi::CString, libc::c_ulong)>,
    /// The forwarder to start inside the namespace (`cig internal-forward
    /// <sock> <port>`), as the argv to exec.
    pub forward: Option<[std::ffi::CString; 4]>,
}

/// The tin read-only to the hop, and the key out of its sight. The state
/// root is bound over itself and remounted read-only, after the subtrees a
/// hop legitimately writes (its own layers, its protocol file) were bound
/// writable underneath, which the non-recursive remount leaves alone; an
/// empty tmpfs goes over the key folder. A guard that cannot be made is
/// noted in `why`, and the hop runs boxed without it (`tin_guard = on`
/// refuses instead).
#[cfg(target_os = "linux")]
pub struct Guard {
    pub home: std::ffi::CString,
    /// The flags the state root's mount must keep when it is remounted
    /// read-only (`locked_flags`).
    pub home_flags: libc::c_ulong,
    pub keys: std::ffi::CString,
    pub writable: Vec<std::ffi::CString>,
    pub why: std::ffi::CString,
    /// `tin_guard = on`: a guard that cannot be made refuses the hop.
    pub required: bool,
    /// The inbox: the one writable place under the state directory, and a
    /// cig inside the hop's own state root.
    pub inbox: PathBuf,
    /// `CIG_INTERNAL_NO_GUARD`: behave as if the mounts were refused.
    pub pretend_unavailable: bool,
    /// The other ways to the key, each covered with `/dev/null`: a key file
    /// named outright (`CIG_SEAL_KEY_FILE`) outside the key folder, and,
    /// while the key may be in the OS keyring, the session bus and systemd's
    /// private socket (see `key_routes`).
    pub covers: Vec<std::ffi::CString>,
    /// The session bus is cut: the hop's environment does not name it.
    pub bus_cut: bool,
}

/// What confines a hop: the box (an overlay over each pack root, with the
/// guard), or the guard alone (`tin_guard = on`, for a hop outside the box).
#[derive(Debug)]
pub enum Plan<'a> {
    Boxed(&'a Layers),
    GuardOnly { run_dir: PathBuf, seq: u64 },
}

impl Plan<'_> {
    /// Where this hop's notes go (`why`, `guard-why`): the layers directory,
    /// or a folder of the hop's own under the run's `hops`.
    pub fn base(&self) -> PathBuf {
        match self {
            Plan::Boxed(l) => l.dir.clone(),
            Plan::GuardOnly { run_dir, seq } => run_dir.join("hops").join(format!("{seq}-guard")),
        }
    }

    /// The run this hop belongs to.
    pub fn run_dir(&self) -> PathBuf {
        match self {
            Plan::Boxed(l) => l
                .dir
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| l.dir.clone()),
            Plan::GuardOnly { run_dir, .. } => run_dir.clone(),
        }
    }

    pub fn run_id(&self) -> String {
        dir_name(&self.run_dir())
    }
}

/// Where the guard writes why it could not be made.
pub fn guard_why_path(base: &Path) -> PathBuf {
    base.join("guard-why")
}

/// What a hop's setup wrote when it could not be made at all (`why`).
pub fn why_at(base: &Path) -> Option<String> {
    fs::read_to_string(base.join("why"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The inbox of a run: under the state directory, named by the run, the
/// one place a guarded hop may write. A cig started inside the hop has
/// `home` under it as its state root and `keys` as its key folder.
pub fn inbox_dir(run_dir: &Path) -> PathBuf {
    let home = super::runs::home_dir();
    let home = std::path::absolute(&home).unwrap_or(home);
    home.join("inbox").join(dir_name(run_dir))
}

/// Make a run's inbox, owner-only, with the parent's settings copied in
/// for the cig inside and the key kept in a file of the inbox's own: the
/// hop must not reach the parent's key through the keyring.
pub fn prepare_inbox(run_dir: &Path) -> io::Result<PathBuf> {
    let inbox = inbox_dir(run_dir);
    let home = inbox.join("home");
    let keys = inbox.join("keys");
    for d in [&inbox, &home, &keys] {
        fs::create_dir_all(d)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(d, fs::Permissions::from_mode(0o700));
        }
    }
    let mut text: String = fs::read_to_string(crate::config::path())
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            l.split_once('=')
                .map(|(k, _)| k.trim() != "seal_key")
                .unwrap_or(true)
        })
        .map(|l| format!("{l}\n"))
        .collect();
    text.push_str("seal_key = file\n");
    fs::write(home.join("config"), text)?;
    Ok(inbox)
}

/// Is the session bus cut from guarded hops? While the key may be in the OS
/// keyring (`seal_key` is not `file`): on Linux that keyring is the Secret
/// Service on the session bus, which hands a secret to any process of the
/// user's that asks for it, and systemd's user manager on the same bus would
/// start a process outside the box that could ask.
pub fn bus_cut() -> bool {
    crate::config::Config::load().get("seal_key") != "file"
}

/// The ways to the key a guarded hop must not have, beyond the key folder
/// the tmpfs hides: a key file named outright by `CIG_SEAL_KEY_FILE` that
/// lies outside that folder, and, when `bus_cut`, the session bus (the
/// `path=` of `DBUS_SESSION_BUS_ADDRESS`, `$XDG_RUNTIME_DIR/bus`) and
/// systemd's private socket. Each is real (canonical), present and not a
/// directory; an abstract socket has no path to cover, and is out of reach
/// only in a network namespace of the hop's own.
#[cfg(target_os = "linux")]
pub fn key_routes(keys: &Path, bus_cut: bool) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        let Ok(meta) = fs::symlink_metadata(&p) else {
            return;
        };
        let real = fs::canonicalize(&p).unwrap_or(p);
        let is_dir = if meta.file_type().is_symlink() {
            real.is_dir()
        } else {
            meta.is_dir()
        };
        if !is_dir && !found.contains(&real) {
            found.push(real);
        }
    };
    if let Some(f) = std::env::var_os("CIG_SEAL_KEY_FILE") {
        let f = PathBuf::from(f);
        let f = std::path::absolute(&f).unwrap_or(f);
        let keys = fs::canonicalize(keys).unwrap_or_else(|_| keys.to_path_buf());
        let real = fs::canonicalize(&f).unwrap_or_else(|_| f.clone());
        if !real.starts_with(&keys) {
            add(f);
        }
    }
    if bus_cut {
        if let Some(addr) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
            for p in bus_paths(&addr.to_string_lossy()) {
                add(p);
            }
        }
        // The runtime folder named, and the uid's own whether named or not:
        // the bus sits at a fixed path, so a hop needs no variable to find it.
        let mut runtime: Vec<PathBuf> = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .into_iter()
            .collect();
        runtime.push(PathBuf::from(format!("/run/user/{}", unsafe {
            libc::geteuid()
        })));
        for rt in runtime {
            add(rt.join("bus"));
            add(rt.join("systemd").join("private"));
        }
    }
    found
}

/// The socket paths in a D-Bus address (`unix:path=/run/user/1000/bus`,
/// alternatives separated by `;`, values %-escaped).
#[cfg(target_os = "linux")]
fn bus_paths(addr: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for alt in addr.split(';') {
        let Some(("unix", params)) = alt.split_once(':') else {
            continue;
        };
        for kv in params.split(',') {
            if let Some(v) = kv.strip_prefix("path=") {
                out.push(PathBuf::from(unescape_bus(v)));
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn unescape_bus(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(n) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(n);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the `tin_guard` setting says: `auto` (the default), `on` or `off`;
/// `CIG_TIN_GUARD=0` is `off` for one run.
pub fn guard_setting() -> String {
    if std::env::var_os("CIG_TIN_GUARD").is_some_and(|v| v == "0") {
        return "off".to_string();
    }
    let s = crate::config::Config::load().get("tin_guard");
    if s == "on" || s == "off" {
        s
    } else {
        "auto".to_string()
    }
}

/// How many trees the child binds at most (a stack array holds their
/// descriptors between fork and exec).
#[cfg(target_os = "linux")]
const MAX_BINDS: usize = 64;

#[cfg(target_os = "linux")]
impl Setup {
    pub fn new(plan: &Plan, cwd: &Path, spec: &BoxSpec) -> io::Result<Setup> {
        use std::os::unix::ffi::OsStrExt;
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let cstr = |p: &Path| -> io::Result<std::ffi::CString> {
            std::ffi::CString::new(p.as_os_str().as_bytes())
                .map_err(|_| io::Error::other("a path holds a NUL byte"))
        };
        let mut mounts = Vec::new();
        let mut binds = Vec::new();
        let layers = match plan {
            Plan::Boxed(l) => Some(*l),
            Plan::GuardOnly { .. } => None,
        };
        for (root, upper, work) in layers.map(|l| l.roots.as_slice()).unwrap_or(&[]) {
            let opts = format!(
                "lowerdir={},upperdir={},workdir={},userxattr",
                escape(root),
                escape(upper),
                escape(work)
            );
            mounts.push((
                cstr(root)?,
                std::ffi::CString::new(opts).map_err(|_| io::Error::other("NUL in options"))?,
            ));
            for name in super::tin::SKIPPED_ALWAYS {
                let dir = root.join(name);
                if fs::symlink_metadata(&dir)
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
                    && binds.len() < MAX_BINDS
                {
                    binds.push(cstr(&dir)?);
                }
            }
        }
        // The guard: the state root, the key folder, and what stays writable.
        let base = plan.base();
        fs::create_dir_all(&base)?;
        let run_dir = plan.run_dir();
        let guard = if guard_setting() == "off" {
            None
        } else {
            let home = std::path::absolute(super::runs::home_dir())
                .unwrap_or_else(|_| super::runs::home_dir());
            // The key folder is made before the hop runs, so that the empty
            // tmpfs has something to cover: a hop must neither read a key
            // nor plant one for the first seal to pick up.
            let keys = super::seal::keys_dir();
            fs::create_dir_all(&keys)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&keys, fs::Permissions::from_mode(0o700));
            }
            let hops = run_dir.join("hops");
            fs::create_dir_all(&hops)?;
            let mut writable = vec![cstr(&hops)?];
            if let Some(l) = layers {
                writable.push(cstr(&l.dir)?);
            }
            // The inbox: a cig inside the hop has it as its own state root.
            let inbox = prepare_inbox(&run_dir)?;
            writable.push(cstr(&inbox)?);
            let bus_cut = bus_cut();
            let mut covers = Vec::new();
            for p in key_routes(&keys, bus_cut) {
                covers.push(cstr(&p)?);
            }
            Some(Guard {
                home_flags: locked_flags(&home),
                home: cstr(&home)?,
                keys: cstr(&keys)?,
                writable,
                why: cstr(&guard_why_path(&base))?,
                required: guard_setting() == "on",
                inbox,
                pretend_unavailable: std::env::var_os("CIG_INTERNAL_NO_GUARD").is_some(),
                covers,
                bus_cut,
            })
        };
        // The namespaces beyond the mounts: only with the box itself.
        let boxed = layers.is_some();
        let netns = boxed && spec.network != Network::On;
        let mut forward = None;
        let mut sock_dir = None;
        if netns && spec.network == Network::Registry {
            let seq = layers.map(seq_of).unwrap_or(0);
            let sock = proxy_sock_path(&run_dir, seq);
            if let Some(dir) = sock.parent() {
                sock_dir = Some((cstr(dir)?, locked_flags(dir)));
            }
            let exe = std::env::current_exe()?;
            forward = Some([
                cstr(&exe)?,
                std::ffi::CString::new("internal-forward").expect("plain"),
                cstr(&sock)?,
                std::ffi::CString::new(PROXY_PORT.to_string()).expect("digits"),
            ]);
        }
        Ok(Setup {
            uid_map: format!("{uid} {uid} 1\n").into_bytes(),
            gid_map: format!("{gid} {gid} 1\n").into_bytes(),
            mounts,
            binds,
            cwd: cstr(cwd)?,
            why: cstr(&base.join("why"))?,
            guard,
            pidns: boxed && spec.pidns,
            netns,
            resolv_null: netns,
            sock_dir,
            forward,
        })
    }

    /// Run in the child, between fork and exec.
    ///
    /// # Safety
    ///
    /// Only async-signal-safe calls are made (unshare, open, write, mount,
    /// chdir, close), on data prepared before the fork (the descriptors the
    /// binds name were opened there); the caller runs it in a `pre_exec`
    /// closure and nowhere else.
    pub unsafe fn apply(&self) -> io::Result<()> {
        let mut flags = libc::CLONE_NEWUSER | libc::CLONE_NEWNS;
        if self.pidns {
            flags |= libc::CLONE_NEWPID;
        }
        if self.netns {
            flags |= libc::CLONE_NEWNET;
        }
        if libc::unshare(flags) != 0 {
            return self.fail(b"unshare(CLONE_NEWUSER|CLONE_NEWNS) refused");
        }
        // Once cig has loaded the tin's key it is non-dumpable
        // (seal::harden_process_memory), and the fork inherits that. A
        // non-dumpable process's /proc/self files are owned by root, so this
        // child could not open its own uid_map to write it. The child is
        // about to exec, which makes it dumpable again regardless, so it is
        // made dumpable now, inside its own user namespace; the parent cig
        // holding the key stays protected.
        libc::prctl(libc::PR_SET_DUMPABLE, 1, 0, 0, 0);
        if !write_file(c"/proc/self/uid_map", &self.uid_map) {
            return self.fail(b"writing /proc/self/uid_map refused");
        }
        write_file(c"/proc/self/setgroups", b"deny\n");
        if !write_file(c"/proc/self/gid_map", &self.gid_map) {
            return self.fail(b"writing /proc/self/gid_map refused");
        }
        // The maps are written: back to non-dumpable. The relay and the init
        // below never exec, so they stay copies of cig's memory, the key
        // among it when one was loaded; non-dumpable, nothing of the user's
        // can trace or read them. The hop's exec makes it dumpable again.
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        // Nothing mounted here must reach the parent's namespace.
        if libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        ) != 0
        {
            return self.fail(b"making the mount tree private refused");
        }
        // The tin guard, before the overlays: the state root bound over
        // itself (read-write), the hop's own subtrees bound writable under
        // it, then the root remounted read-only (not recursively, so those
        // stay writable), and an empty tmpfs over the key folder. A failure
        // here is noted and the hop goes on boxed without the guard; the
        // parent decides what `tin_guard = on` makes of it.
        if let Some(g) = &self.guard {
            let null = std::ptr::null::<libc::c_char>();
            let mut failed: Option<&[u8]> = None;
            if g.pretend_unavailable {
                failed = Some(b"CIG_INTERNAL_NO_GUARD is set");
            }
            if failed.is_none()
                && libc::mount(
                    g.home.as_ptr(),
                    g.home.as_ptr(),
                    null,
                    libc::MS_BIND,
                    std::ptr::null(),
                ) != 0
            {
                failed = Some(b"binding the state directory over itself refused");
            }
            if failed.is_none() {
                for w in &g.writable {
                    if libc::mount(
                        w.as_ptr(),
                        w.as_ptr(),
                        null,
                        libc::MS_BIND,
                        std::ptr::null(),
                    ) != 0
                    {
                        failed = Some(b"binding a writable subtree of the state directory refused");
                        break;
                    }
                }
            }
            if failed.is_none()
                && libc::mount(
                    null,
                    g.home.as_ptr(),
                    null,
                    libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | g.home_flags,
                    std::ptr::null(),
                ) != 0
            {
                failed = Some(b"remounting the state directory read-only refused");
            }
            if failed.is_none()
                && libc::mount(
                    c"tmpfs".as_ptr(),
                    g.keys.as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    c"mode=0700,size=16k".as_ptr() as *const libc::c_void,
                ) != 0
            {
                failed = Some(b"hiding the key folder refused");
            }
            if let Some(what) = failed {
                self.note(&g.why, what);
                if g.required {
                    // Asked for, not available: the hop does not run.
                    return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
                }
            }
        }
        // The build and dependency trees, held open before the overlay hides
        // them. Opened here, after the namespace split: a descriptor from
        // before it names the parent namespace's mounts, which this one
        // cannot bind (EINVAL).
        let mut held: [libc::c_int; MAX_BINDS] = [-1; MAX_BINDS];
        for (i, at) in self.binds.iter().enumerate() {
            held[i] = libc::open(
                at.as_ptr(),
                libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            );
        }
        for (at, opts) in &self.mounts {
            if libc::mount(
                c"overlay".as_ptr(),
                at.as_ptr(),
                c"overlay".as_ptr(),
                0,
                opts.as_ptr() as *const libc::c_void,
            ) != 0
            {
                return self.fail(b"mounting overlayfs over a root refused (a kernel before 5.11, or a filesystem overlayfs cannot layer over)");
            }
        }
        // Those trees come through from the real root: what a hop writes
        // there is not the box's business, as it is not the watch's.
        for (i, at) in self.binds.iter().enumerate() {
            let fd = held[i];
            if fd < 0 {
                // Gone since the plan was made: nothing to bind.
                continue;
            }
            let prefix = b"/proc/self/fd/";
            let mut from = [0u8; 48];
            from[..prefix.len()].copy_from_slice(prefix);
            let mut digits = [0u8; 32];
            let k = itoa(fd, &mut digits);
            from[prefix.len()..prefix.len() + k].copy_from_slice(&digits[..k]);
            if libc::mount(
                from.as_ptr() as *const libc::c_char,
                at.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            ) != 0
            {
                return self.fail(b"binding a build tree through the overlay refused");
            }
            libc::close(fd);
        }
        // The other ways to the key, covered with /dev/null after the
        // overlays, so the cover is in the view the hop has: a key file read
        // gives nothing, a socket refuses the connection. The hop keeps its
        // own uid in the namespace, so after exec it has no capability to
        // take a cover off, and a namespace it makes of its own inherits the
        // cover locked.
        if let Some(g) = &self.guard {
            for c in &g.covers {
                if libc::mount(
                    c"/dev/null".as_ptr(),
                    c.as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND,
                    std::ptr::null(),
                ) != 0
                {
                    self.note(&g.why, b"covering a way to the key refused");
                    if g.required {
                        return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
                    }
                }
            }
        }
        // Into the working directory again, through the overlay this time.
        if libc::chdir(self.cwd.as_ptr()) != 0 {
            return self.fail(b"entering the working directory through the overlay refused");
        }
        // The network namespace has `lo` down and no resolver: bring `lo`
        // up (the forwarder listens on it), and bind an empty file over
        // resolv.conf so a lookup has no one to ask.
        if self.netns {
            let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if s >= 0 {
                let mut ifr: libc::ifreq = std::mem::zeroed();
                ifr.ifr_name[0] = b'l' as libc::c_char;
                ifr.ifr_name[1] = b'o' as libc::c_char;
                ifr.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
                libc::ioctl(s, libc::SIOCSIFFLAGS as _, &ifr);
                libc::close(s);
            }
            if self.resolv_null {
                libc::mount(
                    c"/dev/null".as_ptr(),
                    c"/etc/resolv.conf".as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND,
                    std::ptr::null(),
                );
            }
            // The proxy's socket folder, read-only inside whatever the guard
            // managed: the forwarder connects by path, so the hop must not be
            // able to put a link to another socket in its place.
            if let Some((dir, flags)) = &self.sock_dir {
                if libc::mount(
                    dir.as_ptr(),
                    dir.as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND,
                    std::ptr::null(),
                ) != 0
                    || libc::mount(
                        std::ptr::null(),
                        dir.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | *flags,
                        std::ptr::null(),
                    ) != 0
                {
                    return self.fail(b"making the proxy's socket folder read-only refused");
                }
            }
        }
        // The PID namespace: unshare(CLONE_NEWPID) puts only children in
        // it, so this process (R) stays outside as a relay, forks the
        // namespace's init (A, pid 1 inside), which forks the hop (B) and
        // reaps for it. Each dies with its parent (PR_SET_PDEATHSIG), so a
        // cig that dies takes the whole namespace with it, and the layer is
        // applied only by a live cig after the hop. R and A ignore the
        // termination signals cig sends the group, so the hop gets its
        // grace; the SIGKILL after it reaches everyone.
        if self.pidns {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            let a = libc::fork();
            if a < 0 {
                return self.fail(b"forking the box's init refused");
            }
            if a > 0 {
                // The relay is a forked copy of cig: the parent's signal
                // handlers came with it (the child-exit handler of the
                // waiting crate writes to a pipe of the parent's), and so
                // did every descriptor, among them the standard library's
                // exec-status pipe, which the parent reads to the end
                // before `spawn` returns. The handler goes back to the
                // default and every descriptor above stderr is closed, so
                // nothing here holds `spawn`, or the hop's timeout, until
                // the hop ends.
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
                close_from_3();
                libc::_exit(wait_for(a));
            }
            // A: pid 1 inside, the same forked copy; the same resets.
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            // Its own /proc, best effort (a container may refuse it; the
            // namespace still hides the pids from signals).
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
            libc::mount(
                c"proc".as_ptr(),
                c"/proc".as_ptr(),
                c"proc".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                std::ptr::null(),
            );
            if let Some(f) = &self.forward {
                let p = libc::fork();
                if p == 0 {
                    // The forwarder's own stdio to /dev/null: it must not
                    // hold the hop's pipes open after the hop has ended.
                    let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
                    if null >= 0 {
                        libc::dup2(null, 0);
                        libc::dup2(null, 1);
                        libc::dup2(null, 2);
                    }
                    let argv = [
                        f[0].as_ptr(),
                        f[1].as_ptr(),
                        f[2].as_ptr(),
                        f[3].as_ptr(),
                        std::ptr::null(),
                    ];
                    libc::execv(f[0].as_ptr(), argv.as_ptr());
                    libc::_exit(127);
                }
            }
            let b = libc::fork();
            if b < 0 {
                libc::_exit(127);
            }
            if b > 0 {
                // The init needs no descriptor either (see the relay).
                close_from_3();
                // The init loop: reap everything; the hop's exit is ours.
                let mut status: libc::c_int = 0;
                loop {
                    let r = libc::waitpid(-1, &mut status, 0);
                    if r == b {
                        libc::_exit(exit_code(status));
                    }
                    if r < 0 {
                        let e = *libc::__errno_location();
                        if e == libc::EINTR {
                            continue;
                        }
                        libc::_exit(127);
                    }
                }
            }
            // B: the hop. Signals as the hop expects them.
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
        }
        // Whatever descriptor cig held without close-on-exec (a library's,
        // never cig's own) is closed by the exec: one that named a folder
        // before the mounts above would still write through the old view.
        cloexec_from_3();
        Ok(())
    }

    /// Write a reason with the errno to `at`; the exec goes on.
    unsafe fn note(&self, at: &std::ffi::CString, what: &[u8]) {
        let errno = *libc::__errno_location();
        let fd = libc::open(
            at.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
            0o644,
        );
        if fd >= 0 {
            let _ = libc::write(fd, what.as_ptr() as *const libc::c_void, what.len());
            let mut buf = [0u8; 32];
            let n = itoa(errno, &mut buf);
            let _ = libc::write(fd, b" (errno ".as_ptr() as *const libc::c_void, 8);
            let _ = libc::write(fd, buf.as_ptr() as *const libc::c_void, n);
            let _ = libc::write(fd, b")\n".as_ptr() as *const libc::c_void, 2);
            libc::close(fd);
        }
    }

    /// Write the reason with the errno, then refuse the exec.
    unsafe fn fail(&self, what: &[u8]) -> io::Result<()> {
        let errno = *libc::__errno_location();
        let fd = libc::open(
            self.why.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
            0o644,
        );
        if fd >= 0 {
            let _ = libc::write(fd, what.as_ptr() as *const libc::c_void, what.len());
            let mut buf = [0u8; 32];
            let n = itoa(errno, &mut buf);
            let _ = libc::write(fd, b" (errno ".as_ptr() as *const libc::c_void, 8);
            let _ = libc::write(fd, buf.as_ptr() as *const libc::c_void, n);
            let _ = libc::write(fd, b")\n".as_ptr() as *const libc::c_void, 2);
            libc::close(fd);
        }
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }
}

/// Close every descriptor from 3 up: `close_range` where the kernel has
/// it (5.9), one by one where it does not. Async-signal-safe.
#[cfg(target_os = "linux")]
unsafe fn close_from_3() {
    if libc::syscall(
        libc::SYS_close_range,
        3 as libc::c_long,
        libc::c_uint::MAX as libc::c_long,
        0 as libc::c_long,
    ) < 0
    {
        for fd in 3..4096 {
            libc::close(fd);
        }
    }
}

/// Mark every descriptor above stderr close-on-exec (`close_range` with
/// `CLOSE_RANGE_CLOEXEC`, Linux 5.11, which the box needs anyway; else one
/// at a time). The standard library's exec-status pipe is close-on-exec
/// already, so the exec still reports to the parent.
#[cfg(target_os = "linux")]
unsafe fn cloexec_from_3() {
    const CLOSE_RANGE_CLOEXEC: libc::c_long = 1 << 2;
    if libc::syscall(
        libc::SYS_close_range,
        3 as libc::c_long,
        libc::c_uint::MAX as libc::c_long,
        CLOSE_RANGE_CLOEXEC,
    ) < 0
    {
        for fd in 3..4096 {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// The flags a bind remount of `path` must carry: a mount copied into a
/// user namespace keeps its nosuid, nodev, noexec and atime settings
/// locked, and a remount that would change one is refused (EPERM).
#[cfg(target_os = "linux")]
pub fn locked_flags(path: &Path) -> libc::c_ulong {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return 0;
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    // The kernel's statfs flag for relatime (not in the libc crate's musl
    // bindings, though the kernel sets it there too).
    const ST_RELATIME: libc::c_ulong = 4096;
    let f = st.f_flag as libc::c_ulong;
    let mut out: libc::c_ulong = 0;
    for (st_bit, ms_bit) in [
        (libc::ST_NOSUID as libc::c_ulong, libc::MS_NOSUID),
        (libc::ST_NODEV as libc::c_ulong, libc::MS_NODEV),
        (libc::ST_NOEXEC as libc::c_ulong, libc::MS_NOEXEC),
        (libc::ST_NOATIME as libc::c_ulong, libc::MS_NOATIME),
        (libc::ST_NODIRATIME as libc::c_ulong, libc::MS_NODIRATIME),
        (ST_RELATIME, libc::MS_RELATIME),
    ] {
        if f & st_bit != 0 {
            out |= ms_bit;
        }
    }
    if f & (libc::ST_NOATIME as libc::c_ulong | ST_RELATIME) == 0 {
        out |= libc::MS_STRICTATIME;
    }
    out
}

/// Wait for one child (through EINTR) and give its exit code.
#[cfg(target_os = "linux")]
unsafe fn wait_for(pid: libc::pid_t) -> libc::c_int {
    let mut status: libc::c_int = 0;
    loop {
        let r = libc::waitpid(pid, &mut status, 0);
        if r == pid {
            return exit_code(status);
        }
        if r < 0 && *libc::__errno_location() != libc::EINTR {
            return 127;
        }
    }
}

/// A wait status as an exit code: the code, or 128 plus the signal.
#[cfg(target_os = "linux")]
fn exit_code(status: libc::c_int) -> libc::c_int {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        127
    }
}

/// A directory's own name (a run's id), as text.
pub(crate) fn dir_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Overlayfs options separate directories with `,` and `:`, so those are
/// escaped with a backslash in a path that holds them.
#[cfg(target_os = "linux")]
fn escape(p: &Path) -> String {
    p.to_string_lossy()
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(':', "\\:")
}

#[cfg(target_os = "linux")]
unsafe fn write_file(path: &std::ffi::CStr, bytes: &[u8]) -> bool {
    let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
    if fd < 0 {
        return false;
    }
    let n = libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
    libc::close(fd);
    n == bytes.len() as isize
}

#[cfg(target_os = "linux")]
fn itoa(mut n: i32, buf: &mut [u8; 32]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let neg = n < 0;
    if neg {
        n = -n;
    }
    let mut digits = [0u8; 16];
    let mut k = 0;
    while n > 0 {
        digits[k] = b'0' + (n % 10) as u8;
        n /= 10;
        k += 1;
    }
    let mut len = 0;
    if neg {
        buf[0] = b'-';
        len = 1;
    }
    for i in (0..k).rev() {
        buf[len] = digits[i];
        len += 1;
    }
    len
}

/// One change the upper layer holds, in the order it is applied.
#[derive(Debug)]
pub enum Change {
    /// A directory the hop made (or wrote into): made if missing.
    Dir { real: PathBuf, mode: u32 },
    /// A directory the hop replaced whole (overlayfs marks it opaque): the
    /// real one goes, the upper's contents come.
    Replaced { real: PathBuf },
    /// A file the hop wrote: the real one takes the upper's bytes.
    File { real: PathBuf, upper: PathBuf },
    /// A link the hop made or retargeted.
    Link { real: PathBuf, target: PathBuf },
    /// A path the hop removed (a whiteout).
    Removed { real: PathBuf },
}

impl Change {
    pub fn real(&self) -> &Path {
        match self {
            Change::Dir { real, .. }
            | Change::Replaced { real }
            | Change::File { real, .. }
            | Change::Link { real, .. }
            | Change::Removed { real } => real,
        }
    }
}

/// Is this entry overlayfs's whiteout: a character device 0/0?
#[cfg(target_os = "linux")]
fn is_whiteout(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    meta.file_type().is_char_device() && meta.rdev() == 0
}

/// Does overlayfs mark this upper directory opaque (the lower hidden)?
#[cfg(target_os = "linux")]
fn is_opaque(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    let mut buf = [0u8; 8];
    let n = unsafe {
        libc::getxattr(
            c.as_ptr(),
            c"user.overlay.opaque".as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    };
    n > 0 && buf[0] == b'y'
}

/// Read one root's upper layer into the changes to apply, parents before
/// children, in path order.
pub fn changes(root: &Path, upper: &Path) -> io::Result<Vec<Change>> {
    let mut out = Vec::new();
    walk_upper(root, upper, upper, &mut out)?;
    Ok(out)
}

fn walk_upper(root: &Path, upper: &Path, dir: &Path, out: &mut Vec<Change>) -> io::Result<()> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let rel = p.strip_prefix(upper).unwrap_or(&p);
        let real = root.join(rel);
        let meta = fs::symlink_metadata(&p)?;
        #[cfg(target_os = "linux")]
        if is_whiteout(&meta) {
            out.push(Change::Removed { real });
            continue;
        }
        if meta.file_type().is_symlink() {
            out.push(Change::Link {
                real,
                target: fs::read_link(&p)?,
            });
        } else if meta.is_dir() {
            #[cfg(target_os = "linux")]
            let opaque = is_opaque(&p);
            #[cfg(not(target_os = "linux"))]
            let opaque = false;
            if opaque {
                // The real directory goes whole; the upper's is made in its
                // place (empty, when the hop left it so) and filled below.
                out.push(Change::Replaced { real: real.clone() });
            }
            out.push(Change::Dir {
                real: real.clone(),
                mode: mode_of(&meta),
            });
            walk_upper(root, upper, &p, out)?;
        } else if meta.is_file() {
            out.push(Change::File { real, upper: p });
        }
        // Anything else (a socket, a pipe) a hop left in its layer is not a
        // file of the pack and is dropped.
    }
    Ok(())
}

fn mode_of(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0o755
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn the_bus_paths_come_out_of_an_address() {
        assert_eq!(
            bus_paths("unix:path=/run/user/1000/bus"),
            vec![PathBuf::from("/run/user/1000/bus")]
        );
        assert_eq!(
            bus_paths("unix:abstract=/tmp/dbus-x,guid=1;unix:path=/tmp/a%20b/bus,guid=2"),
            vec![PathBuf::from("/tmp/a b/bus")]
        );
        assert!(bus_paths("tcp:host=localhost,port=1").is_empty());
    }
}
