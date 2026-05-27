//! bootsmith CLI entry point. Argument parsing + dispatch to the pipeline.
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
use bootsmith_core::{BootRecordImpl, ModeRequest, UnattendedConfig};

#[derive(Parser, Debug)]
#[command(
    name = "bootsmith",
    version,
    about = "Native arm64 macOS bootable-USB writer.",
    long_about = "Writes Windows 2000, XP, and Windows 7 install USB sticks \
                  on Apple Silicon without Rosetta. Hybrid raw ISO writes are \
                  available as a utility path, but generic boot-loader support \
                  is not the v1 focus. See https://github.com/jma24/bootsmith."
)]
struct Cli {
    /// Path to the ISO file.
    iso: PathBuf,

    /// Target device path (e.g. /dev/rdisk8). bootsmith only operates on the
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

    /// Backend used to write the MBR boot code and partition boot
    /// record. `bootrec` (default) links the native Rust library
    /// in-process; `ms-sys` shells out to the upstream binary at
    /// `$BOOTSMITH_MS_SYS` / PATH (legacy v0.2 path).
    #[arg(long = "boot-record", value_enum, default_value_t = BootRecordArg::Bootrec)]
    boot_record: BootRecordArg,

    /// Inject I386/WINNT.SIF into a derived NT5 ISO for unattended XP/2000 setup.
    #[arg(long)]
    unattended: bool,

    /// Product key to write into WINNT.SIF. Implies --unattended.
    #[arg(long = "product-key")]
    product_key: Option<String>,

    /// FullName value for WINNT.SIF. Used with --unattended.
    #[arg(long = "full-name", default_value = "bootsmith")]
    full_name: String,

    /// OrgName value for WINNT.SIF. Used with --unattended.
    #[arg(long = "organization", default_value = "")]
    organization: String,

    /// ComputerName value for WINNT.SIF. Use * to let setup generate one.
    #[arg(long = "computer-name", default_value = "*")]
    computer_name: String,

    /// Administrator password for WINNT.SIF. Omit to make setup prompt.
    #[arg(long = "admin-password")]
    admin_password: Option<String>,

    /// Windows setup timezone index for WINNT.SIF.
    #[arg(long = "timezone")]
    timezone: Option<u16>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BootRecordArg {
    Bootrec,
    #[value(name = "ms-sys")]
    MsSys,
}

impl From<BootRecordArg> for BootRecordImpl {
    fn from(b: BootRecordArg) -> Self {
        match b {
            BootRecordArg::Bootrec => BootRecordImpl::Bootrec,
            BootRecordArg::MsSys => BootRecordImpl::MsSys,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Auto,
    Windows,
    #[value(name = "windows-ntxp", alias = "windows-xp")]
    WindowsNtXp,
    #[value(name = "windows-2000", alias = "win2k")]
    Windows2000,
    Linux,
    Hybrid,
    Uefi,
}

impl From<ModeArg> for ModeRequest {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Auto => ModeRequest::Auto,
            ModeArg::Windows => ModeRequest::Windows,
            ModeArg::WindowsNtXp => ModeRequest::WindowsNtXp,
            ModeArg::Windows2000 => ModeRequest::Windows2000,
            ModeArg::Linux => ModeRequest::IsolinuxLinux,
            ModeArg::Hybrid => ModeRequest::Hybrid,
            ModeArg::Uefi => ModeRequest::UefiOnly,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let unattended = build_unattended_config(&cli);

    let filter = if cli.verbose {
        "bootsmith=debug,info"
    } else {
        "bootsmith=info,warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_target(false)
        .init();

    let config = bootsmith_core::Config {
        iso_path: cli.iso,
        device_path: cli.device,
        mode: cli.mode.into(),
        label: cli.label,
        dry_run: cli.dry_run,
        force: cli.force,
        verify: !cli.no_verify,
        verbose: cli.verbose,
        boot_record_impl: cli.boot_record.into(),
        unattended,
    };

    tracing::info!(
        iso = %config.iso_path.display(),
        device = %config.device_path.display(),
        mode = ?config.mode,
        dry_run = config.dry_run,
        "bootsmith: starting pipeline"
    );

    match pipeline::run(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bootsmith: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn build_unattended_config(cli: &Cli) -> Option<UnattendedConfig> {
    if !(cli.unattended
        || cli.product_key.is_some()
        || cli.admin_password.is_some()
        || cli.timezone.is_some())
    {
        return None;
    }
    Some(UnattendedConfig {
        product_key: cli.product_key.clone(),
        full_name: cli.full_name.clone(),
        organization: cli.organization.clone(),
        computer_name: cli.computer_name.clone(),
        admin_password: cli.admin_password.clone(),
        timezone: cli.timezone,
    })
}
