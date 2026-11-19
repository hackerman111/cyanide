use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn cli_doctor_succeeds_and_reports_embedded_gtk4_sink() {
    let bin = env!("CARGO_BIN_EXE_cyanide");
    let out = Command::new(bin).arg("doctor").output().unwrap();
    assert!(out.status.success(), "doctor failed: {:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("gtk4paintablesink") && stdout.contains("OK"),
        "expected embedded gtk4paintablesink OK, got:\n{stdout}"
    );
}

#[test]
fn daemon_ipc_roundtrip_preserves_mute_and_startup_config() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return;
    }

    let bin = env!("CARGO_BIN_EXE_cyanide");
    let tmp = std::env::temp_dir().join(format!("cyanide-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let runtime_dir = tmp.join("run");
    let config_dir = tmp.join("cfg");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();

    // Forward Wayland socket into isolated XDG_RUNTIME_DIR so GTK4 connects cleanly.
    if let (Some(orig_run), Some(wl)) = (
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::var_os("WAYLAND_DISPLAY"),
    ) {
        let src = std::path::PathBuf::from(orig_run).join(&wl);
        if src.exists() {
            let _ = std::os::unix::fs::symlink(src, runtime_dir.join(&wl));
        }
    }

    let mut child = Command::new(bin)
        .args(["--background", "--test-sources"])
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_CONFIG_HOME", &config_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let sock = runtime_dir.join("cyanide.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sock.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(sock.exists(), "daemon did not bind socket within 5s");

    let run_ctl = |args: &[&str]| -> serde_json::Value {
        let out = Command::new(bin)
            .arg("ctl")
            .args(args)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("XDG_CONFIG_HOME", &config_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "ctl {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };

    // Initial startup mode is Live, virtual mic enabled.
    let st = run_ctl(&["status"]);
    assert_eq!(st["status"], "status");
    assert_eq!(st["video"], "live");
    assert_eq!(st["audio"], "mic");
    assert_eq!(st["virtual_mic_enabled"], true);

    // Mute virtual mic, then toggle -> mode flips to Loop while mute is preserved!
    let _ = run_ctl(&["mic", "off"]);
    let st = run_ctl(&["toggle"]);
    assert_eq!(st["video"], "loop");
    assert_eq!(st["audio"], "file");
    assert_eq!(st["virtual_mic_enabled"], false);

    // Switch audio independently -> Custom, then toggle -> Live
    let st = run_ctl(&["audio", "mix"]);
    assert_eq!(st["video"], "loop");
    assert_eq!(st["audio"], "mix");
    let st = run_ctl(&["toggle"]);
    assert_eq!(st["video"], "live");
    assert_eq!(st["audio"], "mic");
    assert_eq!(st["virtual_mic_enabled"], false);

    // Runtime hotkeys must NOT create/overwrite config.toml
    assert!(!config_dir.join("cyanide/config.toml").exists());

    // Graceful quit unlinks socket and exits 0
    let _ = run_ctl(&["quit"]);
    let status = child.wait().unwrap();
    assert!(status.success(), "daemon exit status: {status}");
    assert!(!sock.exists(), "socket not cleaned up on quit");

    let _ = std::fs::remove_dir_all(&tmp);
}
