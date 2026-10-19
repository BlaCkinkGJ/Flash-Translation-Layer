FROM ubuntu:22.04

RUN apt-get update && apt-get clean
RUN apt-get install --no-install-recommends -y git make gcc libc6-dev curl ca-certificates
RUN apt-get install --no-install-recommends -y cppcheck flawfinder cflow
RUN apt-get install --no-install-recommends -y python3-pip
RUN apt-get install --no-install-recommends -y zsh
# The Linux-only library the `zone` feature binds, which is what makes the
# image useful on the macOS beyond a reproducible build.
RUN apt-get install --no-install-recommends -y libzbd-dev
RUN apt-get clean
RUN rm -rf /var/lib/apt/lists/*

# `make` drives cargo now, so the image carries the Rust toolchain. rustup is
# pinned to the stable the tree builds against and needs `curl` plus the CA
# store from above. `cppcheck`, `flawfinder` and `cflow` above keep their
# input: the ABI headers under `include/`.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain 1.99.1

RUN chsh -s $(which zsh)
RUN pip3 install lizard==1.17.10

RUN mkdir -p /ftl
WORKDIR /ftl

CMD [ "make" ]
