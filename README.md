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

Additionally, you must install a valid library for each module. Each module's
requirements are as follows.

- zone: [libzbd](https://github.com/westerndigitalcorporation/libzbd)
- bluedbm: [libmemio](https://github.com/pnuoslab/Flash-Board-Tester)
- raspberry: `libnand` and `wiringPi`

> Moreover, before you run the Zoned Block Device-based program, you must
> check that you give the super-user privileges to run it.

Without a zoned device, the `null_blk` driver can emulate one and
`make zone-smoke` runs the zoned backend against it (see
<https://zonedstorage.io/docs/getting-started/zbd-emulation>). The target
loads the module with `sudo`, needs libzbd, and prints why it skipped when
the emulator, the module or the privileges are missing.

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
`make cargo-clippy` lints the crate with warnings as errors and
`make cargo-coverage` writes an LCOV report.

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

## For building in the macOS

The project targets Linux and does not support the macOS natively. The Rust
build and the test suite nevertheless run with cargo, so `make test` and the
benchmark work without Docker. Docker provides the reproducible Linux build:
the builder image carries the Linux toolchain and `libzbd`, so it builds the
crate and runs the test suite and `make consumer-check` on Linux, including
the `zone` feature.

First, you need to create the builder image for building this project.

```bash
make docker-builder
```

After you create a docker image for building, run commands using
`make docker-make-${TARGET_RULE}`. For example, you can use like:

```bash
make docker-make-test # same as `make test` in the Linux
make docker-make-integration-test # same as `make integration-test` in the Linux
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
{"ts":"2026-10-19T03:32:22Z","level":"INFO","target":"ftl_rust::module","msg":"flash initialize success"}
{"ts":"2026-10-19T03:32:22Z","level":"INFO","target":"ftl_rust::module","msg":"submodule initialize success"}
{"ts":"2026-10-19T03:32:22Z","level":"INFO","target":"ftl_rust::device::ramdisk","msg":"ramdisk generated (size: 1073741824 bytes)"}
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
Processing: 100.00% [759.32 MiB/s]
finish thread 0
finish thread 1
finish thread 2
finish thread 3
[job information]
id  time(s)   bw(MiB/s) iops      avg(ms)   max(ms)   min(ms)   
=====
0   0.1312    762.3844  100       1.3117    1.8362    0.4278    
1   0.1308    764.5279  100       1.3080    2.1736    0.4021    
2   0.1300    769.1356  100       1.3002    2.1736    0.4021    
3   0.1304    766.9909  100       1.3038    2.3047    0.3904    
[crc status]
crc check success
{"ts":"2026-10-19T03:32:25Z","level":"INFO","target":"ftl_rust::module","msg":"submodule deallocates success"}
{"ts":"2026-10-19T03:32:25Z","level":"INFO","target":"ftl_rust::module","msg":"flash deallocates success"}
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

If you encounter a random-related error, please run commands as follows:

```bash
make benchmark.out USE_LEGACY_RANDOM=1
```

## How to get this project's documents

You can get this program's documentation file by using `make documents`
(which runs `doxygen -s Doxyfile`). Also, you can get the flow of each
function using `make flow`.
