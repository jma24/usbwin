; fat32_pbr.asm — usbwin FAT32 Partition Boot Record.
;
; Loaded by the MBR at 0000:7C00 in real mode, with DL = boot drive.
;
; Layout:
;   bytes  0..2   = JMP short body / NOP
;   bytes  3..89  = BPB (RUNTIME data; preserved by usbwin_boot::pbr::
;                   splice_fat32_pbr from the freshly-formatted partition)
;   bytes 90..509 = boot code
;   bytes 510..511= 0x55 0xAA
;
; Algorithm:
;   1. Parse BPB -> FAT_LBA, DATA_LBA
;   2. Walk root directory ONE SECTOR AT A TIME, scanning each for
;      "BOOTMGR    " (the 11-byte 8.3 form). Reading a whole cluster at
;      once into a single-sector buffer corrupts our own boot code when
;      sec_per_clus > 1 (real Win 7 USBs use sec_per_clus = 64).
;   3. When found, walk BOOTMGR's cluster chain, loading to 2000:0000.
;      Each sector advances ES by 0x20 (= 512 bytes linear) so writes
;      stride forward through memory without DI wrapping.
;   4. Far-jump to 2000:0000 with DL = boot drive.
;
; Clean-room: written from FAT32 spec (FATGEN103) + Phoenix BIOS docs.
; See docs/PROVENANCE.md.

BITS 16
ORG 0x7C00

%define BPB_BytsPerSec   0x0B
%define BPB_SecPerClus   0x0D
%define BPB_RsvdSecCnt   0x0E
%define BPB_NumFATs      0x10
%define BPB_HiddSec      0x1C
%define BPB_FATSz32      0x24
%define BPB_RootClus     0x2C

%define BUF              0x0500       ; 1-sector scratch (max 512 bytes)
%define DAP              0x0700       ; disk address packet
%define BOOT_DRV         0x7B00       ; byte
%define DATA_LBA         0x7B04       ; dword
%define FAT_LBA          0x7B08       ; dword
%define READ_LBA         0x7B0C       ; dword: LBA arg to read_one_sector
%define BOOTMGR_SEG      0x2000

start:
    jmp short body
    nop
    times 87 db 0                     ; BPB placeholder

body:
    cli
    xor ax, ax
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov sp, 0x7C00
    sti
    cld
    mov [BOOT_DRV], dl

    ; FAT_LBA = HiddSec + RsvdSecCnt
    mov eax, [0x7C00 + BPB_HiddSec]
    movzx ebx, word [0x7C00 + BPB_RsvdSecCnt]
    add eax, ebx
    mov [FAT_LBA], eax

    ; DATA_LBA = FAT_LBA + NumFATs * FATSz32
    mov cl, [0x7C00 + BPB_NumFATs]
    mov ebx, [0x7C00 + BPB_FATSz32]
.dmul:
    add eax, ebx
    dec cl
    jnz .dmul
    mov [DATA_LBA], eax

    ; Walk root directory, EAX = current cluster.
    mov eax, [0x7C00 + BPB_RootClus]

.dir_cluster:
    ; Compute LBA of cluster's first sector + count of sectors in cluster.
    push eax                          ; save dir cluster
    sub eax, 2
    movzx ecx, byte [0x7C00 + BPB_SecPerClus]
    push ecx                          ; save sec_per_clus
    mul ecx
    add eax, [DATA_LBA]
    mov [READ_LBA], eax
    pop ecx                           ; ECX = sectors remaining in cluster

.dir_sector:
    push cx                           ; save sectors-remaining (2B)
    xor bx, bx
    mov es, bx
    mov di, BUF
    call read_one_sector
    mov si, BUF
    mov cx, 16                        ; 16 entries per 512-byte sector
.scan:
    mov al, [si]
    test al, al
    jz .nf                            ; end-of-dir marker; stack imbalance OK (we halt)
    cmp al, 0xE5
    je .skip
    cmp byte [si + 11], 0x0F
    je .skip
    push si
    push cx
    mov di, name
    mov cx, 11
    repe cmpsb
    pop cx
    pop si
    je .found
.skip:
    add si, 32
    dec cx
    jnz .scan

    ; Sector exhausted; advance to next sector in cluster.
    pop cx                            ; restore sectors-remaining
    inc dword [READ_LBA]
    dec cx
    jnz .dir_sector

    ; Cluster exhausted; follow chain.
    pop eax                           ; dir cluster
    call next_cluster
    cmp eax, 0x0FFFFFF8
    jb .dir_cluster
    ; Walked off end of root without finding BOOTMGR; stack balanced here.
.nf:
    mov al, '1'                       ; error code 1: BOOTMGR not in root
    jmp die

.found:
    ; SI -> dir entry. Stack has [dir_cluster (4B), sectors-remaining (2B)].
    ; Discard them before reusing the stack in .load.
    add sp, 6

    ; cluster = (high<<16) | low
    movzx eax, word [si + 26]
    movzx ebx, word [si + 20]
    shl ebx, 16
    or eax, ebx

    ; Load BOOTMGR cluster chain. ES advances 0x20 per sector (= 512 bytes
    ; in linear address); DI stays 0. This keeps us striding forward
    ; through memory without DI wrap.
    mov bx, BOOTMGR_SEG
    mov es, bx
    xor di, di
.load:
    push eax
    call read_cluster
    pop eax
    call next_cluster
    cmp eax, 0x0FFFFFF8
    jb .load

    mov dl, [BOOT_DRV]
    jmp BOOTMGR_SEG:0x0000

; ----- read_cluster: EAX = cluster, ES:DI = dest (DI must be 0).
;       Reads sec_per_clus sectors. ES advances by 0x20 per sector so we
;       stride forward through memory without overflowing DI.
read_cluster:
    sub eax, 2
    movzx ecx, byte [0x7C00 + BPB_SecPerClus]
    mul ecx                            ; EDX:EAX = sector offset
    add eax, [DATA_LBA]
    mov [READ_LBA], eax
.lr:
    push cx
    call read_one_sector
    inc dword [READ_LBA]
    mov bx, es
    add bx, 0x20                       ; advance ES by 512 bytes (linear)
    mov es, bx
    pop cx
    loop .lr
    ret

; ----- read_one_sector: [READ_LBA] -> ES:DI. Clobbers AX, SI.
read_one_sector:
    mov si, DAP
    mov byte [si], 0x10
    mov byte [si + 1], 0
    mov word [si + 2], 1
    mov [si + 4], di
    mov ax, es
    mov [si + 6], ax
    mov eax, [READ_LBA]
    mov [si + 8], eax
    mov dword [si + 12], 0
    mov dl, [BOOT_DRV]
    mov ah, 0x42
    int 0x13
    jc .err
    ret
.err:
    mov al, '2'                       ; error code 2: INT 13h read failed
    jmp die

; ----- next_cluster: EAX in -> EAX = FAT[in] & 0x0FFFFFFF.
next_cluster:
    push ecx
    push edx
    push es
    push di

    shl eax, 2                         ; cluster*4 = byte offset in FAT
    movzx ecx, word [0x7C00 + BPB_BytsPerSec]
    xor edx, edx
    div ecx                            ; EAX = sector, EDX = byte
    add eax, [FAT_LBA]
    mov [READ_LBA], eax

    xor bx, bx
    mov es, bx
    mov di, BUF
    push edx
    call read_one_sector
    pop edx
    mov eax, [BUF + edx]
    and eax, 0x0FFFFFFF

    pop di
    pop es
    pop edx
    pop ecx
    ret

; die: AL = single-character error code to print via BIOS teletype.
; Also pulses COM1 with the same character for QEMU/serial diagnostics.
; Error code legend (see docs/BOOT_RECORDS.md):
;   '1' = BOOTMGR not found in FAT32 root directory
;   '2' = INT 13h disk read failed
die:
    mov ah, 0x0E
    mov bx, 7
    int 0x10
    mov dx, 0x3F8
    out dx, al
.h: hlt
    jmp .h

name:    db 'BOOTMGR    '

    times 510 - ($ - $$) db 0
    dw 0xAA55
