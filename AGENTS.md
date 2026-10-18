# AGENTS.md

Machine-readable guide for AI coding assistants working in this repository.
For human-facing docs see [README.md](README.md). Per-directory notes may
exist in nested `AGENTS.md` files (nearest file wins).

## Project Snapshot

- **Domain**: NAND Flash Translation Layer (page-mapping FTL).
- **Language**: Rust (page FTL and utilities) plus C (device backends and the
  interface shim).
- **Targets**: Linux (primary), macOS via Docker only.
- **Backends**: ramdisk (default), zoned block device, bluedbm, raspberrypi.
- **Output**: static library `libftl.a` + integration test + benchmark.

## Repository Layout

```
include/           Public C headers (installed to $(PREFIX)/include/ftl)
interface/         Module init/exit (module.c, flash.c)
device/            Backend drivers (device.c, zone/, bluedbm/, raspberry/)
src/               Rust sources (cargo staticlib → libftl_rust.a), including the
                   page FTL under src/ftl/
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
    -O3 -c interface/module.c -o /tmp/module.o

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
- External dependencies are allowed but must be justified. Current
  deps: `log` (standard logging facade) and `env_logger` (default
  backend). The `pr_*!` macros in `src/log.rs` delegate to `log::*!`
  so backend swaps (Loki / OpenTelemetry / etc.) only touch
  initialization, not call sites. Level filtering: `RUST_LOG` env
  var, e.g. `RUST_LOG=debug` to enable `pr_debug!` in release.
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

1. New module under `src/ftl/` (e.g. `page_cache.rs`), declared in
   `src/ftl/mod.rs`.
2. Register the C ABI entry point in `src/ftl/page_interface.rs` and declare it
   in `include/page.h` when C calls it.
3. Keep allocation / GC ownership in one place, the way the C sources did.

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

## AI work instructions (orchestrator only)

**These instructions govern the orchestrating agent alone. A sub agent must not
follow them.** A sub agent follows the single task brief it is handed, plus the
rest of this file.

**Authority.** The orchestrator holds standing authority to commit, push, create
branches and worktrees, and merge pull requests once the gates below are met. It
does not stop to ask for those operations. That is the one exception to the
`Ask before doing` list above, which still governs every other agent and every
other operation.

### Work order

1. Land the pull requests that are already open before starting new work.
2. Then take the remaining issues in dependency order.
3. Run issues that touch disjoint files in parallel, each in its own git
   worktree with its own sub agent, so that no two agents edit the same file.
4. When a pull request merges, tell every agent that is still working to rebase
   onto the new `main` before it finishes.
5. Add a `rust-migration` issue the moment the work uncovers one, then fold it
   into the plan. The job is done when none are left open.

### Per issue

1. Read the issue and the C source it names. The issue body is the binding spec,
   not a summary of it.
2. Implement against the C original, then compare the two behaviours before
   calling the port done: return values, error codes, ownership and free
   ordering, allocation failure paths, null and out of range inputs, struct
   layout, and the counter updates each side performs.
3. CI/CD must pass on the pull request.
4. Line coverage of the lines the pull request adds or changes must reach 90 per
   cent. Measure it, never assume it. The measurement intersects two inputs: the
   changed line ranges from `git diff <base>...HEAD`, and the per line hit counts
   from `cargo llvm-cov --all-targets --lcov`. A changed line counts once the
   tracefile carries a record for it, and counts as covered once that record
   shows a non zero hit count.
5. Open the pull request with `Closes #N` and the title
   `[rust-migration] Task N: <issue title>`.
6. Review it with at least three review agents, one per axis at minimum:
   - **standards**: `/skill:code-review` against this file and the migration
     conventions, plus the baseline code smells
   - **spec**: `/skill:code-review` of the issue body against the diff, which
     also catches scope creep and claims the pull request body gets wrong
   - **correctness and lean**: `/skill:ponytail-review`, which reads the code
     around the diff rather than only the diff
   Fix what they find, then run another round until a round turns up no request
   for a meaningful change. Skip style taste and skip anything tooling already
   enforces. Reject a finding that contradicts a documented repo convention, and
   quote the convention when rejecting it.
7. Merge once the checks are green, the findings are settled, and the coverage
   gate holds. Then run step 4 of the work order.

### Standing rules

- The C implementation stays linked and untouched until the cutover task. Every
  new C ABI symbol carries the `ffi_` prefix.
- No semicolons and no em dashes in prose, comments, or commit messages.
- Never force-push `main`, and never merge a pull request whose checks are red.

## Additional Resources

- [README.md](README.md) — build, install, benchmark, example.
- [Doxyfile](Doxyfile) — generated API reference (`make documents`).
- `.clang-format` — Linux-kernel C style baseline.
- `Makefile` — authoritative list of targets, flags, and switches.
