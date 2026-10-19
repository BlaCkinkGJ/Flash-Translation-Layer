VPATH = device device/zone device/bluedbm device/raspberry
# You can generate the compile_commands.json file by using
# `make clean -j$(nproc)`
# `bear make all -j$(nproc)`
# `compdb -p ./ list > ../compile_commands.json`
# `cp ../compile_commands.json ./
# or
# `make compiledb`
#
# You can get `bear` from [link](https://github.com/rizsotto/Bear)
# You can get `compdb` from [link](https://github.com/Sarcasm/compdb)

# `-fsanitize=address` and `-lasan` for memory leakage checking
# `-g` and `-pg` for tracing the program
# So, you must delete all when you release the program

.SUFFIXES : .c .cpp .o
CC = gcc
AR = ar
CXX = g++
INTEGRATION_TEST_TARGET = integration-test.out
BENCHMARK_TARGET = benchmark.out
LIBRARY_TARGET = libftl.a

DEVICE_INCLUDES = 
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
DEBUG_FLAGS = -g -pg \
              -DCONFIG_ENABLE_MSG \
              -DCONFIG_ENABLE_DEBUG
MACROS = -DUSE_GC_MESSAGE -DDEBUG
MEMORY_CHECK_LIBS = -lasan
MEMORY_CHECK_CFLAGS = 
MEMORY_CHECK_LIBS += -fsanitize=address
else
DEBUG_FLAGS =
MACROS = -DUSE_GC_MESSAGE
MEMORY_CHECK_LIBS =
MEMORY_CHECK_CFLAGS = 
endif

ifeq ($(USE_LOG_SILENT), 1)
DEBUG_FLAGS += -DENABLE_LOG_SILENT
endif

ifeq ($(USE_LEGACY_RANDOM), 1)
MACROS += -DUSE_LEGACY_RANDOM
endif

DEVICE_LIBS =

ifeq ($(USE_ZONE_DEVICE), 1)
# Zoned Device's Setting
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=3 \
               -DDEVICE_NR_CHIPS_BITS=3 \
               -DDEVICE_NR_PAGES_BITS=5 \
               -DDEVICE_NR_BLOCKS_BITS=21

DEVICE_LIBS += -lzbd
else ifeq ($(USE_BLUEDBM_DEVICE), 1)
# BlueDBM Device's Setting
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=3 \
               -DDEVICE_NR_CHIPS_BITS=3 \
               -DDEVICE_NR_PAGES_BITS=7 \
               -DDEVICE_NR_BLOCKS_BITS=19 \
               -DUSER_MODE \
               -DUSE_PMU \
               -DUSE_KTIMER \
               -DUSE_NEW_RMW \
               -D_LARGEFILE64_SOURCE \
               -D_GNU_SOURCE \
               -DNOHOST

DEVICE_LIBS += -lmemio
DEVICE_INCLUDES += -I/usr/local/include/memio
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
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=1 \
               -DDEVICE_NR_CHIPS_BITS=1 \
               -DDEVICE_NR_PAGES_BITS=4 \
               -DDEVICE_NR_BLOCKS_BITS=24 \
               -DDEVICE_PAGE_SIZE=2048
DEVICE_LIBS += -lnand -lwiringPi 
DEVICE_INCLUDES += -I/usr/local/include/nand
else
# Ramdisk Setting (1GiB)
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=2 \
               -DDEVICE_NR_CHIPS_BITS=2 \
               -DDEVICE_NR_PAGES_BITS=7 \
               -DDEVICE_NR_BLOCKS_BITS=19
endif

ifeq ($(USE_ZONE_DEVICE), 1)
DEVICE_INFO += -DDEVICE_USE_ZONED
endif

ifeq ($(USE_BLUEDBM_DEVICE), 1)
DEVICE_INFO += -DDEVICE_USE_BLUEDBM
endif

ifeq ($(USE_RASPBERRY_DEVICE), 1)
DEVICE_INFO += -DDEVICE_USE_RASPBERRY
endif

ARFLAGS := rcs
CFLAGS := -Wall \
          -Wextra \
          -Wpointer-arith \
          -Wcast-align \
          -Wwrite-strings \
          -Wswitch-default \
          -Wunreachable-code \
          -Winit-self \
          -Wmissing-field-initializers \
          -Wno-unknown-pragmas \
          -Wundef \
          -Wconversion \
          -Werror \
          $(DEVICE_INFO) \
          $(DEBUG_FLAGS) \
          $(MEMORY_CHECK_CFLAGS) \
          -O3

CXXFLAGS := $(CFLAGS) \
            -std=c++11

INCLUDES := -I./include $(DEVICE_INCLUDES)
RUST_LIB_DIR := target/$(if $(filter 1,$(USE_DEBUG)),debug,release)
RUST_LIB := $(RUST_LIB_DIR)/libftl_rust.a
LIBS := -lm -lpthread $(DEVICE_LIBS) $(MEMORY_CHECK_LIBS) -L$(RUST_LIB_DIR) -lftl_rust

# Cargo features mirroring the C build switches, so the Rust staticlib and the
# C objects agree on which backend list and log level are compiled in. Each
# backend feature mirrors the USE_*_DEVICE switch that installs the same backend
# in C. Missing these makes the Rust backend table silently disagree with C's.
RUST_FEATURES :=
ifeq ($(USE_LOG_SILENT), 1)
RUST_FEATURES += silent
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

# Task 16: the FTL (`ftl/page/`), the generic helpers (`util/`), the ramdisk
# backend (`device/ramdisk/`) and the module and flash interfaces
# (`interface/`) are Rust only, so their C sources are gone. The C left is the
# device module and its optional backends, which is what `libftl.a` holds.
ZONED_SRCS =
BLUEDBM_SRCS =
RASPBERRY_SRCS =

ifeq ($(USE_ZONE_DEVICE), 1)
ZONED_SRCS += device/zone/*.c
endif

ifeq ($(USE_BLUEDBM_DEVICE), 1)
BLUEDBM_SRCS += device/bluedbm/*.c
endif

ifeq ($(USE_RASPBERRY_DEVICE), 1)
RASPBERRY_SRCS += device/raspberry/*.c
endif

DEVICE_SRCS := $(BLUEDBM_SRCS) \
               $(ZONED_SRCS) \
               $(RASPBERRY_SRCS) \
               device/*.c

SRCS := $(DEVICE_SRCS)

OBJS := $(patsubst %.c,%.o,$(sort $(wildcard $(SRCS))))

ifeq ($(PREFIX),)
PREFIX := /usr/local
endif

all: build_rust $(INTEGRATION_TEST_TARGET) $(BENCHMARK_TARGET)

# `make test` is the Rust suite: the C tests are gone, so this is a thin alias
# for `cargo-test`, which derives the feature flags from the USE_* switches.
test: cargo-test

integration-test: $(INTEGRATION_TEST_TARGET)
	./$(INTEGRATION_TEST_TARGET)

install: $(LIBRARY_TARGET) $(RUST_LIB)
	install -d $(DESTDIR)$(PREFIX)/lib/
	install -m 644 $(LIBRARY_TARGET) $(DESTDIR)$(PREFIX)/lib
	install -m 644 $(RUST_LIB) $(DESTDIR)$(PREFIX)/lib
	install -d $(DESTDIR)$(PREFIX)/include/ftl
	install -m 644 include/*.h $(DESTDIR)$(PREFIX)/include/ftl
	@echo "installed $(LIBRARY_TARGET) and $(RUST_LIB): link with -lftl -lftl_rust"
	@echo "since the Task 14 cutover, libftl.a calls the Rust C ABI it does not define"

# The end-to-end program is the `integration-test` cargo bin. `$(LIBRARY_TARGET)`
# stays a dependency so `make` keeps building the C archive for the device
# backends it still holds. `integration-test.out` is the artifact the
# `integration-test` target runs and `make clean` removes.
$(INTEGRATION_TEST_TARGET): integration-test.rs $(LIBRARY_TARGET) $(RUST_LIB)
	cargo build $(if $(filter 1,$(USE_DEBUG)),,--release) $(RUST_FEATURE_FLAG) --bin integration-test
	cp $(RUST_LIB_DIR)/integration-test $@

$(BENCHMARK_TARGET): benchmark.c $(LIBRARY_TARGET) $(RUST_LIB)
	$(CC) $(MACROS) $(CFLAGS) -c benchmark.c $(INCLUDES)
	$(CXX) $(MACROS) $(CXXFLAGS) -o $@ benchmark.o -L. -lftl -lpthread $(INCLUDES) $(LIBS)

$(LIBRARY_TARGET): $(OBJS)
	$(AR) $(ARFLAGS) $@ $^

%.o: %.c
	$(CC) $(MACROS) $(CFLAGS) -c $< -o $@ $(INCLUDES)


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

check:
	@echo "[[ CPPCHECK ROUTINE ]]"
	cppcheck --quiet --error-exitcode=0 --enable=all --inconclusive --suppress=unusedStructMember --suppress=missingIncludeSystem -I include/ $(SRCS) *.c
	@echo "[[ FLAWFINDER ROUTINE ]]"
	flawfinder $(SRCS) include/*.h
	@echo "[[ STATIC ANALYSIS ROUTINE ]]"
	lizard $(SRCS) include/*.h

documents:
	doxygen -s Doxyfile

flow:
	find . -type f -name '*.[ch]' | xargs -i cflow {}

format:
	find . -type f -name '*.[ch]' | xargs -i clang-format -i {}

compiledb:
	bear -- $(MAKE) all
	compdb -p ./ list > ../compile_commands.json
	mv ../compile_commands.json ./

clean:
	find . -name '*.o' -exec rm -f {} +
	rm -f $(TARGET) $(INTEGRATION_TEST_TARGET) $(LIBRARY_TARGET) $(BENCHMARK_TARGET)
	cargo clean

.PHONY: build_rust cargo-test cargo-clippy cargo-coverage test FORCE
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
	cargo build $(if $(filter 1,$(USE_DEBUG)),,--release) $(RUST_FEATURE_FLAG)

FORCE:
