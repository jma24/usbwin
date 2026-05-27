//! `bootsmith-eval` — VM-based smoke test for produced USB images.
//!
//! Boots a USB image under QEMU (TCG, headless), screenshots the framebuffer
//! every N seconds, runs tesseract over the PPM, and classifies the result
//! as PASS / FAIL / TIMEOUT based on substring matches. Catches the content
//! bugs we keep burning hardware sticks to find (BSOD on boot, missing HIVE,
//! malformed BOOTSECT.DAT, etc).
//!
//! Exit codes:
//!   0  pass — saw a known-good text-mode setup screen
//!   1  fail — saw a BSOD / STOP
//!   2  timeout — neither in time budget
//!   3  harness error (QEMU spawn, tesseract missing, etc.)

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "bootsmith-eval",
    version,
    about = "QEMU-based smoke test for bootsmith-produced USB images.",
    long_about = "Boots --image under qemu-system-i386 (TCG), with a blank \
                  target HDD attached, and looks for a known-good text-mode \
                  setup screen via OCR. Fails fast on BSOD/STOP."
)]
struct Cli {
    /// Path to the produced USB image (raw .img). Attached as IDE primary.
    #[arg(long, value_name = "PATH")]
    image: PathBuf,

    /// How to present --image to QEMU.
    #[arg(long, value_enum, default_value_t = BootMedia::Disk)]
    boot_media: BootMedia,

    /// Expected install flavor — picks the OCR matchers.
    #[arg(long, value_enum, default_value_t = Flavor::WindowsXp)]
    flavor: Flavor,

    /// Total budget for the smoke test, in seconds.
    #[arg(long, default_value_t = 900)]
    timeout: u64,

    /// Seconds between framebuffer captures.
    #[arg(long, default_value_t = 15)]
    interval: u64,

    /// Where to drop screenshots + OCR text. Defaults to a tempdir.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Blank target HDD size (qemu-img -f raw). Sparse, so disk use is small.
    #[arg(long, default_value = "8G")]
    target_size: String,

    /// How to attach the blank target HDD.
    #[arg(long, value_enum, default_value_t = TargetBus::Ide)]
    target_bus: TargetBus,

    /// QEMU RAM in MiB.
    #[arg(long, default_value_t = 512)]
    mem_mib: u32,

    /// QEMU machine type.
    #[arg(long, default_value = "pc")]
    machine: String,

    /// Keep the VM alive after verdict (debug). Default kills on exit.
    #[arg(long)]
    keep_running: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Flavor {
    /// Windows XP text-mode setup.
    WindowsXp,
    /// Windows 7+ bootmgr.
    Windows7,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum TargetBus {
    /// Attach target HDD as IDE, matching the original smoke harness.
    Ide,
    /// Attach target HDD behind a QEMU ICH9 AHCI controller.
    Ahci,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BootMedia {
    /// Boot --image as a raw hard disk.
    Disk,
    /// Boot --image as an IDE CD-ROM.
    Cdrom,
}

impl Cli {
    /// Substrings (case-insensitive) that mean "we got far enough — PASS".
    fn pass_markers(&self) -> &'static [&'static str] {
        match (self.flavor, self.target_bus) {
            // AHCI coverage needs to get past storage-driver load and disk
            // enumeration. The normal early text-mode markers are too early:
            // setup can show them before failing to load iaStor.sys.
            (Flavor::WindowsXp, TargetBus::Ahci) => &[
                "unpartitioned space",
                "the following list shows",
                "to set up windows xp on the selected item",
            ],
            // Text-mode setup banner + the F6/repair prompt that follows.
            (Flavor::WindowsXp, _) => &[
                "welcome to setup",
                "setup is starting",
                "press f6 if you",
                "press r to repair",
            ],
            (Flavor::Windows7, _) => &[
                "windows is loading files",
                "press any key to boot",
                "starting windows",
            ],
        }
    }

    /// Substrings that mean "we hit a fatal — FAIL".
    fn fail_markers(&self) -> &'static [&'static str] {
        // Generic BSOD body text — far more reliable than the STOP code,
        // which OCR sometimes mangles (saw "TRQL_NOT_LESS_OR_EQUAL"
        // for IRQL_NOT_LESS_OR_EQUAL in a real run).
        &[
            "a problem has been detected",
            "has been shut down to prevent damage",
            "stop:",
            "process1_initialization_failed",
            "inaccessible_boot_device",
            "iastor.sys could not be found",
            "the file iastor.sys",
            "setup did not find any hard disk drives",
            "ntldr is missing",
            "ntldr is compressed",
            "bootmgr is missing",
            "disk read error",
            "non-system disk",
            "invalid partition table",
        ]
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match run(cli) {
        Ok(Verdict::Pass {
            last_screenshot,
            matched,
        }) => {
            println!(
                "PASS — matched {:?} in {}",
                matched,
                last_screenshot.display()
            );
            ExitCode::from(0)
        }
        Ok(Verdict::Fail {
            last_screenshot,
            matched,
        }) => {
            println!(
                "FAIL — matched {:?} in {}",
                matched,
                last_screenshot.display()
            );
            ExitCode::from(1)
        }
        Ok(Verdict::Timeout { last_screenshot }) => {
            let path = last_screenshot
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(no screenshot captured)".into());
            println!("TIMEOUT — last screenshot {}", path);
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("eval harness error: {:#}", e);
            ExitCode::from(3)
        }
    }
}

#[derive(Debug)]
enum Verdict {
    Pass {
        last_screenshot: PathBuf,
        matched: String,
    },
    Fail {
        last_screenshot: PathBuf,
        matched: String,
    },
    Timeout {
        last_screenshot: Option<PathBuf>,
    },
}

fn run(cli: Cli) -> Result<Verdict> {
    // --- preflight ---
    if !cli.image.exists() {
        bail!("--image does not exist: {}", cli.image.display());
    }
    require_tool("qemu-system-i386")?;
    require_tool("qemu-img")?;
    require_tool("tesseract")?;

    // --- working dir ---
    let work = match cli.out_dir.clone() {
        Some(p) => {
            fs::create_dir_all(&p).with_context(|| format!("mkdir {}", p.display()))?;
            p
        }
        None => {
            let p = std::env::temp_dir().join(format!("bootsmith-eval-{}", std::process::id()));
            fs::create_dir_all(&p)?;
            p
        }
    };
    tracing::info!("work dir: {}", work.display());

    // --- blank target HDD ---
    let target_img = work.join("target.img");
    if !target_img.exists() {
        let status = Command::new("qemu-img")
            .args(["create", "-f", "raw"])
            .arg(&target_img)
            .arg(&cli.target_size)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .context("spawn qemu-img")?;
        if !status.success() {
            bail!("qemu-img create failed: {status}");
        }
    }

    // --- spawn QEMU headless with HMP monitor on a unix socket ---
    let monitor_sock = work.join("monitor.sock");
    let _ = fs::remove_file(&monitor_sock);

    let mut qemu_cmd = Command::new("qemu-system-i386");
    qemu_cmd.args([
        "-m",
        &cli.mem_mib.to_string(),
        "-machine",
        &cli.machine,
        "-accel",
        "tcg",
        "-cpu",
        "pentium3",
        "-rtc",
        "base=localtime",
        "-vga",
        "std",
        "-display",
        "none",
    ]);
    match cli.boot_media {
        BootMedia::Disk => {
            qemu_cmd.args(["-boot", "c"]).arg("-drive").arg(format!(
                "file={},format=raw,if=ide,index=0,media=disk",
                cli.image.display()
            ));
        }
        BootMedia::Cdrom => {
            qemu_cmd.args(["-boot", "d"]).arg("-cdrom").arg(&cli.image);
        }
    }
    match cli.target_bus {
        TargetBus::Ide => {
            qemu_cmd.arg("-drive").arg(format!(
                "file={},format=raw,if=ide,index=1,media=disk",
                target_img.display()
            ));
        }
        TargetBus::Ahci => {
            qemu_cmd
                .args(["-device", "ich9-ahci,id=ahci"])
                .arg("-drive")
                .arg(format!(
                    "file={},format=raw,if=none,id=targetdisk,media=disk",
                    target_img.display()
                ))
                .args(["-device", "ide-hd,drive=targetdisk,bus=ahci.0"]);
        }
    }
    let qemu = qemu_cmd
        .arg("-qmp")
        .arg(format!("unix:{},server,nowait", monitor_sock.display()))
        // QEMU's stdout/stderr to log files in the work dir.
        .stdout(Stdio::from(fs::File::create(work.join("qemu.stdout"))?))
        .stderr(Stdio::from(fs::File::create(work.join("qemu.stderr"))?))
        .spawn()
        .context("spawn qemu-system-i386")?;

    // Make sure we kill QEMU on every exit path.
    let mut guard = QemuGuard {
        child: Some(qemu),
        keep: cli.keep_running,
    };

    // --- wait for monitor socket ---
    let mut mon = wait_for_monitor(&monitor_sock, Duration::from_secs(15))
        .context("connect to qemu monitor")?;
    qmp_handshake(&mut mon).context("qmp handshake")?;

    // --- screenshot / OCR loop ---
    let deadline = Instant::now() + Duration::from_secs(cli.timeout);
    let interval = Duration::from_secs(cli.interval);
    let mut last_screenshot: Option<PathBuf> = None;
    let mut shot_idx: u32 = 0;

    while Instant::now() < deadline {
        thread::sleep(interval);
        shot_idx += 1;
        let png = work.join(format!("shot-{:04}.png", shot_idx));
        if let Err(e) = screendump(&mut mon, &png) {
            tracing::warn!("screendump failed (shot {}): {:#}", shot_idx, e);
            continue;
        }
        last_screenshot = Some(png.clone());

        let txt = match ocr(&png) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("ocr failed (shot {}): {:#}", shot_idx, e);
                continue;
            }
        };
        // Persist the OCR text alongside the screenshot — useful for triage.
        fs::write(work.join(format!("shot-{:04}.txt", shot_idx)), &txt).ok();

        let lower = txt.to_lowercase();
        for marker in cli.fail_markers() {
            if lower.contains(marker) {
                guard.shutdown();
                return Ok(Verdict::Fail {
                    last_screenshot: png,
                    matched: (*marker).into(),
                });
            }
        }
        for marker in cli.pass_markers() {
            if lower.contains(marker) {
                guard.shutdown();
                return Ok(Verdict::Pass {
                    last_screenshot: png,
                    matched: (*marker).into(),
                });
            }
        }
        tracing::info!(
            "shot {} — {} OCR chars, no marker hit yet ({}s left)",
            shot_idx,
            txt.len(),
            deadline.saturating_duration_since(Instant::now()).as_secs()
        );
    }

    guard.shutdown();
    Ok(Verdict::Timeout { last_screenshot })
}

fn require_tool(name: &str) -> Result<()> {
    let status = Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err(anyhow!(
            "required tool `{name}` not found on PATH (brew install qemu / tesseract)"
        )),
    }
}

fn wait_for_monitor(path: &Path, timeout: Duration) -> Result<UnixStream> {
    let start = Instant::now();
    loop {
        if let Ok(s) = UnixStream::connect(path) {
            s.set_read_timeout(Some(Duration::from_secs(15)))?;
            s.set_write_timeout(Some(Duration::from_secs(5)))?;
            return Ok(s);
        }
        if start.elapsed() > timeout {
            bail!("timed out waiting for monitor socket {}", path.display());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Read one '\n'-terminated JSON message from the QMP socket.
fn read_qmp_line(mon: &mut UnixStream) -> Result<String> {
    let mut buf = [0u8; 1];
    let mut line = Vec::with_capacity(256);
    loop {
        let n = mon.read(&mut buf)?;
        if n == 0 {
            bail!("qmp socket closed");
        }
        if buf[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        line.push(buf[0]);
    }
}

/// Read messages until one matches `pred` (skipping async events).
fn read_qmp_until<F: Fn(&str) -> bool>(mon: &mut UnixStream, pred: F) -> Result<String> {
    for _ in 0..32 {
        let line = read_qmp_line(mon)?;
        if pred(&line) {
            return Ok(line);
        }
        // event / unrelated — skip
    }
    bail!("qmp: no matching reply in 32 messages");
}

fn qmp_handshake(mon: &mut UnixStream) -> Result<()> {
    // Greeting.
    let greeting = read_qmp_line(mon)?;
    if !greeting.contains("QMP") {
        bail!("expected QMP greeting, got: {}", greeting);
    }
    // Enter command mode.
    mon.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")?;
    mon.flush()?;
    let _ = read_qmp_until(mon, |l| l.contains("\"return\"") || l.contains("\"error\""))?;
    Ok(())
}

fn screendump(mon: &mut UnixStream, out: &Path) -> Result<()> {
    // QMP's screendump returns synchronously once the file is written.
    // `format: "png"` since leptonica/tesseract don't recognize PPM here.
    let cmd = format!(
        "{{\"execute\":\"screendump\",\"arguments\":{{\"filename\":{},\"format\":\"png\"}}}}\n",
        json_string(&out.display().to_string())
    );
    mon.write_all(cmd.as_bytes())?;
    mon.flush()?;
    let reply = read_qmp_until(mon, |l| l.contains("\"return\"") || l.contains("\"error\""))?;
    if reply.contains("\"error\"") {
        bail!("qmp screendump error: {}", reply);
    }
    if !out.exists() {
        bail!("screendump returned ok but no file at {}", out.display());
    }
    Ok(())
}

/// Minimal JSON string encoder for paths — escapes `\` and `"` only.
/// Paths from clap/PathBuf on macOS won't contain control chars in practice.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn ocr(img: &Path) -> Result<String> {
    // tesseract/leptonica chokes on some absolute paths under the harness's
    // sandbox; cd into the image's parent and pass the basename.
    let dir = img
        .parent()
        .ok_or_else(|| anyhow!("image has no parent dir"))?;
    let name = img
        .file_name()
        .ok_or_else(|| anyhow!("image has no filename"))?;
    let out = Command::new("tesseract")
        .current_dir(dir)
        .arg(name)
        .arg("-") // stdout
        .args(["-l", "eng", "--psm", "6"]) // psm 6 = uniform block of text
        .stderr(Stdio::piped())
        .output()
        .context("spawn tesseract")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("tesseract exited {}: {}", out.status, err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// RAII-ish handle that kills QEMU on drop unless `keep` is set.
struct QemuGuard {
    child: Option<Child>,
    keep: bool,
}

impl QemuGuard {
    fn shutdown(&mut self) {
        if self.keep {
            return;
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for QemuGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}
