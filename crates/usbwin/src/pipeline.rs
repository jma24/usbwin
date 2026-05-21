//! Pipeline orchestration. Knows about every other crate; ties them together.
//!
//! The split: `usbwin-core` defines the types and traits, `usbwin-disk` is the
//! macOS device backend, `usbwin-boot` produces the byte sequences, and this
//! module wires them all together per the mode-dispatch table.

pub mod boot_records;
pub mod confirm;
pub mod diskutil;
pub mod fat32;
pub mod hybrid;
pub mod ntxp_floppy;
pub mod ntxp_iso;
pub mod windows;
pub mod windows_ntxp;

use anyhow::{anyhow, bail, Context, Result};
use usbwin_core::{BootMode, Config, ModeRequest, WritePlan};

pub fn run(config: &Config) -> Result<()> {
    let plan = build_plan(config)?;

    if config.unattended.is_some() && plan.mode != BootMode::WindowsNtXp {
        bail!("--unattended is currently supported only with --type=windows-ntxp");
    }

    if config.dry_run {
        tracing::info!(?plan, "dry-run: would execute plan");
        println!(
            "dry-run: mode={}, iso={} ({} bytes), label={}, target={}",
            plan.mode.as_str(),
            plan.iso_path.display(),
            plan.iso_bytes,
            plan.label,
            config.device_path.display(),
        );
        return Ok(());
    }

    let target = config.device_path.to_string_lossy().to_string();
    let info = usbwin_disk::macos::info_for(&target)
        .map_err(|e| anyhow!("device lookup for {target}: {e}"))?
        .ok_or_else(|| anyhow!("no such device: {target}"))?;

    let safety = usbwin_disk::SafetyConfig {
        force: config.force,
    };
    info.check_writable(&safety)
        .map_err(|e| anyhow!("safety check failed: {e}"))?;

    if plan.iso_bytes > info.size_bytes {
        bail!(
            "ISO is {} bytes; device {} is only {} bytes",
            plan.iso_bytes,
            info.path,
            info.size_bytes
        );
    }

    if !config.force {
        confirm::prompt(&plan, &info).context("confirmation prompt")?;
    }

    match plan.mode {
        BootMode::Hybrid => {
            hybrid::run(&plan, &info, config.verify).context("hybrid mode pipeline failed")
        }
        BootMode::Windows => {
            windows::run(&plan, &info, config).context("Windows 7+ mode pipeline failed")
        }
        BootMode::WindowsNtXp => windows_ntxp::run(&plan, &info, config)
            .context("Windows NT/XP FiraDisk mode pipeline failed"),
        BootMode::IsolinuxLinux => bail!("isolinux Linux mode lands in v0.4"),
        BootMode::UefiOnly => bail!("UEFI-only mode lands in v0.4"),
    }
}

fn build_plan(config: &Config) -> Result<WritePlan> {
    let iso_metadata = std::fs::metadata(&config.iso_path)
        .with_context(|| format!("opening ISO {}", config.iso_path.display()))?;
    if !iso_metadata.is_file() {
        bail!("{} is not a regular file", config.iso_path.display());
    }
    let iso_bytes = iso_metadata.len();
    let mode = resolve_mode(config)?;
    let label = config.label.clone().unwrap_or_else(|| match mode {
        BootMode::Windows => "WIN7".into(),
        BootMode::WindowsNtXp => "USBWINXP".into(),
        BootMode::Hybrid | BootMode::IsolinuxLinux | BootMode::UefiOnly => "USBWIN".into(),
    });
    Ok(WritePlan {
        iso_path: config.iso_path.clone(),
        iso_bytes,
        mode,
        label,
    })
}

fn resolve_mode(config: &Config) -> Result<BootMode> {
    match config.mode {
        ModeRequest::Auto => {
            let detected = usbwin_iso::classify(&config.iso_path).map_err(|e| {
                anyhow!(
                    "could not auto-classify ISO ({e}); pass --type=windows|windows-ntxp|hybrid|linux|uefi explicitly"
                )
            })?;
            Ok(detected)
        }
        ModeRequest::Windows => Ok(BootMode::Windows),
        ModeRequest::WindowsNtXp => Ok(BootMode::WindowsNtXp),
        ModeRequest::Hybrid => Ok(BootMode::Hybrid),
        ModeRequest::IsolinuxLinux => Ok(BootMode::IsolinuxLinux),
        ModeRequest::UefiOnly => Ok(BootMode::UefiOnly),
    }
}
