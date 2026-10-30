//! The filesystem a root lies on (`design/scanning.md` §9), for the ones where inotify does not
//! report every change, so the log can say that changes there wait for the schedule.

use std::fs;

/// Filesystems where inotify does not report every change: 9p, which is how Docker Desktop on
/// Windows shares a Windows folder, reports no change made from Windows and none made below the
/// folder it shares, and NFS and SMB report none made from another machine.
const UNREPORTED: [&str; 5] = ["9p", "nfs", "nfs4", "cifs", "smb3"];

/// The type of the filesystem on device `dev`, as `/proc/self/mountinfo` names it, when inotify
/// does not report every change on it. `None` for any other, and where the mounts cannot be
/// read, as on macOS, which has no `/proc`.
pub(super) fn unreported(dev: u64) -> Option<String> {
    let mounts = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let kind = mount_type(&mounts, dev)?;
    UNREPORTED.contains(&kind).then(|| kind.to_owned())
}

/// The type of the first mount of device `dev` in `mountinfo`. Every mount of one device is one
/// filesystem, so any of them names it. Each line is the mount's ID, its parent's, its device
/// as `major:minor`, then fields up to a lone `-`, which the type follows.
fn mount_type(mountinfo: &str, dev: u64) -> Option<&str> {
    let device = device(dev);
    mountinfo.lines().find_map(|line| {
        let mut fields = line.split(' ');
        if fields.nth(2)? != device {
            return None;
        }
        fields.skip_while(|field| *field != "-").nth(1)
    })
}

/// `dev` as `major:minor`, unpacked as Linux's C library packs them.
fn device(dev: u64) -> String {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xffff_f000);
    let minor = (dev & 0xff) | ((dev >> 12) & 0xffff_ff00);
    format!("{major}:{minor}")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    /// A Docker Desktop container on Windows: its music is a 9p share of a Windows folder, and
    /// its data is a volume on ext4 that two mounts share.
    const DOCKER_DESKTOP: &str = "\
508 379 0:87 / / rw,relatime - overlay overlay rw,lowerdir=/var/lib/snapshots/156/fs
517 508 0:69 /Users/me/Music /music rw,noatime - 9p C:\\134 rw,aname=drvfs;path=C:\\;uid=0
518 508 8:48 /data/docker/volumes/data/_data /data rw,relatime master:29 - ext4 /dev/sdd rw
519 508 8:48 /data/docker/volumes/target/_data /target rw,relatime master:29 - ext4 /dev/sdd rw";

    #[test]
    fn a_device_is_unpacked_as_the_c_library_packs_it() {
        assert_eq!(device(69), "0:69");
        assert_eq!(device(2096), "8:48");
        // Minors past 255 and majors past 4095 keep their high bits further up.
        assert_eq!(device((300 & 0xff) | ((300 & !0xff) << 12)), "0:300");
        assert_eq!(device(((5000 & 0xfff) << 8) | ((5000 & !0xfff) << 32)), "5000:0");
    }

    #[test]
    fn a_device_finds_its_mount_type() {
        assert_eq!(mount_type(DOCKER_DESKTOP, 69), Some("9p"));
        assert_eq!(mount_type(DOCKER_DESKTOP, 2096), Some("ext4"));
        assert_eq!(mount_type(DOCKER_DESKTOP, 87), Some("overlay"));
        assert_eq!(mount_type(DOCKER_DESKTOP, 70), None);
        assert_eq!(mount_type("", 69), None);
    }

    /// The optional fields before the `-` vary in number, and the type is still the one after it.
    #[test]
    fn optional_fields_do_not_move_the_type() {
        let line = "36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 shared:7 - nfs4 host:/music rw";
        assert_eq!(mount_type(line, 98 << 8), Some("nfs4"));
    }

    /// The folders tests run in report every change, so a root there warns of nothing.
    #[test]
    fn a_local_folder_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let dev = fs::metadata(tmp.path()).unwrap().dev();
        assert_eq!(unreported(dev), None);
    }
}
