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
XP-setup raw-LBA $LDR$ chainloader) lives in the separate
[`mkmsbr`](https://github.com/jma24/mkmsbr) library (renamed from
`bootrec` 2026-05-19), depended on as a path dep today and a published
crate later. usbwin's Cargo.toml uses `package = "mkmsbr"` aliasing so
the rest of the code can keep importing as `bootrec::*` unchanged.

The Windows-mode pipelines have their own submodules under
`crates/usbwin/src/pipeline/`:

- `boot_records` — pure byte-producing wrappers around mkmsbr
  (`build_mbr_win7`, `build_mbr_xp`, `splice_pbr_bootmgr`,
  `splice_pbr_ntldr`). Has golden tests against checked-in `.bin`
  fixtures so any mkmsbr byte drift trips CI immediately.
- `fat32` — minimal read-only FAT32 walker that finds a file's LBA
  list. Used for XP's BOOTSECT.DAT raw-LBA loader (we walk FAT to find
  `\$LDR$`'s on-disk clusters and hand them to mkmsbr's bootsector
  builder).
- `xp_staging` — the XP-specific filesystem dance: stage `\NTLDR`,
  `\NTDETECT.COM`, `\$LDR$`, `\boot.ini`, `\TXTSETUP.SIF` at the root,
  generate `\$WIN_NT$.~BT\BOOTSECT.DAT`, rename `\I386\` →
  `\$WIN_NT$.~BT\` (text-mode setupldr source, no I/O), then ditto
  `\$WIN_NT$.~BT\` → `\$WIN_NT$.~LS\I386\` (GUI-mode source, copied
  to `C:\$WIN_NT$.~LS\` by text-mode setup) and drop the verbatim
  USB_MultiBoot `ren_fold.cmd` / `undoren.cmd` rename scripts inside
  the latter. See [`TECH_DEBT.md`](TECH_DEBT.md) for the remaining
  ~580 MB duplication between `~BT` and `~LS\I386\`.
- `windows_xp_sif`, `windows_xp_unattended` — XP-specific INI editors.

## The five durability calls

1. **Single `DeviceHandle` chokepoint** in `usbwin-disk`. Every raw write goes through it. It refuses the boot disk, internal disks, and disks > 256 GiB without `--force`.
2. **Verify-by-default.** Every `write_bytes()` has a typed `verify()` that re-reads and compares. `--no-verify` to skip; off by default.
3. **Typed errors.** `thiserror` per crate, `anyhow` in the binary, each pipeline step returns a named error variant mapped to a numbered exit code.
4. **Shell-out is rare and centralized.** Partition tables are bytes, not `fdisk` calls. The only allowed shell-outs are `diskutil unmountDisk` / `mountDisk` / `eject`, wrapped in `usbwin-disk::macos` with retry + error context.
5. **Test pyramid that doesn't burn USB sticks.**
   - Unit: BPB splice, MBR layout, ISO classifier on fixture bytes.
   - Golden: byte-for-byte comparison of the four boot-record-producing functions (Win 7 MBR, XP MBR, BOOTMGR multi-sector PBR, NTLDR PBR) against checked-in goldens. Lives in `pipeline::boot_records` (the `#[cfg(test)]` mod). Catches "bootrec bumped and now we produce different bytes" automatically. Goldens at `tests/golden/*.bin`; refresh with `UPDATE_GOLDENS=1 cargo test ...`. The QEMU and real-hardware layers below cover boot-behavior; this layer covers byte stability.
   - QEMU smoke: write to a disk image, boot it under qemu-system-i386, scrape serial output. *(Lives in bootrec, not usbwin — bootrec is where the byte production happens; usbwin's wrapper is what the golden tests cover.)*
   - Hardware (manual): the scenarios in [`HARDWARE_TESTS.md`](HARDWARE_TESTS.md), run before each release.

## MVP target

**Windows 7 install USB.** The Win 7 boot chain (`bootmgr` loaded by a FAT32 PBR, with `sources/install.wim` carrying the installer payload) is shared verbatim with Win 8, 10, and 11 — so one carefully-built code path covers all four versions of Windows.

**Windows XP is *not* in MVP.** XP predates the `bootmgr` + `install.wim` design; it uses `NTLDR` + `i386/` + `txtsetup.sif`, and its text-mode setup was written assuming CD/floppy media. Booting an XP installer from USB requires a Grub4DOS-style chainloader and on-the-fly `txtsetup.sif` rewriting — substantial extra work that's better added as a dedicated `--type=windows-xp` mode on top of a working Vista+ path. This is why Rufus 3.x dropped XP support; we'll add it back as a layer once the foundation is solid.

Staging:

- **v0.1** — hybrid mode (raw write for Linux/BSD ISOs). Ships the safety chokepoint, the verify pass, the macOS device layer. *Done.*
- **v0.2** — Windows 7+ mode end-to-end via `ms-sys` shell-out. Full Windows install USB pipeline. *Done; hardware-verified on Dell E6410.*
- **v0.3** — Windows XP mode (Grub4DOS-style chainloader, `txtsetup.sif` rewriter, USB-driver injection, optional `winnt.sif`). *Done.*
- **v1.0** — `bootrec` library replaces the ms-sys shell-out as the default boot-record source. usbwin's Windows 7+ and XP pipelines link bootrec in-process; ms-sys becomes an opt-in `--boot-record=ms-sys` fallback. *Done for Win 7 (hardware-verified 2026-05-19); XP path implemented, hardware verification pending.*
- **later** — Isolinux Linux mode, UEFI-only mode, ISO9660 auto-classifier.

## The pipeline

```
WritePlan (typed enum: Windows | Hybrid | IsolinuxLinux | UefiOnly)
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
