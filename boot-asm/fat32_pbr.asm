; fat32_pbr.asm — usbwin FAT32 Partition Boot Record.
;
; STUB. Real implementation pending. The contract:
;
;   1. Loaded by the MBR at 0000:7C00 in real mode.
;   2. Bytes 0..2 are the initial JMP — leave 3..89 untouched after assembly
;      because the *runtime* BPB lives there (written by newfs_msdos, spliced
;      in by usbwin-boot::pbr::splice_fat32_pbr). Our boot code must read it,
;      not hardcode it.
;   3. Find the FAT32 root directory using the runtime BPB.
;   4. Walk the root directory to find "BOOTMGR    " (FAT 8.3 form).
;   5. Walk the FAT to enumerate cluster chains and read bootmgr into memory.
;   6. Jump to the loaded bootmgr image.
;   7. Total binary size: exactly 512 bytes, with 0x55 0xAA at offset 510.
;
; Reference: docs/BOOT_RECORDS.md, especially the "preserve the BPB" rule.

BITS 16
ORG 0x7C00

start:
    jmp short body
    nop

    ; Bytes 3..89 reserved for the BPB. NASM emits zeros here; the splice
    ; logic in usbwin-boot replaces these bytes with the partition's actual
    ; BPB before the sector is written back to disk.
    times 87 db 0

body:
    cli
    xor ax, ax
    mov ds, ax
    mov ss, ax
    mov sp, 0x7C00
    sti

    ; STUB body: halt forever. Real implementation pending.
.halt:
    hlt
    jmp .halt

    times 510 - ($ - $$) db 0
    dw 0xAA55
