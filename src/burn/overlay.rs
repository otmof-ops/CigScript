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
pub fn possible() -> Result<(), &'static str> {
    // Test hook: the fallback, on a machine where the box would work.
    if std::env::var_os("CIG_INTERNAL_NO_BOX").is_some() {
        return Err("CIG_INTERNAL_NO_BOX is set");
    }
    #[cfg(target_os = "linux")]
    {
        if fs::read_to_string("/proc/filesystems")
            .map(|t| t.lines().any(|l| l.trim_end().ends_with("overlay")))
            .unwrap_or(false)
            || Path::new("/sys/module/overlay").exists()
        {
            Ok(())
        } else {
            Err("this kernel has no overlayfs")
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("the box is made with overlayfs in a user namespace, which only Linux has")
    }
}

/// What the child does before it becomes the hop's program: new user and
/// mount namespaces, the user mapped to itself, every root mounted as an
/// overlay over itself, then into the working directory again (the
/// directory it stood in is the real root; the mount hid it). Only async-
/// signal-safe calls, since this runs between fork and exec. On any failure
/// the reason is written to `why` and the exec is refused, so the parent
/// runs the hop again without the box.
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
}

/// How many trees the child binds at most (a stack array holds their
/// descriptors between fork and exec).
#[cfg(target_os = "linux")]
const MAX_BINDS: usize = 64;

#[cfg(target_os = "linux")]
impl Setup {
    pub fn new(layers: &Layers, cwd: &Path) -> io::Result<Setup> {
        use std::os::unix::ffi::OsStrExt;
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let cstr = |p: &Path| -> io::Result<std::ffi::CString> {
            std::ffi::CString::new(p.as_os_str().as_bytes())
                .map_err(|_| io::Error::other("a path holds a NUL byte"))
        };
        let mut mounts = Vec::new();
        let mut binds = Vec::new();
        for (root, upper, work) in &layers.roots {
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
        Ok(Setup {
            uid_map: format!("{uid} {uid} 1\n").into_bytes(),
            gid_map: format!("{gid} {gid} 1\n").into_bytes(),
            mounts,
            binds,
            cwd: cstr(cwd)?,
            why: cstr(&layers.dir.join("why"))?,
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
        if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) != 0 {
            return self.fail(b"unshare(CLONE_NEWUSER|CLONE_NEWNS) refused");
        }
        if !write_file(c"/proc/self/uid_map", &self.uid_map) {
            return self.fail(b"writing /proc/self/uid_map refused");
        }
        write_file(c"/proc/self/setgroups", b"deny\n");
        if !write_file(c"/proc/self/gid_map", &self.gid_map) {
            return self.fail(b"writing /proc/self/gid_map refused");
        }
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
        // Into the working directory again, through the overlay this time.
        if libc::chdir(self.cwd.as_ptr()) != 0 {
            return self.fail(b"entering the working directory through the overlay refused");
        }
        Ok(())
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
