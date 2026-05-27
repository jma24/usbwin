# Backlog

Active release and cleanup work. Historical debugging notes live in
`RECOVERY_PLAN.md`, `TECH_DEBT.md`, and `XP_REGRESSION_2026_05_20.md`; do
not treat those files as the current work queue.

Last updated: 2026-05-26.

## v1.0 scope

bootsmith 1.0 is a focused Windows installer USB tool, not a generic boot
loader. The target matrix is **Windows XP and Windows 7**, with unattended
install support and XP-era AHCI/textmode storage support. Windows 2000 was
deferred to 1.1 on 2026-05-26 (see "Deferred to 1.1" below).
Linux/isolinux, generic UEFI-only media, Windows 8+, and broad rescue-disk
coverage are useful follow-up work, but they are not 1.0 blockers. Generic
ISO writing is already covered by tools like `dd`; the 1.0 value is making
old Windows installers work reliably from macOS.

## Release blockers

None outstanding as code. Remaining 1.0 work is the FAT32 cluster
cleanup and release packaging (signed/notarized macOS binary,
fresh-machine verification of the published `mkmsbr` 1.0.1 dependency).
XP AHCI verified 2026-05-26. Win7 SP1 regression re-run green
2026-05-26. Pipeline error reporting / `-v` done 2026-05-26. See
"Before v1.0" below.

## Deferred to 1.2

### Windows NT 4.0

Decided 2026-05-26: NT4 ships in 1.2 (after Win2k in 1.1). User is
exploring the driver-port work in parallel; no 1.0 or 1.1 dependency.

Why 1.2: NT4 needs its own F6 SCSI miniport RAM-disk driver. Nothing
off-the-shelf supports our GRUB4DOS + RAM-mapped-ISO + F6-floppy
chain on NT 4.0 — FiraDisk/SVBus/WinVBlock all target NT 5.x+.

Path is tractable, not research:
1. Stand up an NT4 SP6a dev VM in QEMU on macOS arm64 (TCG
   emulation; `-M pc -no-acpi -vga cirrus -device ne2k_pci`).
2. Install VC++ 4.2 Professional + NT4 DDK inside it. Self-hosted
   period toolchain; user has confirmed ISOs are findable.
3. Start from Gary Nebbett's `ramdisk.cpp` (~252 lines, archived at
   <https://github.com/EzioisAwesome56/nt4-ramscsi>). Original was
   designed to boot an already-installed NT4 from RAM; not a
   drop-in for our chain.
4. Three modifications needed:
   - GRUB4DOS INT 13h handoff convention (replicate the
     physical-address discovery + INT 13h hook that FiraDisk/SVBus
     use).
   - Flip SCSI INQUIRY device type from 0x00 (disk) to 0x05
     (CD-ROM); add the mode pages NT4 text-mode setup reads from a
     CD device. NT4 had full SCSI CD-ROM support — Adaptec + SCSI
     CD was the standard high-end NT4 build.
   - Package as an F6 floppy with `txtsetup.oem` modeled on
     `crates/bootsmith/src/pipeline/win2k_assets/svbus.ima`.
5. Add `BootMode::WindowsNt4` + `--type=windows-nt4` (alias `nt4`)
   plumbed end-to-end. Classifier needs a guard: NT4 install media
   currently misroutes through `is_nt5_install_media` and would
   silently land on the SVBus/Win2k path. SVBus's PE subsystem is
   5.00 (NT 5.0), wouldn't load on NT4 — would BSOD before bootsmith
   could surface a useful error.
6. Hardware target: no period machine on hand; QEMU is the
   development loop; a Pentium III-era box is the natural final
   validation. Dell E6410 might work in BIOS legacy-IDE mode but
   has no NT4 NIC driver — secondary target.

See `reference_nt4_ramdisk_research.md` (Claude memory) for the
research backing this plan.

## Deferred to 1.1

### Windows 2000

Decided 2026-05-26: Win2k ships in 1.1, not 1.0.

Why: Win2k text-mode install is hardware-verified, but the first-boot
`boot.ini` rdisk(1)→rdisk(0) gap means either (a) the user follows a
manual repair procedure or (b) we ship a phase-3 auto-repair (Tiny
Linux initrd, ~20 MB asset, new menu.lst entry, new hardware-verified
boot path). Option (b) is a 1.1-sized feature dressed as polish;
option (a) shifts a sharp edge onto the user. Cutting Win2k from 1.0
lets the release land on the two hardware-verified Windows targets
(XP, Win7) without either compromise.

Resuming work means returning to the "Windows 2000 install support"
and "Win2k boot.ini auto-repair (phase 3)" items below — their
implementation notes are still current.

## Completed for v0.3

### XP production path: first desktop boot

Status: done 2026-05-21.

The hand-staged GRUB4DOS + FiraDisk prototype completed an XP install on
2026-05-20. The production `windows-ntxp` path reached first desktop boot
on the Dell E6410 on 2026-05-21, and post-burn readback verified the
GRUB4DOS MBR entry on the 64 GB SanDisk.

Completed:
- XP SP3 reached first desktop boot from the production `bootsmith` USB.
- Post-test USB sanity check confirmed the USB still contained only the
  staged GRUB4DOS/FiraDisk files and no `\WINDOWS` install tree.
- `HARDWARE_TESTS.md` row 7 was updated from pending to green.

### Release docs for `windows-ntxp`

Status: done 2026-05-21.

- README support matrix reflects the FiraDisk path.
- `ARCHITECTURE.md` points at `XP_FIRADISK_PIPELINE.md` for XP design.
- `V0.3_WINDOWS_XP.md` is a short archival pointer.
- `XP_BOOT_INI.md` is a short archival pointer (FiraDisk path replaced the
  boot.ini chain).
- README links to `ARCHITECTURE.md`, `XP_FIRADISK_PIPELINE.md`, and
  `BACKLOG.md` only — recovery/regression docs are not in the main nav.

### Burn transcript and hardware checklist

Status: done 2026-05-21.

- `HARDWARE_TESTS.md` has the expected `bootsmith` confirmation output for XP
  SP3 media.
- The GRUB4DOS boot choices (entry 1 text-mode, entry 2 GUI-mode
  continuation) and reboot flow are documented in the boot flow section.
- The boot-track readback command and expected 64 GB SanDisk MBR entry are
  documented alongside row 7.

## Before v1.0

### Post-FiraDisk-migration cleanup

Status: code/docs pass done 2026-05-21. Only the FAT32 cluster-size
hardware question (see Cleanup below) remains.

The FiraDisk migration replaced the old NTLDR / boot.ini / I386-staging
pipeline with GRUB4DOS + RAM-mapped ISO. The leftover NTLDR-era code and
docs have been removed:

- Empty `crates/bootsmith/src/pipeline/xp_assets/` directory deleted. ✅
- `crates/bootsmith/src/pipeline/fat32.rs` deleted (was the FAT32 walker for
  the never-shipped `build_xp_setup_chain_bootsect` LDR$ loader). ✅
- `boot_records.rs::build_mbr_xp` and `tests/golden/mbr_xp_64gb.bin`
  deleted. XP mode writes the GRUB4DOS `grldr.mbr` boot track; no MBR_XP
  or MBR_WIN7 is involved. ✅
- `boot_records.rs` doc comments updated to reflect that the Win 7 path
  writes MBR_WIN7 and the XP path writes GRLDR_MBR. ✅
- `docs/XP_BOOT_INI.md` reduced to a short archival pointer. ✅

### Windows 2000 install support

Status: **deferred to 1.1 on 2026-05-26.** See "Deferred to 1.1" section
below for the deferral rationale. The implementation details below
remain accurate for when work resumes.

Text-mode install works on the Dell E6410 (verified 2026-05-22). First
boot of the installed Win2k requires a manual `boot.ini` repair (see
"Win2k boot.ini auto-repair (phase 3)" below). GUI-mode setup and
first-desktop boot are expected to work after the repair but are not
yet hardware-validated through to completion.

Full root-cause story and working install procedure live in
`docs/WIN2K_SVBUS.md`. Quick summary of what's actually shipping:

- `BootMode::Windows2000` + `--type=windows-2000` (alias `win2k`)
  plumbed end-to-end. ISO classifier splits NT5 media on WIN51/WIN52
  markers (present -> XP/2003 path, absent -> Win2k path).
- SVBus V1.3 vendored from SourceForge with `svbusx86.sys` PE
  subsystem version patched 5.02 -> 5.00 for NT 5.0 compatibility.
  See `crates/bootsmith/src/pipeline/win2k_assets/PROVENANCE.md`.
- GRUB4DOS 0.4.5c (2015-05-18) vendored for the Win2k path
  specifically (XP keeps 0.4.6a).
- menu.lst entry 1: no `hd0/hd1` swap, El Torito chainload
  (`chainloader (0xff)`). F6 + manual "SVBus Virtual SCSI Host
  Adapter x86" selection is required at the early text-mode setup
  screen.
- menu.lst entry 2: swap + `chainloader (hd0,0)/ntldr`. Works only
  after the boot.ini repair below.

Remaining work to call this "done" for 1.1:
- Hardware-verify GUI-mode setup completes through to first desktop
  boot AFTER the manual boot.ini repair (Option A or B in
  `docs/WIN2K_SVBUS.md`). Strongly expected to work; iteration this
  week stopped at the boot.ini issue itself.
- Ship the phase 3 auto-repair (next item) OR document the manual
  repair step as part of the supported procedure.
- Add an explicit support-matrix row in the README once the
  end-to-end path is green.

### Win2k boot.ini auto-repair (phase 3)

Status: deferred to 1.1 alongside the rest of Win2k support.

**The conflict** (now hardware-validated 2026-05-22):

- SVBus's text-mode slot enumeration breaks if GRUB4DOS does the
  `hd0/hd1` swap during install (BSOD 0x7B/0xC0000034). Entry 1
  must run with no swap.
- Win2k's text-mode setup writes boot.ini's `rdisk(N)` based on the
  BIOS-visible disk ordering at install time. With no swap, USB is
  0x80 and the internal HDD is 0x81 -> setup writes `rdisk(1)`.
- BUT: NTLDR + NT PBR + ARC-path resolution all hard-code that the
  system disk is BIOS drive 0x80. To boot the installed Win2k via
  GRUB4DOS chainload on the second BIOS HDD, you need the swap (so
  internal HDD becomes 0x80). With the swap, `rdisk(1)` resolves
  to the USB, not the HDD -> ntoskrnl missing. boot.ini needs
  `rdisk(0)`.
- USB hot-removal during install was tested and corrupts the
  install (BIOS caches the USB; Win2k still sees it; setup writes
  the boot loader to the USB instead of the HDD).
- GRUB4DOS 0.4.5c's in-place NTFS write rejects boot.ini with
  "Error 16 Fatal cannot write resident/small file! Enlarge it to
  2Kb and try again" because boot.ini is small enough to be MFT-
  resident. No way to enlarge from outside the FS.

**The fix**: a third menu entry that boots a small environment,
mounts the NTFS partition, rewrites `rdisk(1)` -> `rdisk(0)` in
`C:\boot.ini`, and reboots. Implementation options ranked by
maintainability:

1. **Tiny Linux initrd** (Tinycore/Alpine, ~20 MB asset cost):
   boots, mounts via ntfs-3g, runs `sed`, reboots. Most robust; the
   kernel handles all NTFS edge cases. **Recommended.**
2. **Automated Recovery Console flow**: chain to setupldr with a
   pre-staged response file that runs `set AllowAllPaths = TRUE` +
   `copy con c:\boot.ini` + the new content. No extra binaries
   shipped, but Recovery Console isn't really designed for
   automation and the procedure hasn't been hardware-verified end
   to end (`set AllowAllPaths = TRUE` was suggested by upstream
   research but never tested in our iteration).
3. **FreeDOS + NTFS write tool** (~2 MB): smaller than Linux but
   the FOSS NTFS-write story is unmaintained.
4. **GRUB4DOS raw-sector patch of the MFT entry**: smallest
   footprint, no extra asset. Compute LBA of boot.ini's MFT record,
   patch the resident data bytes. Fragile -- one NTFS layout
   variation away from corrupting the volume.

Done means:
- Pick one implementation (recommendation: option 1).
- Ship as a third GRUB4DOS menu entry; document in the NEXT STEPS
  block as the recommended post-install step.
- Hardware-verify the full install path: install -> reboot ->
  phase 3 (boot.ini fixed) -> reboot -> native boot -> GUI-mode ->
  first desktop.
- Remove the manual boot.ini repair procedure from the NEXT STEPS
  block once phase 3 is hardware-proven across multiple machines.

### XP AHCI/SATA/RAID textmode storage support

Status: done 2026-05-26. Code path landed 2026-05-22, hardware-verified
end-to-end on the Dell E6410 with BIOS SATA mode = AHCI and the Dell
`R274723` (Intel iaStor 9.6.4.1002) driver pack on 2026-05-26.

Remaining 1.0 decision (cosmetic): keep `--ahci-driver-dir` as a
documented stable flag, or gate under `--experimental-*`. Recommendation:
keep as stable — the path is hardware-verified and the BYO model is the
honest API.

Implementation summary (commit-ready 2026-05-22):
- `--ahci-driver-dir <path>` flag wired through CLI → Config →
  `windows_ntxp::stage_files`. BYO: bootsmith does not bundle any
  third-party storage driver.
- New module `pipeline::ntxp_txtsetup` parses both `txtsetup.oem`
  files, renames `disk1`→`disk2` on collision, coalesces `[Disks]`
  and `[SCSI]`/`[scsi]` headers, keeps FiraDisk's `[Defaults]` so
  the unattended path still loads the ramdisk filter first.
- `pipeline::ntxp_floppy` generalised with `add_file` / `remove_file`
  / `replace_file` / `read_file` so the merged `TXTSETUP.OEM` can
  swap in cleanly without leaking clusters.
- Walkthrough doc at `docs/AHCI_DRIVER.md` (Dell E6410 + Intel iaStor
  9.6.4.1002 A00, DUP `R274723.exe` from
  <https://dl.dell.com/SATA/R274723.exe>).

Research findings already incorporated:
- USB controller drivers are NOT required for the bootsmith RAM-mapped
  chain (FiraDisk/SVBus take over INT 13h from RAM after `map --mem`).
- USB 3.0 forward-compat is a docs problem — users on newer machines
  plug into USB 2.0 and/or enable BIOS legacy USB support.

Remaining work:
- Add a row to `HARDWARE_TESTS.md` for the AHCI scenario. ✅

### XP unattended support for FiraDisk ISO path

Status: done 2026-05-21. Implementation landed in commit 22f00ec and was
hardware-verified end-to-end on the Dell E6410.

- `--unattended` injects `I386\WINNT.SIF` into the staged `XP.ISO` and
  `A:\WINNT.SIF` into the staged `FIRADISK.IMA`; the input ISO is never
  mutated. ✅
- Supports product key, computer name, admin password policy, timezone,
  EULA acceptance, install mode, and driver-signing policy. ✅
- Keeps manual partitioning by default (`AutoPartition=0`). ✅
- Hardware-verified: unattended XP install reaches first desktop boot on
  the E6410 with a real product key. ✅

Win2k unattended falls under the Windows 2000 install item below — same
SIF mechanics but verified against a Win2k ISO.

### Windows 7 release hardening

Status: done 2026-05-26. Win 7 SP1 hardware test re-run green after the
XP-path cleanup.

- Re-ran the Win 7 SP1 hardware test after the XP-path cleanup. ✅
- `--type=auto` and explicit `--type=windows` both produce the expected
  Win 7 boot path. ✅
- `--boot-record=ms-sys` retained as an audit fallback; the in-process
  mkmsbr backend is the documented default release path. ✅

### Release packaging

Status: 1.0 blocker (reduced scope 2026-05-26 — see signing note below).

Done means:
- Remove the local sibling `../mkmsbr` requirement from release builds by
  using a published crate, vendored dependency, or pinned git dependency.
  Implemented with the published `mkmsbr` crate. ✅ Fresh-checkout verified
  2026-05-26: clean clone with an isolated CARGO_HOME pulled `mkmsbr 1.0.1`
  from crates.io (registry source + checksum, no local path dep), built
  release offline, and passed all 80 workspace tests.
- Update README install instructions for users who are not building from a
  local multi-repo checkout.
- Ship via the same channels as mkmsbr: crates.io (`cargo install`) and the
  existing Homebrew tap.

Signing/notarization: **optional, not a 1.0 blocker** (decided 2026-05-26,
mirroring mkmsbr, which shipped 1.0.1 unsigned). Gatekeeper only blocks
files carrying `com.apple.quarantine`, which is set on *browser/email
downloads* — not on locally compiled binaries. `cargo install` compiles
locally (no quarantine); Homebrew builds from source or strips the
quarantine xattr from bottles. So the crates.io + Homebrew channels need no
notarization. Notarization is only required for a frictionless prebuilt
binary downloaded from a GitHub Release page. The user may still notarize
for that download path, but it does not gate 1.0.

### Pipeline error reporting and verbose mode

Status: done 2026-05-26.

What landed:
- All five pipeline modules' `anyhow_from_core` helpers now use
  `anyhow::Error::new(e)` instead of `anyhow!("{e}")`, preserving the
  `thiserror` source chain so anyhow's `{:#}` chain printer surfaces
  the underlying `io::Error`/disk error/etc.
- `bootsmith-core::Error::Io` and `bootsmith-disk::DiskError::Io` Display
  strings dropped the `{0}` interpolation. With `#[from]`, the source
  is already walkable; including it in Display caused `{:#}` to print
  the io::Error twice.
- `boot_records.rs` and `pipeline.rs` migrated their lossy
  `map_err(|e| anyhow!("...: {e}"))` calls to `.context("...")` so
  `bootrec::Error` and `bootsmith_disk::DiskError` chains survive.
- `diskutil.rs` logs every `Command` invocation at `tracing::debug!`
  with the argv, and captures stdout/stderr at the same level via a
  shared `log_output` helper. Non-empty stderr on a success exit now
  surfaces as `tracing::warn!` (some tools warn but exit 0).
- `windows.rs` and `windows_ntxp.rs` instrumented with `tracing::debug!`
  for every pipeline step (numbered 1/N..N/N), every file write/copy,
  and every silent `let _ = ...` discard now logs at `debug`/`warn`
  instead of being invisible.
- `main.rs` log filter rewritten: default is silent
  (`bootsmith=warn,...=warn`), `--verbose` enables `debug` across our
  crates, and `RUST_LOG` (e.g. `RUST_LOG=bootsmith=debug`) overrides
  both. Banner moved from `info` to `debug` so default stderr stays
  clean.

### GUI-mode auto-find slipstreamed iaStor without breaking asms

Status: 1.1 polish (acceptable one-click workaround in place for 1.0).

Text-mode AHCI slipstream lands the AHCI driver as inbox (PnP auto-binds
to the target disk during text-mode setup, no F6 needed). But during
GUI-mode XP setup, PnP re-enumerates and the "Files Needed" dialog
defaults to `F:\` instead of `F:\i386`. The user clicks once to navigate
to `F:\i386\iaStor.sys` and the install continues.

Root cause: the vendor's `iaStor.inf` has `[SourceDisksNames] 1 = ...,,,""`
(empty path), so PnP defaults the dialog's source path to the install
media root. The file is actually at `\i386\iaStor.sys` because slipstream
puts everything in I386.

The "asms" regression that bit both auto-find attempts was actually a
secondary symptom of the original ISO9660 sort bug: the old
`inject_winnt_sif` appended WINNT.SIF at the byte-end of I386 without
sorting, leaving the directory sorted-then-unsorted after both
slipstream and the sif injection ran. XP setup's directory walker
choked on the seam. Fixed 2026-05-26 by routing `inject_winnt_sif`
through `append_file_to_i386` so every mutation re-sorts.

`--unattended` + `--ahci-driver-dir` now compose cleanly. But the
GUI-mode "Files Needed: iaStor.sys" prompt at F:\ default still needs
ONE manual click to navigate to F:\i386. That's the actual 1.1 polish
item. Two paths to try:

Two viable paths for 1.1, in order of complexity:

1. **Patch iaStor.inf during slipstream.** Modify `[SourceDisksNames]
   1 = ...,,,"\i386"` so PnP defaults to `\i386` instead of root.
   Smallest change, but breaks `iaStor.cat`'s signature -- XP shows a
   separate "unsigned driver" warning. To suppress, also set
   `DriverSigningPolicy=Ignore` in a synthesised sif; do that without
   tripping the asms regression (likely safe if no other unattended
   directives are added).

2. **Stage via `$OEM$\$1\Drivers\<vendor>\` and `OemPnPDriversPath` in
   sif.** Standard nLite/F6 approach. Drop the .inf/.cat into the
   special $OEM$ directories so XP setup copies them to
   `C:\Drivers\<vendor>\` during install, then set
   `OemPnPDriversPath="Drivers\<vendor>"` (relative to system drive,
   NOT install media). Needs a new ISO mutation path (sibling directory
   injection at ISO root) which current ntxp_iso primitives don't
   support.

Hardware-verify either path on Dell E6410 that asms remains happy
before declaring done.

### Hybrid mode eject race after verify

Status: backlog (cosmetic; data is intact).

`pipeline/hybrid.rs:61-62` ignores the `unmount_disk` error and goes
straight to `eject`. After the verify pass macOS aggressively auto-mounts
any partition it recognizes on the freshly-written ISO (SystemRescue 13.00
triggered this on 2026-05-26), and the subsequent `diskutil eject` fails
with `Volume failed to eject` even though the write+verify already
succeeded. The user has to `diskutil unmountDisk force` and yank manually.

Done means:
- Port the `unmount_disk_force` + short retry pattern from
  `windows_ntxp.rs` into `hybrid.rs` so the eject step actually completes.
- Same fix may be worth auditing in `windows.rs` and `windows_2000.rs`
  while we're in there.

## Compatibility backlog

### WinVBlock and low-RAM fallback

Status: backlog.

FiraDisk + RAM ISO remains the default. Some machines may need WinVBlock or
a reduced-ISO path when RAM is tight.

Done means:
- Add controlled debug switches for FiraDisk-only, WinVBlock-only, and
  combined driver floppy images.
- Add a RAM requirement warning before burning a full RAM-mapped XP ISO.
- Keep the default path simple unless hardware evidence says otherwise.

### Generic Linux/isolinux and UEFI-only modes

Status: post-1.0.

The codebase still has explicit mode names for isolinux and UEFI-only
media, but v1 is Windows-focused. Do not spend 1.0 time turning bootsmith into
a generic boot loader.

Done means:
- Decide whether to keep the current mode names as future placeholders or
  hide them from help output until implementation starts.
- Implement only after the Windows 2000/XP/7 scope is shipped.

## Cleanup

### FAT32 cluster-size assumption

Status: done 2026-05-26. The forced `-c 8` (4 KiB clusters) is removed;
`newfs_msdos_fat32` uses default cluster sizing.

The `-c 8` was pre-FiraDisk debt. Its rationale (XP setupldr's FAT walker
choking on 32 KiB clusters reading `txtsetup.sif`) no longer applies: in
the FiraDisk path setupldr + txtsetup.sif are read from the RAM-mapped
XP.ISO, not the FAT32 partition. Only GRUB4DOS reads this filesystem and
its FAT driver handles 32 KiB clusters fine. Hardware-verified on the
E6410: XP text-mode setup copied files cleanly with default clustering.

### Historical remediation docs

Status: mostly collapsed.

Done.

- `RECOVERY_PLAN.md` is archival only.
- `TECH_DEBT.md` points to this backlog.
- `V0.3_WINDOWS_XP.md` is a historical pointer.
- `XP_REGRESSION_2026_05_20.md` remains as a historical incident log.
- No active docs instruct contributors to read remediation files first.
