# AGENTS.md

Machine-readable guide for AI coding assistants working in this repository.
For human-facing docs see [README.md](README.md). Per-directory notes may
exist in nested `AGENTS.md` files (nearest file wins).

## Project Snapshot

- **Domain**: NAND Flash Translation Layer (page-mapping FTL).
- **Language**: C (core) + Rust (staticlib mixed in via `cargo build`).
- **Targets**: Linux (primary), macOS via Docker only.
- **Backends**: ramdisk (default), zoned block device, bluedbm, raspberrypi.
- **Output**: static library `libftl.a` + integration test + benchmark.

## Repository Layout

```
include/           Public C headers (installed to $(PREFIX)/include/ftl)
ftl/page/          Page-mapping FTL core (core, gc, map, read, write, interface)
interface/         Module init/exit (module.c, flash.c)
device/            Backend drivers (ramdisk/, zone/, bluedbm/, raspberry/)
util/              Generic helpers (lru.c, list.c)
src/               Rust sources (cargo staticlib → libftl_rust.a)
test/              Unity-based unit tests (*-test.c)
unity/             Vendored Unity test framework
docker/            Dockerfile for cross-build (macOS / non-Linux)
example/           Example program using the installed static lib
benchmark.c        Performance benchmark → benchmark.out
integration-test.c End-to-end test → integration-test.out
```

## Environment Setup

### Linux (Ubuntu >= 16.04)
```bash
sudo apt update -y
sudo apt install -y git make gcc g++ libglib2.0-dev libiberty-dev
```

### Optional device backends
- `USE_ZONE_DEVICE=1` → install [libzbd](https://github.com/westerndigitalcorporation/libzbd)
- `USE_BLUEDBM_DEVICE=1` → install [libmemio](https://github.com/pnuoslab/Flash-Board-Tester)
- `USE_RASPBERRY_DEVICE=1` → install `libnand` and `wiringPi`

### Optional analysis / test tooling
```bash
sudo apt install -y cppcheck flawfinder cflow doxygen clang-format
sudo pip3 install lizard==1.17.0
```

### macOS
No native build. Use Docker:
```bash
make docker-builder
make docker-make-test
make docker-make-all
```

## Build Commands

### File-scoped (preferred — fast feedback)
```bash
# Compile a single C source with the project's flags
gcc -Wall -Wextra -Werror -I include -I unity/src \
    -DDEVICE_NR_BUS_BITS=2 -DDEVICE_NR_CHIPS_BITS=2 \
    -DDEVICE_NR_PAGES_BITS=7 -DDEVICE_NR_BLOCKS_BITS=19 \
    -O3 -c ftl/page/page-write.c -o /tmp/page-write.o

# Run a single Unity test
make USE_LOG_SILENT=1 lru-test.out && ./lru-test.out

# Rust crate only
cargo build --release
# Rust tooling — Makefile wrappers are the preferred path (consistent
# flags, fail-fast on warnings, no shell quoting). Raw cargo commands
# shown for reference.
make cargo-test      # = cargo test --all-targets
make cargo-clippy    # = cargo clippy --all-targets -- -D warnings
make cargo-coverage  # = cargo llvm-cov --lcov (needs llvm-tools-preview + cargo-llvm-cov)
```

### Full builds
```bash
make clean && make -j$(nproc)             # all (default: ramdisk)
make clean && make -j$(nproc) test USE_LOG_SILENT=1
make clean && make -j$(nproc) integration-test
make clean && make benchmark.out
make clean && make benchmark.out USE_LEGACY_RANDOM=1   # if random errors hit
```

### Backend toggles (set in Makefile or via `make VAR=1`)
| Variable | Effect |
|---|---|
| `USE_ZONE_DEVICE` | Build zoned-block-device backend (`-lzbd`) |
| `USE_BLUEDBM_DEVICE` | Build bluedbm backend (`-lmemio`) |
| `USE_RASPBERRY_DEVICE` | Build raspberrypi NAND backend |
| `USE_DEBUG=1` | Add `-g -pg`, enable debug + message macros, link `-lasan` |
| `USE_LOG_SILENT=1` | Suppress info logs (`-DENABLE_LOG_SILENT`) |
| `USE_LEGACY_RANDOM=1` | Fall back to legacy random generator |

### Install (produces `libftl.a` + headers under `/usr/local`)
```bash
make clean && make -j$(nproc)
sudo make install
```

### Generate docs / call graph
```bash
make documents    # doxygen -s Doxyfile
make flow         # cflow across all .[ch]
make check        # cppcheck + flawfinder + lizard
```

## Code Style

### C (Linux-kernel style — see `.clang-format`)
- **Indentation**: tabs, width 8, column limit 80.
- **Braces**: Allman for functions/namespaces, K&R-like for control flow.
- **Pointer alignment**: right (`int *p`, not `int* p`).
- **Warnings**: `-Wall -Wextra -Wpointer-arith -Wcast-align -Wwrite-strings
  -Wswitch-default -Wunreachable-code -Winit-self -Wmissing-field-initializers
  -Wundef -Wconversion -Werror`. Treat new warnings as bugs.
- **Format**: `make format` (clang-format against `.clang-format`).
- **Headers**: each `.c`/`.h` carries the doxygen `@file @brief @author @version @date`
  block. Headers guarded with `#ifndef FOO_H / #define FOO_H`. Public headers
  wrap declarations in `extern "C"` for C++ callers.
- **Concurrency**: pthreads; prefer `pthread_mutex_t` / `pthread_rwlock_t`.
  Free page counters use `__atomic_load_n(..., __ATOMIC_SEQ_CST)`.
- **Logging**: `pr_info` / `pr_debug` / `pr_err` style via `log.h`. Production
  builds keep info logs; use `USE_LOG_SILENT=1` to silence them.
- **No C++ in the C core.** `g++` is only used to link because Unity and
  some device libs are C++.

### Rust
- `src/lib.rs` is the crate root. Add modules there; the crate builds as
  `staticlib` and links into the C side via `-lftl_rust`.
- Edition 2021 (see `Cargo.toml`).
- No external dependencies declared yet — keep it that way unless justified.
- `cargo clippy --all-targets -- -D warnings` is treated as a build blocker,
  mirroring the C `-Werror` policy. The Makefile wraps this as
  `make cargo-clippy`.
- The Makefile also wraps `cargo test --all-targets` as `make cargo-test`
  and `cargo llvm-cov --lcov --output-path coverage-rust.lcov` as
  `make cargo-coverage` (requires the `llvm-tools-preview` rustup component
  and the `cargo-llvm-cov` cargo subcommand).

## Testing

### Frameworks
- **Unit tests**: [Unity](https://www.throwtheswitch.org/unity) (vendored
  under `unity/`). One `*-test.c` per module under `test/`.
- **Coverage**: gcov, built into the test binaries via `--coverage`.

### Targets
- `lru-test.out`, `bits-test.out`, `ramdisk-test.out`, `list-test.out`,
  `crc32-test.out` (always)
- `zone-test.out` (only when `USE_ZONE_DEVICE=1`)

### Run
```bash
make clean && make test USE_LOG_SILENT=1   # all unit tests + coverage
make integration-test                      # end-to-end via integration-test.c
./lru-test.out                             # single test binary
```

### Conventions for new tests
- File: `test/<module>-test.c`; pair with `<module>.c` and `unity.o`.
- Add the binary name to `TEST_TARGET` in the `Makefile`.
- Cover both success and error paths. Mock nothing device-specific —
  use the ramdisk backend for hermetic tests.

## Project Conventions

### Adding a new device backend
1. Create `device/<name>/<name>.c` and `device/<name>/<name>.h`.
2. Add a `USE_<NAME>_DEVICE` switch + `DEVICE_INFO` block in the `Makefile`
   mirroring the existing ones.
3. Add the source glob to `DEVICE_SRCS`.
4. If a new test is needed, add `<name>-test.c` to `test/` and append
   `<name>-test.out` to `TEST_TARGET`.

### Adding a new FTL policy
1. New file under `ftl/page/` (e.g. `page-cache.c`).
2. Declare the API in `include/page.h` near the related functions.
3. Wire it in `page-interface.c` or `page-core.c` — keep allocation / GC
   ownership in one place.

### Mixing Rust
- New Rust modules go in `src/`, re-exported from `src/lib.rs`.
- C side consumes them via `extern "C"` declarations in a header under
  `include/`. Keep the surface tiny — Rust owns the logic, C owns the IO.

## Permissions

### Allowed without prompting
- Read any file, list directories.
- Compile a single C/Rust source with project flags (file-scoped above).
- Run a single Unity test binary.
- Run `cargo build` / `cargo check` for the staticlib.
- Edit `*.c`, `*.h`, `*.rs`, `Makefile`, `Cargo.toml`, headers, tests.
- Run `clang-format -i` on touched files.
- Run `make check` (cppcheck / flawfinder / lizard) locally.

### Ask before doing
- `make install` / `sudo make install` (writes to `/usr/local`).
- `make clean && make test` / `make integration-test` / `make benchmark.out`
  (full builds; slow).
- `git commit`, `git push`, branch creation, force-push, history rewrites.
- Modifying `Makefile`, `Cargo.toml`, `.clang-format`, `Doxyfile`, or
  CI workflow (`.github/workflows/build.yml`).
- Adding/removing third-party dependencies (Cargo or system libs).
- Deleting files or directories.
- Anything that touches zoned / bluedbm / raspberry backends (real hardware).

### Never
- Commit secrets, device paths, or user-specific paths.
- Commit build artifacts (`*.o`, `*.out`, `*.a`, `target/`, `doxygen/`).
- Bypass `-Werror` to land a change; fix the warning instead.
- Edit generated files by hand (anything in `doxygen/`, `target/`).

## Security & Secrets

- This codebase is a local userspace library; it does not handle credentials
  or network IO. Keep it that way.
- Never commit device paths from the maintainer's environment. Backend
  configuration is a build-time concern (`USE_*_DEVICE` flags), not data.
- Run `make check` before submitting changes — `flawfinder` and `cppcheck`
  are first-line review.

## Pull Request Checklist

- `make check` passes (cppcheck + flawfinder + lizard clean).
- `make test USE_LOG_SILENT=1` passes locally for the affected backends.
- New code is `clang-format`'d (`make format` or equivalent).
- Doxygen comment block present on every new public symbol.
- Diff is small and focused — one logical change per PR.
- No new warnings under the project's `-Wall -Wextra -Werror` set.

## Maintenance

- This file is living documentation. Update it in the same PR that changes
  a build flag, target, test, or convention.
- Keep under ~200 lines. If it grows, split into per-directory `AGENTS.md`
  files (e.g. `device/zone/AGENTS.md` for zoned-block specifics).
- Test the file: ask an AI assistant to perform a common task using only
  this document. Fix anything it gets wrong.

## Additional Resources

- [README.md](README.md) — build, install, benchmark, example.
- [Doxyfile](Doxyfile) — generated API reference (`make documents`).
- `.clang-format` — Linux-kernel C style baseline.
- `Makefile` — authoritative list of targets, flags, and switches.
