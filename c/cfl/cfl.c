/*
 * CFL — Compact Fair Lock (Manglik & Kim, PPoPP'24)
 *
 * Adapted from the original fairnumas implementation:
 *   https://github.com/rs-ifl/CFL
 *
 * Original code: MIT License, Copyright (c) 2016 Hugo Guiroux.
 * Stripped: COND_VAR, PAPI, LiTL interpose, debug infrastructure.
 * Kept: core CFL algorithm with usage-fair (vLHT-based) queue shuffling.
 *
 * The algorithm in shuffle_waiters(), __cfl_lock(), and __cfl_unlock()
 * is unmodified from the original fairnumas.c.
 */

#include "cfl.h"
#include <unistd.h>

/* ====================================================================
 * Topology detection (auto-detect, matching c/shfllock/aqs.c)
 * ==================================================================== */
static int cpu_number = 0;
static int numa_nodes = 1;

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

/* ====================================================================
 * Per-thread ID (replaces LiTL cur_thread_id)
 * ==================================================================== */
static __thread unsigned int cfl_thread_id = 0;
static unsigned int cfl_next_id = 1;

static inline void ensure_thread_id(void)
{
    if (cfl_thread_id == 0)
        cfl_thread_id = smp_faa(&cfl_next_id, 1);
}

/* ====================================================================
 * Globals (from fairnumas.c, verbatim)
 * ==================================================================== */
unsigned long runtime_checker_core[256];
unsigned long runtime_checker_node[16];
int allowed_node;

/* ====================================================================
 * NUMA helpers (from fairnumas.c, verbatim)
 * ==================================================================== */
static inline int current_numa_node(void)
{
    if (__builtin_expect(cpu_number == 0, 0))
        detect_topology();
    unsigned long a, d, c;
    int core;
    __asm__ volatile("rdtscp" : "=a"(a), "=d"(d), "=c"(c));
    core = c & 0xFFF;
    return core % numa_nodes;
}

static inline int current_numa_core(void)
{
    unsigned long a, d, c;
    int core;
    __asm__ volatile("rdtscp" : "=a"(a), "=d"(d), "=c"(c));
    core = c & 0xFFF;
    return core;
}

/* ====================================================================
 * Shuffle quota PRNG (from fairnumas.c, verbatim)
 * ==================================================================== */
#define THRESHOLD (0xffff)

static inline uint32_t xor_random(void)
{
    static __thread uint32_t rv = 0;

    if (rv == 0)
        rv = cfl_thread_id + 1;

    uint32_t v = rv;
    v ^= v << 6;
    v ^= (uint32_t)(v) >> 21;
    v ^= v << 7;
    rv = v;

    return v & (UNLOCK_COUNT_THRESHOLD - 1);
}

static inline int keep_lock_local(void)
{
    return xor_random() & THRESHOLD;
}

/* ====================================================================
 * Stealing control helpers (from fairnumas.c, verbatim)
 * ==================================================================== */
static inline void enable_stealing(cfl_mutex_t *lock)
{
    atomic_andnot(CFL_NOSTEAL_VAL, &lock->val);
}

static inline void disable_stealing(cfl_mutex_t *lock)
{
    atomic_fetch_or_acquire(CFL_NOSTEAL_VAL, &lock->val);
}

static inline uint8_t is_stealing_disabled(cfl_mutex_t *lock)
{
    return READ_ONCE(lock->no_stealing);
}

/* ====================================================================
 * Shuffle leader helpers (from fairnumas.c, verbatim)
 * ==================================================================== */
static inline void set_sleader(struct cfl_node *node, struct cfl_node *qend)
{
    WRITE_ONCE(node->sleader, 1);
    if (qend != node)
        WRITE_ONCE(node->last_visited, qend);
}

static inline void clear_sleader(struct cfl_node *node)
{
    node->sleader = 0;
}

static inline void set_waitcount(struct cfl_node *node, int count)
{
    WRITE_ONCE(node->wcount, count);
}

/* ====================================================================
 * need_switch() — NUMA fairness decision (from fairnumas.c, verbatim)
 * ==================================================================== */
static inline int need_switch(void)
{
    int i, minid;
    unsigned long max, min, threshold, value;
    max = 0;
    min = runtime_checker_node[0];
    minid = 0;
    threshold = 100000;
    value = 9;
    for (i = 0; i < numa_nodes; i++) {
        value = READ_ONCE(runtime_checker_node[i]);
        if (max < value) {
            max = value;
        }
        if (min > value) {
            min = value;
            minid = i;
        }
    }
    if (max - min <= threshold) {
        WRITE_ONCE(allowed_node, 100);
        return 100;
    } else {
        WRITE_ONCE(allowed_node, minid);
        return minid;
    }
}

/* ====================================================================
 * shuffle_waiters() — core CFL queue reordering
 * (from fairnumas.c, unmodified algorithm)
 * ==================================================================== */
static void shuffle_waiters(cfl_mutex_t *lock, struct cfl_node *node,
                            int is_next_waiter)
{
    cfl_node_t *curr, *prev, *next, *last, *sleader, *qend, *iter, *stand;
    int nid = node->nid;
    int curr_locked_count = node->wcount;
    int one_shuffle = 0;
    uint32_t lock_ready;

    unsigned long standard;

    prev = READ_ONCE(node->last_visited);
    if (!prev)
        prev = node;
    sleader = NULL;
    prev = node;
    last = node;
    curr = NULL;
    next = NULL;
    qend = NULL;
    stand = prev;

    standard = 0;

    iter = NULL;

    if (curr_locked_count == 0)
        set_waitcount(node, ++curr_locked_count);

    clear_sleader(node);

    if (!keep_lock_local()) {
    }

    nid = need_switch();
    if (nid == 100)
        nid = node->nid;

    standard = READ_ONCE(runtime_checker_node[nid]) / 16;
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

        /* got the current for sure */

        /* Check if curr->nid is same as nid */
        if (curr->nid == nid) {
            if (prev == node && prev->nid == nid) {
                set_waitcount(curr, curr_locked_count);
                last = curr;
                prev = curr;
                one_shuffle = 1;
            } else {
                next = READ_ONCE(curr->next);
                if (!next) {
                    sleader = last;
                    qend = prev;
                    goto out;
                }

                if (runtime_checker_core[curr->cid] >= standard) {
                    prev = curr;
                    goto check;
                }
                iter = stand;

                while (iter->next && iter->next->nid == curr->nid &&
                       (runtime_checker_core[curr->cid] >
                        runtime_checker_core[iter->next->cid]) &&
                       iter != last) {
                    iter = iter->next;
                    barrier();
                }

                set_waitcount(curr, curr_locked_count);

                if (iter != prev) {
                    prev->next = next;
                    curr->next = iter->next;
                    iter->next = curr;
                } else
                    prev = curr;

                if (iter == last) {
                    last = curr;
                }
                one_shuffle = 1;
            }
        } else
            prev = curr;

    check:
        lock_ready = !READ_ONCE(lock->locked);
        if (one_shuffle &&
            ((is_next_waiter && lock_ready) ||
             (!is_next_waiter && READ_ONCE(node->lstatus)))) {
            sleader = last;
            qend = prev;
            break;
        }
    }

out:
    if (sleader) {
        set_sleader(sleader, qend);
    }
}

/* ====================================================================
 * __cfl_lock() — lock acquisition
 * (from fairnumas.c __aqs_mutex_lock, unmodified algorithm)
 * ==================================================================== */
static int __cfl_lock(cfl_mutex_t *impl, cfl_node_t *me)
{
    cfl_node_t *prev;

    me->cid = current_numa_core();
    me->nid = current_numa_node();
    me->runtime = 0;

    if (allowed_node == 100 ||
        (me->nid == allowed_node &&
         runtime_checker_core[me->cid] <
             READ_ONCE(runtime_checker_node[me->nid]) / 16)) {
        if (smp_cas(&impl->locked_no_stealing, 0, 1) == 0) {
            goto release;
        }
    }

    me->locked = CFL_STATUS_WAIT;
    me->next = NULL;
    me->last_visited = NULL;

    /*
     * Publish the updated tail.
     */
    prev = smp_swap(&impl->tail, me);

    if (prev) {

        WRITE_ONCE(prev->next, me);

        /*
         * Now, we are waiting for the lock holder to
         * allow us to become the very next lock waiter.
         * In the meantime, we also check whether the node
         * is the shuffle leader, if that's the case,
         * then it goes on shuffling waiters in its socket
         */
        for (;;) {

            if (READ_ONCE(me->lstatus) == CFL_STATUS_LOCKED)
                break;

            if (READ_ONCE(me->sleader)) {
                shuffle_waiters(impl, me, 0);
            }

            CPU_PAUSE();
        }
    } else
        disable_stealing(impl);

    /*
     * we are now the very next waiters, all we have to do is
     * to wait for the @lock->locked to become 0, i.e. unlocked.
     * In the meantime, we will try to be shuffle leader if possible
     * and at least find someone in my socket.
     */
    for (;;) {
        int wcount;

        if (!READ_ONCE(impl->locked))
            break;

        /*
         * There are two ways to become a shuffle leader:
         * 1) my @node->wcount is 0
         * 2) someone or myself (earlier) appointed me
         * (@node->sleader = 1)
         */
        wcount = me->wcount;
        if (!wcount || (wcount && me->sleader)) {
            shuffle_waiters(impl, me, 1);
        }
    }

    /*
     * The biggest catch with our algorithm is that it allows
     * stealing in the fast path.
     * Thus, even if the @lock->locked was 0 above, it doesn't
     * mean that we have the lock. So, we acquire the lock
     * in two ways:
     * 1) Either someone disabled the lock stealing before us
     * that allows us to directly set the lock->locked value 1
     * 2) Or, I will explicitly try to do a cmpxchg operation
     * on the @lock->locked variable. If I am unsuccessful for
     * @impatient_cap times, then I explicitly lock stealing,
     * this is to ensure starvation freedom, and will wait
     * for the lock->locked status to change to 0.
     */
    for (;;) {
        /*
         * If someone has already disable stealing,
         * change locked and proceed forward
         */
        if (smp_cas(&impl->locked, 0, 1) == 0)
            break;

        while (READ_ONCE(impl->locked))
            CPU_PAUSE();
    }

    if (!READ_ONCE(me->next)) {
        if (smp_cas(&impl->tail, me, NULL) == me) {
            enable_stealing(impl);
            goto release;
        }

        while (!READ_ONCE(me->next))
            CPU_PAUSE();
    }

    WRITE_ONCE(me->next->lstatus, 1);

release:
    barrier();
    me->runtime = cfl_rdtsc();
    barrier();
    return 0;
}

/* ====================================================================
 * __cfl_unlock() — lock release
 * (from fairnumas.c __aqs_mutex_unlock, unmodified algorithm)
 * ==================================================================== */
static inline void __cfl_unlock(cfl_mutex_t *impl, cfl_node_t *me)
{
    unsigned long cslength;

    if (me->runtime != 0) {
        cslength = cfl_rdtsc() - me->runtime;
        runtime_checker_core[me->cid] += cslength;
        runtime_checker_node[me->nid] += cslength;
    }
    WRITE_ONCE(impl->locked, 0);
}

/* ====================================================================
 * Public API
 * ==================================================================== */

void cfl_init(cfl_mutex_t *lock)
{
    detect_topology();
    memset(lock, 0, sizeof(*lock));
}

void cfl_lock(cfl_mutex_t *lock, cfl_node_t *me)
{
    ensure_thread_id();
    __cfl_lock(lock, me);
}

void cfl_unlock(cfl_mutex_t *lock, cfl_node_t *me)
{
    __cfl_unlock(lock, me);
}
