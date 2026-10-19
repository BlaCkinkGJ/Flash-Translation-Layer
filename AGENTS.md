# AGENTS.md

Machine-readable guide for AI coding assistants. For human-facing docs see
[README.md](README.md). Per-directory notes may exist in nested `AGENTS.md`
files (nearest file wins).

## Project Snapshot

- **Domain**: NAND Flash Translation Layer (page-mapping FTL), Linux userspace.
- **Language**: Rust only. The FTL, the helpers, the device dispatch, the
  interfaces, the four device backends, the example, the end to end program and
  the benchmark are all Rust. The C surface is the headers in `include/`, which
  declare the C ABI `libftl_rust.a` exports. macOS is Docker only.
- **Backends**: ramdisk (default), zoned block device, bluedbm and raspberrypi,
  all Rust, the three hardware ones behind a cargo feature.

## Repository Layout

```text
include/           Public C headers (installed to $(PREFIX)/include/ftl)
src/               Rust sources (cargo staticlib → libftl_rust.a), FTL under
                   src/ftl/
tests/             Rust integration test (integration.rs)
example/           Rust example program (rw_example), a cargo example
benchmark.rs        Performance benchmark → benchmark.out
integration-test.rs End-to-end test → integration-test.out
```

## Setup And Build

Linux (Ubuntu >= 16.04) needs `git make gcc` and the Rust toolchain (`cargo`).
`gcc` is only used by `make consumer-check`, the one C compilation left.
Backends and tooling are optional: `USE_ZONE_DEVICE=1` needs
[libzbd](https://github.com/westerndigitalcorporation/libzbd),
`USE_BLUEDBM_DEVICE=1` needs
[libmemio](https://github.com/pnuoslab/Flash-Board-Tester),
`USE_RASPBERRY_DEVICE=1` needs `libnand` and `wiringPi`. The backends bind
those libraries by hand, so a consumer of that build links them too.
`make check` needs `cppcheck`, `flawfinder` and `lizard==1.17.0`,
`make flow` needs `cflow`, and `make documents` needs `doxygen`. macOS has no
native build: use Docker (`make docker-builder`, then `make docker-make-test`).

```bash
make clean && make -j$(nproc)   # archive, integration-test.out, benchmark.out
make clean && make test USE_LOG_SILENT=1  # Rust tests
make integration-test                     # end-to-end, takes minutes
make zone-smoke                           # zoned backend vs the null_blk emulator
make clean && make benchmark.out          # USE_LEGACY_RANDOM=1 on random errors
make consumer-check                       # C consumer vs a staging install
sudo make install                         # archive + headers under /usr/local
make documents | make flow | make check   # docs, call graph, static analysis
make cargo-clippy   # = cargo clippy --all-targets -- -D warnings
make cargo-coverage # cargo llvm-cov --lcov (llvm-tools + cargo-llvm-cov)
```

Each `USE_*_DEVICE` switch enables the cargo feature of the same name (`zone`,
`bluedbm`, `raspberry`), which selects that backend into the Rust device table,
and names the library the switch needs in `DEVICE_LIBS`. `USE_DEBUG=1` builds
the debug cargo profile instead of release. The rest: `USE_LOG_SILENT=1`
silences the info logs (`silent`) and `USE_LEGACY_RANDOM=1` falls back to the
legacy generator (`legacy-random`). The switches live in the `Makefile`, which
is authoritative.

## Code Style

### C headers (`include/*.h`, Linux-kernel style, see `.clang-format`)

- Tabs, width 8, column limit 80. Allman braces for functions, K&R-like for
  control flow. Pointer alignment right (`int *p`). `make format` runs
  clang-format against `.clang-format` over `include/*.h`.
- A header declares only what `libftl_rust.a` exports or what a C consumer
  needs to read it, and a declaration goes in the same change that deletes its
  implementation.
- Every `.h` carries the doxygen `@file @brief @author @version @date` block, a
  `#ifndef FOO_H` guard, and `extern "C"` around public declarations for C++
  callers. No C++ in the repository: `g++` is not needed at all.
- Every exported symbol carries the `ffi_` prefix. `crc32` (`src/crc32.rs`,
  `include/crc32.h`) predates the migration and keeps its plain name.
- A struct both sides see stays `#[repr(C)]` with its offsets asserted on both
  sides: `include/device.h`, `include/flash.h`, `include/list.h` and
  `include/lru.h` against `src/device/mod.rs`, `src/flash.rs`, `src/list.rs` and
  `src/lru.rs`. Drift breaks a build, not memory at runtime.
- Logging: one JSON object per line from the Rust side (`src/log.rs`), silenced
  by `USE_LOG_SILENT=1`. `include/log.h` is what a C consumer uses.

### Rust

- `src/lib.rs` is the crate root, built as `staticlib` (what C links) and
  `rlib` (what `tests/` link). Edition 2021. Dependencies: `log`, `env_logger`
  (wrapped by `src/log.rs`) and `libc` for the C ABI types. `RUST_LOG` filters.
- `cargo clippy --all-targets -- -D warnings` is a build blocker and is the
  `-Werror` policy now that no C source is compiled here.
- Every C-visible symbol sits in an `extern "C"` function beside the native code
  it wraps, and documents its safety contract.
- Never run `cargo fmt`: the tree is hand wrapped to 80 columns.

## Testing

Rust tests are unit tests beside the code plus `tests/integration.rs`, run by
`make test` (`cargo test --all-targets` with the derived feature flags). They
cover success and error paths, and mock nothing device specific: use the
ramdisk backend. `make consumer-check` is the C ABI gate: it installs into a
staging prefix, compiles a consumer against the installed headers with
`-Wall -Wextra -Werror`, links the archive and runs it over the ramdisk.

## Project Conventions

### Adding a new device backend

Implement it in `src/device/<name>.rs`, add a `USE_<NAME>_DEVICE` switch, a
`DEVICE_INFO` block (the geometry the headers' inline accessors describe), a
`DEVICE_LIBS` entry (the library the backend binds) and a cargo feature
mirroring the existing ones, wire the entry into the table in
`src/device/mod.rs`, declare its `ffi_` pair in a header under `include/`, and
add a Rust test beside the backend.

### Adding a new FTL policy

New module under `src/ftl/`, declared in `src/ftl/mod.rs`. Register its C ABI
entry point in `src/ftl/page_interface.rs` and declare it in `include/page.h`
when C calls it. Keep allocation, GC ownership and locking in one place, with
Rust owning the FTL state and the device IO.

### Mixing Rust

New Rust modules go in `src/`, re-exported from `src/lib.rs`. A C caller
reaches them through `extern "C"` declarations in a header under `include/`,
which stays the ABI contract: Rust owns the logic and the IO. A struct both
sides see stays `#[repr(C)]` with its offsets asserted on one side, and a
Rust-only type carries no repr at all.

## Permissions

Allowed without prompting: reading any file, compiling one Rust source with the
project flags, `cargo build` / `check` / `test`, editing sources, headers,
manifests and tests, `make check` / `flow` over the headers, `make
consumer-check`, and `clang-format -i` on touched files.

Ask before: `make install` (writes to `/usr/local`), a full `make clean && make
test` / `integration-test` / `benchmark.out` (slow), `git commit`, `git push`,
branch, force-push or history rewrites, modifying `Makefile`, `Cargo.toml`,
`.clang-format`, `Doxyfile` or the CI workflow, adding or removing dependencies,
deleting files or directories, and anything that touches the real hardware
backends (zoned / bluedbm / raspberry).

Never: commit secrets or user-specific paths, commit build artifacts (`*.o`,
`*.out`, `*.a`, `target/`, `doxygen/`), bypass `-Werror` (clippy
`-D warnings` or the consumer check's flags), or hand-edit generated files. One
focused, formatted change per pull request.

This is a local userspace library with no credentials and no network IO. Never
commit device paths from the maintainer's environment: backend configuration is
a build-time concern (`USE_*_DEVICE` flags), not data. `make check` (cppcheck,
flawfinder, lizard) is the first line of review.

## Maintenance

- This file is living documentation. Update it in the same PR that changes a
  build flag, target, test, or convention.
- Keep under ~200 lines. If it grows, split it into per-directory `AGENTS.md`
  files (e.g. `src/device/AGENTS.md` for backend specifics).
- Test the file: ask an AI assistant to do a common task using only it, and fix
  anything it gets wrong.

## AI work instructions (orchestrator only)

**These govern the orchestrating agent alone.** A sub agent follows the single
task brief it is handed, plus the rest of this file. The orchestrator may
commit, push, create branches and worktrees and merge pull requests once the
gates below are met, without asking, as the one exception to `Ask before`.

**Work order.** Land the open pull requests first, then take the rest in
dependency order, running issues that touch disjoint files in parallel, one
worktree and one sub agent each, so that no two agents edit the same file. Tell
every agent still working to rebase onto the new `main` when a pull request
merges, and file an issue the moment the work uncovers one.

**Per issue.** The issue body is the binding spec, not a summary of it, so read
it and read the code it names. For a port, implement against the original and
compare the two behaviours before calling it done, down to return values, error
codes, ownership and free ordering, allocation failure paths, null and out of
range inputs, struct layout and counter updates. CI must pass. Line coverage of
the lines the pull request adds or changes must reach 90 per cent, measured by
intersecting `git diff <base>...HEAD` with the per line hits from
`cargo llvm-cov --all-targets --lcov`. Open the pull request with `Closes #N`
and a title naming the task, then review it with at least three agents, one
axis each: standards (`/skill:code-review` against this file), spec (the issue
body against the diff) and correctness and lean (`/skill:ponytail-review`).
Fix what they find, repeat until a round finds nothing meaningful, rejecting a
finding that contradicts a documented convention by quoting it. Merge once the
checks are green, the findings are settled and the coverage gate holds.

**Standing rules.**

- The cutover is done: every implementation is Rust, and the C surface is the
  exported ABI headers in `include/`. New C ABI symbols carry the `ffi_` prefix,
  dead C sources and the declarations of deleted symbols are deleted with them.
- No semicolons and no em dashes in prose, comments, or commit messages.
- Never force-push `main`, and never merge a pull request whose checks are red.
