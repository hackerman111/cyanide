use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

struct ScannedNode {
    num: u32,
    index: u32,
    parent_dev: Option<PathBuf>,
    dev: VideoDevice,
}

pub fn scan_sysfs(sysfs_root: &Path) -> V4l2Inventory {
    let Ok(entries) = std::fs::read_dir(sysfs_root) else {
        return V4l2Inventory::default();
    };

    let mut nodes: Vec<ScannedNode> = Vec::new();
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
        let index = std::fs::read_to_string(dir.join("index"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let parent_dev = std::fs::canonicalize(dir.join("device")).ok();
        let is_loopback = detect_loopback(&dir, &name);

        nodes.push(ScannedNode {
            num,
            index,
            parent_dev,
            dev: VideoDevice {
                path: format!("/dev/{fname}"),
                name,
                is_loopback,
            },
        });
    }

    // Sort by (index, videoN) so UVC capture node (index 0) precedes metadata nodes (index >= 1).
    nodes.sort_by_key(|n| (n.index, n.num));

    let mut seen_real_groups: HashMap<(PathBuf, String), ()> = HashMap::new();
    let mut kept_real: Vec<(u32, VideoDevice)> = Vec::new();
    let mut kept_loopback: Vec<(u32, VideoDevice)> = Vec::new();

    for node in nodes {
        if node.dev.is_loopback {
            kept_loopback.push((node.num, node.dev));
            continue;
        }
        if let Some(parent) = node.parent_dev {
            let key = (parent, node.dev.name.clone());
            if seen_real_groups.insert(key, ()).is_some() {
                continue;
            }
        }
        kept_real.push((node.num, node.dev));
    }

    kept_real.sort_by_key(|(n, _)| *n);
    kept_loopback.sort_by_key(|(n, _)| *n);

    V4l2Inventory {
        real_cameras: kept_real.into_iter().map(|(_, d)| d).collect(),
        loopback_cameras: kept_loopback.into_iter().map(|(_, d)| d).collect(),
    }
}

fn detect_loopback(video_dir: &Path, name: &str) -> bool {
    if let Ok(uevent) = std::fs::read_to_string(video_dir.join("device/uevent"))
        && uevent.to_ascii_lowercase().contains("v4l2loopback")
    {
        return true;
    }
    if let Ok(canonical) = std::fs::canonicalize(video_dir)
        && canonical
            .to_string_lossy()
            .contains("/devices/virtual/video4linux")
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

    #[test]
    fn scan_sysfs_deduplicates_uvc_metadata_nodes_by_index() {
        let root = std::env::temp_dir().join("cyanide-sysfs-uvc-dedup");
        let _ = std::fs::remove_dir_all(&root);
        let shared_usb = root.join("usb_dev");
        std::fs::create_dir_all(&shared_usb).unwrap();
        std::fs::write(shared_usb.join("uevent"), "DRIVER=uvcvideo\n").unwrap();

        for (node, idx) in [("video0", "0"), ("video1", "1")] {
            std::fs::create_dir_all(root.join(node)).unwrap();
            std::fs::write(root.join(node).join("name"), "HD Camera: HD Camera\n").unwrap();
            std::fs::write(root.join(node).join("index"), format!("{idx}\n")).unwrap();
            std::os::unix::fs::symlink(&shared_usb, root.join(node).join("device")).unwrap();
        }

        let inv = scan_sysfs(&root);
        assert_eq!(inv.real_cameras.len(), 1);
        assert_eq!(inv.real_cameras[0].path, "/dev/video0");
        let _ = std::fs::remove_dir_all(&root);
    }
}
