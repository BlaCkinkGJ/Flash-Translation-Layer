# Nothing here compiles C any more: every implementation is Rust and the C left
# is the ABI headers in `include/`. `make` builds the cargo archive and the two
# cargo bins, `make check` / `flow` / `format` read the headers, and
# `make consumer-check` compiles a C consumer against the installed archive,
# which is what used to keep the headers compiling.
INTEGRATION_TEST_TARGET = integration-test.out
BENCHMARK_TARGET = benchmark.out

DOCKER_TAG_ROOT = ftl

# Device Module Setting
USE_ZONE_DEVICE = 0
USE_BLUEDBM_DEVICE = 0
USE_RASPBERRY_DEVICE = 0
# Debug Setting
USE_DEBUG = 0
USE_LOG_SILENT = 0
# Random Generator Setting
USE_LEGACY_RANDOM = 0

ifeq ($(USE_DEBUG), 1)
CARGO_PROFILE_FLAG =
else
CARGO_PROFILE_FLAG = --release
endif

DEVICE_LIBS =

# The geometry a C consumer compiles against. `include/ftl-config.h` is
# generated from `ftl.yaml`, and each backend section is selected by one
# `-DFTL_BACKEND_<NAME>` macro. The Rust build reads the same `ftl.yaml`
# sections through the matching cargo feature, so the two cannot disagree. An
# explicit `-DDEVICE_NR_*` on a consumer's own compile line still wins, because
# every macro in the generated header keeps its `#ifndef` guard.
DEVICE_INFO =

ifeq ($(USE_ZONE_DEVICE), 1)
# Zoned Device's Setting
DEVICE_INFO := -DFTL_BACKEND_ZONE

# The library the `zone` feature's hand-written binding leaves undefined.
DEVICE_LIBS += -lzbd
else ifeq ($(USE_BLUEDBM_DEVICE), 1)
# BlueDBM Device's Setting
DEVICE_INFO := -DFTL_BACKEND_BLUEDBM

DEVICE_LIBS += -lmemio
else ifeq ($(USE_RASPBERRY_DEVICE), 1)
# Configuration for RaspberryPi Device
# https://www.waveshare.com/wiki/File:K9F1G08U0D.pdf
#
# RaspberryPi does not support the multiple chips and pages.
# For this reason, it does not match with conventional physical address format.
#
# Therefore, I translated conventional physical address format to RaspberryPi compatible format.
#               +----------------+--------------+--------------+-------------+
# Compatible => | Block(24 bits) | Page(4 bits) | Chip(1 bits) | Bus(1 bits) |
#               +----------------+-------------------------------------------+
# Actual     => | Block(24 bits) |                Page(6 bits)               |
#               +----------------+-------------------------------------------+
DEVICE_INFO := -DFTL_BACKEND_RASPBERRY
DEVICE_LIBS += -lnand -lwiringPi
else
# Ramdisk Setting (1GiB)
DEVICE_INFO := -DFTL_BACKEND_RAMDISK
endif

RUST_LIB_DIR := target/$(if $(filter 1,$(USE_DEBUG)),debug,release)
RUST_LIB := $(RUST_LIB_DIR)/libftl_rust.a
HEADERS := include/*.h
# `make consumer-check` is the only C compilation left, so it is the only
# user of the compiler.
CC = gcc
# What a C consumer of the installed archive adds to the link line: the C
# runtime, the pthread the device layer uses, and the library a feature-gated
# Rust backend binds (`zbd_*`, `memio_*`, `nand_*` / `wiringPi`, see
# `DEVICE_LIBS`). The staticlib carries the Rust objects only, so the final link
# supplies these, after `-lftl_rust`.
CONSUMER_LIBS := -lm -lpthread $(DEVICE_LIBS)

# Cargo features mirroring the make switches. Each backend feature mirrors the
# USE_*_DEVICE switch that also names the library the backend binds, and the
# feature is what selects the backend into the Rust device table.
RUST_FEATURES :=
ifeq ($(USE_LOG_SILENT), 1)
RUST_FEATURES += silent
endif
# The benchmark's stream seed. No C source reads `-DUSE_LEGACY_RANDOM` any
# more, so the cargo feature is what the switch drives.
ifeq ($(USE_LEGACY_RANDOM), 1)
RUST_FEATURES += legacy-random
endif
ifeq ($(USE_BLUEDBM_DEVICE), 1)
RUST_FEATURES += bluedbm
endif
ifeq ($(USE_ZONE_DEVICE), 1)
RUST_FEATURES += zone
endif
ifeq ($(USE_RASPBERRY_DEVICE), 1)
RUST_FEATURES += raspberry
endif
RUST_FEATURE_FLAG := $(if $(strip $(RUST_FEATURES)),--features '$(strip $(RUST_FEATURES))')

ifeq ($(PREFIX),)
PREFIX := /usr/local
endif

all: build_rust $(INTEGRATION_TEST_TARGET) $(BENCHMARK_TARGET)

# `ftl.yaml` is the source of truth for the tuning values and the geometry.
# The two generated files are committed, so `make`, `cargo build` and
# `make consumer-check` never run Python. `config` rewrites them and writes
# nothing when they are already current. `check-config` is the drift gate and
# runs the generator's own tests. No target depends on `config`: that would
# make a cargo build shell out to Python for a file that is already in the
# tree.
config:
	python3 tools/gen_config.py

check-config:
	python3 -B tools/gen_config.py --check
	python3 -B -m unittest discover -s tools -p 'gen_config_test.py' -q

# `make test` is the Rust suite: the C tests are gone, so this is a thin alias
# for `cargo-test`, which derives the feature flags from the USE_* switches.
test: cargo-test

integration-test: $(INTEGRATION_TEST_TARGET)
	./$(INTEGRATION_TEST_TARGET)

# One archive, because no C object is left to archive: the headers are the C
# surface and the consumer check below is what proves they still compile and
# link.
install: $(RUST_LIB)
	install -d $(DESTDIR)$(PREFIX)/lib/
	install -m 644 $(RUST_LIB) $(DESTDIR)$(PREFIX)/lib
	install -d $(DESTDIR)$(PREFIX)/include/ftl
	install -m 644 $(HEADERS) $(DESTDIR)$(PREFIX)/include/ftl
	@echo "installed $(RUST_LIB) and the headers: link with -lftl_rust $(strip $(CONSUMER_LIBS))"

# The zoned backend's end-to-end check against an emulated zoned block device.
# Two emulators can make a host-managed device out of nothing and the target
# tries both: `null_blk`
# (https://zonedstorage.io/docs/getting-started/zbd-emulation) and the `zloop`
# driver (`Documentation/admin-guide/blockdev/zoned_loop`). It needs root to
# load a module, libzbd to link and the cargo `zone` feature, so it is its own
# target rather than part of `test` or `make cargo-test`. When no emulator can
# be created it prints why and exits 0, which is what happens on macOS, where
# the Docker builder has no module tree.
#
# The device it asks for is 4 zones of 16MiB with no conventional zone. A zone
# has to hold exactly one segment of the zoned build (2^5 pages * 2^3 bus * 2^3
# chips * 8192 bytes), because the backend compares the zone's byte offset
# against the address the FTL computes, and every zone has to be
# sequential-write-required, because `open` rejects a device that has a
# conventional zone. See `src/device/zone.rs`.
ZONE_DEVICE ?= $(or $(FTL_ZONE_DEVICE),/dev/nullb0)
ZONE_MODULE_ARGS ?= nr_devices=1 zoned=1 zone_size=16 size=64 memory_backed=1
ZLOOP_ARGS ?= capacity_mb=64,zone_size_mb=16,conv_zones=0

.PHONY: zone-smoke
zone-smoke:
	@dev="$(ZONE_DEVICE)"; \
	if [ ! -b "$$dev" ]; then \
		if ! command -v modprobe >/dev/null 2>&1; then \
			echo "zone-smoke: SKIP, modprobe is missing, so no emulator can be created here"; \
			exit 0; \
		fi; \
		echo "zone-smoke: loading null_blk ($(ZONE_MODULE_ARGS))"; \
		sudo modprobe null_blk $(ZONE_MODULE_ARGS) >/dev/null 2>&1 || true; \
		if [ -b /dev/nullb0 ]; then dev=/dev/nullb0; fi; \
		if [ ! -b "$$dev" ]; then \
			echo "zone-smoke: loading zloop ($(ZLOOP_ARGS))"; \
			sudo modprobe zloop >/dev/null 2>&1 || true; \
			if [ -c /dev/zloop-control ]; then \
				sudo mkdir -p /var/local/zloop/0; \
				echo "add $(ZLOOP_ARGS)" | sudo tee /dev/zloop-control >/dev/null; \
				if [ -b /dev/zloop0 ]; then dev=/dev/zloop0; fi; \
			fi; \
		fi; \
		if [ -b "$$dev" ]; then sudo chmod 666 "$$dev"; fi; \
	fi; \
	if [ ! -b "$$dev" ]; then \
		echo "zone-smoke: SKIP, no zoned device. Create one with 'sudo modprobe null_blk $(ZONE_MODULE_ARGS)' or with the zloop driver, then set ZONE_DEVICE"; \
		exit 0; \
	fi; \
	echo "zone-smoke: using $$dev"; \
	FTL_ZONE_DEVICE="$$dev" cargo test --features zone -- --ignored --nocapture emulator_writes_reads_and_resets_a_zone

# The end-to-end program is the `integration-test` cargo bin. `integration-test.out`
# is the artifact the `integration-test` target runs and `make clean` removes.
$(INTEGRATION_TEST_TARGET): integration-test.rs $(RUST_LIB)
	cargo build $(CARGO_PROFILE_FLAG) $(RUST_FEATURE_FLAG) --bin integration-test
	cp $(RUST_LIB_DIR)/integration-test $@

# Task 16: the benchmark is a cargo bin (`benchmark.rs`, declared in
# `Cargo.toml`), so cargo links it and the rule only renames the artifact to the
# name the README uses. It links no C object: the page FTL, the ramdisk and the
# module and flash interfaces are the crate.
$(BENCHMARK_TARGET): benchmark.rs $(RUST_LIB)
	cargo build $(CARGO_PROFILE_FLAG) $(RUST_FEATURE_FLAG) --bin benchmark
	cp $(RUST_LIB_DIR)/benchmark $@


docker-builder:
	docker build -t $(DOCKER_TAG_ROOT)/ftl-builder \
		-f docker/Dockerfile ./docker

docker-make-%:
	docker run --rm -v $(PWD):/ftl \
		$(DOCKER_TAG_ROOT)/ftl-builder /bin/bash -c "make clean && make $*"

docker-console:
	docker run --rm -it -v $(PWD):/ftl \
		-v ${HOME}/.zshrc:/root/.zshrc \
		-v ${HOME}/.vim:/root/.vim \
		-v ${HOME}/.vimrc:/root/.vimrc \
		$(DOCKER_TAG_ROOT)/ftl-builder /bin/bash

# The three analysis tools read the ABI headers, which are all the C left. Each
# keeps the input it had before the last C source went: cppcheck, flawfinder and
# lizard over the headers, cflow over the static inline helpers in them.
check:
	@echo "[[ CPPCHECK ROUTINE ]]"
	cppcheck --quiet --error-exitcode=0 --enable=all --inconclusive --suppress=unusedStructMember --suppress=missingIncludeSystem -I include/ $(HEADERS)
	@echo "[[ FLAWFINDER ROUTINE ]]"
	flawfinder $(HEADERS)
	@echo "[[ STATIC ANALYSIS ROUTINE ]]"
	lizard $(HEADERS)

documents:
	doxygen -s Doxyfile

flow:
	cflow $(HEADERS)

format:
	clang-format -i $(HEADERS)

# The check the `example/rw_example.c` used to provide: a C consumer of the
# installed headers and the installed archive. It writes the consumer to a
# temporary file, installs into a staging prefix, compiles with `-Werror` so a
# header that no longer compiles fails here, links the one archive and runs the
# program against the ramdisk. Nothing is added to the repository.
consumer-check: $(RUST_LIB)
	@set -e; \
	stage=$$(mktemp -d); \
	trap 'rm -rf "$$stage"' EXIT; \
	consumer="$$stage/consumer.c"; \
	printf '%s\n' \
		'#include <assert.h>' \
		'#include <fcntl.h>' \
		'#include <string.h>' \
		'#include "module.h"' \
		'#include "flash.h"' \
		'#include "device.h"' \
		'#include "page.h"' \
		'#include "log.h"' \
		'' \
		'#define BUFFER_SIZE (4096)' \
		'' \
		'int main(void)' \
		'{' \
		'	struct flash_device *flash = NULL;' \
		'	char buffer[BUFFER_SIZE];' \
		'	int value = 42;' \
		'	assert(0 == ffi_module_init(PAGE_FTL_MODULE, &flash, RAMDISK_MODULE));' \
		'	assert(NULL != flash);' \
		'	assert(0 == flash->f_op->open(flash, NULL, O_CREAT | O_RDWR));' \
		'	memset(buffer, 0, sizeof(buffer));' \
		'	memcpy(buffer, &value, sizeof(value));' \
		'	assert(flash->f_op->write(flash, buffer, sizeof(buffer), 0)' \
		'	       == (ssize_t)sizeof(buffer));' \
		'	memset(buffer, 0, sizeof(buffer));' \
		'	assert(flash->f_op->read(flash, buffer, sizeof(buffer), 0)' \
		'	       == (ssize_t)sizeof(buffer));' \
		'	assert(0 == memcmp(buffer, &value, sizeof(value)));' \
		'	assert(0 == flash->f_op->close(flash));' \
		'	assert(0 == ffi_module_exit(flash));' \
		'	pr_info("consumer ran against the installed archive\n");' \
		'	return 0;' \
		'}' > "$$consumer"; \
	$(MAKE) install DESTDIR="$$stage" PREFIX=$(PREFIX); \
	$(CC) -Wall -Wextra -Werror $(DEVICE_INFO) \
		-I"$$stage$(PREFIX)/include/ftl" "$$consumer" \
		-o "$$stage/consumer" \
		-L"$$stage$(PREFIX)/lib" -lftl_rust $(CONSUMER_LIBS); \
	"$$stage/consumer"

clean:
	rm -f $(INTEGRATION_TEST_TARGET) $(BENCHMARK_TARGET)
	cargo clean

.PHONY: build_rust cargo-test cargo-clippy cargo-coverage test consumer-check \
	config check-config FORCE

build_rust: $(RUST_LIB)

# Run the Rust test suite. Requires no extra toolchain components.
cargo-test:
	cargo test --all-targets $(RUST_FEATURE_FLAG)

# Lint the Rust crate with warnings treated as errors, mirroring the C
# `-Werror` policy. Requires `rustup component add clippy`.
cargo-clippy:
	cargo clippy --all-targets $(RUST_FEATURE_FLAG) -- -D warnings

# Emit an LCOV coverage report for the Rust crate.
# Requires: `rustup component add llvm-tools-preview` and
#           `cargo install cargo-llvm-cov`.
cargo-coverage:
	cargo llvm-cov --lcov --output-path coverage-rust.lcov $(RUST_FEATURE_FLAG)

# The Rust sources and the feature switches are cargo's business, so the rule
# does not try to track them: `FORCE` makes every build re-run cargo, which
# re-checks sources and features itself and costs one no-op invocation. Without
# it, a `make test USE_LOG_SILENT=1` after a plain `make` reused the archive
# built without `silent`, because make only sees file timestamps.
#
# RUST_SRCS is kept as the documented input list, so `make -n` and a reader can
# see what the archive is built from.
RUST_SRCS := $(shell find src -name '*.rs') Cargo.toml $(wildcard Cargo.lock)
$(RUST_LIB): $(RUST_SRCS) FORCE
	cargo build $(CARGO_PROFILE_FLAG) $(RUST_FEATURE_FLAG)

FORCE:
