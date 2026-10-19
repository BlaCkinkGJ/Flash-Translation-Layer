# The example is an in-crate cargo target now, so it needs neither the
# installed headers nor the installed archive. `cargo build` names the
# manifest because this directory is not the crate root.
TARGET = rw_example
MANIFEST = ../Cargo.toml
CARGO ?= cargo

.PHONY: all clean

all:
	$(CARGO) build --example $(TARGET) --manifest-path $(MANIFEST)
	cp ../target/debug/examples/$(TARGET) $(TARGET)

clean:
	rm -rf $(TARGET)
