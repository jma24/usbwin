//! Thin wrappers around macOS control-plane tools: `diskutil`,
//! `newfs_msdos`, and `hdiutil`. These are the only shell-outs in bootsmith;
//! the data plane (raw I/O, byte assembly, verify) is all native.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn unmount_disk(bsd_path: &str) -> Result<()> {
    run_diskutil(&["unmountDisk", bsd_path])
        .with_context(|| format!("diskutil unmountDisk {bsd_path}"))
}

/// Forceful variant of [`unmount_disk`]. Use this just before destructive
/// per-partition operations (e.g. `newfs_msdos`) — macOS disk arbitration
/// races to auto-mount any recognized filesystem after a partition-table
/// write, and the non-force unmount can return before that auto-mount
/// settles. `force` waits for and overrides the in-flight mount.
pub fn unmount_disk_force(bsd_path: &str) -> Result<()> {
    run_diskutil(&["unmountDisk", "force", bsd_path])
        .with_context(|| format!("diskutil unmountDisk force {bsd_path}"))
}

#[allow(dead_code)]
pub fn mount_disk(bsd_path: &str) -> Result<()> {
    run_diskutil(&["mountDisk", bsd_path])
        .with_context(|| format!("diskutil mountDisk {bsd_path}"))
}

pub fn eject(bsd_path: &str) -> Result<()> {
    run_diskutil(&["eject", bsd_path])
        .with_context(|| format!("diskutil eject {bsd_path}"))
}

/// Format a partition (e.g. `/dev/rdisk6s1`) as FAT32 with the given
/// volume label. Uses macOS's built-in `newfs_msdos`.
///
/// IMPORTANT: takes the **partition** device (`disk6s1`), not the whole
/// disk (`disk6`). The MBR must already have been written so the partition
/// exists.
pub fn newfs_msdos_fat32(partition_path: &str, label: &str) -> Result<()> {
    // Default newfs_msdos cluster sizing (32 KiB on >32 GiB partitions).
    // An older `-c 8` (4 KiB) forcing lived here, justified by XP setupldr's
    // FAT walker choking on big clusters reading `txtsetup.sif`. That was
    // pre-FiraDisk debt: in the current path setupldr + txtsetup.sif are
    // read from the RAM-mapped XP.ISO (ISO9660), not this FAT32 partition —
    // only GRUB4DOS reads this FS, and its FAT driver handles 32 KiB fine.
    // Removed after a hardware XP text-mode install copied files cleanly
    // with default clustering (E6410, 2026-05-26). 32 KiB clusters also
    // suit our payload (a few large files: XP.ISO, FIRADISK.IMA, grldr).
    let args = ["-F", "32", "-v", label, partition_path];
    tracing::debug!(cmd = "newfs_msdos", ?args, "spawn");
    let output = Command::new("newfs_msdos")
        .args(args)
        .output()
        .with_context(|| format!("spawning newfs_msdos for {partition_path}"))?;
    log_output("newfs_msdos", &output);
    if !output.status.success() {
        bail!(
            "newfs_msdos failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Attach an ISO file as a read-only loopback. Returns the temporary mount
/// point that hdiutil chose (e.g. `/Volumes/disk_image_NN`).
///
/// We use `-nobrowse` so Finder doesn't pop up a window for the mounted
/// ISO. We don't pin the mount point; macOS picks a fresh one each time,
/// which is fine since we read it back from hdiutil's output.
pub fn hdiutil_attach_iso(iso: &Path) -> Result<PathBuf> {
    tracing::debug!(cmd = "hdiutil", iso = %iso.display(), "attach -nobrowse -readonly");
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(iso)
        .output()
        .with_context(|| format!("hdiutil attach {}", iso.display()))?;
    log_output("hdiutil attach", &output);
    if !output.status.success() {
        bail!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mount_point = stdout
        .lines()
        .find_map(|l| {
            l.split_whitespace()
                .find(|p| p.starts_with("/Volumes/"))
                .map(PathBuf::from)
        })
        .ok_or_else(|| anyhow!("hdiutil attach gave no /Volumes/ mount: {stdout}"))?;
    tracing::debug!(mount = %mount_point.display(), "hdiutil attach OK");
    Ok(mount_point)
}

/// Detach an ISO mount point that came from `hdiutil_attach_iso`.
pub fn hdiutil_detach(mount_point: &Path) -> Result<()> {
    tracing::debug!(cmd = "hdiutil", mount = %mount_point.display(), "detach");
    let output = Command::new("hdiutil")
        .arg("detach")
        .arg(mount_point)
        .output()
        .with_context(|| format!("hdiutil detach {}", mount_point.display()))?;
    log_output("hdiutil detach", &output);
    if !output.status.success() {
        bail!(
            "hdiutil detach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Resolve the ms-sys binary path. Checks (in order):
/// 1. `BOOTSMITH_MS_SYS` env var
/// 2. `/usr/local/bin/ms-sys`
/// 3. `/opt/homebrew/bin/ms-sys`
/// 4. `ms-sys` on PATH
///
/// Returns an error with install instructions if none of these resolve.
pub fn find_ms_sys() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("BOOTSMITH_MS_SYS") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }
    for candidate in &["/usr/local/bin/ms-sys", "/opt/homebrew/bin/ms-sys"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Ok(p);
        }
    }
    let out = Command::new("/usr/bin/env")
        .args(["which", "ms-sys"])
        .output()
        .ok();
    if let Some(out) = out {
        if out.status.success() {
            let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !line.is_empty() {
                return Ok(PathBuf::from(line));
            }
        }
    }
    bail!(
        "ms-sys binary not found. Windows-mode v0.2 needs ms-sys for the boot \
         records (see FIELD_FINDINGS_2026_05_18.md). To install:\n  \
         git clone https://gitlab.com/cmaiolino/ms-sys.git /tmp/ms-sys && \
         cd /tmp/ms-sys && make\n\
         Then either: sudo cp /tmp/ms-sys/bin/ms-sys /usr/local/bin/  OR  \
         export BOOTSMITH_MS_SYS=/tmp/ms-sys/bin/ms-sys"
    )
}

/// Write the Windows 7 MBR boot code via `ms-sys --mbr7 /dev/diskN`.
/// `--mbr7` writes only the 440-byte boot code (preserving the partition
/// table) — that's a sub-sector write, so it must go to the **buffered**
/// device. On `/dev/rdiskN` (raw character device) the kernel rejects
/// the partial-sector write and ms-sys reports "Failed writing ..." with
/// no further detail. Validated empirically 2026-05-19.
pub fn ms_sys_mbr7(ms_sys: &Path, buffered_disk_path: &str) -> Result<()> {
    tracing::debug!(cmd = %ms_sys.display(), target_path = buffered_disk_path, "ms-sys --mbr7");
    let output = Command::new(ms_sys)
        .args(["-f", "--mbr7"])
        .arg(buffered_disk_path)
        .output()
        .with_context(|| format!("invoking ms-sys --mbr7 {buffered_disk_path}"))?;
    log_output("ms-sys --mbr7", &output);
    if !output.status.success() {
        bail!("ms-sys --mbr7 failed: {}", format_ms_sys_failure(&output));
    }
    Ok(())
}

fn format_ms_sys_failure(out: &std::process::Output) -> String {
    // ms-sys often prints its error message to stdout, not stderr. Surface
    // both, plus the exit status.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let status = match out.status.code() {
        Some(c) => format!("exit {c}"),
        None => "killed by signal".to_string(),
    };
    let mut parts = vec![status];
    let stderr = stderr.trim();
    let stdout = stdout.trim();
    if !stderr.is_empty() {
        parts.push(format!("stderr: {stderr}"));
    }
    if !stdout.is_empty() {
        parts.push(format!("stdout: {stdout}"));
    }
    parts.join("; ")
}

/// Write the FAT32 PE (Win 7/8/10 BOOTMGR-loading) PBR via
/// `ms-sys --fat32pe /dev/diskNs1`. Per FIELD_FINDINGS §2: ms-sys does
/// sub-sector writes that silently fail on /dev/rdiskN — use buffered
/// `/dev/diskN` for the partition path.
pub fn ms_sys_fat32pe(ms_sys: &Path, partition_buffered_path: &str) -> Result<()> {
    tracing::debug!(cmd = %ms_sys.display(), target_path = partition_buffered_path, "ms-sys --fat32pe");
    let output = Command::new(ms_sys)
        .args(["-f", "--fat32pe"])
        .arg(partition_buffered_path)
        .output()
        .with_context(|| format!("invoking ms-sys --fat32pe {partition_buffered_path}"))?;
    log_output("ms-sys --fat32pe", &output);
    if !output.status.success() {
        bail!("ms-sys --fat32pe failed: {}", format_ms_sys_failure(&output));
    }
    Ok(())
}

fn run_diskutil(args: &[&str]) -> Result<()> {
    tracing::debug!(cmd = "diskutil", args = %args.join(" "), "spawn");
    let output = Command::new("diskutil")
        .args(args)
        .output()
        .with_context(|| format!("spawning `diskutil {}`", args.join(" ")))?;
    log_output(&format!("diskutil {}", args.join(" ")), &output);
    if !output.status.success() {
        bail!(
            "diskutil {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Emit captured stdout/stderr at debug. Surfaces useful detail under
/// `--verbose` without polluting normal output. Logs stderr at warn when
/// non-empty even on success (some tools warn but exit 0).
fn log_output(label: &str, out: &Output) {
    let status = out
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    if !stdout.is_empty() {
        tracing::debug!(target: "bootsmith", cmd = label, status, stdout, "exit");
    } else {
        tracing::debug!(target: "bootsmith", cmd = label, status, "exit");
    }
    if !stderr.is_empty() && out.status.success() {
        tracing::warn!(target: "bootsmith", cmd = label, stderr, "stderr on success");
    }
}
