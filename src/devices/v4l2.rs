use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDevice {
    pub path: String,
    pub name: String,
    pub is_loopback: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct V4l2Inventory {
    pub real_cameras: Vec<VideoDevice>,
    pub loopback_cameras: Vec<VideoDevice>,
}

pub fn discover_cameras() -> V4l2Inventory {
    scan_sysfs(Path::new("/sys/class/video4linux"))
}

pub fn scan_sysfs(sysfs_root: &Path) -> V4l2Inventory {
    let Ok(entries) = std::fs::read_dir(sysfs_root) else {
        return V4l2Inventory::default();
    };

    let mut numbered: Vec<(u32, VideoDevice)> = Vec::new();
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(fname) = fname.to_str() else {
            continue;
        };
        let Some(num_str) = fname.strip_prefix("video") else {
            continue;
        };
        let Ok(num) = num_str.parse::<u32>() else {
            continue;
        };

        let dir = entry.path();
        let name = std::fs::read_to_string(dir.join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| fname.to_string());
        let is_loopback = detect_loopback(&dir, &name);

        numbered.push((
            num,
            VideoDevice {
                path: format!("/dev/{fname}"),
                name,
                is_loopback,
            },
        ));
    }

    numbered.sort_by_key(|(n, _)| *n);

    let mut inv = V4l2Inventory::default();
    for (_, dev) in numbered {
        if dev.is_loopback {
            inv.loopback_cameras.push(dev);
        } else {
            inv.real_cameras.push(dev);
        }
    }
    inv
}

fn detect_loopback(video_dir: &Path, name: &str) -> bool {
    if let Ok(uevent) = std::fs::read_to_string(video_dir.join("device/uevent"))
        && uevent.to_ascii_lowercase().contains("v4l2loopback")
    {
        return true;
    }
    if let Ok(canonical) = std::fs::canonicalize(video_dir)
        && canonical.to_string_lossy().contains("/devices/virtual/video4linux")
    {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.contains("v4l2loopback") || lower.contains("virtualcam")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_sysfs_separates_real_and_loopback_cameras() {
        let root = std::env::temp_dir().join("cyanide-sysfs-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("video0/device")).unwrap();
        std::fs::write(root.join("video0/name"), "HD Camera: HD Camera\n").unwrap();
        std::fs::write(root.join("video0/device/uevent"), "DRIVER=uvcvideo\n").unwrap();
        std::fs::create_dir_all(root.join("video10/device")).unwrap();
        std::fs::write(root.join("video10/name"), "VirtualCam\n").unwrap();
        std::fs::write(root.join("video10/device/uevent"), "DRIVER=v4l2loopback\n").unwrap();

        let inv = scan_sysfs(&root);
        assert_eq!(inv.real_cameras.len(), 1);
        assert_eq!(inv.real_cameras[0].path, "/dev/video0");
        assert_eq!(inv.loopback_cameras.len(), 1);
        assert_eq!(inv.loopback_cameras[0].path, "/dev/video10");
        let _ = std::fs::remove_dir_all(&root);
    }
}
