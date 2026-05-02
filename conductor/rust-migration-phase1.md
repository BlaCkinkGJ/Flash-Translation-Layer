# Rust Migration Phase 1: Infrastructure Setup & Leaf Utility Migration

## 1. Background & Motivation
The Flash Translation Layer (FTL) project is transitioning to Rust to leverage memory safety and modern concurrency primitives. Phase 1 focuses on setting up the infrastructure for a hybrid C/Rust project and migrating the `crc32` utility as a proof of concept.

## 2. Objective
- Integrate Rust (`cargo`) into the C build system.
- Replace `util/crc32.c` with a Rust implementation using `crc32fast`.
- Update project metadata, documentation, and CI/CD pipelines to support the new toolchain.

## 3. Mandatory Testing & Verification (NON-NEGOTIABLE)
**Every migration step MUST include:**
- **Rust Unit Tests:** Each new Rust module must have inline `#[cfg(test)]` tests covering basic functionality and edge cases.
- **FFI Boundary Verification:** Tests ensuring that data passed from C to Rust (and vice-versa) is correctly interpreted.
- **Behavioral Parity:** Existing C test suites (e.g., `test/crc32-test.c`) must pass when linked against the Rust implementation.
- **Performance Benchmarking:** Verify that the Rust implementation (especially when using crates like `crc32fast`) meets or exceeds existing performance.

## 4. Documentation & Metadata Updates
- **README.md:** Update to include Rust as a project dependency. Add instructions for setting up the Rust environment (rustup, cargo).
- **Architecture Docs:** Document the hybrid build process and the FFI boundary management.
- **Source Comments:** Clear documentation on `extern "C"` functions in Rust and their corresponding headers in C.

## 5. Infrastructure (CI/CD) Updates
- **Dockerfile:** 
    - Install `rustup` and the stable Rust toolchain.
    - Pre-fetch dependencies to speed up container builds.
- **GitHub Actions:**
    - Update workflows to include `actions-rs/toolchain`.
    - Add steps to run `cargo test` and `cargo fmt --check`.
    - Ensure `make test` correctly links the Rust library in the CI environment.

## 6. Proposed Implementation Steps
1. **Infrastructure (Cargo):** Create `Cargo.toml` with `staticlib` target and `crc32fast` dependency.
2. **Implementation (Rust):** Implement `crc32` in Rust, including full unit tests.
3. **Integration (Makefile):**
    - Modify `Makefile` to invoke `cargo build`.
    - Link `libftl_rs.a` into binaries.
    - Remove `util/crc32.c` from the source list.
4. **DevOps (CI/CD):** Update `Dockerfile` and GitHub Actions scripts.
5. **Documentation:** Update `README.md` and related documentation files.
6. **Final Validation:** Comprehensive run of all tests (C tests, Rust tests, and Integration tests).

## 7. Migration & Rollback Strategy
- **Rollback:** Work is performed on the `migration/rust-phase-1` branch. Reverting is as simple as switching back to `main`.