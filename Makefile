CC = gcc
CXX = g++

CFLAGS = -Wall
CXXFLAGS = $(CFLAGS)

LIBS = -lm -lpthread

TARGET = a.out

all: $(TARGET)

$(TARGET): *.c
	$(CXX) $(CXXFLAGS) -c *.c $(LIBS)
	$(CC) $(CFLAGS) -o $@ *.c $(LIBS)

clean:
	@$(RM) *.o $(TARGET)

