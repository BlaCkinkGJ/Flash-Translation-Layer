FROM ubuntu:18.04

RUN apt-get update && apt-get clean
# Due to ensure supporting latest packages, mute to specify versioning alert
# shellcheck disable=DL3008
RUN apt-get install --no-install-recommends -y git make gcc g++ libglib2.0-dev libiberty-dev
# shellcheck disable=DL3008
RUN apt-get install --no-install-recommends -y cppcheck flawfinder cflow
# shellcheck disable=DL3008
RUN apt-get install --no-install-recommends -y python3-pip
# shellcheck disable=DL3008
RUN apt-get install --no-install-recommends -y bear
# shellcheck disable=DL3008
RUN apt-get install --no-install-recommends -y zsh
RUN apt-get clean
RUN rm -rf /var/lib/apt/lists/*

# shellcheck disable=SC2046
RUN chsh -s `which zsh`
RUN pip3 install lizard==1.17.10
RUN pip3 install compdb==0.2.0

RUN mkdir -p /ftl
WORKDIR /ftl

CMD make
