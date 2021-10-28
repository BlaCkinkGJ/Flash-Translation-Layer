# You can generate the compile_commands.json file by using
# `make clean -j$(nproc)`
# `bear make all -j$(nproc)`
# `compdb -p ./ list > ../compile_commands.json`
# `cp ../compile_commands.json ./
#
# You can get `bear` from [link](https://github.com/rizsotto/Bear)
# You can get `compdb` from [link](https://github.com/Sarcasm/compdb)

# `-fsanitize=address` and `-lasan` for memory leakage checking
# `-g` and `-pg` for tracing the program
# So, you must delete all when you release the program

.SUFFIXES : .c .cpp .o
CC = gcc
AR = ar
CXX = g++
TARGET = a.out
LIBRARY_TARGET = libftl.a

GLIB_INCLUDES = $(shell pkg-config --cflags glib-2.0)
DEVICE_INCLUDES = 

GLIB_LIBS = $(shell pkg-config --libs glib-2.0)

# Device Module Setting
USE_ZONE_DEVICE = 0
USE_BLUEDBM_DEVICE = 1
# Debug Setting
USE_DEBUG = 0

ifeq ($(USE_DEBUG), 1)
DEBUG_FLAGS = -g -pg \
              -DCONFIG_ENABLE_MSG \
              -DCONFIG_ENABLE_DEBUG
MACROS = -DDEBUG
MEMORY_CHECK_LIBS = -lasan
MEMORY_CHECK_CFLAGS = 
MEMORY_CHECK_LIBS += -fsanitize=address
else
DEBUG_FLAGS =
MACROS = -DUSE_GC_MESSAGE
MEMORY_CHECK_LIBS =
MEMORY_CHECK_CFLAGS = 
endif

TEST_TARGET := lru-test.out \
              bits-test.out \
              ramdisk-test.out

DEVICE_LIBS =

ifeq ($(USE_ZONE_DEVICE), 1)
# Zoned Device's Setting
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=3 \
               -DDEVICE_NR_CHIPS_BITS=3 \
               -DDEVICE_NR_PAGES_BITS=5 \
               -DDEVICE_NR_BLOCKS_BITS=21

TEST_TARGET += zone-test.out
DEVICE_LIBS += -lzbd
else ifeq ($(USE_BLUEDBM_DEVICE), 1)
# BlueDBM Device's Setting
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=3 \
               -DDEVICE_NR_CHIPS_BITS=3 \
               -DDEVICE_NR_PAGES_BITS=7 \
               -DDEVICE_NR_BLOCKS_BITS=19 \
               -DUSER_MODE \
               -DUSE_PMU \
               -DUSE_KTIMER \
               -DUSE_NEW_RMW \
               -D_LARGEFILE64_SOURCE \
               -D_GNU_SOURCE \
               -DNOHOST

DEVICE_LIBS += -lmemio
DEVICE_INCLUDES += -I/usr/local/include/memio
else
# Ramdisk Setting
DEVICE_INFO := -DDEVICE_NR_BUS_BITS=3 \
               -DDEVICE_NR_CHIPS_BITS=3 \
               -DDEVICE_NR_PAGES_BITS=7 \
               -DDEVICE_NR_BLOCKS_BITS=19
endif

ifeq ($(USE_ZONE_DEVICE), 1)
DEVICE_INFO += -DDEVICE_USE_ZONED \
	       -DPAGE_FTL_USE_GLOBAL_RWLOCK
endif

ifeq ($(USE_BLUEDBM_DEVICE), 1)
DEVICE_INFO += -DDEVICE_USE_BLUEDBM
endif

ARFLAGS := rcs
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
          $(DEVICE_INFO) \
          $(DEBUG_FLAGS) \
          $(MEMORY_CHECK_CFLAGS) \
          -O3

CXXFLAGS := $(CFLAGS) \
            -std=c++11

UNITY_ROOT := ./unity
LIBS := -lm -lpthread -lstringlib $(GLIB_LIBS) $(DEVICE_LIBS) $(MEMORY_CHECK_LIBS)

INCLUDES := -I./ -I./unity/src $(GLIB_INCLUDES) $(DEVICE_INCLUDES)

RAMDISK_SRCS = device/ramdisk/*.c
ZONED_SRCS =
BLUEDBM_SRCS =

ifeq ($(USE_ZONE_DEVICE), 1)
ZONED_SRCS += device/zone/*.c
endif

ifeq ($(USE_BLUEDBM_DEVICE), 1)
BLUEDBM_SRCS += device/bluedbm/*.c
endif

DEVICE_SRCS := $(RAMDISK_SRCS) \
               $(BLUEDBM_SRCS) \
               $(ZONED_SRCS) \
               device/*.c

UTIL_SRCS := util/*.c

FTL_SRCS := ftl/page/*.c

INTERFACE_SRCS := interface/*.c

SRCS := $(DEVICE_SRCS) \
        $(UTIL_SRCS) \
        $(FTL_SRCS) \
        $(INTERFACE_SRCS)

OBJS := *.o

ifeq ($(PREFIX),)
PREFIX := /usr/local
endif

all: $(TARGET)

test: $(TEST_TARGET)

install:
	install -d $(DESTDIR)$(PREFIX)/lib/
	install -m 644 $(LIBRARY_TARGET) $(DESTDIR)$(PREFIX)/lib
	install -d $(DESTDIR)$(PREFIX)/include/ftl
	install -m 644 include/*.h $(DESTDIR)$(PREFIX)/include/ftl

$(TARGET): main.c $(LIBRARY_TARGET)
	$(CXX) $(MACROS) $(CXXFLAGS) -c main.c $(INCLUDES) $(LIBS)
	$(CXX) $(MACROS) $(CXXFLAGS) -o $@ main.o -L. -lftl -lpthread $(LIBS) $(INCLUDES)

$(LIBRARY_TARGET): $(OBJS)
	$(AR) $(ARFLAGS) $@ $^

$(OBJS): $(SRCS)
	$(CXX) $(MACROS) $(CFLAGS) -c $^ $(LIBS) $(INCLUDES)

lru-test.out: $(UNITY_ROOT)/src/unity.c ./util/lru.c ./test/lru-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

bits-test.out: $(UNITY_ROOT)/src/unity.c ./test/bits-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

ramdisk-test.out: $(UNITY_ROOT)/src/unity.c $(DEVICE_SRCS) ./test/ramdisk-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

ifeq ($(USE_ZONE_DEVICE), 1)
zone-test.out: $(UNITY_ROOT)/src/unity.c $(DEVICE_SRCS) ./test/zone-test.c
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)
endif

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
	rm -f $(TARGET) $(TEST_TARGET) $(LIBRARY_TARGET)
	rm -rf doxygen/

