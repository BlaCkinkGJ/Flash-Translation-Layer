FROM ubuntu:18.04

RUN apt-get update && apt-get clean
RUN apt-get install --no-install-recommends -y git make gcc g++ curl
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"
RUN apt-get install --no-install-recommends -y cppcheck flawfinder cflow
RUN apt-get install --no-install-recommends -y python3-pip
RUN apt-get install --no-install-recommends -y bear
RUN apt-get install --no-install-recommends -y zsh
RUN apt-get clean
RUN rm -rf /var/lib/apt/lists/*

RUN chsh -s $(which zsh)
RUN pip3 install lizard==1.17.10
RUN pip3 install compdb==0.2.0

RUN mkdir -p /ftl
WORKDIR /ftl

CMD [ "make" ]
