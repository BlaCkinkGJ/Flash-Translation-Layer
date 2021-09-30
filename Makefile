CC = gcc
CXX = g++
TARGET = a.out
TEST_TARGET = van-emde-test.out lru-test.out bits-test.out

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

$(TARGET): *.c
	$(CXX) $(MACROS) $(CXXFLAGS) $(INCLUDES) -c *.c $(LIBS)
	$(CC) $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ *.c $(LIBS)

van-emde-test.out: $(UNITY_ROOT)/src/unity.c ./van-emde-boas.c ./test/van-emde-boas-test.c
	$(CC) -g -pg $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

lru-test.out: $(UNITY_ROOT)/src/unity.c ./lru.c ./test/lru-test.c
	$(CC) -g -pg $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

bits-test.out: $(UNITY_ROOT)/src/unity.c ./test/bits-test.c
	$(CC) -g -pg $(MACROS) $(CFLAGS) $(INCLUDES) -o $@ $^ $(LIBS)

clean:
	@$(RM) *.o ./test/*.o ./unity/src/*.o $(TARGET) $(TEST_TARGET)

