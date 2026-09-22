/*
 * test_myalloc.c -- automated tests for the myalloc allocator.
 *
 * Built with -DMYALLOC_DEBUG=1 so mya_debug_validate() and the layout
 * accessors are available.  The harness itself uses the C runtime
 * allocator (printf/strcmp only, plus no allocation at all in the test
 * bodies), which is the point of the my_ prefix.
 *
 * Build / run (from the project root):
 *   gcc -std=c17 -O2 -Wall -Wextra -I. -DMYALLOC_DEBUG=1 \
 *       myalloc.c tests/test_myalloc.c -o myalloc_test.exe
 *   myalloc_test.exe
 */

#include "myalloc.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if !MYALLOC_DEBUG
#error "build the tests with -DMYALLOC_DEBUG=1"
#endif

/* ================================================================== */
/* Tiny test harness                                                   */
/* ================================================================== */

static int g_checks;
static int g_failures;

#define CHECK(cond)                                                     \
    do {                                                                \
        g_checks++;                                                     \
        if (!(cond)) {                                                  \
            printf("  FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);    \
            g_failures++;                                               \
        }                                                               \
    } while (0)

#define CHECK_VALID()                                                   \
    do {                                                                \
        g_checks++;                                                     \
        if (mya_debug_validate() != 0) {                                \
            printf("  FAIL %s:%d: mya_debug_validate()\n", __FILE__, __LINE__); \
            g_failures++;                                               \
        }                                                               \
    } while (0)

#define FMT_U64 "%llu"
#define U64(x) ((unsigned long long)(x))

/* ================================================================== */
/* Counting / failing OS layer                                         */
/* ================================================================== */

static const mya_os_ops_t *g_default_ops;
static long g_reserve_calls;
static long g_release_calls;
static long g_fail_countdown = -1; /* -1: never fail; 0: fail the next reserve */
static size_t g_os_live_bytes;

static void *counting_reserve(size_t size)
{
    void *p;
    if (g_fail_countdown == 0) {
        return NULL;
    }
    if (g_fail_countdown > 0) {
        g_fail_countdown--;
    }
    p = g_default_ops->reserve(size);
    if (p != NULL) {
        g_reserve_calls++;
        g_os_live_bytes += size;
    }
    return p;
}

static int counting_release(void *base, size_t size)
{
    g_release_calls++;
    g_os_live_bytes -= size;
    return g_default_ops->release(base, size);
}

static size_t counting_granularity(void)
{
    return g_default_ops->granularity();
}

static const mya_os_ops_t g_counting_ops = {
    counting_reserve,
    counting_release,
    counting_granularity
};

static void reset_os_counters(void)
{
    g_reserve_calls = 0;
    g_release_calls = 0;
    g_os_live_bytes = 0;
    g_fail_countdown = -1;
}

/* Fresh allocator state, optionally behind the counting OS layer. */
static void reset_allocator(int counting)
{
    mya_teardown(); /* releases with the currently installed layer */
    reset_os_counters();
    mya_set_os_ops(counting ? &g_counting_ops : NULL);
}

/* ================================================================== */
/* Pattern helpers                                                     */
/* ================================================================== */

static void fill_pattern(void *p, size_t n, uint64_t seed)
{
    unsigned char *b = (unsigned char *)p;
    size_t i;
    for (i = 0; i < n; i++) {
        b[i] = (unsigned char)(seed + i * 131u + (i >> 7));
    }
}

/* Returns the index of the first mismatching byte, or SIZE_MAX if intact. */
static size_t pattern_mismatch(const void *p, size_t n, uint64_t seed)
{
    const unsigned char *b = (const unsigned char *)p;
    size_t i;
    for (i = 0; i < n; i++) {
        if (b[i] != (unsigned char)(seed + i * 131u + (i >> 7))) {
            return i;
        }
    }
    return SIZE_MAX;
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

static size_t align_up_public(size_t v, size_t a)
{
    return (v + (a - 1u)) & ~(a - 1u);
}

/* ================================================================== */
/* Tests                                                               */
/* ================================================================== */

static void test_zero_null_and_realloc_edges(void)
{
    mya_stats_t st;
    void *p;

    reset_allocator(0);
    printf("test_zero_null_and_realloc_edges\n");

    CHECK(my_malloc(0) == NULL);
    CHECK(my_realloc(NULL, 0) == NULL);

    my_free(NULL); /* documented no-op */

    p = my_malloc(32);
    CHECK(p != NULL);
    CHECK(my_realloc(p, 0) == NULL); /* frees and returns NULL */

    mya_get_stats(&st);
    CHECK(st.live_allocations == 0);
    CHECK(st.live_requested_bytes == 0);
    CHECK_VALID();

    /* my_realloc(NULL, n) behaves like my_malloc(n). */
    p = my_realloc(NULL, 777);
    CHECK(p != NULL);
    CHECK(((uintptr_t)p % mya_debug_align()) == 0);
    memset(p, 0x5A, 777);
    my_free(p);
    CHECK_VALID();
}

static void test_tiny_and_alignment(void)
{
    static const size_t special[] = {
        1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65,
        127, 128, 129, 255, 256, 257, 511, 512, 513, 1023, 1024, 1025,
        4095, 4096, 4097, 65535, 65536, 65537
    };
    const size_t align = mya_debug_align();
    size_t i;

    reset_allocator(0);
    printf("test_tiny_and_alignment\n");

    for (i = 1; i <= 300; i++) {
        void *p = my_malloc(i);
        CHECK(p != NULL);
        CHECK(((uintptr_t)p % align) == 0);
        CHECK(((uintptr_t)p % _Alignof(max_align_t)) == 0);
        memset(p, 0xAB, i);
        CHECK(((unsigned char *)p)[i - 1] == 0xAB);
        my_free(p);
    }
    for (i = 0; i < sizeof special / sizeof special[0]; i++) {
        size_t s = special[i];
        void *p = my_malloc(s);
        CHECK(p != NULL);
        CHECK(((uintptr_t)p % align) == 0);
        fill_pattern(p, s, s);
        CHECK(pattern_mismatch(p, s, s) == SIZE_MAX);
        my_free(p);
    }

    {
        mya_stats_t st;
        mya_get_stats(&st);
        CHECK(st.live_allocations == 0);
        CHECK(st.free_bytes_managed > 0);
    }
    CHECK_VALID();
}

static void test_pattern_isolation(void)
{
    enum { N = 256 };
    void *p[N];
    size_t sz[N];
    size_t i;

    reset_allocator(0);
    printf("test_pattern_isolation\n");

    for (i = 0; i < N; i++) {
        sz[i] = 1 + (i * 37u) % 512u;
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i);
    }
    for (i = 0; i < N; i++) {
        CHECK(pattern_mismatch(p[i], sz[i], i) == SIZE_MAX);
    }

    /* Free every other block: the survivors must be untouched. */
    for (i = 0; i < N; i += 2) {
        my_free(p[i]);
        p[i] = NULL;
    }
    for (i = 1; i < N; i += 2) {
        CHECK(pattern_mismatch(p[i], sz[i], i) == SIZE_MAX);
    }
    CHECK_VALID();

    /* Refill the holes with different content. */
    for (i = 0; i < N; i += 2) {
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i + 1000u);
    }
    for (i = 0; i < N; i++) {
        CHECK(pattern_mismatch(p[i], sz[i], i + (i % 2 ? 0u : 1000u)) == SIZE_MAX);
    }

    for (i = 0; i < N; i++) {
        my_free(p[i]);
    }
    {
        mya_stats_t st;
        mya_get_stats(&st);
        CHECK(st.live_allocations == 0);
    }
    CHECK_VALID();
}

static void test_large_allocations(void)
{
    static const size_t sizes[] = { 1u << 20, 3u << 20, (4u << 20) + 12345u };
    void *p[3];
    size_t i, j;

    reset_allocator(0);
    printf("test_large_allocations\n");

    for (i = 0; i < 3; i++) {
        p[i] = my_malloc(sizes[i]);
        CHECK(p[i] != NULL);
        CHECK(((uintptr_t)p[i] % mya_debug_align()) == 0);
        fill_pattern(p[i], sizes[i], i * 3u + 1u);
    }
    for (i = 0; i < 3; i++) {
        CHECK(pattern_mismatch(p[i], sizes[i], i * 3u + 1u) == SIZE_MAX);
        /* no two live large allocations may overlap */
        for (j = i + 1; j < 3; j++) {
            const char *a = (const char *)p[i];
            const char *b = (const char *)p[j];
            int disjoint = (a + sizes[i] <= b) || (b + sizes[j] <= a);
            CHECK(disjoint);
        }
    }

    /* Mixed small + large in the same heap. */
    {
        void *small = my_malloc(48);
        CHECK(small != NULL);
        fill_pattern(small, 48, 0x1234u);
        my_free(p[1]);
        p[1] = NULL;
        CHECK(pattern_mismatch(small, 48, 0x1234u) == SIZE_MAX);
        my_free(small);
    }

    for (i = 0; i < 3; i++) {
        if (p[i] != NULL) {
            my_free(p[i]);
        }
    }
    {
        mya_stats_t st;
        mya_get_stats(&st);
        CHECK(st.live_allocations == 0);
        /* Dedicated regions are given back; at most one spare may remain. */
        CHECK(st.regions_live <= 1);
    }
    CHECK_VALID();
}

static void test_impossible_sizes(void)
{
    mya_stats_t before, after;
    void *p;

    reset_allocator(0);
    printf("test_impossible_sizes\n");

    p = my_malloc(128);
    CHECK(p != NULL);
    fill_pattern(p, 128, 9);
    mya_get_stats(&before);

    CHECK(my_malloc(SIZE_MAX) == NULL);
    CHECK(my_malloc(SIZE_MAX - 1u) == NULL);
    CHECK(my_malloc(SIZE_MAX - 100u) == NULL);
    CHECK(my_malloc(SIZE_MAX / 2u) == NULL);       /* must not wrap, must fail */
    CHECK(my_malloc((SIZE_MAX / 2u) + 1u) == NULL);
    CHECK(my_malloc(SIZE_MAX - mya_debug_header_size()) == NULL);
    CHECK_VALID();

    /* A failing huge realloc leaves the original block valid. */
    CHECK(my_realloc(p, SIZE_MAX) == NULL);
    CHECK(my_realloc(p, SIZE_MAX - 32u) == NULL);
    CHECK(pattern_mismatch(p, 128, 9) == SIZE_MAX);

    mya_get_stats(&after);
    CHECK(after.live_allocations == before.live_allocations);
    CHECK(after.live_requested_bytes == before.live_requested_bytes);
    CHECK(after.os_reserve_count == before.os_reserve_count);
    CHECK_VALID();

    my_free(p);
    CHECK_VALID();
}

static void test_free_orders_and_reuse(void)
{
    enum { N = 512 };
    void *p[N];
    size_t sz[N];
    size_t i;
    long reserves_before;

    reset_allocator(1);
    printf("test_free_orders_and_reuse\n");

    for (i = 0; i < N; i++) {
        sz[i] = 64 + (i * 7919u) % 8192u;
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i);
    }

    /* forward order */
    for (i = 0; i < N; i++) {
        my_free(p[i]);
    }
    CHECK_VALID();

    /* reverse order */
    for (i = 0; i < N; i++) {
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i + 7u);
    }
    for (i = N; i-- > 0;) {
        my_free(p[i]);
    }
    CHECK_VALID();

    /* random order */
    for (i = 0; i < N; i++) {
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i + 13u);
    }
    {
        uint64_t rs = 0x9E3779B97F4A7C15ull;
        size_t remaining = N;
        int *order = (int *)malloc(N * sizeof *order);
        CHECK(order != NULL);
        for (i = 0; i < N; i++) {
            order[i] = (int)i;
        }
        while (remaining > 0) {
            size_t k = (size_t)(rng_next(&rs) % remaining);
            int tmp = order[k];
            order[k] = order[remaining - 1];
            order[remaining - 1] = tmp;
            my_free(p[order[remaining - 1]]);
            remaining--;
        }
        free(order);
    }
    CHECK_VALID();

    /* Reuse: the same sizes must come back out of the freed blocks with
       no new region needed. */
    for (i = 0; i < N; i++) {
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i + 21u);
    }
    for (i = 0; i < N; i += 2) {
        my_free(p[i]);
        p[i] = NULL;
    }
    reserves_before = g_reserve_calls;
    for (i = 0; i < N; i += 2) {
        p[i] = my_malloc(sz[i]);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], sz[i], i + 33u);
    }
    CHECK(g_reserve_calls == reserves_before); /* pure reuse, no OS churn */
    for (i = 1; i < N; i += 2) {
        CHECK(pattern_mismatch(p[i], sz[i], i + 21u) == SIZE_MAX);
    }
    for (i = 0; i < N; i++) {
        my_free(p[i]);
    }
    CHECK_VALID();

    {
        mya_stats_t st;
        mya_get_stats(&st);
        CHECK(st.live_allocations == 0);
        CHECK(st.regions_live <= 1);
        CHECK(st.os_release_count > 0);
        CHECK(g_os_live_bytes == st.os_bytes_live);
    }
    mya_teardown();
    CHECK(g_os_live_bytes == 0);
    mya_set_os_ops(NULL);
}

static void test_split_and_coalesce(void)
{
    mya_stats_t st;
    const size_t hdr = mya_debug_header_size();
    void *a, *b, *c, *p2;

    reset_allocator(0);
    printf("test_split_and_coalesce\n");

    /* One big allocation, freed: the region is empty and fully coalesced. */
    a = my_malloc(200000);
    CHECK(a != NULL);
    my_free(a);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 1);
    CHECK(st.blocks_free == 1);
    CHECK(st.live_allocations == 0);

    /* Splitting: a small allocation cuts the single free block in two. */
    a = my_malloc(64);
    CHECK(a != NULL);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 2);
    CHECK(st.blocks_free == 1);

    /* Coalescing: freeing it merges the two blocks back into one. */
    my_free(a);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 1);
    CHECK_VALID();

    /* Three live blocks; freeing the two lower ones must merge them into
       a block large enough for a request that fits neither. */
    a = my_malloc(100);
    b = my_malloc(100);
    c = my_malloc(100);
    CHECK(a && b && c);
    fill_pattern(a, 100, 1);
    fill_pattern(b, 100, 2);
    fill_pattern(c, 100, 3);
    CHECK((char *)b == (char *)a + hdr + align_up_public(100, mya_debug_align()));

    my_free(a);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 4); /* a free, b, c, tail */

    my_free(b);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 3); /* a+b merged */

    p2 = my_malloc(250);
    CHECK(p2 == a); /* coalesced block was reused and split */
    CHECK(pattern_mismatch(c, 100, 3) == SIZE_MAX);
    CHECK_VALID();

    my_free(p2);
    my_free(c);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 1);
    CHECK(st.live_allocations == 0);
    CHECK_VALID();

    /* Non-adjacent free blocks must NOT merge: keep the middle live. */
    a = my_malloc(1000);
    b = my_malloc(1000);
    c = my_malloc(1000);
    CHECK(a && b && c);
    fill_pattern(b, 1000, 42);
    my_free(a);
    my_free(c);
    mya_get_stats(&st);
    CHECK(st.blocks_total >= 3); /* a and c did not merge across b */
    CHECK(pattern_mismatch(b, 1000, 42) == SIZE_MAX);
    my_free(b);
    mya_get_stats(&st);
    CHECK(st.blocks_total == 1); /* now everything coalesces */
    CHECK_VALID();
}

static void test_empty_region_release(void)
{
    enum { N = 4096 };
    void *p[N];
    size_t i;
    mya_stats_t st;

    reset_allocator(1);
    printf("test_empty_region_release\n");

    for (i = 0; i < N; i++) {
        p[i] = my_malloc(1024);
        CHECK(p[i] != NULL);
        fill_pattern(p[i], 1024, i);
    }
    mya_get_stats(&st);
    CHECK(st.os_reserve_count > 1); /* more than one region was needed */
    CHECK(st.os_bytes_live > (1u << 20));
    CHECK(st.peak_os_bytes_live >= st.os_bytes_live);

    for (i = 0; i < N; i++) {
        CHECK(pattern_mismatch(p[i], 1024, i) == SIZE_MAX);
        my_free(p[i]);
    }

    mya_get_stats(&st);
    CHECK(st.live_allocations == 0);
    CHECK(st.regions_live <= 1);                 /* at most the spare is kept */
    CHECK(st.os_bytes_live <= mya_debug_normal_region_size());
    CHECK(st.os_release_count > 0);
    CHECK(g_os_live_bytes == st.os_bytes_live);
    CHECK_VALID();

    /* The retained spare is immediately reusable without OS churn. */
    {
        long reserves = g_reserve_calls;
        void *q = my_malloc(900000);
        CHECK(q != NULL);
        CHECK(g_reserve_calls == reserves);
        fill_pattern(q, 900000, 77);
        CHECK(pattern_mismatch(q, 900000, 77) == SIZE_MAX);
        my_free(q);
    }

    mya_teardown();
    CHECK(g_os_live_bytes == 0);
    mya_get_stats(&st);
    CHECK(st.regions_live == 0);
    CHECK(st.os_bytes_live == 0);
    mya_set_os_ops(NULL);
}

static void test_realloc_shrink(void)
{
    const size_t hdr = mya_debug_header_size();
    const size_t align = mya_debug_align();
    void *p, *q, *r;

    reset_allocator(0);
    printf("test_realloc_shrink\n");

    p = my_malloc(4096);
    CHECK(p != NULL);
    fill_pattern(p, 4096, 0xABCDu);

    q = my_realloc(p, 64);
    CHECK(q == p);                                   /* same pointer */
    CHECK(pattern_mismatch(q, 64, 0xABCDu) == SIZE_MAX);

    /* The remainder must be usable and start right after the shrunk block. */
    r = my_malloc(3900);
    CHECK(r != NULL);
    CHECK((char *)r == (char *)p + hdr + align_up_public(64, align));
    CHECK(pattern_mismatch(p, 64, 0xABCDu) == SIZE_MAX);
    CHECK_VALID();

    my_free(r);
    my_free(q);

    /* A shrink too small to split keeps the capacity. */
    p = my_malloc(1000);
    CHECK(p != NULL);
    fill_pattern(p, 1000, 5);
    q = my_realloc(p, 900);
    CHECK(q == p);
    CHECK(pattern_mismatch(q, 900, 5) == SIZE_MAX);
    my_free(q);
    CHECK_VALID();
}

static void test_realloc_grow_in_place(void)
{
    const size_t hdr = mya_debug_header_size();
    const size_t align = mya_debug_align();
    void *p, *q, *r;

    reset_allocator(0);
    printf("test_realloc_grow_in_place\n");

    p = my_malloc(100);
    CHECK(p != NULL);
    q = my_malloc(200);
    CHECK(q != NULL);
    CHECK((char *)q == (char *)p + hdr + align_up_public(100, align));
    fill_pattern(p, 100, 11);
    fill_pattern(q, 200, 22);

    my_free(q); /* free the block immediately after p */

    r = my_realloc(p, 250);
    CHECK(r == p); /* grew in place */
    CHECK(pattern_mismatch(r, 100, 11) == SIZE_MAX);

    /* The leftover capacity is a valid free block again. */
    {
        void *s = my_malloc(64);
        CHECK(s != NULL);
        CHECK((char *)s >= (char *)p);
        CHECK((char *)s < (char *)p + 4096);
        my_free(s);
    }
    my_free(r);
    CHECK_VALID();

    /* Growing into a live neighbour must not happen: the block moves. */
    p = my_malloc(100);
    q = my_malloc(100);
    CHECK(p && q);
    fill_pattern(p, 100, 33);
    fill_pattern(q, 100, 44);
    r = my_realloc(p, 150); /* needs 160 > 112 and the neighbour is live */
    CHECK(r != NULL);
    CHECK(r != p);
    CHECK(pattern_mismatch(r, 100, 33) == SIZE_MAX);
    CHECK(pattern_mismatch(q, 100, 44) == SIZE_MAX);
    my_free(r);
    my_free(q);
    CHECK_VALID();
}

static void test_realloc_moved(void)
{
    void *p, *q, *r, *s;

    reset_allocator(0);
    printf("test_realloc_moved\n");

    p = my_malloc(100);
    CHECK(p != NULL);
    q = my_malloc(100);
    CHECK(q != NULL);
    fill_pattern(p, 100, 101);
    fill_pattern(q, 100, 202);

    r = my_realloc(p, 5000);
    CHECK(r != NULL);
    CHECK(r != p); /* had to move: the neighbour is live */
    CHECK(pattern_mismatch(r, 100, 101) == SIZE_MAX);
    CHECK(pattern_mismatch(q, 100, 202) == SIZE_MAX); /* neighbour untouched */
    fill_pattern(r, 5000, 303);
    CHECK(pattern_mismatch(r, 5000, 303) == SIZE_MAX);
    CHECK_VALID();

    /* The old block was released and is handed out again. */
    s = my_malloc(100);
    CHECK(s == p);
    CHECK(pattern_mismatch(q, 100, 202) == SIZE_MAX);
    my_free(s);
    my_free(r);
    my_free(q);
    CHECK_VALID();

    /* Growing a dedicated (large) allocation moves it too. */
    p = my_malloc(2u << 20);
    CHECK(p != NULL);
    fill_pattern(p, 2u << 20, 404);
    r = my_realloc(p, (3u << 20) + 7u);
    CHECK(r != NULL);
    CHECK(r != p);
    CHECK(pattern_mismatch(r, 2u << 20, 404) == SIZE_MAX);
    fill_pattern(r, (3u << 20) + 7u, 505);
    CHECK(pattern_mismatch(r, (3u << 20) + 7u, 505) == SIZE_MAX);
    my_free(r);
    CHECK_VALID();

    /* Shrinking a dedicated allocation down to a normal size keeps it put. */
    p = my_malloc(2u << 20);
    CHECK(p != NULL);
    fill_pattern(p, 2u << 20, 606);
    r = my_realloc(p, 1024);
    CHECK(r == p);
    CHECK(pattern_mismatch(r, 1024, 606) == SIZE_MAX);
    my_free(r);
    CHECK_VALID();
}

static void test_realloc_failure(void)
{
    mya_stats_t before, after;
    void *p, *q;

    reset_allocator(1);
    printf("test_realloc_failure\n");

    p = my_malloc(200);
    CHECK(p != NULL);
    fill_pattern(p, 200, 0x77u);
    mya_get_stats(&before);

    g_fail_countdown = 0; /* the next OS reserve fails */
    q = my_realloc(p, 8u << 20);
    CHECK(q == NULL);
    g_fail_countdown = -1;

    CHECK(pattern_mismatch(p, 200, 0x77u) == SIZE_MAX); /* contents intact */
    mya_get_stats(&after);
    CHECK(after.live_allocations == before.live_allocations);
    CHECK(after.live_requested_bytes == before.live_requested_bytes);
    CHECK(after.live_capacity_bytes == before.live_capacity_bytes);
    CHECK(after.os_reserve_count == before.os_reserve_count); /* no leak */
    CHECK(after.os_bytes_live == before.os_bytes_live);
    CHECK_VALID();

    /* Once the OS cooperates again the same call succeeds. */
    q = my_realloc(p, 8u << 20);
    CHECK(q != NULL);
    CHECK(pattern_mismatch(q, 200, 0x77u) == SIZE_MAX);
    my_free(q);
    CHECK_VALID();
}

static void test_os_failure_injection(void)
{
    mya_stats_t st;
    void *p;

    reset_allocator(1);
    printf("test_os_failure_injection\n");

    /* No region available and the OS refuses: clean failure. */
    g_fail_countdown = 0;
    CHECK(my_malloc(64) == NULL);
    CHECK(my_malloc(1u << 20) == NULL);
    mya_get_stats(&st);
    CHECK(st.live_allocations == 0);
    CHECK(st.os_reserve_count == 0);
    CHECK_VALID();

    /* Exactly one reserve succeeds; the region then serves many blocks. */
    g_fail_countdown = 1;
    p = my_malloc(64);
    CHECK(p != NULL);
    g_fail_countdown = 0;
    {
        void *more[64];
        size_t i;
        for (i = 0; i < 64; i++) {
            more[i] = my_malloc(64);
            CHECK(more[i] != NULL); /* satisfied from the existing region */
        }
        CHECK(my_malloc(4u << 20) == NULL); /* needs a new region: refused */
        for (i = 0; i < 64; i++) {
            my_free(more[i]);
        }
    }
    my_free(p);
    g_fail_countdown = -1;

    mya_get_stats(&st);
    CHECK(st.live_allocations == 0);
    CHECK(st.os_reserve_count == 1);
    CHECK(st.regions_live <= 1);
    CHECK_VALID();

    /* A refused reserve after a successful one: the failure is clean and
       the successful region is given back. */
    reset_allocator(1);
    g_fail_countdown = 1;
    {
        void *q = my_malloc(2u << 20); /* dedicated region */
        CHECK(q != NULL);
        fill_pattern(q, 2u << 20, 0x55u);
        CHECK(pattern_mismatch(q, 2u << 20, 0x55u) == SIZE_MAX);
        my_free(q);
        CHECK(g_release_calls == 1);

        q = my_malloc(2u << 20);
        CHECK(q == NULL); /* second reserve refused */
        CHECK(g_reserve_calls == 1);
        CHECK_VALID();
    }
    g_fail_countdown = -1;
    {
        void *q = my_malloc(2u << 20);
        CHECK(q != NULL);
        my_free(q);
    }
    CHECK_VALID();
    mya_teardown();
    CHECK(g_os_live_bytes == 0);
    mya_set_os_ops(NULL);
}

/* ------------------------------------------------------------------ */
/* Deterministic randomized stress test                                */
/* ------------------------------------------------------------------ */

typedef struct {
    void *p;
    size_t size;
    uint64_t seed;
} slot_t;

#define RAND_SLOTS 512
#define RAND_OPS 20000

static size_t pick_size(uint64_t *rs, size_t live_bytes)
{
    uint64_t r = rng_next(rs);
    if ((r % 211u) == 0u && live_bytes < (8u << 20)) {
        /* large enough to need a dedicated region */
        return 1u + (size_t)(rng_next(rs) % ((1u << 20) + 12345u));
    }
    return 1u + (size_t)(r % 4096u);
}

static void test_randomized(void)
{
    static slot_t slots[RAND_SLOTS];
    uint64_t rs = 0x243F6A8885A308D3ull;
    size_t live_bytes = 0;
    size_t op;
    unsigned k;

    reset_allocator(1);
    printf("test_randomized\n");
    memset(slots, 0, sizeof slots);

    for (op = 0; op < RAND_OPS; op++) {
        uint64_t r = rng_next(&rs);
        unsigned idx = (unsigned)(r % RAND_SLOTS);
        unsigned act = (unsigned)((r >> 40) % 100u);
        slot_t *s = &slots[idx];
        unsigned action;

        /* An empty slot can only be allocated into; an occupied slot can
           only be freed or reallocated (overwriting it would leak). */
        if (s->p == NULL) {
            action = 0; /* allocate */
        } else if (act < 25u) {
            action = 1; /* free */
        } else if (act < 50u) {
            action = 2; /* realloc */
        } else if (act < 82u) {
            action = 1; /* free */
        } else {
            action = 2; /* realloc */
        }

        if (action == 0) {
            size_t sz = pick_size(&rs, live_bytes);
            void *p = my_malloc(sz);
            if (p == NULL) {
                printf("  FAIL op %llu: my_malloc(" FMT_U64 ") returned NULL\n",
                       (unsigned long long)op, U64(sz));
                g_failures++;
                break;
            }
            fill_pattern(p, sz, r);
            s->p = p;
            s->size = sz;
            s->seed = r;
            live_bytes += sz;
        } else if (action == 1) {
            if (pattern_mismatch(s->p, s->size, s->seed) != SIZE_MAX) {
                printf("  FAIL op %llu: block corrupted before free\n", (unsigned long long)op);
                g_failures++;
                break;
            }
            live_bytes -= s->size;
            my_free(s->p);
            s->p = NULL;
            s->size = 0;
        } else {
            size_t nsz = pick_size(&rs, live_bytes);
            size_t old_size = s->size;
            uint64_t old_seed = s->seed;
            size_t keep = old_size < nsz ? old_size : nsz;
            void *np = my_realloc(s->p, nsz);
            if (np == NULL) {
                printf("  FAIL op %llu: my_realloc(" FMT_U64 ") returned NULL\n",
                       (unsigned long long)op, U64(nsz));
                g_failures++;
                break;
            }
            /* From here the old pointer is invalid: track the new one first
               so that a failure below cannot turn into a double free. */
            s->p = np;
            s->size = nsz;
            live_bytes = live_bytes - old_size + nsz;
            if (pattern_mismatch(np, keep, old_seed) != SIZE_MAX) {
                printf("  FAIL op %llu: realloc lost the preserved prefix\n",
                       (unsigned long long)op);
                g_failures++;
                s->seed = r;
                break;
            }
            fill_pattern(np, nsz, r);
            s->seed = r;
        }

        g_checks++;
        if (mya_debug_validate() != 0) {
            printf("  FAIL op %llu: mya_debug_validate()\n", (unsigned long long)op);
            g_failures++;
            break;
        }

        for (k = 0; k < 8; k++) {
            unsigned vi = (unsigned)((op * 7u + k * 61u) % RAND_SLOTS);
            if (slots[vi].p != NULL && pattern_mismatch(slots[vi].p, slots[vi].size, slots[vi].seed) != SIZE_MAX) {
                printf("  FAIL op %llu: unrelated block %u corrupted\n",
                       (unsigned long long)op, vi);
                g_failures++;
                break;
            }
        }
        if (op % 512u == 0u) {
            for (k = 0; k < RAND_SLOTS; k++) {
                if (slots[k].p != NULL &&
                    pattern_mismatch(slots[k].p, slots[k].size, slots[k].seed) != SIZE_MAX) {
                    printf("  FAIL op %llu: full sweep found corruption in slot %u\n",
                           (unsigned long long)op, k);
                    g_failures++;
                    break;
                }
            }
        }
    }

    for (k = 0; k < RAND_SLOTS; k++) {
        if (slots[k].p != NULL) {
            CHECK(pattern_mismatch(slots[k].p, slots[k].size, slots[k].seed) == SIZE_MAX);
            my_free(slots[k].p);
            slots[k].p = NULL;
        }
    }

    {
        mya_stats_t st;
        mya_get_stats(&st);
        CHECK(st.live_allocations == 0);
        CHECK(st.live_requested_bytes == 0);
        CHECK(st.regions_live <= 1);
        CHECK(st.os_reserve_count > 0);
        CHECK(st.os_release_count > 0);
        CHECK(g_os_live_bytes == st.os_bytes_live);
        CHECK(st.peak_live_allocations > 0);
    }
    CHECK_VALID();
    mya_teardown();
    CHECK(g_os_live_bytes == 0);
    mya_set_os_ops(NULL);
}

/* ================================================================== */
/* Main                                                                */
/* ================================================================== */

int main(void)
{
    g_default_ops = mya_os_default_ops();

    printf("myalloc tests: MYA_ALIGN=%llu header=%llu normal region=%llu\n",
           U64(mya_debug_align()), U64(mya_debug_header_size()),
           U64(mya_debug_normal_region_size()));

    test_zero_null_and_realloc_edges();
    test_tiny_and_alignment();
    test_pattern_isolation();
    test_large_allocations();
    test_impossible_sizes();
    test_free_orders_and_reuse();
    test_split_and_coalesce();
    test_empty_region_release();
    test_realloc_shrink();
    test_realloc_grow_in_place();
    test_realloc_moved();
    test_realloc_failure();
    test_os_failure_injection();
    test_randomized();

    mya_teardown();
    mya_set_os_ops(NULL);

    printf("\n%llu checks, %d failures\n", U64(g_checks), g_failures);
    if (g_failures == 0) {
        printf("ALL TESTS PASSED\n");
        return 0;
    }
    printf("TESTS FAILED\n");
    return 1;
}
