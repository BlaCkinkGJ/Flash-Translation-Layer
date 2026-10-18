.SUFFIXES : .c .cpp .o
FTL_INCLUDE_PATH = /usr/local/include/ftl

CXX = g++
# `libftl.a` calls into `libftl_rust.a`, so an installed consumer links both.
LIBS = -lftl -lftl_rust -lpthread
CFLAGS = -I$(FTL_INCLUDE_PATH)
TARGET = rw_example

all: $(TARGET)

rw_example: rw_example.o
	$(CXX) $(CFLAGS) -o $@ $^ $(LIBS)

clean:
	rm -rf $(TARGET) *.o
