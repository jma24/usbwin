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
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Auto,
    Windows,
    Linux,
    Hybrid,
    Uefi,
}

impl From<ModeArg> for ModeRequest {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Auto => ModeRequest::Auto,
            ModeArg::Windows => ModeRequest::Windows,
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
    };

    tracing::info!(
        iso = %config.iso_path.display(),
        device = %config.device_path.display(),
        mode = ?config.mode,
        dry_run = config.dry_run,
        "usbwin: configuration parsed (pipeline not yet implemented)"
    );

    // TODO: hand off to usbwin_core::run(&config). For the scaffold commit
    // we just print what we'd do.
    println!(
        "usbwin (scaffold): would process {} -> {} (mode {:?}, dry_run={}, verify={})",
        config.iso_path.display(),
        config.device_path.display(),
        config.mode,
        config.dry_run,
        config.verify
    );

    ExitCode::SUCCESS
}
