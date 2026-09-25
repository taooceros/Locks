#ifndef LOCKS_UPSCALEDB_PRIVATE_OPS_H
#define LOCKS_UPSCALEDB_PRIVATE_OPS_H
#include <ups/upscaledb.h>
#include "bridge.h"

// Internal integration symbols, NOT UpScaleDB public APIs. The only supported
// caller is the private Store owning the Env and DB for their entire lifetime.
// submit synchronously invokes callback, possibly on another requesting worker.
typedef int32_t (*ups_dlock_submit)(dlock_bridge *, dlock_bridge_callback,
                                     void *, dlock_request_metrics *);
extern "C" {
ups_status_t ups_dlock_private_find(ups_db_t *, ups_key_t *, ups_record_t *,
                                    dlock_bridge *, ups_dlock_submit,
                                    dlock_request_metrics *, int32_t *);
ups_status_t ups_dlock_private_insert(ups_db_t *, ups_key_t *, ups_record_t *,
                                      dlock_bridge *, ups_dlock_submit,
                                      dlock_request_metrics *, int32_t *);
void *ups_dlock_private_native_mutex(ups_db_t *);
// Exactly eight real nontransactional inserts per one Env-mutex acquisition
// or bridge submission. Per-record statuses remain caller-owned until return;
// partial success is not rolled back on an error.
ups_status_t ups_dlock_private_batch(ups_db_t *, ups_key_t *, ups_record_t *,
                                    ups_status_t *, dlock_bridge *, ups_dlock_submit,
                                    dlock_request_metrics *, int32_t *);
ups_status_t ups_dlock_private_native_batch(ups_db_t *, ups_key_t *, ups_record_t *,
                                           ups_status_t *, dlock_request_metrics *);
ups_status_t ups_dlock_private_native_find(ups_db_t *, ups_key_t *, ups_record_t *,
                                          dlock_request_metrics *);
ups_status_t ups_dlock_private_native_insert(ups_db_t *, ups_key_t *, ups_record_t *,
                                            dlock_request_metrics *);
#ifdef DLOCK_BRIDGE_TEST_HOOKS
// Test-only: exercises the actual production callback catch-all.
int32_t ups_dlock_private_test_throw(dlock_bridge *, ups_dlock_submit);
ups_status_t ups_dlock_private_test_exception(dlock_bridge *, ups_dlock_submit);
void ups_dlock_private_test_pause(int);
int ups_dlock_private_test_entered();
#endif
}
#endif
