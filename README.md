# usbwin

**This is the tool Rufus refuses to be.**

A native macOS arm64 CLI for writing Windows install USB sticks — Windows
2000, XP, and Windows 7 — without Rosetta, without a Windows VM, without
Boot Camp Assistant.

## Status

**Alpha.** The Windows 7 and XP paths are hardware-verified. The 1.0 scope
is Windows 2000/XP/7 with unattended install support and XP/2000-era AHCI
textmode storage drivers.

| Mode | State |
|------|-------|
| Hybrid (Linux/BSD ISO raw write) | ✅ working since v0.1; maintained as a utility path, not part of the v1 Windows scope. |
| Windows 7+ (BOOTMGR chain) | ✅ hardware-verified on Dell E6410 (2026-05-19) with both `--boot-record=bootrec` (default) and `--boot-record=ms-sys`. Same code path covers Win 8/10/11. |
| Windows NT/XP (`windows-ntxp`) | ✅ GRUB4DOS + FiraDisk production path hardware-verified end-to-end on Dell E6410 (2026-05-21). |
| Windows 2000 | 1.0 target; auto-detection exists, install support still pending. |
| XP/2000 unattended installs | 1.0 target. |
| XP/2000 AHCI textmode storage | 1.0 target. |
| Linux/isolinux | deferred; not a v1 goal. |
| UEFI-only | deferred; not a v1 goal. |

The MBR + FAT32 PBR bytes come from the sibling [`mkmsbr`](https://github.com/jma24/mkmsbr) library (renamed from `bootrec` 2026-05-19) by default, linked in-process — no external `ms-sys` binary required. The legacy `ms-sys` shell-out is still available as `--boot-record=ms-sys` for byte-equality auditing of Win 7 mode. usbwin imports it as `bootrec::*` via a Cargo `package = "mkmsbr"` alias for source-compat.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the design, [`docs/XP_FIRADISK_PIPELINE.md`](docs/XP_FIRADISK_PIPELINE.md) for the active XP recipe, and [`docs/BACKLOG.md`](docs/BACKLOG.md) for release blockers and follow-up work.

## Why

- UNetbootin requires Rosetta.
- Rufus is Windows-only.
- Boot Camp Assistant was removed on Apple Silicon.
- `dd` works for hybrid ISOs but silently produces a non-bootable Windows USB.
- Generic ISO writing is already well-served by `dd` and other tools; old
  Windows installer media is the awkward gap.

There is currently no native macOS arm64 binary that writes a bootable
Windows 2000/XP/7 install USB. This is that binary.

## Install

```sh
# Build from source (requires Rust stable + NASM for mkmsbr's NASM blobs)
brew install nasm
git clone https://github.com/jma24/mkmsbr ../mkmsbr   # path dep — see Cargo.toml
git clone https://github.com/jma24/usbwin
cd usbwin
cargo build --release
sudo cp target/release/usbwin /usr/local/bin/
```

The `embed-boot-asm` feature on `bootrec` is enabled by usbwin's `Cargo.toml`,
so a plain `cargo build --release` is enough — no extra flags.

### Optional: ms-sys fallback

By default, usbwin uses the in-process `bootrec` library for MBR and FAT32
PBR bytes. If you want byte-for-byte equivalence with the upstream tool
(useful for auditing or comparison against a known-good Win 7 USB), pass
`--boot-record=ms-sys` and install ms-sys once:

```sh
git clone https://gitlab.com/cmaiolino/ms-sys.git /tmp/ms-sys
cd /tmp/ms-sys && make
sudo cp bin/ms-sys /usr/local/bin/
# Or without sudo: export USBWIN_MS_SYS=/tmp/ms-sys/bin/ms-sys
```

Hybrid mode (Linux/BSD ISOs) does not touch the boot-record path at all.

Notarized signed binaries via GitHub Releases: TODO.

## Test prerequisites

The default `cargo test` only needs Rust. Boot-record-level integration tests
(QEMU smoke, ms-sys byte-equality) live in the [bootrec](https://github.com/jma24/bootrec)
repo — run them there.

## Usage

```sh
usbwin <iso-path> <device>
       [--type=auto|windows|windows-ntxp|linux|hybrid|uefi]
       [--label=<volume-label>]
       [--boot-record=bootrec|ms-sys]
       [--dry-run]
       [--force]
       [--verbose]
       [--no-verify]
```

Examples:

```sh
sudo usbwin Win7_SP1.iso /dev/disk8
sudo usbwin ubuntu-22.04.iso /dev/disk8 --type=hybrid
sudo usbwin winxp_sp3.iso /dev/rdisk6 --type=windows-ntxp
usbwin --dry-run Win7_SP1.iso /dev/disk8     # no sudo needed; emits bytes to /tmp
```

For NT5-class Windows 2000/XP/2003 media, `--type=auto` resolves to the
newer `windows-ntxp` GRUB4DOS + FiraDisk path. `--type=windows-xp` remains
accepted as a compatibility alias for `windows-ntxp`. Windows 2000 media is
recognized as NT5-class by auto-detect, but Win2k install support is still
1.0 work.

## Safety

usbwin will **refuse** to write to:

- The boot disk
- Any disk flagged `internal: true` by DiskArbitration
- Disks larger than 256 GiB without `--force`

Every write is verified by re-reading and byte-comparing unless `--no-verify` is passed.

## License

MIT. See [`LICENSE`](LICENSE).

Boot record source code lives in the separate [`bootrec`](https://github.com/jma24/bootrec) repo, with its own clean-room provenance trail.
