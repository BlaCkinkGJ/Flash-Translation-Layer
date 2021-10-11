# You can generate the compile_commands.json file by using
# `bear make all test`

# `-fsanitize=address` and `-lasan` for memory leakage checking
# `-g` and `-pg` for tracing the program
# So, you must delete all when you release the program

CC = gcc
CXX = g++
TARGET = a.out
TEST_TARGET = lru-test.out bits-test.out ramdisk-test.out zone-test.out

MACROS := -DDEBUG

GLIB_INCLUDES = $(shell pkg-config --cflags glib-2.0)
DEVICE_INCLUDES = 

GLIB_LIBS = $(shell pkg-config --libs glib-2.0)
DEVICE_LIBS = -lzbd

# Ramdisk and BlueDBM Setting
# DEVICE_INFO := -D DEVICE_NR_BUS_BITS=3 \
#                -D DEVICE_NR_CHIPS_BITS=3 \
#                -D DEVICE_NR_PAGES_BITS=7 \
#                -D DEVICE_NR_BLOCKS_BITS=19 \

# Zoned Device's Setting
DEVICE_INFO := -D DEVICE_NR_BUS_BITS=3 \
               -D DEVICE_NR_CHIPS_BITS=3 \
               -D DEVICE_NR_PAGES_BITS=5 \
               -D DEVICE_NR_BLOCKS_BITS=21 \

CFLAGS := -Wall \
          -Wextra \
          -Wpointer-arith \
          -Wcast-align \
          -Wwrite-strings \
          -Wswitch-default \
          -Wunreachable-code \
          -Winit-self \
          -Wmissing-field-initializers \
          -Wno-unknown-pragmas \
          -Wundef \
          -fsanitize=address \
          $(DEVICE_INFO) \
          -g -pg
CXXFLAGS := $(CFLAGS)

UNITY_ROOT := ./unity
LIBS := -lm -lpthread -lasan $(GLIB_LIBS) $(DEVICE_LIBS)

INCLUDES := -I./ -I./unity/src $(GLIB_INCLUDES) $(DEVICE_INCLUDES)
DEVICE_SRCS := device/ramdisk/*.c \
               device/bluedbm/*.c \
               device/zone/*.c \
               device/*.c

UTIL_SRCS := util/*.c

FTL_SRCS := ftl/page/*.c

INTERFACE_SRCS := interface/*.c

SRCS := $(DEVICE_SRCS) \
        $(UTIL_SRCS) \
        $(FTL_SRCS) \
        $(INTERFACE_SRCS) \
        main.c

all: $(TARGET)

test: $(TEST_TARGET)

$(TARGET): $(SRCS)
	$(CXX) $(MACROS) $(CXXFLAGS) $(INCLUDES) -c $^ $(LIBS)
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

lru-test.out: $(UNITY_ROOT)/src/unity.c ./util/lru.c ./test/lru-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

bits-test.out: $(UNITY_ROOT)/src/unity.c ./test/bits-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

ramdisk-test.out: $(UNITY_ROOT)/src/unity.c $(DEVICE_SRCS) ./test/ramdisk-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

zone-test.out: $(UNITY_ROOT)/src/unity.c $(DEVICE_SRCS) ./test/zone-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

check:
	@echo "[[ CPPCHECK ROUTINE ]]"
	cppcheck --quiet --enable=all --inconclusive --std=posix -I include/ $(SRCS)
	@echo "[[ FLAWFINDER ROUTINE ]]"
	flawfinder $(SRCS) include/*.h
	@echo "[[ STATIC ANALYSIS ROUTINE ]]"
	lizard $(SRCS) include/*.h

documents:
	doxygen -s Doxyfile

flow:
	find . -type f -name '*.[ch]' ! -path "./unity/*" ! -path "./test/*" | xargs -i cflow {}

clean:
	find . -name '*.o'  | xargs -i rm -f {}
	rm -f $(TARGET) $(TEST_TARGET)
	rm -rf doxygen/

