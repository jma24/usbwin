# Architecture

## Goals

1. **Hard to misuse.** Writing to the wrong block device formats it irreversibly. Every code path that opens a raw device for write funnels through one guarded type that refuses obviously-wrong targets.
2. **Verifiable.** Every byte we emit can be re-derived and compared. Boot records are assembled from versioned NASM source; partition tables are written as bytes (no shelling out to `fdisk`); writes are verified by read-back.
3. **Durable.** A workspace of focused crates with explicit traits at module boundaries. The library half (`usbwin-core`) is testable without root, without a USB stick, and without macOS.

## Workspace layout

```
crates/
├── usbwin           binary: CLI parsing, top-level orchestration, user prompts
├── usbwin-core      pipeline types, errors, the WritePlan trait, the four modes
├── usbwin-iso       ISO9660 inspection + auto-classification
└── usbwin-disk      Device trait + macOS implementation (DiskArbitration + raw I/O)

docs/                this file, HARDWARE_TESTS.md, FIELD_FINDINGS, etc.
tests/               integration tests + golden fixtures
```

Boot-record assembly (MBR, FAT32-PBR-with-preserved-BPB splice, NTFS PBR,
and the historical XP boot records) lives in the separate
[`mkmsbr`](https://github.com/jma24/mkmsbr) crate. usbwin's Cargo.toml
uses `package = "mkmsbr"` aliasing so the boot-record wrapper can keep
importing it as `bootrec::*` internally.

The Windows-mode pipelines have their own submodules under
`crates/usbwin/src/pipeline/`:

- `boot_records` — pure byte-producing wrappers around mkmsbr
  (`build_mbr_win7`, `build_mbr_xp`, `splice_pbr_bootmgr`).
  Has golden tests against checked-in `.bin`
  fixtures so any mkmsbr byte drift trips CI immediately.
- `fat32` — minimal read-only FAT32 walker that finds a file's LBA
  list. Kept for boot-record diagnostics and possible future filesystem
  verification.
- `windows_ntxp` — the active NT5/XP path. It writes the chenall GRUB4DOS
  `grldr.mbr` boot track, formats one active FAT32 partition, stages
  `GRLDR`, `menu.lst`, `XP.ISO`, and `FIRADISK.IMA`, then boots XP setup
  by RAM-mapping the original ISO as a virtual CD.

## The five durability calls

1. **Single `DeviceHandle` chokepoint** in `usbwin-disk`. Every raw write goes through it. It refuses the boot disk, internal disks, and disks > 256 GiB without `--force`.
2. **Verify-by-default.** Every `write_bytes()` has a typed `verify()` that re-reads and compares. `--no-verify` to skip; off by default.
3. **Typed errors.** `thiserror` per crate, `anyhow` in the binary, each pipeline step returns a named error variant mapped to a numbered exit code.
4. **Shell-out is rare and centralized.** Partition tables are bytes, not `fdisk` calls. The only allowed shell-outs are `diskutil unmountDisk` / `mountDisk` / `eject`, wrapped in `usbwin-disk::macos` with retry + error context.
5. **Test pyramid that doesn't burn USB sticks.**
   - Unit: BPB splice, MBR layout, ISO classifier on fixture bytes.
   - Golden: byte-for-byte comparison of the boot-record-producing functions (Win 7 MBR, XP MBR, BOOTMGR multi-sector PBR) against checked-in goldens. Lives in `pipeline::boot_records` (the `#[cfg(test)]` mod). Catches "bootrec bumped and now we produce different bytes" automatically. Goldens at `tests/golden/*.bin`; refresh with `UPDATE_GOLDENS=1 cargo test ...`. The QEMU and real-hardware layers below cover boot-behavior; this layer covers byte stability.
   - QEMU smoke: write to a disk image, boot it under qemu-system-i386, scrape serial output. *(Lives in bootrec, not usbwin — bootrec is where the byte production happens; usbwin's wrapper is what the golden tests cover.)*
   - Hardware (manual): the scenarios in [`HARDWARE_TESTS.md`](HARDWARE_TESTS.md), run before each release.

## Release targets

**v1.0 is Windows 2000/XP/7.** usbwin is intentionally not a generic boot
loader for the first stable release. The Windows 7 boot chain (`bootmgr`
loaded by a FAT32 PBR, with `sources/install.wim` carrying the installer
payload) is the NT6 path. The NT5 path covers Windows 2000 and XP through
GRUB4DOS + FiraDisk, with unattended installs and AHCI/textmode storage as
1.0 requirements.

**Windows XP is now handled by `windows-ntxp`.** XP predates the
`bootmgr` + `install.wim` design, so usbwin uses GRUB4DOS + FiraDisk instead of the
deleted three-tree FAT32 staging path. Auto-detection maps NT5-class media
(Windows 2000/XP/2003) to `windows-ntxp`; `--type=windows-xp` is a
compatibility alias. Windows 2000 is classifier-covered but not yet a
supported install target.

Staging:

- **v0.1** — hybrid mode (raw write for Linux/BSD ISOs). Ships the safety chokepoint, the verify pass, the macOS device layer. *Done.*
- **v0.2** — Windows 7+ mode end-to-end via `ms-sys` shell-out. Full Windows install USB pipeline. *Done; hardware-verified on Dell E6410.*
- **v0.3** — Windows XP mode via GRUB4DOS + FiraDisk. *Implemented and hardware-verified on Dell E6410.*
- **v1.0** — Windows 2000/XP/7 release: Win2k install support, XP/2000 unattended install support, XP/2000 AHCI/textmode storage support, release packaging, and hardened diagnostics. The `bootrec`/mkmsbr in-process backend is already the default for Win 7; `ms-sys` remains an opt-in audit fallback.
- **later** — Isolinux Linux mode, generic UEFI-only mode, broader Windows 8+ release claims, rescue disks, and GUI/frontend work.

## The pipeline

```
WritePlan (typed enum: Windows | WindowsNtXp | Hybrid | IsolinuxLinux | UefiOnly)
    │
    ▼
DryRun ──► byte stream into Vec<u8>      (no root, no device)
   or
Execute ──► DeviceHandle ──► macOS impl  (root, real /dev/rdiskN)
                                │
                                ▼
                          verify() re-read
```

Both `DryRun` and `Execute` walk the same step sequence; the difference is the `Device` implementation under the hood. This is what makes dry-run golden tests honest — the production code path is what produces the bytes.

## Step sequence (Windows 7+ mode, the hard one)

1. **Validate.** ISO exists, size fits, device path looks like `/dev/rdiskN`.
2. **Confirm.** Print device fingerprint (size, model, removable bit); demand `y` unless `--force`.
3. **Unmount.** `diskutil unmountDisk /dev/diskN`.
4. **Write partition table.** Single primary FAT32 partition, active flag set.
5. **Format.** `newfs_msdos -F 32 -v <label> /dev/rdiskNs1`.
6. **Mount.** `diskutil mountDisk /dev/diskN`.
7. **Mount ISO.** `hdiutil attach <iso> -nobrowse -mountpoint /tmp/usbwin-iso`.
8. **Copy.** File-by-file from ISO to USB.
9. **Unmount ISO.**
10. **Unmount USB.**
11. **Write MBR boot code.** 440 bytes at offset 0 of `/dev/rdiskN`, sourced from `bootrec::mbr_win7` (default) or `ms-sys --mbr7` (with `--boot-record=ms-sys`).
12. **Splice FAT32 PBR.** Read the freshly-formatted partition's reserved sectors, splice in `bootrec::FAT32_PBR_BOOTMGR_MULTI_BOOT` (sectors 0, 1, 2) while preserving the BPB the formatter wrote (bytes 3..90 of sector 0) and the FSInfo sector at LBA 1. The ms-sys fallback runs `--fat32pe` against the buffered device `/dev/diskNs1` (sub-sector writes silently fail on `/dev/rdiskN`; see [`FIELD_FINDINGS`](FIELD_FINDINGS_2026_05_18.md) §2).
13. **Verify.** Re-read MBR and PBR, byte-compare against the planned bytes.
14. **Eject.**

Hybrid mode collapses to steps 1–3, raw `dd`-equivalent write, verify, eject. UEFI-only mode swaps step 4 for GPT layout and step 11/12 for nothing. Isolinux Linux mirrors Windows mode with the syslinux boot code in place of the Windows blobs.

## Why a workspace and not a single crate

`usbwin-core` is the part that should still compile and run unit tests on Linux/CI, where there's no DiskArbitration framework. Putting macOS-specific code behind a trait in a separate crate forces this discipline. Future v2 work (Linux device backend, GUI frontend) plugs in without touching the core.

## Open architectural questions

- **DiskArbitration framework via FFI** vs `diskutil` shell-out: v1 uses shell-out for `unmount`/`mount`/`eject`. If we hit auto-mount races we can't bracket around, we'll wire `DASessionCreate` via the `core-foundation` crate and a small FFI binding. Tracked as an issue, not a blocker.
- **Whether to read ISO9660 directly** instead of shelling to `hdiutil`. Direct read would let us copy files without a temporary mount, and would make Windows-mode dry-run produce an accurate byte stream. Probably v1.1.
