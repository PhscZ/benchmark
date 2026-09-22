/*
 * myalloc.c -- implementation of the allocator described in myalloc.h.
 *
 * Everything the allocator hands out comes from the injectable OS layer
 * (VirtualAlloc by default); nothing here calls a runtime allocator.
 */

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include "myalloc.h"

#include <stdint.h>
#include <string.h>

#if MYALLOC_DEBUG
#include <stdio.h>
#include <stdlib.h>
#endif

/* ================================================================== */
/* Layout constants                                                    */
/* ================================================================== */

#define MYA_ALIGN ((size_t)_Alignof(max_align_t))

_Static_assert((MYA_ALIGN & (MYA_ALIGN - 1u)) == 0u,
               "max_align_t alignment must be a power of two");
_Static_assert(MYA_ALIGN >= 8u && MYA_ALIGN <= 64u,
               "unsupported max_align_t alignment");

#define MYA_BLOCK_MAGIC 0x4D59424Bu  /* 'MYBK' */
#define MYA_REGION_MAGIC 0x4D595247u /* 'MYRG' */

#define MYA_BLOCK_FREE 0x1u
#define MYA_REGION_SPARE 0x1u

typedef struct mya_region mya_region_t;

typedef struct mya_block {
    struct mya_block *next_phys; /* next block in the same region, by address */
    struct mya_block *prev_phys; /* previous block in the same region        */
    struct mya_block *next_free; /* segregated free-list links               */
    struct mya_block *prev_free;
    size_t cap;                  /* usable payload bytes                     */
    size_t req;                  /* bytes the caller asked for               */
    mya_region_t *region;        /* owning region (fixed for the block's life)*/
    uint32_t flags;              /* MYA_BLOCK_FREE while in a free list       */
    uint32_t magic;              /* MYA_BLOCK_MAGIC (debug)                   */
} mya_block_t;

struct mya_region {
    mya_region_t *next;
    mya_region_t *prev;
    mya_block_t *first;          /* first physical block                      */
    size_t total;                /* bytes obtained from the OS                */
    size_t block_count;          /* physical blocks in the region             */
    size_t live_count;           /* blocks currently handed out               */
    size_t live_req_bytes;       /* sum of req over live blocks               */
    size_t free_bytes;           /* sum of cap over free blocks               */
    uint32_t magic;
    uint32_t flags;              /* MYA_REGION_SPARE                          */
};

#define MYA_HDR_SIZE ((sizeof(mya_block_t) + (MYA_ALIGN - 1u)) & ~(MYA_ALIGN - 1u))
#define MYA_REGION_HDR_SIZE ((sizeof(mya_region_t) + (MYA_ALIGN - 1u)) & ~(MYA_ALIGN - 1u))
#define MYA_MIN_PAYLOAD MYA_ALIGN
#define MYA_MIN_BLOCK (MYA_HDR_SIZE + MYA_MIN_PAYLOAD)

_Static_assert(MYA_HDR_SIZE % MYA_ALIGN == 0u, "block header must preserve payload alignment");
_Static_assert(MYA_REGION_HDR_SIZE % MYA_ALIGN == 0u, "region header must preserve payload alignment");
_Static_assert(MYA_HDR_SIZE >= sizeof(mya_block_t), "block header padding underflow");

/* Normal region size; also the size of the single retained spare region. */
#define MYA_NORMAL_REGION_SIZE ((size_t)1u << 20)

/* ================================================================== */
/* Segregated free lists                                               */
/* ================================================================== */

#define MYA_SMALL_STEP 16u
#define MYA_SMALL_MAX 256u
#define MYA_SMALL_BINS (MYA_SMALL_MAX / MYA_SMALL_STEP) /* exact classes 16..256 */
#define MYA_LARGE_BINS 56u                              /* power-of-two classes  */
#define MYA_BIN_COUNT (MYA_SMALL_BINS + MYA_LARGE_BINS)

static mya_block_t *g_bins[MYA_BIN_COUNT];

/* Global state.  Not synchronised: the allocator is single-threaded. */
static mya_region_t *g_regions;
static mya_region_t *g_spare; /* at most one empty normal region is retained */
static mya_stats_t g_stats;
static const mya_os_ops_t *g_ops; /* NULL -> built-in VirtualAlloc layer */

/* ================================================================== */
/* Operating-system layer                                              */
/* ================================================================== */

static void *mya_os_virtual_reserve(size_t size)
{
    return VirtualAlloc(NULL, size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
}

static int mya_os_virtual_release(void *base, size_t size)
{
    (void)size; /* MEM_RELEASE frees the whole reservation; size must be 0 */
    return VirtualFree(base, 0, MEM_RELEASE) ? 0 : -1;
}

static size_t mya_os_virtual_granularity(void)
{
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    return (size_t)si.dwAllocationGranularity;
}

static const mya_os_ops_t g_default_ops = {
    mya_os_virtual_reserve,
    mya_os_virtual_release,
    mya_os_virtual_granularity
};

const mya_os_ops_t *mya_os_default_ops(void)
{
    return &g_default_ops;
}

const mya_os_ops_t *mya_get_os_ops(void)
{
    return g_ops ? g_ops : &g_default_ops;
}

void mya_set_os_ops(const mya_os_ops_t *ops)
{
    g_ops = ops;
}

/* ================================================================== */
/* Arithmetic helpers (overflow checked)                               */
/* ================================================================== */

static int mya_add_overflow(size_t a, size_t b, size_t *out)
{
    if (a > SIZE_MAX - b) {
        return 1;
    }
    *out = a + b;
    return 0;
}

static int mya_align_up(size_t value, size_t align, size_t *out)
{
    if (value > SIZE_MAX - (align - 1u)) {
        return 1; /* the rounding itself would wrap */
    }
    *out = (value + (align - 1u)) & ~(align - 1u);
    return 0;
}

/* ================================================================== */
/* Free-list primitives                                                */
/* ================================================================== */

static unsigned mya_ilog2(size_t value)
{
    unsigned n = 0;
    while ((value >>= 1) != 0) {
        n++;
    }
    return n;
}

/*
 * Class of a block capacity.  Monotone non-decreasing in cap: every
 * capacity in class i is strictly smaller than every capacity in class
 * i+1, which is what lets mya_find_free() stop at the first class with a
 * fitting block.
 */
static unsigned mya_bin_index(size_t cap)
{
    unsigned idx;

    if (cap <= MYA_SMALL_MAX) {
        idx = (unsigned)((cap - 1u) / MYA_SMALL_STEP);
        return idx < MYA_SMALL_BINS ? idx : MYA_SMALL_BINS - 1u;
    }
    idx = MYA_SMALL_BINS + (mya_ilog2(cap) - mya_ilog2(MYA_SMALL_MAX));
    return idx < MYA_BIN_COUNT ? idx : MYA_BIN_COUNT - 1u;
}

/* Insert into the free list; also marks the block free and accounts it. */
static void mya_bin_insert(mya_block_t *b)
{
    unsigned idx = mya_bin_index(b->cap);
    mya_region_t *r = b->region;

    b->next_free = g_bins[idx];
    b->prev_free = NULL;
    if (g_bins[idx] != NULL) {
        g_bins[idx]->prev_free = b;
    }
    g_bins[idx] = b;

    b->flags |= MYA_BLOCK_FREE;
    r->free_bytes += b->cap;
    g_stats.free_bytes_managed += b->cap;
    g_stats.blocks_free++;
}

/* Remove from the free list; also clears the free flag and accounts it. */
static void mya_bin_remove(mya_block_t *b)
{
    unsigned idx = mya_bin_index(b->cap);
    mya_region_t *r = b->region;

    if (b->prev_free != NULL) {
        b->prev_free->next_free = b->next_free;
    } else {
        g_bins[idx] = b->next_free;
    }
    if (b->next_free != NULL) {
        b->next_free->prev_free = b->prev_free;
    }
    b->next_free = NULL;
    b->prev_free = NULL;

    b->flags &= ~MYA_BLOCK_FREE;
    r->free_bytes -= b->cap;
    g_stats.free_bytes_managed -= b->cap;
    g_stats.blocks_free--;
}

/* ================================================================== */
/* Regions                                                             */
/* ================================================================== */

static mya_region_t *mya_region_create(size_t total)
{
    const mya_os_ops_t *ops = mya_get_os_ops();
    mya_region_t *r;
    mya_block_t *b;
    void *mem;

    if (total < MYA_REGION_HDR_SIZE + MYA_MIN_BLOCK) {
        return NULL;
    }
    mem = ops->reserve(total);
    if (mem == NULL) {
        return NULL;
    }

    r = (mya_region_t *)mem;
    b = (mya_block_t *)((char *)mem + MYA_REGION_HDR_SIZE);

    r->next = g_regions;
    r->prev = NULL;
    if (g_regions != NULL) {
        g_regions->prev = r;
    }
    g_regions = r;
    r->first = b;
    r->total = total;
    r->block_count = 1;
    r->live_count = 0;
    r->live_req_bytes = 0;
    r->free_bytes = 0; /* mya_bin_insert() accounts the whole block below */
    r->magic = MYA_REGION_MAGIC;
    r->flags = 0;

    b->next_phys = NULL;
    b->prev_phys = NULL;
    b->next_free = NULL;
    b->prev_free = NULL;
    b->cap = total - MYA_REGION_HDR_SIZE - MYA_HDR_SIZE;
    b->req = 0;
    b->region = r;
    b->flags = 0;
    b->magic = MYA_BLOCK_MAGIC;

    mya_bin_insert(b);

    g_stats.os_reserve_count++;
    g_stats.os_bytes_reserved += total;
    g_stats.os_bytes_live += total;
    if (g_stats.os_bytes_live > g_stats.peak_os_bytes_live) {
        g_stats.peak_os_bytes_live = g_stats.os_bytes_live;
    }
    g_stats.regions_live++;
    g_stats.blocks_total++;

    return r;
}

/* The region's blocks must already be out of the free lists. */
static void mya_region_destroy(mya_region_t *r)
{
    size_t total = r->total;
    size_t blocks = r->block_count;

    if (r->prev != NULL) {
        r->prev->next = r->next;
    } else {
        g_regions = r->next;
    }
    if (r->next != NULL) {
        r->next->prev = r->prev;
    }
    if (g_spare == r) {
        g_spare = NULL;
    }

    g_stats.regions_live--;
    g_stats.blocks_total -= blocks;
    g_stats.os_release_count++;
    g_stats.os_bytes_released += total;
    g_stats.os_bytes_live -= total;

    (void)mya_get_os_ops()->release(r, total);
}

/*
 * Called after a free left the region with no live block.  Coalescing
 * guarantees the region is then a single free block, so the choice is
 * between keeping it as the one allowed spare and giving it back.
 * Dedicated regions are always larger than MYA_NORMAL_REGION_SIZE, so
 * only normal-sized regions can become the spare.
 */
static void mya_region_retire_if_empty(mya_region_t *r)
{
    mya_block_t *b;

    if (r->live_count != 0) {
        return; /* still in use */
    }
    if (r == g_spare) {
        return; /* already the retained spare */
    }
    if (r->block_count == 1 && r->total == MYA_NORMAL_REGION_SIZE && g_spare == NULL) {
        g_spare = r;
        r->flags |= MYA_REGION_SPARE;
        return; /* its free block stays in the free lists, still reusable */
    }
    /* Give the region back.  Coalescing normally leaves a single free
       block here; removing them all keeps this correct either way. */
    for (b = r->first; b != NULL; b = b->next_phys) {
        if ((b->flags & MYA_BLOCK_FREE) != 0u) {
            mya_bin_remove(b);
        }
    }
    mya_region_destroy(r);
}

/* Region that can hold a single block of `need` payload bytes. */
static int mya_pool_grow(size_t need)
{
    size_t total;

    if (need <= MYA_NORMAL_REGION_SIZE - MYA_REGION_HDR_SIZE - MYA_HDR_SIZE) {
        total = MYA_NORMAL_REGION_SIZE;
    } else {
        size_t gran = mya_get_os_ops()->granularity();
        if (mya_add_overflow(need, MYA_HDR_SIZE, &total) != 0) {
            return 0;
        }
        if (mya_add_overflow(total, MYA_REGION_HDR_SIZE, &total) != 0) {
            return 0;
        }
        if (mya_align_up(total, gran, &total) != 0) {
            return 0;
        }
    }
    return mya_region_create(total) != NULL;
}

/* ================================================================== */
/* Block primitives                                                    */
/* ================================================================== */

/*
 * Split `b` (not in a free list) so that it holds exactly `need` payload
 * bytes; the tail becomes a free block.  The caller guarantees
 * b->cap - need >= MYA_HDR_SIZE + MYA_ALIGN.
 */
static void mya_split_tail(mya_block_t *b, size_t need)
{
    mya_region_t *r = b->region;
    mya_block_t *tail = (mya_block_t *)((char *)b + MYA_HDR_SIZE + need);

    tail->cap = b->cap - need - MYA_HDR_SIZE;
    tail->req = 0;
    tail->region = r;
    tail->flags = MYA_BLOCK_FREE;
    tail->magic = MYA_BLOCK_MAGIC;
    tail->prev_phys = b;
    tail->next_phys = b->next_phys;
    if (tail->next_phys != NULL) {
        tail->next_phys->prev_phys = tail;
    }
    b->next_phys = tail;
    b->cap = need;
    r->block_count++;
    g_stats.blocks_total++;

    /* Keep the "no two adjacent free blocks" invariant. */
    if (tail->next_phys != NULL && (tail->next_phys->flags & MYA_BLOCK_FREE) != 0u) {
        mya_block_t *n = tail->next_phys;
        mya_bin_remove(n);
        tail->cap += MYA_HDR_SIZE + n->cap;
        tail->next_phys = n->next_phys;
        if (n->next_phys != NULL) {
            n->next_phys->prev_phys = tail;
        }
        r->block_count--;
        g_stats.blocks_total--;
    }

    mya_bin_insert(tail);
}

/*
 * Merge `b` (already marked free, not yet in a free list) with its free
 * physical neighbours.  Returns the resulting block, which the caller
 * must insert into a free list.
 */
static mya_block_t *mya_coalesce(mya_block_t *b)
{
    mya_region_t *r = b->region;
    mya_block_t *p = b->prev_phys;
    mya_block_t *n;

    if (p != NULL && (p->flags & MYA_BLOCK_FREE) != 0u) {
        mya_bin_remove(p);
        p->cap += MYA_HDR_SIZE + b->cap;
        p->next_phys = b->next_phys;
        if (b->next_phys != NULL) {
            b->next_phys->prev_phys = p;
        }
        r->block_count--;
        g_stats.blocks_total--;
        b = p;
    }

    n = b->next_phys;
    if (n != NULL && (n->flags & MYA_BLOCK_FREE) != 0u) {
        mya_bin_remove(n);
        b->cap += MYA_HDR_SIZE + n->cap;
        b->next_phys = n->next_phys;
        if (n->next_phys != NULL) {
            n->next_phys->prev_phys = b;
        }
        r->block_count--;
        g_stats.blocks_total--;
    }

    if (b->prev_phys == NULL) {
        r->first = b;
    }
    return b;
}

/*
 * Free-list search: first fit inside the starting size class, then the
 * first fitting block of the next class that has one.  A block that
 * cannot be split is used only when its class offers nothing better.
 */
static mya_block_t *mya_find_free(size_t need)
{
    unsigned start = mya_bin_index(need);
    mya_block_t *fallback = NULL;
    unsigned i;

    for (i = start; i < MYA_BIN_COUNT; i++) {
        mya_block_t *b;
        for (b = g_bins[i]; b != NULL; b = b->next_free) {
            if (b->cap >= need) {
                if (b->cap - need >= MYA_HDR_SIZE + MYA_ALIGN) {
                    return b;
                }
                if (fallback == NULL) {
                    fallback = b;
                }
            }
        }
        if (fallback != NULL) {
            return fallback;
        }
    }
    return fallback;
}

static void mya_stats_capacity_delta(size_t old_cap, size_t new_cap)
{
    if (new_cap >= old_cap) {
        g_stats.live_capacity_bytes += new_cap - old_cap;
    } else {
        g_stats.live_capacity_bytes -= old_cap - new_cap;
    }
}

/* Record a new requested size for a live block. */
static void mya_set_req(mya_block_t *b, size_t size)
{
    mya_region_t *r = b->region;
    size_t old = b->req;

    b->req = size;
    if (size >= old) {
        r->live_req_bytes += size - old;
        g_stats.live_requested_bytes += size - old;
    } else {
        r->live_req_bytes -= old - size;
        g_stats.live_requested_bytes -= old - size;
    }
    if (g_stats.live_requested_bytes > g_stats.peak_live_requested_bytes) {
        g_stats.peak_live_requested_bytes = g_stats.live_requested_bytes;
    }
}

/* Hand `b` (free, in a free list, cap >= need) to the caller. */
static void *mya_block_alloc(mya_block_t *b, size_t size, size_t need)
{
    mya_region_t *r = b->region;

    mya_bin_remove(b);
    if (b->cap - need >= MYA_HDR_SIZE + MYA_ALIGN) {
        mya_split_tail(b, need);
    }
    b->req = size;

    if (g_spare == r) {
        g_spare = NULL; /* the retained region is in use again */
        r->flags &= ~MYA_REGION_SPARE;
    }

    r->live_count++;
    r->live_req_bytes += size;
    g_stats.live_allocations++;
    g_stats.live_requested_bytes += size;
    g_stats.live_capacity_bytes += b->cap;
    if (g_stats.live_allocations > g_stats.peak_live_allocations) {
        g_stats.peak_live_allocations = g_stats.live_allocations;
    }
    if (g_stats.live_requested_bytes > g_stats.peak_live_requested_bytes) {
        g_stats.peak_live_requested_bytes = g_stats.live_requested_bytes;
    }

    return (char *)b + MYA_HDR_SIZE;
}

/* ================================================================== */
/* Public API                                                          */
/* ================================================================== */

void *my_malloc(size_t size)
{
    size_t need;
    mya_block_t *b;

    if (size == 0) {
        return NULL; /* documented: my_malloc(0) == NULL */
    }
    if (mya_align_up(size, MYA_ALIGN, &need) != 0 || need == 0) {
        return NULL; /* size near SIZE_MAX would wrap the header math */
    }

    b = mya_find_free(need);
    if (b == NULL) {
        if (!mya_pool_grow(need)) {
            return NULL;
        }
        b = mya_find_free(need);
        if (b == NULL) {
            return NULL; /* cannot happen unless the OS layer lies */
        }
    }
    return mya_block_alloc(b, size, need);
}

void my_free(void *ptr)
{
    mya_block_t *b;
    mya_region_t *r;

    if (ptr == NULL) {
        return;
    }
    b = (mya_block_t *)((char *)ptr - MYA_HDR_SIZE);
    r = b->region;

#if MYALLOC_DEBUG
    if (b->magic != MYA_BLOCK_MAGIC || r == NULL || r->magic != MYA_REGION_MAGIC ||
        (b->flags & MYA_BLOCK_FREE) != 0u) {
        fprintf(stderr, "myalloc: my_free(%p): invalid or already freed pointer "
                        "(undefined behaviour)\n", ptr);
        abort();
    }
#endif

    r->live_count--;
    r->live_req_bytes -= b->req;
    g_stats.live_allocations--;
    g_stats.live_requested_bytes -= b->req;
    g_stats.live_capacity_bytes -= b->cap;
    b->req = 0;
    b->flags |= MYA_BLOCK_FREE;

    b = mya_coalesce(b);
    mya_bin_insert(b);

    if (r->live_count == 0) {
        mya_region_retire_if_empty(r);
    }
}

void *my_realloc(void *ptr, size_t size)
{
    mya_block_t *b;
    mya_region_t *r;
    size_t need;
    size_t old_cap;

    if (ptr == NULL) {
        return my_malloc(size);
    }
    if (size == 0) {
        my_free(ptr);
        return NULL;
    }

    b = (mya_block_t *)((char *)ptr - MYA_HDR_SIZE);
    r = b->region;

#if MYALLOC_DEBUG
    if (b->magic != MYA_BLOCK_MAGIC || r == NULL || r->magic != MYA_REGION_MAGIC ||
        (b->flags & MYA_BLOCK_FREE) != 0u) {
        fprintf(stderr, "myalloc: my_realloc(%p): invalid or already freed pointer "
                        "(undefined behaviour)\n", ptr);
        abort();
    }
#endif

    if (mya_align_up(size, MYA_ALIGN, &need) != 0 || need == 0) {
        return NULL; /* impossible size: original block untouched */
    }

    old_cap = b->cap;

    if (need <= b->cap) {
        /* Shrink in place; the remainder becomes reusable. */
        if (b->cap - need >= MYA_HDR_SIZE + MYA_ALIGN) {
            mya_split_tail(b, need);
        }
        mya_set_req(b, size);
        mya_stats_capacity_delta(old_cap, b->cap);
        return ptr;
    }

    {
        /*
         * Grow in place when the immediately following block is free and
         * big enough.  Both blocks live in the same region, so the sum is
         * bounded by the region size and cannot overflow.
         */
        mya_block_t *n = b->next_phys;
        if (n != NULL && (n->flags & MYA_BLOCK_FREE) != 0u &&
            b->cap + MYA_HDR_SIZE + n->cap >= need) {
            mya_bin_remove(n);
            b->cap += MYA_HDR_SIZE + n->cap;
            b->next_phys = n->next_phys;
            if (n->next_phys != NULL) {
                n->next_phys->prev_phys = b;
            }
            r->block_count--;
            g_stats.blocks_total--;
            if (b->cap - need >= MYA_HDR_SIZE + MYA_ALIGN) {
                mya_split_tail(b, need);
            }
            mya_set_req(b, size);
            mya_stats_capacity_delta(old_cap, b->cap);
            return ptr;
        }
    }

    {
        /* Move: allocate, copy the preserved prefix, release the original. */
        size_t keep = b->req < size ? b->req : size;
        void *np = my_malloc(size);
        if (np == NULL) {
            return NULL; /* original allocation and contents untouched */
        }
        memcpy(np, ptr, keep);
        my_free(ptr);
        return np;
    }
}

/* ================================================================== */
/* Diagnostics                                                         */
/* ================================================================== */

void mya_get_stats(mya_stats_t *out)
{
    if (out != NULL) {
        *out = g_stats;
    }
}

void mya_teardown(void)
{
    mya_region_t *r = g_regions;

    while (r != NULL) {
        mya_region_t *next = r->next;
        mya_block_t *b = r->first;
        while (b != NULL) {
            mya_block_t *nb = b->next_phys;
            if ((b->flags & MYA_BLOCK_FREE) != 0u) {
                mya_bin_remove(b);
            }
            b = nb;
        }
        mya_region_destroy(r);
        r = next;
    }

    g_regions = NULL;
    g_spare = NULL;
    memset(g_bins, 0, sizeof g_bins);
    memset(&g_stats, 0, sizeof g_stats);
}

/* ================================================================== */
/* Debug-only validation                                               */
/* ================================================================== */

#if MYALLOC_DEBUG

#define MYA_FMT_U64 "%llu"
#define MYA_U64(x) ((unsigned long long)(x))

static int mya_vfail(const char *msg)
{
    fprintf(stderr, "myalloc: mya_debug_validate: %s\n", msg);
    return 1;
}

#define MYA_VCHECK(cond, msg) \
    do {                      \
        if (!(cond)) {        \
            return mya_vfail(msg); \
        }                     \
    } while (0)

size_t mya_debug_header_size(void)
{
    return MYA_HDR_SIZE;
}

size_t mya_debug_align(void)
{
    return MYA_ALIGN;
}

size_t mya_debug_normal_region_size(void)
{
    return MYA_NORMAL_REGION_SIZE;
}

/*
 * The region walk proves the region is exactly partitioned by blocks of
 * the recorded capacities, so live blocks cannot overlap: each block
 * starts where the previous one ends and the chain covers the region.
 */
int mya_debug_validate(void)
{
    size_t live = 0, live_req = 0, live_cap = 0, free_bytes = 0;
    size_t blocks = 0, free_blocks = 0, empty_regions = 0;
    size_t listed = 0;
    mya_region_t *r;
    unsigned i;

    for (r = g_regions; r != NULL; r = r->next) {
        char *base = (char *)r;
        char *end = base + r->total;
        char *expect = base + MYA_REGION_HDR_SIZE;
        mya_block_t *prev = NULL;
        mya_block_t *b;
        size_t rblocks = 0, rfree = 0, rlive = 0, rreq = 0;

        MYA_VCHECK(r->magic == MYA_REGION_MAGIC, "region magic corrupted");
        MYA_VCHECK(r->next == NULL || r->next->prev == r, "region list links broken");
        MYA_VCHECK(((uintptr_t)r % MYA_ALIGN) == 0, "region base is misaligned");
        MYA_VCHECK(r->total >= MYA_REGION_HDR_SIZE + MYA_MIN_BLOCK,
                   "region smaller than its headers");
        MYA_VCHECK(r->total % MYA_ALIGN == 0, "region size is not aligned");
        MYA_VCHECK((r->flags & ~MYA_REGION_SPARE) == 0u, "unknown region flags");

        for (b = r->first; b != NULL; b = b->next_phys) {
            MYA_VCHECK(b->magic == MYA_BLOCK_MAGIC, "block magic corrupted");
            MYA_VCHECK((char *)b == expect, "blocks are not contiguous inside the region");
            MYA_VCHECK(((uintptr_t)b % MYA_ALIGN) == 0, "block is misaligned");
            MYA_VCHECK(b->prev_phys == prev, "physical back-link broken");
            MYA_VCHECK(b->region == r, "block region back-pointer broken");
            MYA_VCHECK(b->cap >= MYA_MIN_PAYLOAD, "block smaller than the minimum block");
            MYA_VCHECK(b->cap % MYA_ALIGN == 0, "block capacity is not aligned");
            MYA_VCHECK((char *)b + MYA_HDR_SIZE + b->cap <= end,
                       "block extends past the end of its region");

            expect = (char *)b + MYA_HDR_SIZE + b->cap;
            prev = b;
            rblocks++;

            if ((b->flags & MYA_BLOCK_FREE) != 0u) {
                MYA_VCHECK(b->req == 0, "free block has a non-zero requested size");
                rfree += b->cap;
                free_blocks++;
            } else {
                MYA_VCHECK(b->req <= b->cap, "requested size exceeds capacity");
                rlive++;
                rreq += b->req;
                live_cap += b->cap;
            }
        }

        MYA_VCHECK(expect == end, "region tail is not covered by blocks");
        MYA_VCHECK(prev == NULL || prev->next_phys == NULL, "last block has a successor");
        MYA_VCHECK(rblocks == r->block_count, "region block count mismatch");
        MYA_VCHECK(rfree == r->free_bytes, "region free byte count mismatch");
        MYA_VCHECK(rlive == r->live_count, "region live block count mismatch");
        MYA_VCHECK(rreq == r->live_req_bytes, "region live byte count mismatch");

        if (r->live_count == 0) {
            empty_regions++;
            MYA_VCHECK(rblocks == 1, "empty region was not fully coalesced");
            MYA_VCHECK(r == g_spare, "empty region retained (only one spare is allowed)");
        }
        if (r == g_spare) {
            MYA_VCHECK(r->total == MYA_NORMAL_REGION_SIZE, "spare region has the wrong size");
            MYA_VCHECK((r->flags & MYA_REGION_SPARE) != 0u, "spare region flag missing");
        } else {
            MYA_VCHECK((r->flags & MYA_REGION_SPARE) == 0u, "non-spare region carries the spare flag");
        }

        blocks += rblocks;
        free_bytes += rfree;
        live += rlive;
        live_req += rreq;
    }

    for (i = 0; i < MYA_BIN_COUNT; i++) {
        mya_block_t *prevf = NULL;
        mya_block_t *b;
        for (b = g_bins[i]; b != NULL; b = b->next_free) {
            MYA_VCHECK(b->magic == MYA_BLOCK_MAGIC, "free-list block magic corrupted");
            MYA_VCHECK((b->flags & MYA_BLOCK_FREE) != 0u, "block in a free list is not marked free");
            MYA_VCHECK(mya_bin_index(b->cap) == i, "block stored in the wrong size class");
            MYA_VCHECK(b->prev_free == prevf, "free-list back-link broken");
            prevf = b;
            listed++;
            MYA_VCHECK(listed <= blocks, "free list is longer than the block count (cycle?)");
        }
    }

    MYA_VCHECK(listed == free_blocks, "free lists do not match the free blocks in regions");
    MYA_VCHECK(live == g_stats.live_allocations, "stats: live allocation count mismatch");
    MYA_VCHECK(live_req == g_stats.live_requested_bytes, "stats: live requested bytes mismatch");
    MYA_VCHECK(live_cap == g_stats.live_capacity_bytes, "stats: live capacity mismatch");
    MYA_VCHECK(free_bytes == g_stats.free_bytes_managed, "stats: managed free bytes mismatch");
    MYA_VCHECK(blocks == g_stats.blocks_total, "stats: block count mismatch");
    MYA_VCHECK(listed == g_stats.blocks_free, "stats: free block count mismatch");
    MYA_VCHECK(empty_regions <= 1, "more than one empty region is retained");
    if (g_spare != NULL) {
        MYA_VCHECK(g_spare->magic == MYA_REGION_MAGIC, "spare region magic corrupted");
        MYA_VCHECK(g_spare->live_count == 0, "spare region has live blocks");
    }
    return 0;
}

void mya_debug_dump(void)
{
    mya_region_t *r;

    printf("myalloc: %s regions\n",
           g_regions == NULL ? "no" : "some");
    for (r = g_regions; r != NULL; r = r->next) {
        mya_block_t *b;
        printf("  region %p total=" MYA_FMT_U64 " blocks=" MYA_FMT_U64
               " live=" MYA_FMT_U64 " free=" MYA_FMT_U64 "%s\n",
               (void *)r, MYA_U64(r->total), MYA_U64(r->block_count),
               MYA_U64(r->live_count), MYA_U64(r->free_bytes),
               r == g_spare ? " [spare]" : "");
        for (b = r->first; b != NULL; b = b->next_phys) {
            printf("    block %p cap=" MYA_FMT_U64 " req=" MYA_FMT_U64 " %s\n",
                   (void *)b, MYA_U64(b->cap), MYA_U64(b->req),
                   (b->flags & MYA_BLOCK_FREE) != 0u ? "free" : "used");
        }
    }
}

#endif /* MYALLOC_DEBUG */
