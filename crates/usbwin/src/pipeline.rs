//! Pipeline orchestration. Knows about every other crate; ties them together.
//!
//! The split: `usbwin-core` defines the types and traits, `usbwin-disk` is the
//! macOS device backend, `usbwin-boot` produces the byte sequences, and this
//! module wires them all together per the mode-dispatch table.

pub mod boot_records;
pub mod confirm;
pub mod diskutil;
pub mod hybrid;
pub mod ntxp_floppy;
pub mod ntxp_iso;
pub mod windows;
pub mod windows_2000;
pub mod windows_ntxp;

use anyhow::{anyhow, bail, Context, Result};
use usbwin_core::{BootMode, Config, ModeRequest, WritePlan};

pub fn run(config: &Config) -> Result<()> {
    let plan = build_plan(config)?;

    if config.unattended.is_some()
        && !matches!(plan.mode, BootMode::WindowsNtXp | BootMode::Windows2000)
    {
        bail!("--unattended is currently supported only with --type=windows-ntxp or --type=windows-2000");
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
            hybrid::run(&plan, &info, config.verify).context("hybrid mode pipeline failed")?
        }
        BootMode::Windows => {
            windows::run(&plan, &info, config).context("Windows 7+ mode pipeline failed")?
        }
        BootMode::WindowsNtXp => windows_ntxp::run(&plan, &info, config)
            .context("Windows NT/XP FiraDisk mode pipeline failed")?,
        BootMode::Windows2000 => windows_2000::run(&plan, &info, config)
            .context("Windows 2000 SVBus mode pipeline failed")?,
        BootMode::IsolinuxLinux => bail!("isolinux Linux mode lands in v0.4"),
        BootMode::UefiOnly => bail!("UEFI-only mode lands in v0.4"),
    };

    print_next_steps(plan.mode);
    Ok(())
}

fn print_next_steps(mode: BootMode) {
    println!();
    println!("================================================================");
    println!("NEXT STEPS");
    println!("================================================================");
    match mode {
        BootMode::Hybrid => {
            println!("1. Move the USB stick to the target PC and boot from it.");
            println!("2. The Linux/BSD installer should come up directly.");
        }
        BootMode::Windows => {
            println!("1. Move the USB stick to the target PC and boot from it.");
            println!("2. Windows Setup will load directly from the USB.");
            println!("3. Follow the on-screen prompts to install.");
        }
        BootMode::WindowsNtXp => {
            println!("Windows XP / Server 2003 install via GRUB4DOS + FiraDisk:");
            println!();
            println!("1. Move the USB stick to the target PC and boot from it.");
            println!("2. At the GRUB4DOS menu, press Enter on entry 1");
            println!("   (\"XP text-mode setup from RAM ISO (FiraDisk)\").");
            println!("3. Text-mode setup runs from the RAM-mapped ISO.");
            println!("   Partition / format the target disk and let it copy files.");
            println!("4. Setup reboots. **Boot from the USB AGAIN** -- do NOT let the");
            println!("   machine boot from the internal disk.");
            println!("5. At the GRUB4DOS menu, choose entry 2");
            println!("   (\"Continue XP GUI-mode setup from internal HDD\").");
            println!("6. GUI-mode setup finishes; the machine reboots into Windows.");
            println!();
            println!("Note: target SATA controller must be in BIOS ATA mode, not AHCI.");
            println!("XP SP3 ships no inbox AHCI driver. AHCI support is a separate");
            println!("1.0 blocker -- see docs/BACKLOG.md.");
        }
        BootMode::Windows2000 => {
            println!("Windows 2000 install via GRUB4DOS + SVBus:");
            println!();
            println!("1. Move the USB stick to the target PC and boot from it.");
            println!("2. At the GRUB4DOS menu, press Enter on entry 1");
            println!("   (\"Win2k text-mode setup from RAM ISO (SVBus)\").");
            println!("3. The moment text-mode setup begins (\"Setup is inspecting your");
            println!("   computer's hardware configuration...\"), **PRESS F6**. The F6");
            println!("   prompt appears briefly at the bottom of the screen.");
            println!("4. Press **S** to specify an additional mass-storage device.");
            println!("5. Select \"SVBus Virtual SCSI Host Adapter x86\" and press Enter.");
            println!("   Press Enter again at the confirmation prompt.");
            println!("6. Continue setup: partition / format the disk, let text-mode finish.");
            println!("7. Setup reboots. **Boot from the USB AGAIN** -- do NOT let the");
            println!("   machine boot from the internal disk.");
            println!("8. At the GRUB4DOS menu, choose entry 2");
            println!("   (\"Continue Win2k GUI-mode setup from internal HDD\").");
            println!("9. GUI-mode setup finishes; the machine reboots into Windows 2000.");
            println!();
            println!("Notes:");
            println!("- BIOS SATA must be in ATA (compatibility) mode, not AHCI.");
            println!("- Win2k support is pre-1.0 and not yet hardware-validated end-to-end.");
        }
        BootMode::IsolinuxLinux | BootMode::UefiOnly => {}
    }
    println!("================================================================");
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
        BootMode::Windows2000 => "USBWIN2K".into(),
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
        ModeRequest::Windows2000 => Ok(BootMode::Windows2000),
        ModeRequest::Hybrid => Ok(BootMode::Hybrid),
        ModeRequest::IsolinuxLinux => Ok(BootMode::IsolinuxLinux),
        ModeRequest::UefiOnly => Ok(BootMode::UefiOnly),
    }
}
