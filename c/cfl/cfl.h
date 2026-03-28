/*
 * CFL — Compact Fair Lock (Manglik & Kim, PPoPP'24)
 *
 * Adapted from the original fairnumas implementation:
 *   https://github.com/rs-ifl/CFL
 *
 * Original code: MIT License, Copyright (c) 2016 Hugo Guiroux.
 * This adaptation strips COND_VAR, PAPI, LiTL interpose, and debug
 * infrastructure, keeping only the core CFL algorithm with usage-fair
 * (vLHT-based) queue shuffling.
 */
#ifndef __CFL_H__
#define __CFL_H__

#include <stdint.h>
#include <stddef.h>
#include <string.h>

/*
 * When included alongside aqs.h (ShflLock), reuse its shared primitives
 * to avoid redefinition errors.  aqs.h defines __AQS_H__.
 */
#ifdef __AQS_H__

/* Reuse aqs.h definitions for cache-line, barriers, atomics, READ/WRITE_ONCE */

#else /* standalone compilation */

/* --------------------------------------------------------------------
 * Cache-line size
 * -------------------------------------------------------------------- */
#ifndef L_CACHE_LINE_SIZE
#define L_CACHE_LINE_SIZE 128
#endif

#define r_align(n, r) (((n) + (r) - 1) & -(r))
#define cache_align(n) r_align(n, L_CACHE_LINE_SIZE)
#define pad_to_cache_line(n) (cache_align(n) - (n))

/* --------------------------------------------------------------------
 * Compiler / memory barriers
 * -------------------------------------------------------------------- */
#define barrier() __asm__ __volatile__("" ::: "memory")

static inline void smp_rmb(void)
{
    __asm__ __volatile__("lfence" ::: "memory");
}

static inline void smp_wmb(void)
{
    __asm__ __volatile__("sfence" ::: "memory");
}

static inline void smp_cmb(void)
{
    __asm__ __volatile__("" ::: "memory");
}

/* --------------------------------------------------------------------
 * Volatile access helpers (prevent compiler reordering)
 * -------------------------------------------------------------------- */
static inline void __write_once_size(volatile void *p, void *res, int size)
{
    switch (size) {
    case 1: *(volatile uint8_t *)p = *(uint8_t *)res; break;
    case 2: *(volatile uint16_t *)p = *(uint16_t *)res; break;
    case 4: *(volatile uint32_t *)p = *(uint32_t *)res; break;
    case 8: *(volatile uint64_t *)p = *(uint64_t *)res; break;
    default:
        barrier();
        memcpy((void *)p, (const void *)res, size);
        barrier();
    }
}

static inline void __read_once_size(volatile void *p, void *res, int size)
{
    switch (size) {
    case 1: *(uint8_t *)res = *(volatile uint8_t *)p; break;
    case 2: *(uint16_t *)res = *(volatile uint16_t *)p; break;
    case 4: *(uint32_t *)res = *(volatile uint32_t *)p; break;
    case 8: *(uint64_t *)res = *(volatile uint64_t *)p; break;
    default:
        barrier();
        memcpy((void *)res, (const void *)p, size);
        barrier();
    }
}

#define WRITE_ONCE(x, val)                                      \
    ({                                                          \
        union { typeof(x) __val; char __c[1]; } __u =           \
            { .__val = (typeof(x))(val) };                      \
        __write_once_size(&(x), __u.__c, sizeof(x));            \
        __u.__val;                                              \
    })

#define READ_ONCE(x)                                            \
    ({                                                          \
        union { typeof(x) __val; char __c[1]; } __u;            \
        __read_once_size(&(x), __u.__c, sizeof(x));             \
        __u.__val;                                              \
    })

/* --------------------------------------------------------------------
 * Atomic primitives
 * -------------------------------------------------------------------- */
#define smp_cas(__ptr, __old_val, __new_val)     \
    __sync_val_compare_and_swap(__ptr, __old_val, __new_val)
#define smp_swap(__ptr, __val)                   \
    __sync_lock_test_and_set(__ptr, __val)
#define smp_faa(__ptr, __val)                    \
    __sync_fetch_and_add(__ptr, __val)

#define atomic_andnot(val, ptr) \
    __sync_fetch_and_and((ptr), ~(val))
#define atomic_fetch_or_acquire(val, ptr) \
    __sync_fetch_and_or((ptr), (val))

#ifndef CPU_PAUSE
#define CPU_PAUSE() __asm__ __volatile__("pause" ::: "memory")
#endif

#endif /* __AQS_H__ guard */

/* --------------------------------------------------------------------
 * TSC reading
 * -------------------------------------------------------------------- */
static inline uint64_t cfl_rdtsc(void)
{
    uint32_t low, high;
    __asm__ __volatile__("rdtsc" : "=a"(low), "=d"(high));
    return (uint64_t)low | ((uint64_t)high << 32);
}

/* --------------------------------------------------------------------
 * CFL constants (from fairnumas.h)
 * Prefixed with CFL_ to avoid clashes with aqs.h constants.
 * -------------------------------------------------------------------- */
#define CFL_LOCKED_OFFSET           0
#define CFL_LOCKED_BITS             8

#define CFL_NOSTEAL_VAL             (1U << (CFL_LOCKED_OFFSET + CFL_LOCKED_BITS))
#define CFL_STATUS_WAIT             0
#define CFL_STATUS_LOCKED           1

#ifndef UNLOCK_COUNT_THRESHOLD
#define UNLOCK_COUNT_THRESHOLD      1024
#endif

/* --------------------------------------------------------------------
 * Data structures (from fairnumas.h, verbatim layout)
 * -------------------------------------------------------------------- */
typedef struct cfl_node {
    struct cfl_node *next;
    union {
        uint32_t locked;
        struct {
            uint8_t lstatus;
            uint8_t sleader;
            uint16_t wcount;
        };
    };
    int nid;
    int cid;
    struct cfl_node *last_visited;

    unsigned long runtime;
    int type;
    char __pad2[pad_to_cache_line(sizeof(uint32_t))];
} __attribute__((aligned(L_CACHE_LINE_SIZE))) cfl_node_t;

typedef struct cfl_mutex {
    struct cfl_node *tail;
    union {
        uint32_t val;
        struct {
            uint8_t locked;
            uint8_t no_stealing;
        };
        struct {
            uint16_t locked_no_stealing;
            uint8_t __pad[2];
        };
    };
    char __pad2[pad_to_cache_line(sizeof(uint32_t))];
} __attribute__((aligned(L_CACHE_LINE_SIZE))) cfl_mutex_t;

/* --------------------------------------------------------------------
 * API
 * -------------------------------------------------------------------- */
void cfl_init(cfl_mutex_t *lock);
void cfl_lock(cfl_mutex_t *lock, cfl_node_t *me);
void cfl_unlock(cfl_mutex_t *lock, cfl_node_t *me);

#endif /* __CFL_H__ */
