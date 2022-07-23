.SUFFIXES : .c .cpp .o
FTL_INCLUDE_PATH = /usr/local/include/ftl

CXX = g++
LIBS = -lftl -lpthread $(shell pkg-config --libs glib-2.0)
CFLAGS = $(shell pkg-config --cflags glib-2.0) -I$(FTL_INCLUDE_PATH)
TARGET = rw_example

all: $(TARGET)

rw_example: rw_example.o
	$(CXX) $(CFLAGS) -o $@ $^ $(LIBS)

clean:
	rm -rf $(TARGET) *.o
