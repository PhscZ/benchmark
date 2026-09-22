/*
 * bench_myalloc.c -- throughput and footprint benchmark for myalloc.
 *
 * Build with MYALLOC_DEBUG=0 so no validation code is in the measured
 * path (the Makefile's `bench` target does exactly that):
 *   gcc -std=c17 -O2 -Wall -Wextra -I. -DMYALLOC_DEBUG=0 \
 *       myalloc.c tests/bench_myalloc.c -o myalloc_bench.exe
 *   myalloc_bench.exe
 *
 * Reports, per workload: operations per second, nanoseconds per
 * operation, and the allocator's own view of peak managed memory and OS
 * region activity (mya_get_stats()).
 */

#include "myalloc.h"

#include <stdint.h>
#include <stdio.h>

#if MYALLOC_DEBUG
#error "build the benchmark with -DMYALLOC_DEBUG=0 (release measurements)"
#endif

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#define FMT_U64 "%llu"
#define U64(x) ((unsigned long long)(x))

static double now_seconds(void)
{
    static LARGE_INTEGER freq;
    LARGE_INTEGER counter;

    if (freq.QuadPart == 0) {
        QueryPerformanceFrequency(&freq);
    }
    QueryPerformanceCounter(&counter);
    return (double)counter.QuadPart / (double)freq.QuadPart;
}

static uint64_t rng_next(uint64_t *state)
{
    uint64_t x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    return x;
}

static volatile unsigned char g_sink;

static void report(const char *label, size_t ops, double seconds, size_t touched)
{
    mya_stats_t st;
    mya_get_stats(&st);
    printf("%-26s %10llu ops  %8.2f Mops/s  %8.1f ns/op  "
           "peak_os=%8llu KiB  reserves=%6llu  releases=%6llu  "
           "peak_live=%9llu B\n",
           label, U64(ops),
           seconds > 0.0 ? (double)ops / seconds / 1e6 : 0.0,
           ops > 0 && seconds > 0.0 ? seconds * 1e9 / (double)ops : 0.0,
           U64(st.peak_os_bytes_live / 1024u), U64(st.os_reserve_count),
           U64(st.os_release_count), U64(st.peak_live_requested_bytes));
    g_sink = (unsigned char)(touched + st.blocks_total);
}

/* malloc + touch + free, one block at a time. */
static void bench_pair(const char *label, size_t size, size_t iters)
{
    size_t i;
    double t0, dt;

    mya_teardown();
    t0 = now_seconds();
    for (i = 0; i < iters; i++) {
        void *p = my_malloc(size);
        if (p == NULL) {
            printf("%s: allocation failed\n", label);
            return;
        }
        ((unsigned char *)p)[0] = (unsigned char)i;
        ((unsigned char *)p)[size - 1] = (unsigned char)i;
        my_free(p);
    }
    dt = now_seconds() - t0;
    report(label, iters, dt, size);
}

/* Keep `count` blocks alive, then release them all, repeatedly. */
static void bench_pool(const char *label, size_t size, size_t count, size_t rounds)
{
    enum { MAX_SLOTS = 8192 };
    static void *slots[MAX_SLOTS];
    size_t r, i;
    double t0, dt;

    if (count > MAX_SLOTS) {
        count = MAX_SLOTS;
    }
    mya_teardown();
    t0 = now_seconds();
    for (r = 0; r < rounds; r++) {
        for (i = 0; i < count; i++) {
            void *p = my_malloc(size);
            if (p == NULL) {
                printf("%s: allocation failed\n", label);
                return;
            }
            ((unsigned char *)p)[0] = (unsigned char)i;
            slots[i] = p;
        }
        for (i = 0; i < count; i++) {
            my_free(slots[i]);
        }
    }
    dt = now_seconds() - t0;
    report(label, rounds * count * 2u, dt, size * count);
}

/* Repeated realloc growth on a single allocation. */
static void bench_realloc_grow(const char *label, size_t rounds)
{
    size_t r;
    double t0, dt;
    size_t ops = 0;

    mya_teardown();
    t0 = now_seconds();
    for (r = 0; r < rounds; r++) {
        size_t size = 64;
        void *p = my_malloc(size);
        if (p == NULL) {
            printf("%s: allocation failed\n", label);
            return;
        }
        while (size < (1u << 18)) {
            void *np = my_realloc(p, size * 2u);
            ops++;
            if (np == NULL) {
                break;
            }
            p = np;
            size *= 2u;
        }
        my_free(p);
    }
    dt = now_seconds() - t0;
    report(label, ops, dt, 1u << 18);
}

/* Deterministic mixed malloc/free/realloc workload, random sizes. */
static void bench_mixed(const char *label, size_t ops)
{
    enum { SLOTS = 1024 };
    static void *slots[SLOTS];
    uint64_t rs = 0x9E3779B97F4A7C15ull;
    size_t i;
    double t0, dt;

    mya_teardown();
    t0 = now_seconds();
    for (i = 0; i < ops; i++) {
        uint64_t r = rng_next(&rs);
        size_t idx = (size_t)(r % SLOTS);
        if (slots[idx] == NULL) {
            size_t sz = 1u + (size_t)((r >> 20) % 8192u);
            void *p = my_malloc(sz);
            if (p != NULL) {
                ((unsigned char *)p)[0] = 1;
                slots[idx] = p;
            }
        } else if (((r >> 32) % 3u) == 0u) {
            size_t nsz = 1u + (size_t)((r >> 40) % 8192u);
            void *np = my_realloc(slots[idx], nsz);
            if (np != NULL) {
                ((unsigned char *)np)[0] = 1;
                slots[idx] = np;
            }
        } else {
            my_free(slots[idx]);
            slots[idx] = NULL;
        }
    }
    for (i = 0; i < SLOTS; i++) {
        if (slots[i] != NULL) {
            my_free(slots[i]);
            slots[i] = NULL;
        }
    }
    dt = now_seconds() - t0;
    report(label, ops, dt, 1);
}

int main(void)
{
    printf("myalloc benchmark (MYALLOC_DEBUG=0, release measurements)\n");
    printf("64-bit Windows, MinGW-w64 GCC, C17\n\n");

    bench_pair("malloc/free 16 B", 16, 4000000u);
    bench_pair("malloc/free 64 B", 64, 4000000u);
    bench_pair("malloc/free 256 B", 256, 3000000u);
    bench_pair("malloc/free 1 KiB", 1024, 3000000u);
    bench_pair("malloc/free 4 KiB", 4096, 2000000u);
    bench_pair("malloc/free 512 KiB", 512u << 10, 500000u);   /* spare region reuse */
    bench_pair("malloc/free 2 MiB", 2u << 20, 100000u);       /* dedicated region  */
    printf("\n");
    bench_pool("pool 64 B x1024", 64, 1024, 2000u);
    bench_pool("pool 1 KiB x1024", 1024, 1024, 2000u);
    bench_pool("pool 32 KiB x256", 32u << 10, 256, 2000u);
    printf("\n");
    bench_realloc_grow("realloc grow 64->256K", 20000u);
    printf("\n");
    bench_mixed("mixed random", 2000000u);
    printf("\n");

    {
        mya_stats_t st;
        mya_teardown();
        mya_get_stats(&st);
        printf("after teardown: regions=%llu live=%llu os_bytes_live=%llu\n",
               U64(st.regions_live), U64(st.live_allocations), U64(st.os_bytes_live));
    }
    return 0;
}
