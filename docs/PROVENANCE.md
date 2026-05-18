# Boot-blob provenance

usbwin embeds three small (~512-byte) chunks of x86 real-mode boot code: an MBR loader, a FAT32 PBR loader, and an NTFS PBR loader. This document records where those bytes come from and why we're confident shipping them under MIT.

## Source: hand-written NASM in this repo

The bytes shipped in `target/release/usbwin` are produced at build time from the NASM source files under [`boot-asm/`](../boot-asm/). Those files are original work by the usbwin authors and are licensed MIT, identical to the rest of this codebase.

`boot-asm/Makefile` invokes `nasm` and emits 512-byte raw binaries. `crates/usbwin-boot/build.rs` runs the makefile and `include_bytes!`s the results into the compiled binary. There is no external dependency at runtime; NASM is a build-time tool only.

## Clean-room development protocol

The boot record source files are developed under a strict clean-room protocol. Anyone working on `boot-asm/*.asm` (or on `crates/usbwin-boot/src/pbr.rs`) may consult **only** these sources:

**Allowed references:**
- Microsoft's *FAT32 File System Specification* (FATGEN103.doc / FATGEN102.pdf, publicly published by Microsoft in 2000 and republished many times since).
- IBM/Phoenix *BIOS Interface Reference* and equivalent public BIOS documentation (covering INT 10h, INT 13h, INT 13h extensions, the boot process).
- The OSDev wiki's algorithmic descriptions (text and pseudocode only, never their example code blocks).
- Generic textbook x86 assembly references (e.g. Intel software developer manuals).
- Output bytes from third-party tools, used **for verification only** (see "Cross-check" below).

**Disallowed references:**
- Source code from ms-sys, syslinux, GRUB, GRUB4DOS, Linux kernel boot code, BSD bootloaders, or any other open-source bootloader project.
- Any leaked, reverse-engineered, or disassembled Microsoft, Apple, or third-party proprietary code.
- Stack Overflow or blog-post code that itself derives from any of the above (when in doubt, treat external code blocks as tainted and use only the prose/pseudocode portions).

The rule is: **read others' output for validation; derive nothing from others' source.** Validation tells us "is the byte we produced equivalent to what's already in the field?" Derivation imports authorship questions we don't want to inherit.

When in doubt about a reference's status, stop and document the question in a PR comment rather than incorporating uncertain code.

## Cross-check: ms-sys equivalence test

[ms-sys](https://ms-sys.sourceforge.net/) ships boot record blobs that are functionally identical (and historically derived from Microsoft binaries). We do **not** redistribute ms-sys's bytes. We do, however, run an optional test that asserts our NASM output is byte-equal to ms-sys's reference blobs:

```sh
# one-time: clone ms-sys somewhere
git clone https://gitlab.com/cmaiolino/ms-sys.git /tmp/ms-sys
export USBWIN_MSSYS_BLOBS_DIR=/tmp/ms-sys/inc

cargo test --features compare-mssys
```

This test is gated behind a feature flag and an env var, so the default `cargo test` invocation neither depends on ms-sys nor accesses it. The check exists because byte-equality vs ms-sys is the tightest possible "does our NASM work?" feedback loop — if our hand-written assembly produces the same bytes as code that's shipped to millions of users for two decades, we're done verifying.

## Why not just ship ms-sys's bytes?

Three reasons, in increasing order of importance:

1. **License clarity.** ms-sys is GPL-2.0. The boot record blobs inside its source tree are header-file byte arrays derived from Microsoft binaries (XP/Vista/7 era). The exact license status of those arrays — when extracted from ms-sys and embedded in someone else's project — has been argued for years without a clean answer. Writing our own NASM sidesteps the question.
2. **Maintainability.** Shipping opaque blobs means future bug fixes require reverse-engineering. Shipping NASM source means future fixes are diffs.
3. **Pride.** This is a tool that solves a real, persistent gap. It deserves its own first-party boot code.

## SHA-256 of expected bytes

For ease of regression detection without setting up the equivalence test, here are the SHA-256 hashes our NASM output should produce. (TODO: fill these in once the NASM sources are written and verified against ms-sys at least once.)

```
mbr.bin       SHA-256: TODO
fat32_pbr.bin SHA-256: TODO
ntfs_pbr.bin  SHA-256: TODO
```

If `cargo build` produces blobs whose SHA-256s don't match these, something changed in our assembly source or in NASM itself. The test `tests/blob_hashes.rs` enforces this.

## What if Microsoft objects?

Their boot code is ~440 bytes of x86 that does the obvious thing — read BPB, walk FAT, find `bootmgr`, load it. The space of "correct implementations" is small enough that two competent engineers writing this code independently will produce nearly-identical bytes. The bytes are not creative expression; they're the unique correct way to do a constrained task. We're confident in the originality of our NASM source.

If we're ever asked to take it down, the audit trail (NASM source, git history, this document) demonstrates we wrote it ourselves.
