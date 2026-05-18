# Assemble the three boot-record blobs to 512-byte raw binaries.
# Output: build/{mbr,fat32_pbr,ntfs_pbr}.bin
#
# Build is also invoked from crates/usbwin-boot/build.rs directly via `nasm`
# when the `embed-boot-asm` cargo feature is on. This Makefile exists for
# manual iteration during NASM development.

NASM ?= nasm
NASM_FLAGS := -f bin

SRC := mbr.asm fat32_pbr.asm ntfs_pbr.asm
OUT := $(SRC:%.asm=build/%.bin)

all: $(OUT)

build:
	mkdir -p build

build/%.bin: %.asm | build
	$(NASM) $(NASM_FLAGS) -o $@ $<
	@size=$$(stat -f%z $@ 2>/dev/null || stat -c%s $@); \
	  if [ "$$size" != "512" ]; then \
	    echo "ERROR: $@ is $$size bytes; must be exactly 512"; \
	    exit 1; \
	  fi

clean:
	rm -rf build

.PHONY: all clean
