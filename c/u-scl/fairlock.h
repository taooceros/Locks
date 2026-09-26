// GCC-VERSION - Not tested for -O3 flag for GCC version above 5.

#ifndef __FAIRLOCK_H__
#define __FAIRLOCK_H__

#define _GNU_SOURCE
#include <stddef.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>
#include <sched.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <linux/futex.h>
#include <pthread.h>
#include "rdtsc.h"
#include "common.h"

typedef unsigned long long ull;

#ifdef DEBUG
typedef struct stats {
    ull reenter;
    ull banned_time;
    ull start;
    ull next_runnable_wait;
    ull prev_slice_wait;
    ull own_slice_wait;
    ull runnable_wait;
    ull succ_wait;
    ull release_succ_wait;
} stats_t;
#endif

typedef struct fairlock fairlock_t;
typedef struct flthread_info {
    ull banned_until;
    ull weight;
    ull slice;
    fairlock_t *owner;
    ull start_ticks;
    int banned;
#ifdef DEBUG
    stats_t stat;
#endif
} flthread_info_t;

enum qnode_state {
    INIT = 0, // not waiting or after next runnable node
    NEXT,
    RUNNABLE,
    RUNNING
};

typedef struct qnode {
    int state __attribute__ ((aligned (CACHELINE)));
    struct qnode *next __attribute__ ((aligned (CACHELINE)));
} qnode_t __attribute__ ((aligned (CACHELINE)));

struct fairlock {
    qnode_t *qtail __attribute__ ((aligned (CACHELINE)));
    qnode_t *qnext __attribute__ ((aligned (CACHELINE)));
    ull slice __attribute__ ((aligned (CACHELINE)));
    int slice_valid __attribute__ ((aligned (CACHELINE)));
    pthread_key_t flthread_info_key;
    ull total_weight;
    int bridge_mode;
} __attribute__ ((aligned (CACHELINE)));

static inline qnode_t *flqnode(fairlock_t *lock) {
    return (qnode_t *) ((char *) &lock->qnext - offsetof(qnode_t, next));
}

static inline int futex(int *uaddr, int futex_op, int val, const struct timespec *timeout) {
    return syscall(SYS_futex, uaddr, futex_op, val, timeout, NULL, 0);
}

static void flthread_info_destroy(void *value) {
    flthread_info_t *info = (flthread_info_t *)value;
    if (info) {
        __atomic_fetch_sub(&info->owner->total_weight, info->weight, __ATOMIC_RELAXED);
        free(info);
    }
}

static int fairlock_init_with_destructor(fairlock_t *lock,
                                         void (*destructor)(void *), int bridge_mode) {
    int rc;
    lock->qtail = NULL;
    lock->qnext = NULL;
    lock->total_weight = 0;
    lock->slice = 0;
    lock->slice_valid = 0;
    lock->bridge_mode = bridge_mode;
    if (0 != (rc = pthread_key_create(&lock->flthread_info_key, destructor)))
        return rc;
    return 0;
}

/* Preserve legacy USCL<T>'s no-destructor/no-owner-pointer behavior: that
 * wrapper still moves its lock after initialization and has no Drop. */
int fairlock_init(fairlock_t *lock) {
    return fairlock_init_with_destructor(lock, NULL, 0);
}

/* Only the bridge allocates the lock at its permanent address before init. */
int fairlock_bridge_init(fairlock_t *lock) {
    return fairlock_init_with_destructor(lock, flthread_info_destroy, 1);
}

static flthread_info_t *flthread_info_create(fairlock_t *lock, int weight) {
    flthread_info_t *info;
    info = malloc(sizeof(flthread_info_t));
    if (!info)
        abort();
    info->owner = lock->bridge_mode ? lock : NULL;
    info->banned_until = rdtsc();
    if (weight == 0) {
        int prio = getpriority(PRIO_PROCESS, 0);
        if (prio < -20 || prio > 19)
            prio = 0;
        weight = prio_to_weight[prio+20];
    }
    if (weight <= 0)
        abort();
    info->weight = weight;
    __sync_add_and_fetch(&lock->total_weight, weight);
    info->banned = 0;
    info->slice = 0;
    info->start_ticks = 0;
#ifdef DEBUG
    memset(&info->stat, 0, sizeof(stats_t));
    info->stat.start = info->banned_until;
#endif
    return info;
}

void fairlock_thread_init(fairlock_t *lock, int weight) {
    flthread_info_t *info = (flthread_info_t *)pthread_getspecific(lock->flthread_info_key);
    if (info) {
        if (pthread_setspecific(lock->flthread_info_key, NULL))
            abort();
        if (info->owner)
            flthread_info_destroy(info);
        else
            free(info);
    }
    info = flthread_info_create(lock, weight);
    if (pthread_setspecific(lock->flthread_info_key, info)) {
        if (info->owner)
            flthread_info_destroy(info);
        else
            free(info);
        abort();
    }
}

/* One stable registration per physical worker and lock, never reset per call. */
void fairlock_thread_register(fairlock_t *lock, int weight) {
    if (!pthread_getspecific(lock->flthread_info_key))
        fairlock_thread_init(lock, weight);
}

/* The legacy entry point never deleted its key; keep that contract while
 * legacy USCL<T> may still move/drop without joining its registered users. */
int fairlock_destroy(fairlock_t *lock) {
    (void)lock;
    return 0;
}

/* Only after every other bridge worker has exited and joined. */
int fairlock_bridge_destroy(fairlock_t *lock) {
    if (!lock->bridge_mode)
        abort();
    flthread_info_t *info = (flthread_info_t *)pthread_getspecific(lock->flthread_info_key);
    if (info) {
        if (pthread_setspecific(lock->flthread_info_key, NULL))
            abort();
        flthread_info_destroy(info);
    }
    return pthread_key_delete(lock->flthread_info_key);
}

void fairlock_acquire(fairlock_t *lock) {
    flthread_info_t *info;
    ull now;

    info = (flthread_info_t *) pthread_getspecific(lock->flthread_info_key);
    if (NULL == info) {
        fairlock_thread_init(lock, 0);
        info = (flthread_info_t *)pthread_getspecific(lock->flthread_info_key);
    }
    if (readvol(lock->slice_valid)) {
        ull curr_slice = lock->slice;
        // If owner of current slice, try to reenter at the beginning of the queue
        if (curr_slice == info->slice && (now = rdtsc()) < curr_slice) {
            qnode_t *succ = readvol(lock->qnext);
            if (NULL == succ) {
                if (__sync_bool_compare_and_swap(&lock->qtail, NULL, flqnode(lock)))
                    goto reenter;
                spin_then_yield(SPIN_LIMIT, (now = rdtsc()) < curr_slice && NULL == (succ = readvol(lock->qnext)));
#ifdef DEBUG
                info->stat.own_slice_wait += rdtsc() - now;
#endif
                // let the succ invalidate the slice, and don't need to wake it up because slice expires naturally
                if (now >= curr_slice)
                    goto begin;
            }
            // if state < RUNNABLE, it won't become RUNNABLE unless someone releases lock,
            // but as no one is holding the lock, there is no race
            if (succ->state < RUNNABLE || __sync_bool_compare_and_swap(&succ->state, RUNNABLE, NEXT)) {
reenter:
#ifdef DEBUG
                info->stat.reenter++;
#endif
                info->start_ticks = now;
                return;
            }
        }
    }
begin:

    if (info->banned) {
        if ((now = rdtsc()) < info->banned_until) {
            ull banned_time = info->banned_until - now;
#ifdef DEBUG
            info->stat.banned_time += banned_time;
#endif
            // sleep with granularity of SLEEP_GRANULARITY us
            while (banned_time > CYCLE_PER_US * SLEEP_GRANULARITY) {
                struct timespec req = {
                    .tv_sec = banned_time / CYCLE_PER_S,
                    .tv_nsec = (banned_time % CYCLE_PER_S / CYCLE_PER_US / SLEEP_GRANULARITY) * SLEEP_GRANULARITY * 1000,
                };
                nanosleep(&req, NULL);
                if ((now = rdtsc()) >= info->banned_until)
                    break;
                banned_time = info->banned_until - now;
            }
            // spin for the remaining (<SLEEP_GRANULARITY us)
            spin_then_yield(SPIN_LIMIT, (now = rdtsc()) < info->banned_until);
        }
    }

    qnode_t n = { 0 };
    while (1) {
        qnode_t *prev = readvol(lock->qtail);
        if (__sync_bool_compare_and_swap(&lock->qtail, prev, &n)) {
            // enter the lock queue
            if (NULL == prev) {
                n.state = RUNNABLE;
                lock->qnext = &n;
            } else {
                if (prev == flqnode(lock)) {
                    n.state = NEXT;
                    prev->next = &n;
                } else {
                    prev->next = &n;
                    // wait until we become the next runnable
#ifdef DEBUG
                    now = rdtsc();
#endif
                    do {
                        futex(&n.state, FUTEX_WAIT_PRIVATE, INIT, NULL);
                    } while (INIT == readvol(n.state));
#ifdef DEBUG
                    info->stat.next_runnable_wait += rdtsc() - now;
#endif
                }
            }
            // invariant: n.state >= NEXT

            // wait until the current slice expires
            int slice_valid;
            ull curr_slice;
            while ((slice_valid = readvol(lock->slice_valid)) && (now = rdtsc()) + SLEEP_GRANULARITY < (curr_slice = readvol(lock->slice))) {
                ull slice_left = curr_slice - now;
                struct timespec timeout = {
                    .tv_sec = 0, // slice will be less then 1 sec
                    .tv_nsec = (slice_left / (CYCLE_PER_US * SLEEP_GRANULARITY)) * SLEEP_GRANULARITY * 1000,
                };
                futex(&lock->slice_valid, FUTEX_WAIT_PRIVATE, 0, &timeout);
#ifdef DEBUG
                info->stat.prev_slice_wait += rdtsc() - now;
#endif
            }
            if (slice_valid) {
                spin_then_yield(SPIN_LIMIT, (slice_valid = readvol(lock->slice_valid)) && rdtsc() < readvol(lock->slice));
                if (slice_valid)
                    lock->slice_valid = 0;
            }
            // invariant: rdtsc() >= curr_slice && lock->slice_valid == 0

#ifdef DEBUG
            now = rdtsc();
#endif
            // spin until RUNNABLE and try to grab the lock
            spin_then_yield(SPIN_LIMIT, RUNNABLE != readvol(n.state) || 0 == __sync_bool_compare_and_swap(&n.state, RUNNABLE, RUNNING));
            // invariant: n.state == RUNNING
#ifdef DEBUG
            info->stat.runnable_wait += rdtsc() - now;
#endif

            // record the successor in the lock so we can notify it when we release
            qnode_t *succ = readvol(n.next);
            if (NULL == succ) {
                lock->qnext = NULL;
                if (0 == __sync_bool_compare_and_swap(&lock->qtail, &n, flqnode(lock))) {
                    spin_then_yield(SPIN_LIMIT, NULL == (succ = readvol(n.next)));
#ifdef DEBUG
                    info->stat.succ_wait += rdtsc() - now;
#endif
                    lock->qnext = succ;
                }
            } else {
                lock->qnext = succ;
            }
            // invariant: NULL == succ <=> lock->qtail == flqnode(lock)

            now = rdtsc();
            info->start_ticks = now;
            info->slice = now + FAIRLOCK_GRANULARITY;
            lock->slice = info->slice;
            lock->slice_valid = 1;
            // wake up successor if necessary
            if (succ) {
                succ->state = NEXT;
                futex(&succ->state, FUTEX_WAKE_PRIVATE, 1, NULL);
            }
            return;
        }
    }
}

void fairlock_release(fairlock_t *lock) {
    ull now, cs;
#ifdef DEBUG
    ull succ_start = 0, succ_end = 0;
#endif
    flthread_info_t *info;

    qnode_t *succ = lock->qnext;
    if (NULL == succ) {
        if (__sync_bool_compare_and_swap(&lock->qtail, flqnode(lock), NULL))
            goto accounting;
#ifdef DEBUG
        succ_start = rdtsc();
#endif
        spin_then_yield(SPIN_LIMIT, NULL == (succ = readvol(lock->qnext)));
#ifdef DEBUG
        succ_end = rdtsc();
#endif
    }
    succ->state = RUNNABLE;

accounting:
    // invariant: NULL == succ || succ->state = RUNNABLE
    info = (flthread_info_t *) pthread_getspecific(lock->flthread_info_key);
    now = rdtsc();
    cs = now - info->start_ticks;
    info->banned_until += cs * (__atomic_load_n(&lock->total_weight, __ATOMIC_RELAXED) / info->weight);
    info->banned = now < info->banned_until;

    if (info->banned) {
        if (__sync_bool_compare_and_swap(&lock->slice_valid, 1, 0)) {
            futex(&lock->slice_valid, FUTEX_WAKE_PRIVATE, 1, NULL);
        }
    }
#ifdef DEBUG
    info->stat.release_succ_wait += succ_end - succ_start;
#endif
}

#endif // __FAIRLOCK_H__
