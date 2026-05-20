# Tech debt

What we know is wrong but kept because something else was more urgent.
This file exists so the next refactor pass has a checklist instead of a
git-log-archaeology project.

Last updated: 2026-05-19 (after the XP GUI-mode-CDROM-prompt fix landed and
hardware-verified end-to-end on the E6410).

## Severity legend

- 🟥 **Architectural**: design decision that's actively wrong and will
  bite anyone trying to extend the code.
- 🟧 **Behavioral**: works but the wrongness is observable (extra disk
  use, duplicated files, misleading log lines, etc.).
- 🟨 **Cosmetic**: dead code, stale comments, naming.

## XP mode

### ✅ I386 replicated to $WIN_NT$.~BT (resolved 2026-05-20, commit 8f68b44)

Was `replicate_i386_to_bt` doing a ~580 MB `ditto`. Now `move_i386_to_bt`
does a FAT32 directory-entry rename `\I386\` → `\$WIN_NT$.~BT\` —
instant, no I/O. Setupldr finds the same files at the same path.
Hardware-verified on Dell E6410 (no BSOD, no status-18).

### 🟧 Two full I386 trees on the USB (~BT + ~LS\I386\, ~1.16 GB)

Was three trees (~1.74 GB); 2026-05-20 work brought it to two (rename
+ ISO-root trim). The remaining duplication: `\$WIN_NT$.~BT\` and
`\$WIN_NT$.~LS\I386\` are byte-identical clones of the I386 tree.

- `\$WIN_NT$.~BT\` — text-mode setupldr source. **Required as a full
  mirror** — setupldr-byte-patch attempts (`I386` + 8 spaces, and
  `$WIN_NT$.~LS`) both BSOD'd at PROCESS1_INITIALIZATION_FAILED
  0x6B / 0xC000003A because the patch only touches setupldr.bin; the
  setupdd.sys it loads still reads source paths verbatim and produces
  a broken SYSTEM hive. A slim-BT variant (only DOSNET.INF
  `[FloppyFiles.0..3]`) failed for the same reason — setupdd reads
  HIVE\*.INF and other non-FloppyFiles entries from ~BT at runtime.
- `\$WIN_NT$.~LS\I386\` — GUI-mode source, copied to
  `C:\$WIN_NT$.~LS\I386\` on the target HDD by text-mode setup.

Recovering the remaining ~580 MB needs a profile of what setupdd
actually opens from `~BT` during text-mode (not derivable from
DOSNET.INF alone). Hardware-trace or kernel-debugger territory.
Deferred.

Sub-item (the original TXTSETUP.SIF triple-copy): we stage TXTSETUP.SIF
at root, in `\$WIN_NT$.~BT\` (from the rename), AND in
`\$WIN_NT$.~LS\I386\`. Still don't empirically know which setupldr
reads. ~480 KB × 3 is fine but the smell remains.

What needs to happen:
- Empirically determine which TXTSETUP.SIF copy setupldr actually reads
  (single-file marker trick); delete the other two.
- For the bigger ~580 MB win: instrument setupdd via debugger or QEMU
  trace to identify the file set it opens from ~BT, then stage exactly
  that subset.

### 🟧 SIF modifier persistence — assertion landed, root cause TBD

**Status (2026-05-20):** post-write + post-staging assertions added in
`pipeline::windows_xp_sif::verify_usb_boot_mods_persisted` and
`pipeline::windows_xp::verify_all_sif_copies`. The pipeline now re-reads
each of the three on-disk TXTSETUP.SIF copies and asserts the moved
drivers are in `[BootBusExtenders.Load]` and absent from
`[InputDevicesSupport.Load]`. Three new unit tests cover the verifier
(well-formed accept, unmoved reject, partial-persistence reject) and
all 41 workspace tests still pass.

**Original report (2026-05-19):** `apply_usb_boot_mods` returned "moved
5 USB drivers" but a `grep` on the resulting on-disk SIF allegedly
showed `[BootBusExtenders.Load]` without the moved entries. Research
2026-05-20 (worktree review of the fixture, section names, and pipeline
ordering) couldn't reproduce: the unit test against the real XP SP3
fixture confirms the in-memory transform is correct and the disk
write/copy/ditto chain has no obvious overwrites. Most likely the
original grep targeted the wrong file or the wrong section.

**Next:** if the new assertion ever fires on a real hardware burn, we
have hard evidence (the error names the file and what's missing). If
it never fires across the next few installs, the symptom was a
diagnostic artefact and this item closes.

### ✅ Dead code: `patch_setupldr_for_i386_lookup` (resolved 2026-05-20, commit 8f68b44)

Deleted along with its tests. `build_bootsect_dat` remains in use as
the PBR-patch fallback for the `--boot-record=ms-sys` path; not dead.

### 🟧 boot.ini's 2nd entry hardcodes rdisk(1)

`pipeline::xp_staging::BOOT_INI` second entry is
`multi(0)disk(0)rdisk(1)partition(1)\WINDOWS="2nd, GUI mode setup"`.
This assumes the user's target HDD enumerates as `rdisk(1)` — true on
most single-HDD machines where USB is `rdisk(0)`. On rigs with multiple
internal disks, this is wrong.

The bigger issue: on rigs that already have a Windows install on
`rdisk(1)`, selecting this entry tries to boot the wrong Windows and
produces a misleading `hal.dll missing` error. This wasted ~30 minutes
of debugging on the Dell E6410 test rig (which had an existing Win 7).

What needs to happen:
- Either drop the 2nd entry entirely (the user runs setup manually on
  the post-text-mode reboot), or make it a non-existent path that
  fails fast with a clear "device not found" error.
- Document the 2nd-entry's purpose in the comment if kept.

### 🟧 `-c 8` 4 KiB cluster forcing without root-cause understanding

`pipeline::diskutil::newfs_msdos_fat32` passes `-c 8` to force 4 KiB
clusters. We did this because the research agent flagged that XP
setupldr "doesn't tolerate" 32 KiB clusters on >32 GiB partitions. We
never verified the actual failure mode — the symptom we were chasing
(status 18) turned out to have other causes (boot.ini syntax, missing
files in $WIN_NT$.~BT). The cluster fix may or may not be necessary.

What needs to happen:
- Test with 32 KiB clusters (omit `-c 8`) to see if status 18 returns.
- If yes, document the precise failure mode. If no, remove `-c 8`.

### 🟧 MBR_WIN7 used for XP mode based on "side effects" never proved

`pipeline::windows_xp::write_mbr_sector` uses `build_mbr_win7` instead
of `build_mbr_xp`, based on the theory that MBR_WIN7's register-state
side effects might be needed downstream. We never proved this matters.

What needs to happen:
- Test with `build_mbr_xp` to see if XP boot chain still completes.
- If yes, revert to MBR_XP for symmetry (XP mode → XP MBR).
- If no, document what specific side effect is required.

## Win 7 mode

### 🟨 ms-sys backend untested with `--fat32nt` on E6410

`--boot-record=ms-sys` for Win 7 mode (which uses `ms-sys --mbr7`
+ `ms-sys --fat32pe`) is hardware-verified. The XP-side `ms-sys
--fat32nt` is NOT hardware-verified on the E6410 — it hangs at a
flashing cursor (see commit `7dfdf35` body). Unknown if it's an
ms-sys-side issue, a hardware-quirk issue, or something we'd see if we
dug into it.

## Cross-cutting

### 🟧 Pipeline error reporting hides actual write errors

Several places in `windows_xp.rs` and `xp_staging.rs` use `.context()`
chains that produce useful messages but don't always surface the
underlying `io::Error` clearly (especially permission errors on FAT32
mounts, which can be subtle). When the user reports "weird hang" or
"silent fail" we often need a second diagnostic run.

What needs to happen:
- Pass through underlying errors via `anyhow!("{:#}", err)` style
  formatting that shows the full chain, not just the top context.
- Add a `-v` / `RUST_LOG=usbwin=debug` mode that shows every file
  operation as it happens.
