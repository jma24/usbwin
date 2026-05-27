# bootsmith

**This is the tool Rufus refuses to be.**

A native macOS arm64 CLI for writing Windows install USB sticks — Windows
XP and Windows 7 — without Rosetta, without a Windows VM, without
Boot Camp Assistant.

## Status

**1.0 — released.** Windows XP and Windows 7 are hardware-verified end-to-end
(XP unattended installs and XP-era AHCI textmode storage included). Caveat:
verification so far is on a single test machine (Dell E6410) — treat
reliability on other hardware as unproven until the test matrix widens. The
1.0 scope is Windows XP and Windows 7; Windows 2000 is deferred to 1.1
(text-mode install works, but first boot needs a `boot.ini` repair — see
[`docs/BACKLOG.md`](docs/BACKLOG.md)). Remaining 1.0 work is release
packaging.

| Mode | State |
|------|-------|
| Hybrid (Linux/BSD ISO raw write) | ✅ working since v0.1; maintained as a utility path, not part of the v1 Windows scope. |
| Windows 7+ (BOOTMGR chain) | ✅ hardware-verified on Dell E6410 with both `--boot-record=bootrec` (default) and `--boot-record=ms-sys`; SP1 regression re-run green 2026-05-26. Same code path covers Vista and Win 8/10/11 — auto-classifier routes any ISO with `\bootmgr` + `\sources\install.wim` here. Vista hardware-verified on E6410 (2026-05-26). |
| Windows NT/XP (`windows-ntxp`) | ✅ GRUB4DOS + FiraDisk production path hardware-verified end-to-end on Dell E6410 (2026-05-21). |
| Windows 2000 | ⏳ deferred to 1.1. Text-mode install hardware-verified on Dell E6410 (2026-05-22) via GRUB4DOS + SVBus; first boot needs a manual `boot.ini` `rdisk(1)→rdisk(0)` repair. See [`docs/WIN2K_SVBUS.md`](docs/WIN2K_SVBUS.md). |
| XP unattended installs | ✅ hardware-verified on Dell E6410 — `--unattended` injects `WINNT.SIF` (product key, computer name, timezone, admin password, EULA). Win2k unattended tracks the 1.1 Win2k work. |
| XP AHCI textmode storage | ✅ hardware-verified on Dell E6410 (2026-05-26) with BIOS SATA mode set to AHCI and the Dell `R274723` (Intel iaStor 9.6.4.1002) driver pack. `--ahci-driver-dir <path>` slipstreams a BYO vendor F6 driver pack into the staged XP.ISO's I386 directory and patches `TXTSETUP.SIF`/`DOSNET.INF` so XP treats the driver as inbox. See [`docs/AHCI_DRIVER.md`](docs/AHCI_DRIVER.md). |
| Linux/isolinux | deferred; not a v1 goal. |
| UEFI-only | deferred; not a v1 goal. |

The MBR + FAT32 PBR bytes come from the published [`mkmsbr`](https://github.com/jma24/mkmsbr) crate by default, linked in-process — no external `ms-sys` binary required. The legacy `ms-sys` shell-out is still available as `--boot-record=ms-sys` for byte-equality auditing of Win 7 mode. bootsmith imports it as `bootrec::*` internally via a Cargo `package = "mkmsbr"` alias.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the design, [`docs/XP_FIRADISK_PIPELINE.md`](docs/XP_FIRADISK_PIPELINE.md) for the active XP recipe, and [`docs/BACKLOG.md`](docs/BACKLOG.md) for release blockers and follow-up work.

## Why

- UNetbootin requires Rosetta.
- Rufus is Windows-only.
- Boot Camp Assistant was removed on Apple Silicon.
- `dd` works for hybrid ISOs but silently produces a non-bootable Windows USB.
- Generic ISO writing is already well-served by `dd` and other tools; old
  Windows installer media is the awkward gap.

There is currently no native macOS arm64 binary that writes a bootable
Windows XP/7 install USB. This is that binary.

## Install

```sh
# Homebrew
brew install jma24/bootsmith/bootsmith

# Or from crates.io (requires Rust stable)
cargo install bootsmith

# Or build from source
git clone https://github.com/jma24/bootsmith
cd bootsmith
cargo build --release
sudo cp target/release/bootsmith /usr/local/bin/
```

The published `mkmsbr` crate is pulled by Cargo automatically, so a plain
`cargo build --release` is enough — no sibling checkout or local bootloader
repo is required.

### Optional: ms-sys fallback

By default, bootsmith uses the in-process `bootrec` library for MBR and FAT32
PBR bytes. If you want byte-for-byte equivalence with the upstream tool
(useful for auditing or comparison against a known-good Win 7 USB), pass
`--boot-record=ms-sys` and install ms-sys once:

```sh
git clone https://gitlab.com/cmaiolino/ms-sys.git /tmp/ms-sys
cd /tmp/ms-sys && make
sudo cp bin/ms-sys /usr/local/bin/
# Or without sudo: export BOOTSMITH_MS_SYS=/tmp/ms-sys/bin/ms-sys
```

Hybrid mode (Linux/BSD ISOs) does not touch the boot-record path at all.

Notarized signed binaries via GitHub Releases: TODO.

## Test prerequisites

The default `cargo test` only needs Rust. Boot-record-level integration tests
(QEMU smoke, ms-sys byte-equality) live in the upstream boot-record crate
repo — run them there.

## Usage

```sh
bootsmith <iso-path> <device>
       [--type=auto|windows|windows-ntxp|windows-2000|linux|hybrid|uefi]
       [--label=<volume-label>]
       [--boot-record=bootrec|ms-sys]
       [--unattended] [--product-key=...] [--admin-password=...] ...
       [--ahci-driver-dir=<f6-folder>]
       [--dry-run]
       [--force]
       [--verbose]
       [--no-verify]
```

Examples:

```sh
sudo bootsmith Win7_SP1.iso /dev/disk8
sudo bootsmith ubuntu-22.04.iso /dev/disk8 --type=hybrid
sudo bootsmith winxp_sp3.iso /dev/rdisk6 --type=windows-ntxp
bootsmith --dry-run Win7_SP1.iso /dev/disk8     # no sudo needed; emits bytes to /tmp
```

For NT5-class Windows 2000/XP/2003 media, `--type=auto` resolves to the
newer `windows-ntxp` GRUB4DOS + FiraDisk path. `--type=windows-xp` remains
accepted as a compatibility alias for `windows-ntxp`. Windows 2000 media is
recognized as NT5-class by auto-detect and has an experimental
`--type=windows-2000` path, but full Win2k install support is deferred to 1.1.

## Safety

bootsmith will **refuse** to write to:

- The boot disk
- Any disk flagged `internal: true` by DiskArbitration
- Disks larger than 256 GiB without `--force`

Every write is verified by re-reading and byte-comparing unless `--no-verify` is passed.

## License

MIT. See [`LICENSE`](LICENSE).

Boot record source code lives in the separate [`mkmsbr`](https://github.com/jma24/mkmsbr) repo, with its own clean-room provenance trail.
