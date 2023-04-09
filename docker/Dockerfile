FROM ubuntu:18.04

ENV LIBCLANG_PATH=/usr/lib/llvm-6.0/lib
ENV PATH="/root/.cargo/bin:$PATH"

RUN apt-get update && apt-get clean
RUN apt-get install --no-install-recommends -y git make gcc g++ libglib2.0-dev libiberty-dev
RUN apt-get install --no-install-recommends -y cppcheck flawfinder cflow
RUN apt-get install --no-install-recommends -y python3-pip
RUN apt-get install --no-install-recommends -y bear
RUN apt-get install --no-install-recommends -y curl
RUN apt-get install --no-install-recommends -y zsh
RUN apt-get install --no-install-recommends -y libclang-dev clang libbz2-dev
RUN apt-get clean
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN curl https://sh.rustup.rs -sSf | bash -s -- -y
RUN echo 'source $HOME/.cargo/env' >> $HOME/.bashrc
RUN rm -rf /var/lib/apt/lists/*

RUN chsh -s $(which zsh)
RUN pip3 install lizard==1.17.10
RUN pip3 install compdb==0.2.0

RUN mkdir -p /ftl
WORKDIR /ftl

CMD [ "make" ]
