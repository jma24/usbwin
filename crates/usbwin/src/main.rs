//! usbwin CLI entry point. Argument parsing + dispatch to the pipeline.
//!
//! Exit codes (documented in --help):
//!   0  success
//!   1  generic error (anyhow context)
//!   2  CLI usage error (clap default)
//!   3  device refused (boot disk, internal, too large without --force)
//!   4  verification failed
//!   5  ISO classification failed
//!   6  boot blobs not embedded (built without --features embed-boot-asm)

mod pipeline;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use usbwin_core::ModeRequest;

#[derive(Parser, Debug)]
#[command(
    name = "usbwin",
    version,
    about = "Native arm64 macOS bootable-USB writer.",
    long_about = "Writes a bootable USB stick from any ISO (Windows XP through 11, \
                  hybrid Linux/BSD, isolinux Linux, or UEFI-only) on Apple Silicon \
                  without Rosetta. See https://github.com/jmappleby/usbwin."
)]
struct Cli {
    /// Path to the ISO file.
    iso: PathBuf,

    /// Target device path (e.g. /dev/rdisk8). usbwin only operates on the
    /// raw character device for speed and correctness.
    device: PathBuf,

    /// Override auto-detection of the ISO type.
    #[arg(long = "type", value_enum, default_value_t = ModeArg::Auto)]
    mode: ModeArg,

    /// Volume label for the formatted partition.
    #[arg(long)]
    label: Option<String>,

    /// Don't touch the device — emit the byte stream to a file for inspection.
    #[arg(long)]
    dry_run: bool,

    /// Skip the "are you sure" prompt and the internal-disk / size guardrails.
    #[arg(long)]
    force: bool,

    /// Verbose logging to stderr.
    #[arg(long, short)]
    verbose: bool,

    /// Skip the verify-by-default re-read pass. Not recommended.
    #[arg(long)]
    no_verify: bool,

    /// XP mode only: directory containing `WaitBT.sys` and `Wait4UFD.sys`
    /// kernel drivers. usbwin copies them to the USB's I386/ folder and
    /// declares them in TXTSETUP.SIF as BootBusExtenders. Recommended on
    /// hardware that hits `0x7B INACCESSIBLE_BOOT_DEVICE` after text-mode
    /// setup. See docs/V0.3_WINDOWS_XP.md chunk 6 for where to get them.
    #[arg(long, value_name = "DIR")]
    xp_waiters: Option<PathBuf>,

    /// XP mode only: generate a `winnt.sif` answer file on the USB so
    /// the installer doesn't stop at every prompt. Combines with
    /// --xp-product-key / --xp-computer-name / --xp-full-name.
    #[arg(long)]
    xp_unattended: bool,

    /// XP mode only: product key written into the generated `winnt.sif`.
    /// Format: XXXXX-XXXXX-XXXXX-XXXXX-XXXXX. If not provided, setup will
    /// still prompt for it.
    #[arg(long, value_name = "KEY")]
    xp_product_key: Option<String>,

    /// XP mode only: computer name in the generated `winnt.sif`. Defaults
    /// to "*" (XP setup auto-generates one).
    #[arg(long, value_name = "NAME")]
    xp_computer_name: Option<String>,

    /// XP mode only: full name / registered owner in the generated
    /// `winnt.sif`. Defaults to "usbwin user".
    #[arg(long, value_name = "NAME")]
    xp_full_name: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Auto,
    Windows,
    #[value(name = "windows-xp")]
    WindowsXp,
    Linux,
    Hybrid,
    Uefi,
}

impl From<ModeArg> for ModeRequest {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Auto => ModeRequest::Auto,
            ModeArg::Windows => ModeRequest::Windows,
            ModeArg::WindowsXp => ModeRequest::WindowsXp,
            ModeArg::Linux => ModeRequest::IsolinuxLinux,
            ModeArg::Hybrid => ModeRequest::Hybrid,
            ModeArg::Uefi => ModeRequest::UefiOnly,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let filter = if cli.verbose { "usbwin=debug,info" } else { "usbwin=info,warn" };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_target(false)
        .init();

    let config = usbwin_core::Config {
        iso_path: cli.iso,
        device_path: cli.device,
        mode: cli.mode.into(),
        label: cli.label,
        dry_run: cli.dry_run,
        force: cli.force,
        verify: !cli.no_verify,
        verbose: cli.verbose,
        xp_waiters_dir: cli.xp_waiters,
        xp_unattended: cli.xp_unattended,
        xp_product_key: cli.xp_product_key,
        xp_computer_name: cli.xp_computer_name,
        xp_full_name: cli.xp_full_name,
    };

    tracing::info!(
        iso = %config.iso_path.display(),
        device = %config.device_path.display(),
        mode = ?config.mode,
        dry_run = config.dry_run,
        "usbwin: starting pipeline"
    );

    match pipeline::run(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("usbwin: {e:#}");
            ExitCode::from(1)
        }
    }
}
