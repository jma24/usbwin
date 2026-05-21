# Tech debt

What we know is wrong but kept because something else was more urgent.
This file exists so the next refactor pass has a checklist instead of a
git-log-archaeology project.

Last updated: 2026-05-20 PM (RECOVERY_PLAN.md FiraDisk pivot — several XP
items below are *pending-obsolete* if the pivot lands).

> **Read first:** [`RECOVERY_PLAN.md`](RECOVERY_PLAN.md) is the live plan
> for unblocking XP. Several items below are flagged "pending-obsolete
> by FiraDisk pivot" — keep them documented but don't invest cleanup
> effort until the pivot is hardware-green or rejected.

## Severity legend

- 🟥 **Architectural**: design decision that's actively wrong and will
  bite anyone trying to extend the code.
- 🟧 **Behavioral**: works but the wrongness is observable (extra disk
  use, duplicated files, misleading log lines, etc.).
- 🟨 **Cosmetic**: dead code, stale comments, naming.

## XP mode

### ❌ Rename-not-replicate refactor reverted (8f68b44 reverted 2026-05-20 PM)

8f68b44 replaced the `\I386\` → `\$WIN_NT$.~BT\` `ditto` with a
`std::fs::rename` to save ~580 MB and ~30s. Looked correct through
text-mode setup (file copy, reboot) and the commit was marked
"hardware-verified on Dell E6410." It wasn't verified through
GUI-mode setup — the verification ran text-mode only.

GUI-mode XP setup walks every drive letter looking for `\I386\`
sentinel files (setupreg.hiv, layout.inf) when its primary source
path `\$WIN_NT$.~LS\I386\` is unavailable. That path is renamed to
`\WIN_NT.LS\I386\` by `ren_fold.cmd` at the text→GUI transition, so
the ~LS path doesn't survive past phase 1. With `\I386\` also gone
(renamed to ~BT), GUI-mode setup has nowhere to look → drops into
the "Insert the CD labeled Windows XP Professional Service Pack 3 CD
into your CD-ROM drive — Press ENTER when ready" prompt loop.
Pressing Enter does nothing because there's no CD; pressing it
forever is the user's only escape.

Reverted to the ditto-based `replicate_i386_to_bt`. `\I386\` stays
at the root. 30s and ~580 MB are the cost of working GUI-mode setup.

**Don't try this again** without first instrumenting setupdd's source-
discovery path. The TODO below is the only safe way to claw back this
disk space.

### 🟧 Three full I386 trees on the USB (\I386\ + ~BT + ~LS\I386\, ~1.74 GB) — *pending-obsolete by FiraDisk pivot, see RECOVERY_PLAN.md §4*

Each one is needed for a different setup phase:

- `\I386\` — GUI-mode setup's drive-letter scan target. Required for
  setupdd to find a source via the `\I386\setupreg.hiv` / `layout.inf`
  sentinel check. Cannot be eliminated until we know exactly which
  files setupdd probes and stage only those.
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
  `ren_fold.cmd` renames the USB-side `~LS` to `WIN_NT.LS` at the
  text→GUI transition so GUI-mode's boot-volume sanity check passes
  — which is *why* GUI-mode has to fall back to drive-walking `\I386\`.

Recovering ~1.16 GB (collapsing to one tree) needs a profile of what
setupdd actually opens at every phase, not derivable from DOSNET.INF
alone. Hardware-trace or kernel-debugger territory. Deferred.

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

### 🟧 SIF modifier persistence — assertion landed, root cause TBD — *pending-obsolete by FiraDisk pivot (winnt.sif lives inside ISO; no triple-copy)*

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

### 🟧 boot.ini's 2nd entry hardcodes rdisk(1) — *pending-obsolete by FiraDisk pivot (no boot.ini on USB; GRUB4DOS handles boot)*

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

### 🟧 MBR_WIN7 used for XP mode based on "side effects" never proved — *pending-obsolete by FiraDisk pivot (mkmsbr writes GRUB4DOS MBR, this question moot)*

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
