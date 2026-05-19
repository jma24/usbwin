# usbwin

**This is the tool Rufus refuses to be.**

A native macOS arm64 CLI for writing bootable USB sticks from any ISO — Windows (7 through 11), Linux/BSD (hybrid or isolinux), or UEFI-only — without Rosetta, without a Windows VM, without Boot Camp Assistant.

## Status

Pre-alpha. The repo exists. **MVP target is Windows 7 install USB** (the Win 7 boot chain is shared with Win 8/10/11, so one code path covers all four). Hybrid Linux/BSD mode ships alongside as v0.1. Isolinux Linux, UEFI-only, and a dedicated **Windows XP mode** (Grub4DOS-style chainloader + `txtsetup.sif` handling, separate from the Vista+ path) come later.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the design. Boot-record assembly lives in the separate [`bootrec`](https://github.com/jma24/bootrec) library — including the most important technical detail (why we splice the FAT32 PBR instead of replacing it, to preserve the BPB).

## Why

- UNetbootin requires Rosetta.
- Rufus is Windows-only.
- Boot Camp Assistant was removed on Apple Silicon.
- `dd` works for hybrid ISOs but silently produces a non-bootable Windows USB.

There is currently no native macOS arm64 binary that writes a bootable Windows install USB. This is that binary.

## Install

```sh
# Build from source (requires Rust stable + NASM for the bootrec build step)
brew install nasm
git clone https://github.com/jma24/bootrec ../bootrec   # path dep — see Cargo.toml
git clone https://github.com/jmappleby/usbwin
cd usbwin
cargo build --release --features bootrec/embed-boot-asm
sudo cp target/release/usbwin /usr/local/bin/
```

### Windows-mode dependency

For `--type=windows` (Win 7/8/10/11 install USBs), v0.2 shells out to `ms-sys`
for the boot-record bytes (the clean-room PBR is bootrec v1.0 work). One-time
setup:

```sh
git clone https://gitlab.com/cmaiolino/ms-sys.git /tmp/ms-sys
cd /tmp/ms-sys && make
sudo cp bin/ms-sys /usr/local/bin/
```

Or without sudo: `export USBWIN_MS_SYS=/tmp/ms-sys/bin/ms-sys` in your shell.

Hybrid mode (Linux/BSD ISOs) needs no ms-sys.

Notarized signed binaries via GitHub Releases: TODO.

## Test prerequisites

The default `cargo test` only needs Rust. Boot-record-level integration tests
(QEMU smoke, ms-sys byte-equality) live in the [bootrec](https://github.com/jma24/bootrec)
repo — run them there.

## Usage

```sh
usbwin <iso-path> <device>
       [--type=auto|windows|linux|hybrid|uefi]
       [--label=<volume-label>]
       [--dry-run]
       [--force]
       [--verbose]
       [--no-verify]
```

Examples:

```sh
sudo usbwin Win7_SP1.iso /dev/disk8
sudo usbwin ubuntu-22.04.iso /dev/disk8 --type=hybrid
usbwin --dry-run Win7_SP1.iso /dev/disk8     # no sudo needed; emits bytes to /tmp
```

## Safety

usbwin will **refuse** to write to:

- The boot disk
- Any disk flagged `internal: true` by DiskArbitration
- Disks larger than 256 GiB without `--force`

Every write is verified by re-reading and byte-comparing unless `--no-verify` is passed.

## License

MIT. See [`LICENSE`](LICENSE).

Boot record source code lives in the separate [`bootrec`](https://github.com/jma24/bootrec) repo, with its own clean-room provenance trail.
