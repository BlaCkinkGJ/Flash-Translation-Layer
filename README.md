# Flash Translation Layer

![build-and-test](https://github.com/BlaCkinkGJ/Flash-Translation-Layer/actions/workflows//build.yml/badge.svg)[![Codacy Badge](https://app.codacy.com/project/badge/Grade/9b16f37d8a314e14a049312b5cfad674)](https://www.codacy.com/gh/BlaCkinkGJ/Flash-Translation-Layer/dashboard?utm_source=github.com&amp;utm_medium=referral&amp;utm_content=BlaCkinkGJ/Flash-Translation-Layer&amp;utm_campaign=Badge_Grade)


## Overview

This repository contains the simple Flash Translation Layers made on the Linux Environments. Furthermore, this repository works on the Ramdisk and various devices(e.g., Zoned Block Device, bluedbm, etc.).

If you have any questions about this project, don't hesitate to contact the maintainer (ss5kijun@gmail.com).

## Build

> These instructions are based on the Ubuntu(>=16.04) environments

### Prerequisite

Before you build this repository, you must install some packages from the package manager.

```bash
sudo apt update -y
sudo apt install -y git make gcc g++ libglib2.0-dev libiberty-dev
```

After you download the packages, you must receive this project code using `git clone` like below.

```
git clone --recursive ${REPOSITORY_URL}
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

You can check the source code status by using `make check`. If you want to generate test files, execute the below command.

```bash
make clean && make -j$(nproc) test USE_LOG_SILENT=1
```

After the build finish, you can get the various test files from the results. Run those files to test whether the project's module works correctly.

### Execution

Suppose you want to generate a release program through the `main.c`, then you must execute the below commands.

```bash
make clean && make -j$(nproc)
```

Now, you can run that program by using `./a.out`. Note that this repository is `main.c` file conducts the integration test of this project.

### Installation

This project also supports generating the static library for using the external project. If you want to use it, please follow the below commands.

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

After installing our shared library on your system, you can make your own program. You can refer to making your program from the `example` directory.

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
INFO:[interface/module.c:module_init(49)] flash initialize success
INFO:[interface/module.c:module_init(55)] submodule initialize success
INFO:[device/ramdisk/ramdisk.c:ramdisk_open(60)] ramdisk generated (size: 1073741824 bytes)
INFO:[device/ramdisk/ramdisk.c:ramdisk_open(77)] bitmap generated (size: 16392 bytes)
[parameters]
        - modules     pgftl
        - devices     ramdisk
        - workloads   write
        - jobs        4
        - block size  1048576
        - # of block  100
        - io size     100MiB
        - path        (null)
fill data start!
ready to read!
Processing: 100.00% [1829.49 MiB/s]
finish thread 0
finish thread 1
finish thread 2
finish thread 3
[job information]
id  time(s)   bw(MiB/s) iops      avg(ms)   max(ms)   min(ms)
=====
0   0.0644    1552.6847 100       0.6440    2.0855    0.1686
1   0.0530    1886.6408 100       0.5300    2.0855    0.1624
2   0.0522    1915.5273 100       0.5220    2.1562    0.1530
3   0.0541    1847.9744 100       0.5411    2.1562    0.1530
[crc status]
crc check success
INFO:[interface/module.c:module_exit(75)] submodule deallocates success
INFO:[interface/module.c:module_exit(83)] flash deallocates success
[parameters]
        - modules     pgftl
        - devices     ramdisk
        - workloads   write
        - jobs        4
        - block size  1048576
        - # of block  100
        - io size     100MiB
        - path        (null)
```

If you encounter a random-related error, please run commands as follows:

```bash
make benchmark.out USE_LEGACY_RANDOM=1
```

## How to get this project's documents

You can get this program's documentation file by using `doxygen -s Doxyfile`. Also, you can get the flow of each function using `make flow`.
