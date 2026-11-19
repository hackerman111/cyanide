use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::config::is_forbidden_mic;
use crate::devices::{VIRTUAL_MIC_NAME, VIRTUAL_SINK_NAME};

#[derive(Debug, Error)]
pub enum PipeWireError {
    #[error("pactl execution failed: {0}")]
    Exec(#[from] std::io::Error),
    #[error("pactl command failed: {0}")]
    Command(String),
    #[error("invalid pactl module id: {0}")]
    InvalidModuleId(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub name: String,
    pub label: String,
}

#[derive(Debug)]
pub struct VirtualMicGuard {
    null_sink_module_id: u32,
    remap_module_id: u32,
    state_file: PathBuf,
}

impl VirtualMicGuard {
    pub fn create(state_file: &Path) -> Result<Self, PipeWireError> {
        cleanup_previous_modules(state_file);

        let sink_arg = format!("sink_name={VIRTUAL_SINK_NAME}");
        let sink_desc = format!("sink_properties=device.description={VIRTUAL_SINK_NAME}");
        let sink_out = run_pactl(&[
            "load-module",
            "module-null-sink",
            &sink_arg,
            &sink_desc,
            "rate=48000",
            "channels=2",
        ])?;
        let null_sink_module_id = parse_module_id(&sink_out)?;

        let master_arg = format!("master={VIRTUAL_SINK_NAME}.monitor");
        let source_arg = format!("source_name={VIRTUAL_MIC_NAME}");
        let source_desc = format!("source_properties=device.description={VIRTUAL_MIC_NAME}");
        let remap_res = run_pactl(&[
            "load-module",
            "module-remap-source",
            &master_arg,
            &source_arg,
            &source_desc,
        ])
        .and_then(|out| parse_module_id(&out));

        let remap_module_id = match remap_res {
            Ok(id) => id,
            Err(err) => {
                let _ = unload_module(null_sink_module_id);
                return Err(err);
            }
        };

        if let Some(parent) = state_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            state_file,
            format!("{remap_module_id}\n{null_sink_module_id}\n"),
        );

        Ok(Self {
            null_sink_module_id,
            remap_module_id,
            state_file: state_file.to_path_buf(),
        })
    }

    pub fn module_ids(&self) -> (u32, u32) {
        (self.null_sink_module_id, self.remap_module_id)
    }
}

impl Drop for VirtualMicGuard {
    fn drop(&mut self) {
        let _ = unload_module(self.remap_module_id);
        let _ = unload_module(self.null_sink_module_id);
        let _ = std::fs::remove_file(&self.state_file);
    }
}

pub fn parse_pactl_sources(stdout: &str) -> Vec<AudioDevice> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let _idx = cols.next()?;
            let name = cols.next()?.trim();
            if name.is_empty() || is_forbidden_mic(name) {
                return None;
            }
            Some(AudioDevice {
                name: name.to_string(),
                label: name.to_string(),
            })
        })
        .collect()
}

pub fn parse_module_id(stdout: &str) -> Result<u32, PipeWireError> {
    let trimmed = stdout.trim();
    trimmed
        .parse::<u32>()
        .map_err(|_| PipeWireError::InvalidModuleId(trimmed.to_string()))
}

pub fn discover_real_mics() -> Result<Vec<AudioDevice>, PipeWireError> {
    let out = run_pactl(&["list", "short", "sources"])?;
    Ok(parse_pactl_sources(&out))
}

pub fn pactl_available() -> bool {
    run_pactl(&["info"]).is_ok()
}

fn cleanup_previous_modules(state_file: &Path) {
    if let Ok(saved) = std::fs::read_to_string(state_file) {
        for line in saved.lines() {
            if let Ok(id) = parse_module_id(line) {
                let _ = unload_module(id);
            }
        }
        let _ = std::fs::remove_file(state_file);
    }

    if let Ok(modules) = run_pactl(&["list", "short", "modules"]) {
        // Unload remap-source before null-sink so no dangling monitor reference remains.
        let mut to_unload: Vec<u32> = Vec::new();
        for line in modules.lines() {
            if line.contains(&format!("source_name={VIRTUAL_MIC_NAME}"))
                && let Some(id) = line.split('\t').next().and_then(|s| s.trim().parse().ok())
            {
                to_unload.push(id);
            }
        }
        for line in modules.lines() {
            if line.contains(&format!("sink_name={VIRTUAL_SINK_NAME}"))
                && let Some(id) = line.split('\t').next().and_then(|s| s.trim().parse().ok())
            {
                to_unload.push(id);
            }
        }
        for id in to_unload {
            let _ = unload_module(id);
        }
    }
}

fn unload_module(id: u32) -> Result<(), PipeWireError> {
    let id_str = id.to_string();
    run_pactl(&["unload-module", &id_str]).map(|_| ())
}

fn run_pactl(args: &[&str]) -> Result<String, PipeWireError> {
    let out = Command::new("pactl").args(args).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(PipeWireError::Command(stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pactl_sources_excludes_virtual_and_monitors() {
        let sample = "\
42\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tRUNNING
43\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
55\tVirtualCamAudio.monitor\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
56\tVirtualCamMic\tPipeWire\tfloat32le 2ch 48000Hz\tIDLE
";
        let mics = parse_pactl_sources(sample);
        assert_eq!(mics.len(), 1);
        assert_eq!(mics[0].name, "alsa_input.pci-0000_00_1f.3.analog-stereo");
    }

    #[test]
    fn parse_module_id_reads_trimmed_integer() {
        assert_eq!(parse_module_id("536870912\n").unwrap(), 536870912);
        assert!(parse_module_id("not-a-number").is_err());
    }
}
