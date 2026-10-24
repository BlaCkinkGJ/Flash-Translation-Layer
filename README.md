# Flash Translation Layer

![build-and-test](https://github.com/BlaCkinkGJ/Flash-Translation-Layer/actions/workflows/build.yml/badge.svg)[![Codacy Badge](https://app.codacy.com/project/badge/Grade/9b16f37d8a314e14a049312b5cfad674)](https://www.codacy.com/gh/BlaCkinkGJ/Flash-Translation-Layer/dashboard?utm_source=github.com&amp;utm_medium=referral&amp;utm_content=BlaCkinkGJ/Flash-Translation-Layer&amp;utm_campaign=Badge_Grade)

## Overview

This repository contains the simple Flash Translation Layers made on the
Linux Environments. Furthermore, this repository works on the Ramdisk and
various devices(e.g., Zoned Block Device, bluedbm, etc.).

Every implementation is Rust, and the library is `libftl_rust.a`. The four
device backends (ramdisk by default, zoned, bluedbm and raspberry) are Rust as
well, the last three behind their cargo features. The C surface is the headers
in `include/`, which declare the C ABI the archive exports, so a consumer of
the installed library links the one archive (see the Installation section
below).

If you have any questions about this project, don't hesitate to contact the
maintainer (<ss5kijun@gmail.com>).

## Documentation

Three documents cover the parts this README does not:

- [docs/architecture.md](docs/architecture.md): what the layers are, how a
  read, a write and a garbage collection pass move through them, and where the
  locks sit.
- [docs/c-abi.md](docs/c-abi.md): what `include/` exposes to C and C++, who
  owns what across the boundary, and how `make consumer-check` proves it.
- [docs/guards.md](docs/guards.md): which header guard, cargo feature, `USE_*`
  switch and CI job guards what, and what breaks without it.

[AGENTS.md](AGENTS.md) is the machine-readable guide, and it holds the
authoritative repository layout. Read it before sending a patch: it lists every
target and the conventions a change has to follow.

The tree, in one table:

| Path | What is there |
| --- | --- |
| `Makefile` | every target and every `USE_*` switch, the authoritative list |
| `Cargo.toml` | the crate manifest, the cargo features the switches map to |
| `src/` | the crate: the device layer, the four backends and the page FTL |
| `src/ftl_config.rs` | generated from `ftl.yaml`, do not edit |
| `include/` | the C ABI headers, the only C surface left |
| `include/ftl-config.h` | generated from `ftl.yaml`, do not edit |
| `ftl.yaml` | the GC tuning and the per-backend geometry |
| `tools/` | the generator from `ftl.yaml`, and its tests |
| `tests/` | the cargo integration test |
| `example/` | a small cargo example, `rw_example` |
| `docs/` | the three documents above |
| `benchmark.rs` | a cargo bin, the benchmark, at the crate root |
| `integration-test.rs` | a cargo bin, the end-to-end test, at the crate root |

## Build

> These instructions are based on the Ubuntu(>=16.04) environments

### Prerequisite

Before you build this repository, you must install some packages from the
package manager.

```bash
sudo apt update -y
sudo apt install -y git make gcc
```

The Rust toolchain is required as well: install it from
[rustup](https://rustup.rs/) so `cargo` is on `PATH`. `gcc` is only used by
`make consumer-check`, the one C compile left in the tree.

`python3` is needed by `make config` and `make check-config`, which regenerate
the two committed config files from `ftl.yaml` and check them for drift, and by
`make check`, because `lizard` is a Python program. No other target runs it, so
a plain `make` or `cargo build` needs no Python at all.

A backend that talks to hardware binds its library by hand, so build and link
against the one you enable. The default build needs none of them.

- zone: [libzbd](https://github.com/westerndigitalcorporation/libzbd)
- bluedbm: [libmemio](https://github.com/pnuoslab/Flash-Board-Tester)
- raspberry: `libnand` and `wiringPi`

After you download the packages, you must receive this project code using
`git clone` like below.

```bash
git clone https://github.com/BlaCkinkGJ/Flash-Translation-Layer.git
cd Flash-Translation-Layer
```

Now you are in the project directory root, and `make` builds the Rust archive
and the two cargo programs. The `USE_*` switches select cargo features:
`USE_ZONE_DEVICE=1`, `USE_BLUEDBM_DEVICE=1` and `USE_RASPBERRY_DEVICE=1` pick
the matching device backend, `USE_LOG_SILENT=1` silences the info, warning and
error logs (`silent`), `USE_LEGACY_RANDOM=1` selects the legacy random
generator (`legacy-random`) and `USE_DEBUG=1` builds the debug cargo profile
instead of the release one. Pass them on the command line, for example
`make USE_ZONE_DEVICE=1`. The `Makefile` reads them and is the authoritative
list, and `docs/guards.md` has the table of what each one enables.

### Tuning and geometry

`ftl.yaml` at the repository root is the source of truth for the page FTL's
three GC tuning values (`gc_ratio`, `gc_all`, `gc_threshold`) and for the
geometry of each backend (`ramdisk`, `zone`, `bluedbm`, `raspberry` and
`default`). A geometry section holds `bus_bits`, `chip_bits`, `page_bits`,
`block_bits` and `page_size`.

`make config` runs `tools/gen_config.py`, which reads that file and writes
`src/ftl_config.rs` and `include/ftl-config.h`. It writes nothing when they are
already current. Both files are committed, so `make` and a plain `cargo build`
need no Python. `make check-config` is the drift gate: it exits non-zero with a
diff when either generated file differs from the YAML, and it runs the
generator's own tests. Do not edit the two generated files by hand, because the
next `make config` would undo it.

A `USE_*_DEVICE` switch selects the matching geometry section together with the
cargo feature. For a C consumer the Makefile passes one `-DFTL_BACKEND_<NAME>`
(`RAMDISK`, `ZONE`, `BLUEDBM` or `RASPBERRY`) to select the section in
`include/ftl-config.h`, and an explicit `-DDEVICE_NR_*` on the consumer's own
compile line still wins. With no `-DFTL_BACKEND_*` the header keeps its
historical default, the bluedbm set, which is not the geometry the default
archive is built with. `PAGE_FTL_CACHE_SIZE` and `PAGE_FTL_USE_GLOBAL_RWLOCK`
are gone with the C port, so they are not keys in `ftl.yaml`.

The generator parses a small YAML subset and rejects anything outside it: two
space indentation, `key: value`, `#` comments, integers and plain decimals. That
subset is spelled out in the file's own header comment. A typo therefore fails
`make config` and the `make check-config` gate instead of quietly changing a
compiled constant.

> Moreover, before you run the Zoned Block Device-based program, you must
> check that you give the super-user privileges to run it.

Without a zoned device, the `null_blk` driver or the `zloop` driver can
emulate one and `make zone-smoke` runs the zoned backend against it (see
<https://zonedstorage.io/docs/getting-started/zbd-emulation>). The target
loads the module with `sudo`, needs libzbd, and prints why it skipped and
exits 0 when it cannot create a device.

### Tests and static analysis

The unit and integration tests are Rust and driven by cargo, so the suite needs
nothing but the toolchain above:

```bash
make test USE_LOG_SILENT=1
```

`make test` runs `cargo test --all-targets` with the feature flags the
`USE_*` switches derive, so `USE_LOG_SILENT=1` selects the `silent` feature.
`make cargo-clippy` lints the crate with warnings as errors, and
`make cargo-coverage` writes an LCOV report once
`rustup component add llvm-tools-preview` and `cargo install cargo-llvm-cov`
have run.

`make check` is a separate gate. It reads the headers in `include/`, not the
Rust sources, so `make cargo-clippy` is what covers the implementation. Install
these three first:

- [flawfinder](https://dwheeler.com/flawfinder/)
- [cppcheck](https://cppcheck.sourceforge.io/)
- [lizard](https://github.com/terryyin/lizard), version 1.17.0

CI runs `make check` before it builds. `make format` runs `clang-format` over
the same headers with the repository's `.clang-format`, and `clang-format` is
not in the apt list above, so install it separately. CI does not run it. The
headers are hand wrapped to 80 columns rather than formatter output, so
`make format` currently rewrites a few of them, and running it is a choice a
change makes deliberately.

### Execution

Suppose you want to build and run the end-to-end test in
`integration-test.rs`, then you must execute the below command.

```bash
make -j$(nproc) integration-test
```

That target builds `integration-test.out` out of the `integration-test`
cargo bin and runs it over the ramdisk backend, so a single command builds
the Rust library, the program and the test. Switching a `USE_*` value rebuilds
both, because the archive is newer than the artifact that links it.

### Installation

This project also supports generating the static library for using the
external project. `make install` puts `libftl_rust.a` and the headers from
`include/` under `/usr/local` (the headers into `include/ftl/`), and a
consumer links with `-lftl_rust -lm -lpthread`, plus the library a device
switch needs (`-lzbd`, `-lmemio` or `-lnand -lwiringPi`).
`make consumer-check` compiles and runs such a consumer against a staging
install. If you want to use it, please follow the below commands.

```bash
make clean
make -j$(nproc)
sudo make install
```

`make install` depends on the archive, so it runs cargo. Run it as root only
where root's `PATH` and rustup environment reach the toolchain, or use
`make install DESTDIR=<dir>` to stage the same tree without root.

A hand-written consumer compiles against the installed tree like this, with one
`-DFTL_BACKEND_<NAME>` naming the geometry the archive was built with:

```bash
gcc -I"$PREFIX/include/ftl" -DFTL_BACKEND_RAMDISK consumer.c \
    -L"$PREFIX/lib" -lftl_rust -lm -lpthread
```

Pass the macro for the build you link: `FTL_BACKEND_RAMDISK` for the default
archive, `FTL_BACKEND_ZONE`, `FTL_BACKEND_BLUEDBM` or `FTL_BACKEND_RASPBERRY`
for the others. Leaving it out gives you the header's historical default, the
bluedbm geometry, which is not what the default archive uses and would decode
every device address wrong. `make consumer-check` does exactly this, and is the
working example to copy.

## Building on macOS

The project targets Linux and does not support the macOS natively. The Rust
build and the test suite nevertheless run with cargo, so `make test` and the
benchmark work without Docker. Docker provides the reproducible Linux build:
the builder image carries the Linux toolchain and `libzbd`, so
`make docker-make-<target>` runs that target on Linux. The `USE_*` switches
are not forwarded into the container, so pass them there, for example
`make docker-console` and then `make test USE_ZONE_DEVICE=1`.

First, you need to create the builder image for building this project.

```bash
make docker-builder
```

After you create a docker image for building, run commands using
`make docker-make-${TARGET_RULE}`. For example, you can use like:

```bash
make docker-make-test # same as `make test` in the Linux
make docker-make-integration-test # same as `make integration-test` in the Linux
make docker-make-consumer-check # same as `make consumer-check` in the Linux
make docker-make-all
make docker-make-check
make docker-make-flow
```

## Example

You can refer to making your own program from the `example` directory. It is
a cargo example of this crate, so it builds against the crate's sources
rather than an installed archive, and it does not demonstrate the installed
library any more (`make consumer-check` is the C consumer that does). You can
run this example as follows:

```bash
pushd example
make
./rw_example
make clean
popd
```

## Benchmark

Build benchmark program by using:

```bash
make benchmark.out
```

See its usage by using:

```bash
./benchmark.out -h
```

For example, if you want to see sequential write performance on the ramdisk,
type like:

```bash
./benchmark.out -m pgftl -d ramdisk -t write -j 4 -b 1048576 -n 100
```

It shows results like:

<!-- markdownlint-disable MD013 -->

```text
{"ts":"2026-10-19T08:31:07Z","level":"INFO","target":"ftl_rust::module","msg":"flash initialize success"}
{"ts":"2026-10-19T08:31:07Z","level":"INFO","target":"ftl_rust::module","msg":"submodule initialize success"}
{"ts":"2026-10-19T08:31:07Z","level":"INFO","target":"ftl_rust::device::ramdisk","msg":"ramdisk generated (size: 1073741824 bytes)"}
[parameters]
        - modules     pgftl
        - devices     ramdisk
        - workloads   write
        - jobs        4
        - block size  1048576
        - # of block  100
        - io size     100MiB
        - path        (null)
        - warm-up     enabled
fill data start!
ready to read!
Processing: 0.00% [nan MiB/s]
Processing: 100.00% [395.27 MiB/s]
finish thread 0
finish thread 1
finish thread 2
finish thread 3
[job information]
id  time(s)   bw(MiB/s) iops      avg(ms)   max(ms)   min(ms)
=====
0   0.2607    383.5946  100       2.6069    14.2342   0.3500
1   0.2465    405.7136  100       2.4648    14.2342   0.3500
2   0.2420    413.2702  100       2.4197    14.2342   0.3500
3   0.2505    399.2655  100       2.5046    14.2342   0.3500
[crc status]
crc check success
{"ts":"2026-10-19T08:31:10Z","level":"INFO","target":"ftl_rust::module","msg":"submodule deallocates success"}
{"ts":"2026-10-19T08:31:10Z","level":"INFO","target":"ftl_rust::module","msg":"flash deallocates success"}
[parameters]
        - modules     pgftl
        - devices     ramdisk
        - workloads   write
        - jobs        4
        - block size  1048576
        - # of block  100
        - io size     100MiB
        - path        (null)
        - warm-up     enabled
```

<!-- markdownlint-enable MD013 -->

CI builds the benchmark in both random modes and then runs it once over the
ramdisk under `timeout`, so a panic or a hang fails the job. If you encounter
a random-related error, please run commands as follows:

```bash
make benchmark.out USE_LEGACY_RANDOM=1
```

## How to get this project's documents

`make documents` runs `doxygen -s Doxyfile` and needs `doxygen`. The
`Doxyfile` covers the C headers, the Markdown in the repository and the Python
tool, so the output is the header reference and the prose documents. The Rust
sources are not in it, and their reference is `cargo doc`.

`make flow` runs `cflow include/*.h` and needs `cflow`. You get the call graph
of the static inline helpers the headers define, which is all the C left. It
does not graph the Rust implementation.
