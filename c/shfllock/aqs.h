/*
 * Adaptive Queued Spinlock (AQS) with NUMA-aware shuffling.
 *
 * Adapted from the ShflLock project (Kashyap et al., SOSP'19):
 *   https://github.com/sslab-gatech/shfllock
 *
 * Original code: MIT License, Copyright (c) 2016 Hugo Guiroux.
 * This adaptation strips COND_VAR, PAPI, LiTL interpose, and debug
 * infrastructure, keeping only the core non-blocking AQS algorithm.
 */
#ifndef __AQS_H__
#define __AQS_H__

#include <stdint.h>
#include <stddef.h>
#include <string.h>

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

/* --------------------------------------------------------------------
 * AQS constants
 * -------------------------------------------------------------------- */
#define AQS_STATUS_WAIT     0
#define AQS_STATUS_LOCKED   1

#define _AQS_LOCKED_OFFSET          0
#define _AQS_LOCKED_BITS            8
#define _AQS_NOSTEAL_VAL            (1U << (_AQS_LOCKED_OFFSET + _AQS_LOCKED_BITS))

#ifndef UNLOCK_COUNT_THRESHOLD
#define UNLOCK_COUNT_THRESHOLD 1024
#endif

/* --------------------------------------------------------------------
 * Data structures
 * -------------------------------------------------------------------- */
typedef struct aqs_node {
    struct aqs_node *next;
    union {
        uint32_t locked;
        struct {
            uint8_t lstatus;
            uint8_t sleader;
            uint16_t wcount;
        };
    };
    int nid;                        /* NUMA node ID */
    int cid;                        /* core ID (debug) */
    struct aqs_node *last_visited;
    char __pad[pad_to_cache_line(
        sizeof(void *) +           /* next */
        sizeof(uint32_t) +         /* locked union */
        sizeof(int) +              /* nid */
        sizeof(int) +              /* cid */
        sizeof(void *)             /* last_visited */
    )];
} __attribute__((aligned(L_CACHE_LINE_SIZE))) aqs_node_t;

typedef struct aqs_mutex {
    struct aqs_node *tail;
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
    char __pad2[pad_to_cache_line(
        sizeof(void *) +           /* tail */
        sizeof(uint32_t)           /* val union */
    )];
} __attribute__((aligned(L_CACHE_LINE_SIZE))) aqs_mutex_t;

/* --------------------------------------------------------------------
 * API
 * -------------------------------------------------------------------- */
void aqs_init(aqs_mutex_t *lock);
void aqs_lock(aqs_mutex_t *lock, aqs_node_t *me);
int  aqs_trylock(aqs_mutex_t *lock);
void aqs_unlock(aqs_mutex_t *lock);

#endif /* __AQS_H__ */
