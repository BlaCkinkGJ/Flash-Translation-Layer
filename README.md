# Flash Translation Layer

![build-and-test](https://github.com/BlaCkinkGJ/Flash-Translation-Layer/actions/workflows//build.yml/badge.svg)[![Codacy Badge](https://app.codacy.com/project/badge/Grade/9b16f37d8a314e14a049312b5cfad674)](https://www.codacy.com/gh/BlaCkinkGJ/Flash-Translation-Layer/dashboard?utm_source=github.com&amp;utm_medium=referral&amp;utm_content=BlaCkinkGJ/Flash-Translation-Layer&amp;utm_campaign=Badge_Grade)


## Overview

This repository contains the simple Flash Translation Layers made on the Linux Environments. Furthermore, this repository works on the Ramdisk and various devices(e.g., Zoned Block Device, bluedbm, etc.).

The page FTL, the utilities, the device dispatch and the default ramdisk
backend are Rust, and the Rust library is `libftl_rust.a`. The optional
zoned and bluedbm backends stay C and are archived into
`libftl.a`, which calls into the Rust library, so a consumer of the
installed library links both (see the Installation section below).

If you have any questions about this project, don't hesitate to contact the maintainer (ss5kijun@gmail.com).

## Build

> These instructions are based on the Ubuntu(>=16.04) environments

### Prerequisite

Before you build this repository, you must install some packages from the package manager.

```bash
sudo apt update -y
sudo apt install -y git make gcc g++
```

The Rust toolchain is required as well: install it from
[rustup](https://rustup.rs/) so `cargo` is on `PATH`.

After you download the packages, you must receive this project code using `git clone` like below.

```
git clone ${REPOSITORY_URL}
```

Now you move to this repository's project directory root by using `mv`.

> If you want to use a module like the Zoned Block Device module, you must set the value
> of the `USE_ZONE_DEVICE` variable to 1 in the `Makefile`.

Additionally, you must install a valid library for each module. Each module's requirements are as follows.

- zone: [libzbd](https://github.com/westerndigitalcorporation/libzbd)
- bluedbm: [libmemio](https://github.com/pnuoslab/Flash-Board-Tester)

> Moreover, before you run the Zoned Block Device-based program,
> You must check that you give the super-user privileges to run
> the Zoned Block Device-based programs.

### Test

Before you execute the test and related things, you must install the below tools.

- [flawfinder](https://dwheeler.com/flawfinder/)
- [cppcheck](https://cppcheck.sourceforge.io/)
- [lizard](https://github.com/terryyin/lizard)

You can check the source code status by using `make check`. The unit and
integration tests are Rust and driven by cargo, so run the suite with:

```bash
make test USE_LOG_SILENT=1
```

`make test` runs `cargo test --all-targets` with the feature flags the `USE_*`
switches derive, so `USE_LOG_SILENT=1` selects the `silent` feature.

### Execution

Suppose you want to build and run the end-to-end test in the
`integration-test.rs`, then you must execute the below commands.

```bash
make clean && make -j$(nproc) integration-test
```

That target builds `integration-test.out` out of the `integration-test` cargo
bin and runs it over the ramdisk backend, so a single command builds the Rust
library, the program and the test.

### Installation

This project also supports generating the static libraries for using the
external project. `make install` puts `libftl.a`, `libftl_rust.a` and the
headers from `include/` under `/usr/local`, and a consumer links with
`-lftl -lftl_rust` because the C archive calls the Rust one. If you want to
use it, please follow the below commands.

```bash
make clean
make -j$(nproc)
sudo make install
```
## For building in the macOS

We are not providing native support for the macOS but providing Docker-based support for the macOS.

First, you need to create the builder image for building this project.

```bash
make docker-builder
```

After you create a docker image for building, run commands using `make docker-make-${TARGET_RULE}`. For example, you can use like:

```bash
make docker-make-test # same as `make test` in the Linux
make docker-make-integration-test # same as `make integration-test` in the Linux
make docker-make-all
make docker-make-check
make docker-make-flow
```

## Example

You can refer to making your own program from the `example` directory. It is a
cargo example of this crate, so it builds against the sources instead of an
installed library.

You can run this example as follows:

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

For example, if you want to see sequential write performance on the ramdisk, type like:

```bash
./benchmark.out -m pgftl -d ramdisk -t write -j 4 -b 1048576 -n 100
```

It shows results like:

```bash
{"ts":"2026-10-18T13:38:55Z","level":"INFO","target":"ftl_rust::module",
  "msg":"flash initialize success"}
{"ts":"2026-10-18T13:38:55Z","level":"INFO","target":"ftl_rust::module",
  "msg":"submodule initialize success"}
{"ts":"2026-10-18T13:38:55Z","level":"INFO",
  "target":"ftl_rust::device::ramdisk",
  "msg":"ramdisk generated (size: 1073741824 bytes)"}
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
Processing: 100.00% [1055.57 MiB/s]
finish thread 0
finish thread 1
finish thread 2
finish thread 3
[job information]
id  time(s)   bw(MiB/s) iops      avg(ms)   max(ms)   min(ms)   
=====
0   0.0935    1069.8505 100       0.9347    1.3310    0.4010    
1   0.0944    1059.6700 100       0.9437    1.6810    0.4010    
2   0.0936    1067.8627 100       0.9365    1.6810    0.2980    
3   0.0938    1066.2345 100       0.9379    1.6810    0.2980    
[crc status]
crc check success
{"ts":"2026-10-18T13:38:58Z","level":"INFO","target":"ftl_rust::module",
  "msg":"submodule deallocates success"}
{"ts":"2026-10-18T13:38:58Z","level":"INFO","target":"ftl_rust::module",
  "msg":"flash deallocates success"}
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

If you encounter a random-related error, please run commands as follows:

```bash
make benchmark.out USE_LEGACY_RANDOM=1
```

## How to get this project's documents

You can get this program's documentation file by using `doxygen -s Doxyfile`. Also, you can get the flow of each function using `make flow`.
