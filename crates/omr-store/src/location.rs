//! Refused project locations (SYS-015, spec §14.2): network file systems and sync folders.
//!
//! Sync clients copy `-wal`/`-shm` out of step with the database and put Restricted data on
//! personal cloud storage; network file systems break SQLite locking. Detection covers the
//! Linux signals of §14.2 (mount types from `/proc/self/mountinfo`, remote FUSE subtypes,
//! ancestor markers, well-known sync folders), the macOS sync-folder paths and Windows UNC
//! paths. The Windows `GetDriveTypeW`/Cloud Files and macOS `statfs`/iCloud checks need OS
//! APIs and are left to the platform layer (coverage unverified, as the spec says).

use crate::error::{Result, StoreError};
use std::path::{Path, PathBuf};

/// Mount types refused on Linux.
pub const REMOTE_FS_TYPES: &[&str] = &[
    "nfs", "nfs4", "cifs", "smb3", "smbfs", "afs", "ncpfs", "ceph", "glusterfs",
];

/// Remote FUSE subtypes (local `ntfs-3g`/`exfat-fuse` disks pass).
pub const REMOTE_FUSE: &[&str] = &[
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.gvfsd-fuse",
];

/// Marker files or directories of sync clients, looked for in every ancestor.
pub const SYNC_MARKERS: &[&str] = &[".dropbox", ".dropbox.cache", ".owncloudsync.log"];

/// Sync folders under the home directory.
pub const HOME_SYNC_DIRS: &[&str] = &[
    "Dropbox",
    "Nextcloud",
    "OneDrive",
    "Library/Mobile Documents",
    "Library/CloudStorage",
];

/// Why `path` is unsafe for a live project, or `None` when no signal fired.
pub fn unsafe_reason(path: &Path) -> Option<String> {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy();
    if (s.starts_with(r"\\") && !s.starts_with(r"\\?\")) || s.starts_with(r"\\?\UNC\") {
        return Some(format!("UNC network path {s}"));
    }
    for anc in abs.ancestors() {
        for m in SYNC_MARKERS {
            if anc.join(m).exists() {
                return Some(format!("sync-client marker {m} in {}", anc.display()));
            }
        }
        if let Ok(rd) = std::fs::read_dir(anc) {
            for e in rd.flatten() {
                let n = e.file_name();
                let n = n.to_string_lossy();
                if n.starts_with(".sync_") && n.ends_with(".db") {
                    return Some(format!("sync-client marker {n} in {}", anc.display()));
                }
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for d in HOME_SYNC_DIRS {
            let p = home.join(d);
            if abs.starts_with(&p) {
                return Some(format!("inside the sync folder {}", p.display()));
            }
        }
    }
    if let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo")
        && let Some(fs) = mount_type_for(&info, &abs)
        && (REMOTE_FS_TYPES.contains(&fs.as_str()) || REMOTE_FUSE.contains(&fs.as_str()))
    {
        return Some(format!("network or remote file system ({fs})"));
    }
    None
}

/// SYS-015 unless the location is safe.
pub fn check_location(path: &Path) -> Result<()> {
    match unsafe_reason(path) {
        Some(r) => Err(StoreError::UnsafeLocation(r)),
        None => Ok(()),
    }
}

/// File-system type of the longest mount point containing `path`, from mountinfo text.
pub fn mount_type_for(mountinfo: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        // id parent major:minor root mount-point options [optional…] - fstype source super
        let mut parts = line.split(" - ");
        let (Some(left), Some(right)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(mp) = left.split_whitespace().nth(4) else {
            continue;
        };
        let Some(fs) = right.split_whitespace().next() else {
            continue;
        };
        let mp = unescape_octal(mp);
        if path.starts_with(&mp) {
            let depth = mp.len();
            if best.as_ref().is_none_or(|(d, _)| depth >= *d) {
                best = Some((depth, fs.to_owned()));
            }
        }
    }
    best.map(|(_, fs)| fs)
}

fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let d = |k: usize| u32::from(b[i + k] - b'0');
            out.push((d(1) * 64 + d(2) * 8 + d(3)) as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
40 22 0:40 / /mnt/share rw,relatime shared:2 - cifs //srv/share rw
41 22 0:41 / /home/u/remote rw - fuse.sshfs u@h:/ rw
42 22 0:42 / /media/usb\\040disk rw - fuseblk /dev/sdb1 rw";

    #[test]
    fn mountinfo_longest_prefix() {
        assert_eq!(
            mount_type_for(INFO, Path::new("/mnt/share/exam")).as_deref(),
            Some("cifs")
        );
        assert_eq!(
            mount_type_for(INFO, Path::new("/home/u/remote/x")).as_deref(),
            Some("fuse.sshfs")
        );
        assert_eq!(
            mount_type_for(INFO, Path::new("/home/u/local")).as_deref(),
            Some("ext4")
        );
        assert_eq!(
            mount_type_for(INFO, Path::new("/media/usb disk/p")).as_deref(),
            Some("fuseblk")
        );
    }

    #[test]
    fn sync_markers_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("a/b/Exam.omrproj");
        std::fs::create_dir_all(&proj).unwrap();
        assert!(check_location(&proj).is_ok());
        std::fs::write(d.path().join("a/.sync_1234.db"), b"").unwrap();
        let e = check_location(&proj).unwrap_err();
        assert_eq!(e.exception().unwrap().code, "SYS-015");
        std::fs::remove_file(d.path().join("a/.sync_1234.db")).unwrap();
        std::fs::create_dir(d.path().join("a/.dropbox")).unwrap();
        assert!(check_location(&proj).is_err());
    }
}
