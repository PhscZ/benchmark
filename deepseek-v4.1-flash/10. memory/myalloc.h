/*
 * myalloc.h -- a from-scratch, VirtualAlloc-backed memory allocator.
 *
 * Target: 64-bit Windows, C17, MinGW-w64 GCC.
 * Nothing inside the implementation calls malloc/calloc/realloc/free,
 * HeapAlloc, or any other allocator library: every byte the allocator
 * hands out comes from VirtualAlloc, and every byte is returned with
 * VirtualFree.
 *
 * ---------------------------------------------------------------------
 * API
 * ---------------------------------------------------------------------
 *   void *my_malloc(size_t size);          NULL when size == 0 or on failure
 *   void  my_free(void *ptr);              my_free(NULL) is a no-op
 *   void *my_realloc(void *ptr, size_t size);
 *
 * The names are prefixed so a test harness can keep using the C runtime
 * allocator side by side.  Replacing the process-wide allocator is not
 * part of this design.
 *
 * ---------------------------------------------------------------------
 * Memory layout
 * ---------------------------------------------------------------------
 * The allocator owns "regions".  A region is one contiguous VirtualAlloc
 * reservation; it is never merged with, or split across, another region.
 *
 *   region base
 *   +---------------------------------------------------------------+
 *   | mya_region_t   (region header, padded to MYA_ALIGN)           |
 *   +---------------------------------------------------------------+
 *   | mya_block_t    (block header, padded to MYA_ALIGN)            |
 *   | payload        (cap bytes, usable by the caller)              |
 *   +---------------------------------------------------------------+
 *   | mya_block_t                                                   |
 *   | payload                                                       |
 *   +---------------------------------------------------------------+
 *   | ...                                                           |
 *   +---------------------------------------------------------------+  <-- base + total
 *
 * A region is therefore a chain of physically adjacent blocks that
 * exactly tiles [base + REGION_HDR_SIZE, base + total).  Blocks are
 * linked both physically (next_phys/prev_phys, within one region only)
 * and, when free, through the segregated free lists (next_free/prev_free).
 *
 * Block header (64 bytes on x86_64, padded so that payload stays aligned):
 *
 *   next_phys, prev_phys  physical neighbours inside the same region
 *   next_free, prev_free  segregated free-list links (only when free)
 *   cap                   usable payload bytes in this block
 *   req                   bytes the caller originally asked for
 *   region                owning region (never changes for a block)
 *   flags                 MYA_BLOCK_FREE
 *   magic                 MYA_BLOCK_MAGIC, verified in debug builds only
 *
 * Alignment: MYA_ALIGN is _Alignof(max_align_t) (16 on x86_64).  Region
 * bases come from VirtualAlloc and are 64 KiB aligned.  The region header
 * and the block header are both padded to a multiple of MYA_ALIGN, and
 * every cap is a multiple of MYA_ALIGN, so every payload pointer is
 * MYA_ALIGN aligned.  The minimum block is MYA_HDR_SIZE + MYA_ALIGN.
 *
 * ---------------------------------------------------------------------
 * Search, splitting, coalescing
 * ---------------------------------------------------------------------
 * Free blocks live in MYA_BIN_COUNT segregated lists: exact 16-byte
 * classes up to 256 bytes, then power-of-two classes above that.  A
 * request starts at the class of the requested size and walks upward;
 * inside a class it takes the first block that fits.  A block that
 * cannot be split is remembered as a fallback and used only if the class
 * contains no splittable block.  This is an approximate best fit with
 * O(1) insert and O(bins) worst-case search.
 *
 * Splitting: when a free block is at least MYA_HDR_SIZE + MYA_ALIGN bytes
 * larger than the request, the tail is cut off into a new free block that
 * is immediately coalesced with the following free block, if any.
 *
 * Coalescing: my_free() merges the block with its physically adjacent
 * neighbours when they are free.  Physical links exist only inside one
 * region, so blocks are never merged across regions.
 *
 * Region lifecycle: a region whose live block count reaches zero is
 * released with VirtualFree immediately.  One empty normal-sized region
 * (MYA_NORMAL_REGION_SIZE, 1 MiB) is kept as a "spare" so that a
 * free-everything / allocate-again cycle does not churn the OS; its free
 * block stays in the free lists and is reusable like any other.  Requests
 * that do not fit a normal region get a dedicated region sized for that
 * single allocation; such a region is always released when freed.
 *
 * ---------------------------------------------------------------------
 * Thread safety
 * ---------------------------------------------------------------------
 * The allocator is NOT thread-safe.  It keeps unsynchronised global state
 * (region list, free lists, statistics) and is intended for
 * single-threaded use.
 *
 * ---------------------------------------------------------------------
 * Undefined behaviour (not detected, not supported)
 * ---------------------------------------------------------------------
 *   - my_free()/my_realloc() on a pointer that was not returned by
 *     my_malloc()/my_realloc(), including interior pointers.
 *   - Double free.
 *   - Use after free, and writing outside [ptr, ptr + requested size).
 *   - Concurrent use from more than one thread.
 * In a MYALLOC_DEBUG build the allocator aborts with a diagnostic when it
 * notices a header magic mismatch or a free block that is freed again,
 * but that is a debugging aid, not part of the contract.
 *
 * ---------------------------------------------------------------------
 * Diagnostics
 * ---------------------------------------------------------------------
 * mya_get_stats() is always available and cheap (plain counters).
 * Validation (full region/free-list/stats cross-check) is compiled in
 * only when MYALLOC_DEBUG is 1, so release performance measurements are
 * never polluted by it.  The OS layer is injectable so tests can fail
 * VirtualAlloc deterministically.
 *
 * ---------------------------------------------------------------------
 * Build / run (from the project root, MinGW-w64 GCC on 64-bit Windows)
 * ---------------------------------------------------------------------
 *   gcc -std=c17 -O2 -Wall -Wextra -I. -DMYALLOC_DEBUG=1 \
 *       myalloc.c tests/test_myalloc.c -o myalloc_test.exe
 *   gcc -std=c17 -O2 -Wall -Wextra -I. -DMYALLOC_DEBUG=0 \
 *       myalloc.c tests/bench_myalloc.c -o myalloc_bench.exe
 *   myalloc_test.exe
 *   myalloc_bench.exe
 *
 * or, with make (mingw32-make):
 *   mingw32-make test      # builds and runs the automated tests
 *   mingw32-make bench     # builds and runs the benchmark
 *   mingw32-make clean
 *
 * or simply: build.bat
 */

#ifndef MYALLOC_H
#define MYALLOC_H

#include <stddef.h>

/* Set to 1 to compile in magic-value checking and mya_debug_validate(). */
#ifndef MYALLOC_DEBUG
#define MYALLOC_DEBUG 0
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/* Core API                                                           */
/* ------------------------------------------------------------------ */

void *my_malloc(size_t size);
void my_free(void *ptr);
void *my_realloc(void *ptr, size_t size);

/* ------------------------------------------------------------------ */
/* Injectable operating-system layer                                  */
/* ------------------------------------------------------------------ */
/*
 * reserve(size)  must return `size` bytes of readable/writable memory
 *                aligned to at least `granularity()`, or NULL.  The
 *                granularity must be a multiple of _Alignof(max_align_t).
 * release(base, size) must give those bytes back; 0 means success.
 *
 * Replace these only while the allocator holds no regions (i.e. after
 * mya_teardown()); mya_set_os_ops(NULL) restores the VirtualAlloc layer.
 */
typedef struct mya_os_ops {
    void *(*reserve)(size_t size);
    int (*release)(void *base, size_t size);
    size_t (*granularity)(void);
} mya_os_ops_t;

const mya_os_ops_t *mya_os_default_ops(void);
const mya_os_ops_t *mya_get_os_ops(void);
void mya_set_os_ops(const mya_os_ops_t *ops);

/* ------------------------------------------------------------------ */
/* Statistics                                                         */
/* ------------------------------------------------------------------ */

typedef struct mya_stats {
    size_t live_allocations;         /* outstanding my_malloc/my_realloc blocks */
    size_t live_requested_bytes;     /* sum of the sizes callers asked for      */
    size_t live_capacity_bytes;      /* sum of the capacities handed out        */
    size_t free_bytes_managed;       /* sum of capacities of free blocks        */
    size_t blocks_total;             /* physical blocks, live + free            */
    size_t blocks_free;              /* blocks currently in the free lists      */
    size_t regions_live;             /* regions currently held from the OS      */
    size_t os_reserve_count;         /* successful OS reserve calls             */
    size_t os_release_count;         /* OS release calls                        */
    size_t os_bytes_reserved;        /* bytes ever taken from the OS            */
    size_t os_bytes_released;        /* bytes ever given back to the OS         */
    size_t os_bytes_live;            /* bytes currently held from the OS        */
    size_t peak_os_bytes_live;       /* peak of os_bytes_live (managed memory)  */
    size_t peak_live_requested_bytes;
    size_t peak_live_allocations;
} mya_stats_t;

void mya_get_stats(mya_stats_t *out);

/*
 * Releases every region back to the OS and resets all state (including
 * statistics).  Every pointer previously returned becomes invalid.
 * Intended for tests and process shutdown, not for normal use.
 */
void mya_teardown(void);

/* ------------------------------------------------------------------ */
/* Debug-only facilities (MYALLOC_DEBUG=1)                            */
/* ------------------------------------------------------------------ */

#if MYALLOC_DEBUG
/*
 * Full structural check: region boundaries, block alignment, contiguity
 * and non-overlap of live blocks, free-list membership/ordering/size
 * classes, empty-region policy, and statistics cross-checking.
 * Returns 0 when the heap is consistent, non-zero (with a message on
 * stderr) otherwise.
 */
int mya_debug_validate(void);

/* Dumps regions and blocks to stdout. */
void mya_debug_dump(void);

/* Layout facts, so tests can assert on placement without guessing. */
size_t mya_debug_header_size(void);       /* MYA_HDR_SIZE (bytes before payload) */
size_t mya_debug_align(void);             /* MYA_ALIGN */
size_t mya_debug_normal_region_size(void);/* MYA_NORMAL_REGION_SIZE */
#endif /* MYALLOC_DEBUG */

#ifdef __cplusplus
}
#endif

#endif /* MYALLOC_H */
