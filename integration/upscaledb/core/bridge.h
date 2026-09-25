#ifndef LOCKS_UPSCALEDB_BRIDGE_H
#define LOCKS_UPSCALEDB_BRIDGE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct dlock_bridge dlock_bridge;
typedef void (*dlock_bridge_callback)(void *context);

enum dlock_bridge_kind {
  DLOCK_BRIDGE_MUTEX = 0,
  DLOCK_BRIDGE_FC = 1,
  DLOCK_BRIDGE_FC_PQ = 2,
  DLOCK_BRIDGE_USCL = 3,
  DLOCK_BRIDGE_CFL_LOCAL = 4,
  DLOCK_BRIDGE_SPINLOCK = 5,
  DLOCK_BRIDGE_MCS = 6,
  DLOCK_BRIDGE_TICKET = 7,
  DLOCK_BRIDGE_CLH = 8
};
enum dlock_bridge_status {
  DLOCK_BRIDGE_OK = 0,
  DLOCK_BRIDGE_INVALID = 1,
  DLOCK_BRIDGE_REENTRANT = 3
};

/* Profiling-only serialized elapsed opportunity, not CPU time or scheduler
 * virtual usage. Bridge modes time the callback; the native profile times
 * the original Env-protected body before unlock. Primary modes omit samples. */
typedef struct dlock_request_metrics {
  uint64_t service_tsc_ticks;
  uint64_t service_ns;
  uint32_t enabled;
  uint32_t reserved;
} dlock_request_metrics;

typedef struct dlock_thread_metrics {
  uint64_t executed_callbacks;
  uint64_t executed_service_tsc_ticks;
  uint64_t executed_service_ns;
  uint64_t combiner_pass_tsc_ticks;
  uint32_t enabled;
  uint32_t has_combiner_pass_ticks;
} dlock_thread_metrics;

typedef struct dlock_global_metrics {
  uint64_t accepted_calls;
  uint64_t completed_calls;
  uint64_t rejected_reentrant;
  uint64_t peak_inflight_calls;
  uint64_t active_calls;
  uint32_t enabled;
  uint32_t reserved;
} dlock_global_metrics;

/* Linux/x86_64 integration. Mutex mode borrows an initialized pthread_mutex_t*
 * obtained from the original Boost Env mutex's native_handle(). The caller
 * keeps that mutex alive until bridge destruction; Rust never destroys it.
 * native_mutex is ignored in every other mode. Conventional backends execute
 * the callback on the submitting physical worker while holding their own lock;
 * FC/FC-PQ may execute it on another worker. cfl_local is an adapted local
 * fairnumas port, NOT a verified faithful published CFL implementation.
 * CLH retains its raw queue-node allocations until process exit; the bridge
 * preserves that existing implementation rather than replacing its algorithm.
 * USCL uses explicit equal weight 1024 per worker, per-lock pthread TLS
 * registration, a 4,800,000-TSC-cycle nominal slice (fixed assumption
 * 2400 ticks/us), and pthread key deletion only after worker joins.
 * The caller exclusively owns the output pointer and must not publish the
 * resulting handle until initialization succeeds. */
int32_t dlock_bridge_create(uint32_t kind, void *native_mutex,
                           dlock_bridge **out_handle);

/* Synchronous submission from the actual requesting worker. Context and all
 * reachable input/output storage remain alive until return. The requester must
 * not access callback-mutated storage while pending. The callback can run on
 * another requesting worker only in FC/FC-PQ; all conventional lock
 * callbacks run on the requester. It retains no pointers and catches C++
 * exceptions itself. No exception, Rust unwind, longjmp or thread cancellation
 * crosses the ABI. Unexpected failures are fail-stop, not catch-and-reuse.
 *
 * All nested bridge submissions are rejected before publication-node access
 * (a conservative same-handle recursion/lock-cycle rule). Application/DB status
 * belongs in caller-owned context; this return value is ONLY bridge status.
 * Metrics is optional and, when supplied, must point to writable exclusive
 * storage until return. A successful return publishes callback output. */
int32_t dlock_bridge_execute(dlock_bridge *handle, dlock_bridge_callback callback,
                            void *context, dlock_request_metrics *metrics);

/* UNSAFE external lifetime contract: before destroying, the caller joins ALL
 * possible ABI callers, including thread/global metrics readers and USCL
 * pthread TLS-destructor workers. No ABI call may be concurrent with destroy
 * and no future use of the pointer is allowed. Destroy does not stop, drain,
 * count, or exclude concurrent/future submissions. Calling it from inside a
 * bridge execution/callback returns REENTRANT without freeing the handle.
 * The borrowed mutex remains initialized through successful destruction. */
int32_t dlock_bridge_destroy(dlock_bridge *handle);

/* Thread metrics describe the CALLING physical worker's callback execution
 * and combining passes, not requests submitted by that worker. Request metrics
 * above attribute service to the original requester. Every conventional lock
 * reports has_combiner_pass_ticks=0 (unavailable, not a measured zero). Global
 * counters and in-flight peak are profile-only instrumentation, not lifecycle
 * synchronization. Getters do not register TLS
 * workers or execute a database operation; the handle remains alive until
 * each call returns. */
int32_t dlock_bridge_get_thread_metrics(dlock_bridge *handle,
                                      dlock_thread_metrics *out_metrics);
int32_t dlock_bridge_get_global_metrics(dlock_bridge *handle,
                                      dlock_global_metrics *out_metrics);

#ifdef DLOCK_BRIDGE_TEST_HOOKS
/* Separate test build only: panic inside the actual protected delegate, to
 * verify process fail-stop. This function must never return successfully. */
int32_t dlock_bridge_test_panic(dlock_bridge *handle);
#endif

#ifdef __cplusplus
}
#endif
#endif
