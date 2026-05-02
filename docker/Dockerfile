FROM rust:latest

# Install C/C++ build tools and analysis tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    make \
    gcc \
    g++ \
    cppcheck \
    flawfinder \
    cflow \
    python3-pip \
    bear \
    zsh \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Install Python-based tools
RUN pip3 install --no-cache-dir lizard==1.17.10 compdb==0.2.0 --break-system-packages

# Set up work directory
RUN mkdir -p /ftl
WORKDIR /ftl

# Set up ZSH (optional for dev)
RUN chsh -s $(which zsh)

CMD [ "make" ]
