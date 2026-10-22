//! Filesystem usage (statvfs) for the configured volumes.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::config::VolumeConfig;
use crate::model::Volume;

/// Mount points listed in /proc/self/mountinfo.
pub fn parse_mountinfo(text: &str) -> HashSet<PathBuf> {
    text.lines()
        .filter_map(|line| {
            // id parent major:minor root mount-point options ...
            let mount_point = line.split(' ').nth(4)?;
            Some(PathBuf::from(unescape_mount(mount_point)))
        })
        .collect()
}

/// mountinfo escapes space, tab, newline and backslash as `\NNN` octal.
fn unescape_mount(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && let Some(digits) = bytes.get(index + 1..index + 4)
            && digits.iter().all(|d| (b'0'..=b'7').contains(d))
        {
            let value = digits
                .iter()
                .fold(0u32, |acc, digit| acc * 8 + u32::from(digit - b'0'));
            if let Ok(byte) = u8::try_from(value) {
                out.push(byte);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn sample(configs: &[VolumeConfig], mountinfo: &str) -> Vec<Volume> {
    let mounts = parse_mountinfo(mountinfo);
    configs
        .iter()
        .map(|config| {
            let resolved = fs::canonicalize(&config.path).unwrap_or_else(|_| config.path.clone());
            let mut volume = Volume {
                label: config.label.clone(),
                mounted: mounts.contains(&resolved),
                ..Volume::default()
            };
            if volume.mounted
                && let Ok(stat) = rustix::fs::statvfs(&resolved)
            {
                let unit = if stat.f_frsize > 0 {
                    stat.f_frsize
                } else {
                    stat.f_bsize
                };
                volume.total = stat.f_blocks.saturating_mul(unit);
                volume.used = stat
                    .f_blocks
                    .saturating_sub(stat.f_bfree)
                    .saturating_mul(unit);
            }
            volume
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
39 1 259:4 / / rw,noatime shared:1 - ext4 /dev/nvme0n1p2 rw
56 39 259:5 / /mnt/volumes/services-02 rw,noatime shared:115 - ext4 /dev/nvme0n1p3 rw
60 39 259:1 / /mnt/volumes/services-01 rw,noatime shared:119 - ext4 /dev/nvme1n1p1 rw
71 39 0:50 / /mnt/with\\040space rw - tmpfs tmpfs rw
";

    #[test]
    fn mount_points_are_extracted_and_unescaped() {
        let mounts = parse_mountinfo(MOUNTINFO);
        assert!(mounts.contains(&PathBuf::from("/")));
        assert!(mounts.contains(&PathBuf::from("/mnt/volumes/services-01")));
        assert!(mounts.contains(&PathBuf::from("/mnt/with space")));
        assert!(!mounts.contains(&PathBuf::from("/mnt/volumes")));
        assert_eq!(mounts.len(), 4);
    }

    #[test]
    fn unescape_handles_backslash_and_plain_text() {
        assert_eq!(unescape_mount("/a\\134b"), "/a\\b");
        assert_eq!(unescape_mount("/plain"), "/plain");
        assert_eq!(unescape_mount("/bad\\9"), "/bad\\9");
        assert_eq!(unescape_mount("/tab\\011x"), "/tab\tx");
    }
}
