@echo off
rem myalloc -- build, test and benchmark with MinGW-w64 GCC on 64-bit Windows.
rem Usage: build.bat   (run from the directory containing myalloc.c)
setlocal

set CC=gcc
set CFLAGS=-std=c17 -O2 -Wall -Wextra -Wshadow -Wundef -Wpointer-arith -Wstrict-prototypes -Wmissing-prototypes -Wwrite-strings -Wpedantic

echo [build] tests      (MYALLOC_DEBUG=1)
%CC% %CFLAGS% -DMYALLOC_DEBUG=1 -I. myalloc.c tests\test_myalloc.c -o myalloc_test.exe || exit /b 1

echo [build] benchmark  (MYALLOC_DEBUG=0)
%CC% %CFLAGS% -DMYALLOC_DEBUG=0 -I. myalloc.c tests\bench_myalloc.c -o myalloc_bench.exe || exit /b 1

echo [run] tests
myalloc_test.exe || exit /b 1

echo [run] benchmark
myalloc_bench.exe || exit /b 1

endlocal
