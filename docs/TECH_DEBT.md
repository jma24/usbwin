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

### 🟥 I386 replicated to $WIN_NT$.~BT (~580 MB duplication)

`pipeline::xp_staging::replicate_i386_to_bt` shells out to `ditto` to
copy the entire I386 tree (~5886 files, ~580 MB) into `$WIN_NT$.~BT/`.
The canonical WinSetupFromUSB recipe achieves the same outcome by
*byte-patching* setupldr.bin to look in `\I386\` directly — no
duplication. We attempted the patch (null padding, then space padding);
neither worked for our specific PBR + FAT-walker combination, so we
gave up and copied the directory.

What needs to happen to fix this properly:
- Identify why the WinSetupFromUSB byte patch works in their pipeline
  but not ours. Differences to investigate: the gsar-default padding
  is actually space-padding (we tried that and got the same status 18,
  so probably not it); the offset of $WIN_NT$.~BT in their setupldr.bin
  vs ours; whether their setupldr.bin is from a different XP SP/edition
  with different path-construction logic.
- Decide whether to invest in the byte-patch fix or just live with the
  duplication. 580 MB on a 64 GB stick is ~1%; the time cost is ~5
  seconds on USB 3. Honestly: probably fine to leave forever and
  document as a deliberate choice.

### 🟥 Three full I386 trees on the USB (root + ~BT + ~LS, ~1.7 GB)

After the GUI-mode CDROM-prompt fix landed, the USB now has the I386
contents replicated THREE times:

- `\I386\` — original ISO layout, ~580 MB
- `\$WIN_NT$.~BT\` — text-mode setupldr source, ~580 MB (replicated
  because we couldn't get the `\I386\`-redirect byte patch on setupldr
  to work; see "kept by design" note below)
- `\$WIN_NT$.~LS\I386\` — GUI-mode source for text-mode setup to copy
  to `C:\$WIN_NT$.~LS\I386\` on the target HDD, ~580 MB

Total cost on a 64 GB stick: ~1.7 GB, ~2.7%. Tolerable for now, but the
right end state is **one** I386 tree on the USB. Canonical
WinSetupFromUSB recipe has I386 only under `~LS` and byte-patches
setupldr to look there instead of `~BT`. We deferred the byte patch
because of FAT-walker / padding issues; revisiting it now that the rest
of the chain is verified would be a net cleanup.

Sub-item (the original TXTSETUP.SIF triple-copy): we stage TXTSETUP.SIF
at root, in I386/, AND in $WIN_NT$.~BT/. Still don't empirically know
which setupldr reads. ~480 KB × 3 is fine but the smell remains.

What needs to happen:
- Investigate the byte-patch route on setupldr again with fresh eyes
  (the gsar default is space padding; we had this on the second
  attempt and still got status 18 — root cause was likely the missing
  ~LS folder, not the patch itself). If patch works, we can stage I386
  ONCE under ~LS and delete the ~BT replication entirely.
- Empirically determine which TXTSETUP.SIF copy setupldr actually reads
  (single-file marker trick); delete the other two.

### 🟥 SIF modifier reports `moved 5` but persistence unverified

`pipeline::windows_xp_sif::apply_usb_boot_mods` returns "moved 5 USB
drivers" but `grep` on the resulting on-disk SIF shows the
`[BootBusExtenders.Load]` section without the moved entries
(2026-05-19 diagnostic). Either:

- The move modifies the in-memory `Sif` correctly but the write-back
  doesn't persist (file-permission issue? wrong path?)
- The move modifies a section that happens to share a prefix and isn't
  the one we want (e.g. `[BootBusExtenders.Load.x86]` vs `[BootBusExtenders.Load]`)
- `grep` was looking in the wrong file (less likely — we checked all
  three copies)

Until we fix this, the XP install loses USB drivers across the
post-text-mode reboot and the USB key becomes unreachable (classic 0x7B
INACCESSIBLE_BOOT_DEVICE). The user worked around with BIOS-level SATA
mode change instead of fighting it.

What needs to happen:
- Add an end-of-pipeline assertion: after `apply_usb_boot_mods` writes,
  re-read the file and verify the moved keys are in the destination
  section. Fail the pipeline if not — silent persistence failures are
  the worst kind.
- Once the assertion is in place, find the actual bug.

### 🟧 Dead code: `patch_setupldr_for_i386_lookup` and `build_bootsect_dat`

Both functions are kept in `pipeline::xp_staging` with `#[allow(dead_code)]`
and "for documentation" comments. Neither is called from the production
pipeline anymore. They were debugging artifacts.

What needs to happen:
- Once XP is verified end-to-end and we're confident in the chosen
  recipe, delete them. Their tests too. The git history is the
  documentation.
- If we want them as alternatives behind a flag, gate properly.

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
