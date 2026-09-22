# myalloc -- build/test/benchmark.
#
#   mingw32-make test     build and run the automated tests (MYALLOC_DEBUG=1)
#   mingw32-make bench    build and run the benchmark      (MYALLOC_DEBUG=0)
#   mingw32-make all      build both, run neither
#   mingw32-make clean    remove the executables
#
# Equivalent direct commands (from this directory):
#   gcc -std=c17 -O2 -Wall -Wextra -I. -DMYALLOC_DEBUG=1 myalloc.c tests/test_myalloc.c  -o myalloc_test.exe
#   gcc -std=c17 -O2 -Wall -Wextra -I. -DMYALLOC_DEBUG=0 myalloc.c tests/bench_myalloc.c -o myalloc_bench.exe
#
# The tests need MYALLOC_DEBUG=1 (validation + layout accessors); the
# benchmark needs MYALLOC_DEBUG=0 so validation never pollutes timing.

# `CC ?= gcc` would not work: make predefines CC as "cc".  Only replace
# that built-in default; an explicit `make CC=...` or environment CC wins.
ifeq ($(origin CC),default)
CC := gcc
endif
CSTD    ?= -std=c17
OPT     ?= -O2
WARN    := -Wall -Wextra -Wshadow -Wundef -Wpointer-arith -Wstrict-prototypes \
           -Wmissing-prototypes -Wwrite-strings -Wpedantic
CFLAGS  ?= $(CSTD) $(OPT) $(WARN)
LDFLAGS ?=

TESTSRC := tests/test_myalloc.c
BENCHSRC := tests/bench_myalloc.c
HDRS    := myalloc.h

all: myalloc_test.exe myalloc_bench.exe

myalloc_test.exe: myalloc.c $(HDRS) $(TESTSRC)
	$(CC) $(CFLAGS) -DMYALLOC_DEBUG=1 -I. myalloc.c $(TESTSRC) $(LDFLAGS) -o $@

myalloc_bench.exe: myalloc.c $(HDRS) $(BENCHSRC)
	$(CC) $(CFLAGS) -DMYALLOC_DEBUG=0 -I. myalloc.c $(BENCHSRC) $(LDFLAGS) -o $@

test: myalloc_test.exe
	./myalloc_test.exe

bench: myalloc_bench.exe
	./myalloc_bench.exe

clean:
	-$(RM) myalloc_test.exe myalloc_bench.exe

.PHONY: all test bench clean
