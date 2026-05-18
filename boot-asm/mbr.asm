; mbr.asm — usbwin Master Boot Record.
;
; Loaded by BIOS at 0000:7C00 in real mode. The BIOS hands us:
;   DL = boot drive number (e.g. 0x80 for first hard disk / USB stick)
;
; Job:
;   1. Relocate self from 0000:7C00 to 0000:0600 (so we can load the
;      target PBR at 0000:7C00 without overwriting ourselves).
;   2. Find the active primary partition in the partition table at
;      offset 0x1BE..0x1FE of this sector.
;   3. Read its first sector (the PBR) to 0000:7C00 using INT 13h
;      extended read (function 42h, LBA addressing).
;   4. Pass DL = boot drive unchanged to the PBR.
;   5. Far-jump to 0000:7C00 to chain-load the PBR.
;
; Output: exactly 512 bytes. Bytes 446..509 are the partition table
; (zeroed by nasm; written by usbwin-boot at install time). Bytes
; 510..511 = 0x55 0xAA.

BITS 16
ORG 0x7C00

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00          ; stack just below the load address
    sti

    ; Save boot drive number; we'll need it for INT 13h reads and to
    ; pass to the PBR.
    mov [boot_drive], dl

    ; Relocate ourselves to 0000:0600 so the loaded PBR can occupy 0000:7C00.
    cld
    mov si, 0x7C00
    mov di, 0x0600
    mov cx, 256             ; 256 words = 512 bytes
    rep movsw
    jmp 0x0000:relocated

relocated:
    ; Find the active partition entry. Partition table starts at
    ; relocated_base + 0x1BE = 0x600 + 0x1BE = 0x07BE.
    mov si, 0x07BE
    mov cx, 4               ; four 16-byte primary partition entries
.scan:
    cmp byte [si], 0x80     ; active flag
    je .found
    add si, 16
    loop .scan

    ; No active partition: print and halt.
    mov si, msg_no_active
    call print
    jmp halt

.found:
    ; SI -> active partition entry. Bytes 8..11 = LBA start (little-endian).
    ; Build a disk address packet for INT 13h extended read.
    push si
    mov si, dap
    mov word [si + 0], 0x10         ; packet size
    mov word [si + 2], 1            ; sectors to read = 1 (just the PBR)
    mov word [si + 4], 0x7C00       ; dest offset
    mov word [si + 6], 0x0000       ; dest segment
    pop bx                          ; partition entry pointer
    mov ax, [bx + 8]
    mov [si + 8], ax                ; LBA low 16
    mov ax, [bx + 10]
    mov [si + 10], ax               ; LBA next 16
    mov word [si + 12], 0           ; LBA bits 32..47
    mov word [si + 14], 0           ; LBA bits 48..63

    mov dl, [boot_drive]
    mov ah, 0x42                    ; extended read
    int 0x13
    jc .io_error

    ; Check the loaded sector's boot signature.
    cmp word [0x7C00 + 510], 0xAA55
    jne .bad_signature

    ; Hand off to the PBR. DL still holds the boot drive number.
    mov dl, [boot_drive]
    jmp 0x0000:0x7C00

.io_error:
    mov si, msg_io_err
    call print
    jmp halt

.bad_signature:
    mov si, msg_no_sig
    call print
    jmp halt

; print: SI -> NUL-terminated string. Uses BIOS teletype (INT 10h, AH=0Eh).
print:
.loop:
    lodsb
    or al, al
    jz .done
    mov ah, 0x0E
    mov bx, 0x0007          ; page 0, attribute 7
    int 0x10
    jmp .loop
.done:
    ret

halt:
    cli
.h:
    hlt
    jmp .h

; Data
boot_drive:   db 0
msg_no_active: db 'No active partition', 13, 10, 0
msg_io_err:    db 'IO error reading PBR', 13, 10, 0
msg_no_sig:    db 'Bad boot signature', 13, 10, 0

dap_pad:
    ; Disk address packet lives at the end of the code area, before the
    ; partition table. We assemble it here so it doesn't collide with
    ; anything else.

dap:
    times 16 db 0

; Pad to the partition table location (offset 0x1BE = 446).
    times 446 - ($ - $$) db 0

; Partition table: 4 × 16-byte entries, zeroed. usbwin-boot writes the
; real partition entries during pipeline execution.
    times 64 db 0

; Boot signature.
    dw 0xAA55
