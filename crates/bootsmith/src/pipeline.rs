//! Pipeline orchestration. Knows about every other crate; ties them together.
//!
//! The split: `bootsmith-core` defines the types and traits, `bootsmith-disk` is the
//! macOS device backend, `bootsmith-boot` produces the byte sequences, and this
//! module wires them all together per the mode-dispatch table.

pub mod boot_records;
pub mod confirm;
pub mod diskutil;
pub mod hybrid;
pub mod ntxp_floppy;
pub mod ntxp_iso;
pub mod ntxp_slipstream;
pub mod ntxp_txtsetup;
pub mod windows;
pub mod windows_2000;
pub mod windows_ntxp;

use anyhow::{anyhow, bail, Context, Result};
use bootsmith_core::{BootMode, Config, ModeRequest, WritePlan};

pub fn run(config: &Config) -> Result<()> {
    let plan = build_plan(config)?;

    if config.unattended.is_some()
        && !matches!(plan.mode, BootMode::WindowsNtXp | BootMode::Windows2000)
    {
        bail!("--unattended is currently supported only with --type=windows-ntxp or --type=windows-2000");
    }

    if config.ahci_driver_dir.is_some() && !matches!(plan.mode, BootMode::WindowsNtXp) {
        bail!("--ahci-driver-dir is currently supported only with --type=windows-ntxp");
    }

    if config.dry_run {
        tracing::debug!(?plan, "dry-run: would execute plan");
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
    let info = bootsmith_disk::macos::info_for(&target)
        .with_context(|| format!("device lookup for {target}"))?
        .ok_or_else(|| anyhow!("no such device: {target}"))?;

    let safety = bootsmith_disk::SafetyConfig {
        force: config.force,
    };
    info.check_writable(&safety).context("safety check")?;

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

    print_next_steps(plan.mode, config.ahci_driver_dir.is_some());
    Ok(())
}

fn print_next_steps(mode: BootMode, ahci_driver_staged: bool) {
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
            if ahci_driver_staged {
                println!("AHCI: a vendor F6 driver pack was slipstreamed into I386 of the");
                println!("staged XP.ISO via --ahci-driver-dir. XP text-mode setup treats");
                println!("the driver as inbox: its PCI HwIDs were added to TXTSETUP.SIF's");
                println!("[HardwareIdsDatabase] and PnP auto-binds the matching controller");
                println!("during enumeration. **No F6 prompt is needed** -- the target SATA");
                println!("controller can stay in AHCI mode and the internal disk will");
                println!("appear at the partition screen automatically.");
                println!();
                println!("If the disk still doesn't appear, the pack doesn't cover this");
                println!("chipset -- check the PCI device ID (`lspci -nn` on Linux) and");
                println!("pick a driver pack whose TXTSETUP.OEM lists a matching HwID.");
                println!("See docs/AHCI_DRIVER.md.");
            } else {
                println!("Note: target SATA controller must be in BIOS ATA mode, not AHCI.");
                println!("XP SP3 ships no inbox AHCI driver. To install on an AHCI-only");
                println!("system, re-run with --ahci-driver-dir <vendor F6 folder>;");
                println!("see docs/AHCI_DRIVER.md.");
            }
        }
        BootMode::Windows2000 => {
            println!("Windows 2000 install via GRUB4DOS + SVBus:");
            println!();
            println!("This is a TWO-PHASE process. Text-mode install works via the");
            println!("bootsmith USB; first-boot of the installed system currently needs a");
            println!("one-time manual boot.ini repair. See \"After install\" below.");
            println!();
            println!("Phase 1 -- text-mode install:");
            println!();
            println!("1. Move the USB stick to the target PC and boot from it.");
            println!("2. At the GRUB4DOS menu, press Enter on entry 1");
            println!("   (\"Win2k text-mode setup from RAM ISO (SVBus)\").");
            println!("3. \"Press any key to boot from CD...\" appears for ~5 seconds.");
            println!("   Press any key. (Leave the USB inserted -- removing it");
            println!("   mid-install was tested 2026-05-22 and corrupts the install.)");
            println!("4. The moment text-mode setup begins (\"Setup is inspecting your");
            println!("   computer's hardware configuration...\"), **PRESS F6**. The F6");
            println!("   prompt appears briefly at the bottom of the screen.");
            println!("5. Press **S** to specify an additional mass-storage device.");
            println!("6. Select \"SVBus Virtual SCSI Host Adapter x86\" and press Enter.");
            println!("   Press Enter again at the confirmation prompt.");
            println!("7. Continue setup. At the partition screen: delete any existing");
            println!("   partition, create a new primary partition (2-4 GB is fine),");
            println!("   format NTFS. Let text-mode copy files and reboot.");
            println!();
            println!("After install -- repair boot.ini, then boot Win2k:");
            println!();
            println!("Win2k's text-mode setup writes boot.ini with `rdisk(1)` because");
            println!("during install the USB is BIOS drive 0x80 and the internal HDD");
            println!("is 0x81. This is wrong for both native boot and the GRUB4DOS");
            println!("entry-2 chainload; both need `rdisk(0)` to match how NT loaders");
            println!("resolve the system disk. Until the bootsmith phase-3 fixer lands");
            println!("(see docs/BACKLOG.md), repair is manual:");
            println!();
            println!("8a. (Recommended) Win2k Recovery Console route:");
            println!("    - Boot USB -> entry 1 -> press any key -> F6 -> S -> SVBus");
            println!("      -> Welcome to Setup -> **R** for Repair -> **C** for");
            println!("      Console -> pick the install (typically `1`) -> Enter for");
            println!("      empty admin password.");
            println!("    - At `C:\\WINNT>` prompt:");
            println!("        set AllowAllPaths = TRUE");
            println!("        attrib -h c:\\boot.ini");
            println!("        attrib -r c:\\boot.ini");
            println!("        attrib -s c:\\boot.ini");
            println!("        copy con c:\\boot.ini");
            println!("        [boot loader]");
            println!("        timeout=1");
            println!("        default=multi(0)disk(0)rdisk(0)partition(1)\\WINNT");
            println!("        [operating systems]");
            println!("        multi(0)disk(0)rdisk(0)partition(1)\\WINNT=\"Windows 2000\" /fastdetect");
            println!("    - Press **Ctrl+Z** then **Enter** to finish copy con.");
            println!("    - `exit` to reboot.");
            println!();
            println!("8b. (Fallback) Any Linux live USB:");
            println!("    - Boot Linux, mount the internal HDD's NTFS partition,");
            println!("      edit `boot.ini` to change both `rdisk(1)` -> `rdisk(0)`.");
            println!();
            println!("9. With boot.ini repaired, two ways to boot Win2k:");
            println!("    - Remove the bootsmith USB and boot natively. Win2k's GUI-mode");
            println!("      setup resumes on first boot, completes, lands on desktop.");
            println!("    - OR keep USB inserted, pick entry 2 (\"Boot installed");
            println!("      Windows 2000\"). The GRUB4DOS chain swaps drives so NT");
            println!("      loaders see the internal HDD as 0x80, matching the");
            println!("      repaired boot.ini.");
            println!();
            println!("Other notes:");
            println!("- BIOS SATA must be in ATA (compatibility) mode, not AHCI.");
            println!("- Win2k support is pre-1.0; the manual boot.ini step is the");
            println!("  remaining blocker. See docs/BACKLOG.md \"Win2k boot.ini");
            println!("  auto-repair (phase 3)\" for the planned automation.");
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
        BootMode::WindowsNtXp => "BOOTSMITHXP".into(),
        BootMode::Windows2000 => "BOOTSMITH2K".into(),
        BootMode::Hybrid | BootMode::IsolinuxLinux | BootMode::UefiOnly => "BOOTSMITH".into(),
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
            let detected = bootsmith_iso::classify(&config.iso_path).map_err(|e| {
                anyhow!(
                    "could not auto-classify ISO ({e}); pass --type=windows|windows-ntxp|windows-2000|hybrid|linux|uefi explicitly"
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
