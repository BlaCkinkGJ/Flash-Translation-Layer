FROM ubuntu:22.04

RUN apt-get update
# The Linux toolchain: `gcc` plus `libc6-dev` for the linker the Rust build and
# `make consumer-check` need, `curl` and `ca-certificates` for rustup below,
# and `libzbd-dev` for the `zone` feature's hand-written binding.
RUN apt-get install --no-install-recommends -y git make gcc libc6-dev curl ca-certificates libzbd-dev
# The static analysis tools keep the ABI headers under `include/` as input.
RUN apt-get install --no-install-recommends -y cppcheck flawfinder cflow
RUN apt-get install --no-install-recommends -y python3-pip
RUN apt-get install --no-install-recommends -y zsh
RUN apt-get clean
RUN rm -rf /var/lib/apt/lists/*

# `make` drives cargo now, so the image carries the Rust toolchain. rustup
# installs the pinned stable release the tree builds against. The installer is
# fetched to a file first, because a `curl ... | sh` pipe trips hadolint's
# DL4006.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.99.1

RUN chsh -s $(which zsh)
RUN pip3 install lizard==1.17.0

WORKDIR /ftl

CMD [ "make" ]
