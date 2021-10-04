# You can generate the compile_commands.json file by using
# `bear make all test`

CC = gcc
CXX = g++
TARGET = a.out
TEST_TARGET = lru-test.out bits-test.out ramdisk-test.out

MACROS := -DDEBUG

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
				-Wstrict-prototypes \
				-Wundef \
				-Wold-style-definition
CXXFLAGS := -Wall

UNITY_ROOT := ./unity
LIBS := -lm -lpthread -liberty

INCLUDES := -I./ -I./unity/src

all: $(TARGET)

test: $(TEST_TARGET)
	./lru-test.out
	./bits-test.out

$(TARGET): *.c
	$(CXX) $(MACROS) $(CXXFLAGS) $(INCLUDES) -c *.c $(LIBS)
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ *.c $(LIBS)

lru-test.out: $(UNITY_ROOT)/src/unity.c ./lru.c ./test/lru-test.c
	$(CC) -g -pg $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

bits-test.out: $(UNITY_ROOT)/src/unity.c ./test/bits-test.c
	$(CC) -g -pg $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

ramdisk-test.out: $(UNITY_ROOT)/src/unity.c ./ramdisk.c ./device.c ./bluedbm.c ./test/ramdisk-test.c
	$(CC) -g -pg $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

check:
	@echo "[[ CPPCHECK ROUTINE ]]"
	cppcheck --quiet --enable=all --inconclusive --std=posix -I include/ *.c 
	@echo "[[ FLAWFINDER ROUTINE ]]"
	flawfinder *.c include/*.h

documents:
	doxygen -s Doxyfile

clean:
	@$(RM) *.o ./test/*.o ./unity/src/*.o $(TARGET) $(TEST_TARGET)
	@$(RM) -rf doxygen/

