/*
 * Adaptive Queued Spinlock (AQS) with NUMA-aware shuffling.
 *
 * Adapted from the ShflLock project (Kashyap et al., SOSP'19):
 *   https://github.com/sslab-gatech/shfllock
 *   ulocks/src/aqs.c
 *
 * Original code: MIT License, Copyright (c) 2016 Hugo Guiroux.
 * Stripped: COND_VAR, PAPI, LiTL interpose, debug, BLOCKING_FAIRNESS,
 *           VRUNTIME_FAIRNESS.  Kept: core non-blocking AQS algorithm.
 */

#include "aqs.h"
#include <unistd.h>

/* --------------------------------------------------------------------
 * NUMA topology (auto-detected at first use)
 * -------------------------------------------------------------------- */
static int cpu_number  = 0;
static int numa_nodes  = 1;

static void detect_topology(void)
{
    if (cpu_number != 0)
        return;

    long n = sysconf(_SC_NPROCESSORS_ONLN);
    cpu_number = (n > 0) ? (int)n : 1;

    /* Try to count NUMA nodes from sysfs. */
    int nodes = 0;
    for (int i = 0; i < 256; i++) {
        char path[128];
        __builtin_snprintf(path, sizeof(path),
                           "/sys/devices/system/node/node%d", i);
        if (access(path, F_OK) == 0)
            nodes++;
        else if (i > 0)
            break;
    }
    numa_nodes = (nodes > 0) ? nodes : 1;
}

static inline int current_numa_node(void)
{
    if (__builtin_expect(cpu_number == 0, 0))
        detect_topology();
    unsigned int a, d, c;
    __asm__ __volatile__("rdtscp" : "=a"(a), "=d"(d), "=c"(c));
    int core = c & 0xFFF;
    return core / (cpu_number / numa_nodes);
}

/* --------------------------------------------------------------------
 * Shuffle quota PRNG (per-thread XOR-shift)
 * -------------------------------------------------------------------- */
static __thread uint32_t xor_rv = 0;
static __thread unsigned int aqs_thread_id = 0;
static unsigned int aqs_next_id = 1;

static inline uint32_t xor_random(void)
{
    if (xor_rv == 0)
        xor_rv = smp_faa(&aqs_next_id, 1);

    uint32_t v = xor_rv;
    v ^= v << 6;
    v ^= v >> 21;
    v ^= v << 7;
    xor_rv = v;

    return v & (UNLOCK_COUNT_THRESHOLD - 1);
}

#define THRESHOLD 0xffff

static inline int keep_lock_local(void)
{
    return xor_random() & THRESHOLD;
}

/* --------------------------------------------------------------------
 * Stealing control helpers
 * -------------------------------------------------------------------- */
static inline void enable_stealing(aqs_mutex_t *lock)
{
    atomic_andnot(_AQS_NOSTEAL_VAL, &lock->val);
}

static inline void disable_stealing(aqs_mutex_t *lock)
{
    atomic_fetch_or_acquire(_AQS_NOSTEAL_VAL, &lock->val);
}

/* --------------------------------------------------------------------
 * Shuffle leader helpers
 * -------------------------------------------------------------------- */
static inline void set_sleader(struct aqs_node *node, struct aqs_node *qend)
{
    WRITE_ONCE(node->sleader, 1);
    if (qend != node)
        WRITE_ONCE(node->last_visited, qend);
}

static inline void clear_sleader(struct aqs_node *node)
{
    node->sleader = 0;
}

static inline void set_waitcount(struct aqs_node *node, int count)
{
    WRITE_ONCE(node->wcount, count);
}

/* --------------------------------------------------------------------
 * shuffle_waiters — core NUMA-aware queue reordering
 *
 * Walks the queue from node->last_visited, grouping same-socket
 * waiters together by pointer surgery.  Only one thread (the shuffle
 * leader) runs this at a time.
 * -------------------------------------------------------------------- */
static void shuffle_waiters(aqs_mutex_t *lock, struct aqs_node *node,
                            int is_next_waiter)
{
    struct aqs_node *curr, *prev, *next, *last, *sleader, *qend;
    int nid = node->nid;
    int curr_locked_count = node->wcount;
    int one_shuffle = 0;
    uint32_t lock_ready;

    prev = READ_ONCE(node->last_visited);
    if (!prev)
        prev = node;

    sleader = NULL;
    last = node;
    curr = NULL;
    next = NULL;
    qend = NULL;

    if (curr_locked_count == 0)
        set_waitcount(node, ++curr_locked_count);

    clear_sleader(node);

    if (!keep_lock_local()) {
        sleader = READ_ONCE(node->next);
        goto out;
    }

    for (;;) {
        curr = READ_ONCE(prev->next);

        barrier();

        if (!curr) {
            sleader = last;
            qend = prev;
            break;
        }

        if (curr == READ_ONCE(lock->tail)) {
            sleader = last;
            qend = prev;
            break;
        }

        /* Check if curr->nid matches our socket */
        if (curr->nid == nid) {
            if (prev->nid == nid) {
                /* Already adjacent to same-socket group */
                set_waitcount(curr, curr_locked_count);
                last = curr;
                prev = curr;
                one_shuffle = 1;
            } else {
                /* Move curr after last same-socket node */
                next = READ_ONCE(curr->next);
                if (!next) {
                    sleader = last;
                    qend = prev;
                    goto out;
                }

                set_waitcount(curr, curr_locked_count);
                prev->next = next;
                curr->next = last->next;
                last->next = curr;
                last = curr;
                one_shuffle = 1;
            }
        } else {
            prev = curr;
        }

        lock_ready = !READ_ONCE(lock->locked);
        if (one_shuffle && ((is_next_waiter && lock_ready) ||
                            (!is_next_waiter && READ_ONCE(node->lstatus)))) {
            sleader = last;
            qend = prev;
            break;
        }
    }

out:
    if (sleader)
        set_sleader(sleader, qend);
}

/* --------------------------------------------------------------------
 * Public API
 * -------------------------------------------------------------------- */

void aqs_init(aqs_mutex_t *lock)
{
    detect_topology();
    memset(lock, 0, sizeof(*lock));
}

void aqs_lock(aqs_mutex_t *lock, aqs_node_t *me)
{
    struct aqs_node *prev;

    /* Fast path: uncontended CAS */
    if (smp_cas(&lock->locked_no_stealing, 0, 1) == 0)
        goto acquired;

    /* Slow path: MCS-style enqueue */
    me->next = NULL;
    me->locked = AQS_STATUS_WAIT;   /* clears lstatus, sleader, wcount */
    me->nid = current_numa_node();
    me->last_visited = NULL;

    prev = smp_swap(&lock->tail, me);

    if (prev) {
        /* Link into predecessor */
        WRITE_ONCE(prev->next, me);

        /*
         * Wait for lock holder to mark us as the next waiter.
         * While waiting, participate in shuffling if elected.
         */
        for (;;) {
            if (READ_ONCE(me->lstatus) == AQS_STATUS_LOCKED)
                break;

            if (READ_ONCE(me->sleader))
                shuffle_waiters(lock, me, 0);

            CPU_PAUSE();
        }
    } else {
        /* We are the queue head — disable fast-path stealing */
        disable_stealing(lock);
    }

    /*
     * We are now the very next waiter.  Spin for the lock byte,
     * shuffling while we wait.
     */
    for (;;) {
        if (!READ_ONCE(lock->locked))
            break;

        {
            int wcount = me->wcount;
            if (!wcount || (wcount && me->sleader))
                shuffle_waiters(lock, me, 1);
        }
    }

    /*
     * Acquire: CAS the lock byte.  A fast-path stealer may race us,
     * so we must retry.
     */
    for (;;) {
        if (smp_cas(&lock->locked, 0, 1) == 0)
            break;

        while (READ_ONCE(lock->locked))
            CPU_PAUSE();
    }

    /*
     * Hand off to the next waiter in the queue (set their lstatus).
     * If we are the last in the queue, detach and re-enable stealing.
     */
    if (!READ_ONCE(me->next)) {
        if (smp_cas(&lock->tail, me, NULL) == me) {
            enable_stealing(lock);
            goto acquired;
        }
        /* Successor is linking; wait for it. */
        while (!READ_ONCE(me->next))
            CPU_PAUSE();
    }

    WRITE_ONCE(me->next->lstatus, AQS_STATUS_LOCKED);

acquired:
    return;
}

int aqs_trylock(aqs_mutex_t *lock)
{
    return (smp_cas(&lock->locked, 0, 1) == 0) ? 0 : 1;
}

void aqs_unlock(aqs_mutex_t *lock)
{
    WRITE_ONCE(lock->locked, 0);
}
