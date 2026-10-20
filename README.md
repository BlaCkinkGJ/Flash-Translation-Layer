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

After you download the packages, you must receive this project code using
`git clone` like below.

```bash
git clone https://github.com/BlaCkinkGJ/Flash-Translation-Layer.git
```

Now you move to this repository's project directory root by using `cd`, and
`make` builds the Rust archive and the two cargo programs. The `USE_*`
switches select cargo features: `USE_ZONE_DEVICE=1`, `USE_BLUEDBM_DEVICE=1`
and `USE_RASPBERRY_DEVICE=1` pick the matching device backend,
`USE_LOG_SILENT=1` silences the info logs (`silent`) and
`USE_LEGACY_RANDOM=1` selects the legacy random generator (`legacy-random`).

> If you want to use a module like the Zoned Block Device module, you must
> set the value of the `USE_ZONE_DEVICE` variable to 1 in the `Makefile`.

### Tuning and geometry

`ftl.yaml` at the repository root is the source of truth for the page FTL's
three GC tuning values (`gc_ratio`, `gc_all`, `gc_threshold`) and for the
geometry of each backend (`ramdisk`, `zone`, `bluedbm`, `raspberry` and
`default`). A geometry section holds `bus_bits`, `chip_bits`, `page_bits`,
`block_bits` and `page_size`.

`make config` regenerates `src/ftl_config.rs` and `include/ftl-config.h`, and
writes nothing when they are already current. Both files are committed, so
`make` and a plain `cargo build` need no Python. `make check-config` is the
drift gate: it exits non-zero with a diff when either generated file differs
from the YAML, and it runs the generator's tests.

A `USE_*_DEVICE` switch selects the matching geometry section together with the
cargo feature. For a C consumer the Makefile passes one `-DFTL_BACKEND_<NAME>`
(`RAMDISK`, `ZONE`, `BLUEDBM` or `RASPBERRY`) to select the section in
`include/ftl-config.h`, and an explicit `-DDEVICE_NR_*` on the consumer's own
compile line still wins. With no `-DFTL_BACKEND_*` the header keeps its
historical default, the bluedbm set. `PAGE_FTL_CACHE_SIZE` and
`PAGE_FTL_USE_GLOBAL_RWLOCK` are gone with the C port, so they are not keys in
`ftl.yaml`.

Additionally, you must install a valid library for each module. Each module's
requirements are as follows.

- zone: [libzbd](https://github.com/westerndigitalcorporation/libzbd)
- bluedbm: [libmemio](https://github.com/pnuoslab/Flash-Board-Tester)
- raspberry: `libnand` and `wiringPi`

> Moreover, before you run the Zoned Block Device-based program, you must
> check that you give the super-user privileges to run it.

Without a zoned device, the `null_blk` driver or the `zloop` driver can
emulate one and `make zone-smoke` runs the zoned backend against it (see
<https://zonedstorage.io/docs/getting-started/zbd-emulation>). The target
loads the module with `sudo`, needs libzbd, and prints why it skipped and
exits 0 when it cannot create a device.

### Test

Before you run `make check`, you must install the below tools.

- [flawfinder](https://dwheeler.com/flawfinder/)
- [cppcheck](https://cppcheck.sourceforge.io/)
- [lizard](https://github.com/terryyin/lizard)

You can check the source code status by using `make check`. The unit and
integration tests are Rust and driven by cargo, so run the suite with:

```bash
make test USE_LOG_SILENT=1
```

`make test` runs `cargo test --all-targets` with the feature flags the
`USE_*` switches derive, so `USE_LOG_SILENT=1` selects the `silent` feature.
`make cargo-clippy` lints the crate with warnings as errors, and
`make cargo-coverage` writes an LCOV report once
`rustup component add llvm-tools-preview` and `cargo install cargo-llvm-cov`
have run.

### Execution

Suppose you want to build and run the end-to-end test in the
`integration-test.rs`, then you must execute the below commands.

```bash
make clean && make -j$(nproc) integration-test
```

That target builds `integration-test.out` out of the `integration-test`
cargo bin and runs it over the ramdisk backend, so a single command builds
the Rust library, the program and the test.

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

## For building in the macOS

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

You can get this program's documentation file by using `make documents`
(which runs `doxygen -s Doxyfile`). Also, you can get the flow of each
function using `make flow`. The first needs `doxygen`, the second `cflow`.
