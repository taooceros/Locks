// H2 synthetic handoff, NOT database throughput. This file is compiled as both
// C++ (driver) and, only for the observed companion, C (unchanged local USCL).
#include <stdint.h>
typedef struct h2_state {
  uint64_t slice, own_slice, banned_until, total_weight;
  int available, valid, banned;
} h2_state;

#ifndef __cplusplus
#include "fairlock.h"
// No lock algorithm or futex code is copied/modified. The C compiler includes
// the provenance-checked original implementation, in a separate executable.
void *h2_create(void) {
  fairlock_t *p = NULL;
  if (posix_memalign((void **)&p, 64, sizeof(*p))) return NULL;
  if (fairlock_bridge_init(p)) { free(p); return NULL; }
  return p;
}
void h2_acquire(void *p) {
  fairlock_thread_register(p, 1024);
  fairlock_acquire(p);
}
void h2_release(void *p) { fairlock_release(p); }
int h2_destroy(void *p) {
  int rc = fairlock_bridge_destroy(p);
  free(p);
  return rc;
}
h2_state h2_snapshot(void *p) {
  fairlock_t *lock = p;
  flthread_info_t *info = pthread_getspecific(lock->flthread_info_key);
  // ONLY called in the exclusive quiescent interval: A has returned from
  // release; B has not called acquire; both actors remain alive. Atomic gates
  // in the C++ driver establish happens-before. Never poll these nonatomics.
  h2_state s = {lock->slice, info ? info->slice : 0,
                info ? info->banned_until : 0,
                __atomic_load_n(&lock->total_weight, __ATOMIC_RELAXED),
                1, lock->slice_valid, info ? info->banned : 0};
  return s;
}
#else
#include "bridge.h"
#include <algorithm>
#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <linux/mempolicy.h>
#include <pthread.h>
#include <sched.h>
#include <sys/syscall.h>
#include <thread>
#include <time.h>
#include <unistd.h>
#include <x86intrin.h>

extern "C" {
void *h2_create(void);
void h2_acquire(void *);
void h2_release(void *);
int h2_destroy(void *);
h2_state h2_snapshot(void *);
}

static uint64_t ns() {
  timespec t{};
  if (clock_gettime(CLOCK_MONOTONIC, &t)) std::_Exit(90);
  return uint64_t(t.tv_sec) * 1000000000ULL + t.tv_nsec;
}
static uint64_t ticks() { _mm_lfence(); auto t = __rdtsc(); _mm_lfence(); return t; }
[[noreturn]] static void fail(const char *what) {
  std::fprintf(stderr, "H2 error: %s (errno=%d)\n", what, errno);
  std::fflush(stderr);
  std::_Exit(2); // fail-stop, never destroy a handle still used by a worker
}
static void check(bool ok, const char *what) { if (!ok) fail(what); }
static uint64_t deadline;
static void bounded() { check(ns() < deadline, "internal deadline"); }
static int pin(int cpu) {
  cpu_set_t mask, actual;
  CPU_ZERO(&mask); CPU_SET(cpu, &mask);
  check(pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask) == 0, "pin");
  CPU_ZERO(&actual);
  check(pthread_getaffinity_np(pthread_self(), sizeof(actual), &actual) == 0,
        "get affinity");
  check(CPU_COUNT(&actual) == 1 && CPU_ISSET(cpu, &actual), "affinity mismatch");
  check(sched_getcpu() == cpu, "actual CPU mismatch");
  return cpu;
}
static void sleep_ns(uint64_t duration) {
  timespec req{time_t(duration / 1000000000), long(duration % 1000000000)};
  while (nanosleep(&req, &req)) { check(errno == EINTR, "nanosleep"); bounded(); }
}
static void wait_for(std::atomic<int>& gate, int value, bool sleepy) {
  while (gate.load(std::memory_order_acquire) < value) {
    bounded();
    if (sleepy) sleep_ns(50000); else _mm_pause();
  }
}

struct Lock {
#ifdef H2_DIRECT
  void *handle = nullptr;
  explicit Lock(int) { handle = h2_create(); check(handle, "USCL create"); }
  h2_state snapshot() { return h2_snapshot(handle); }
#else
  dlock_bridge *handle = nullptr;
  pthread_mutex_t mutex{};
  explicit Lock(int kind) {
    check(pthread_mutex_init(&mutex, nullptr) == 0, "mutex init");
    check(dlock_bridge_create(kind, &mutex, &handle) == 0 && handle, "bridge create");
  }
  h2_state snapshot() { return {}; } // opaque bridge: NEVER guess private layout
#endif
  void execute(dlock_bridge_callback callback, void *context) {
#ifdef H2_DIRECT
    h2_acquire(handle); callback(context); h2_release(handle);
#else
    check(dlock_bridge_execute(handle, callback, context, nullptr) == 0, "bridge execute");
#endif
  }
  void destroy() { // explicit, called only after both pthread TLS destructors/join
#ifdef H2_DIRECT
    check(h2_destroy(handle) == 0, "USCL destroy");
#else
    check(dlock_bridge_destroy(handle) == 0, "bridge destroy");
    check(pthread_mutex_destroy(&mutex) == 0, "mutex destroy");
#endif
  }
};
struct Oracle {
  std::atomic<unsigned> active{0};
  std::atomic<uint64_t> calls{0}, sum{0};
};
struct Call {
  Oracle *oracle;
  uint64_t token, request_ns = 0, request_tsc = 0, entry_ns = 0, entry_tsc = 0;
  uint64_t exit_ns = 0, returned_ns = 0, result = 0;
  unsigned invocations = 0;
  int executor_cpu = -1;
};
static void callback(void *p) noexcept {
  auto& c = *static_cast<Call *>(p);
  check(c.oracle->active.fetch_add(1) == 0, "overlapping callbacks");
  c.entry_tsc = ticks(); c.entry_ns = ns();
  ++c.invocations;
  c.executor_cpu = sched_getcpu();
  c.result = c.token ^ 0x5a5a5a5aULL;
  c.oracle->sum.fetch_add(c.token);
  c.oracle->calls.fetch_add(1);
  c.exit_ns = ns();
  check(c.oracle->active.fetch_sub(1) == 1, "callback active count");
}
static Call call(Lock& lock, Oracle& oracle, uint64_t token) {
  Call c{&oracle, token};
  c.request_ns = ns(); c.request_tsc = ticks();
  lock.execute(callback, &c);
  c.returned_ns = ns();
  check(c.invocations == 1 && c.result == (token ^ 0x5a5a5a5aULL), "callback result/count");
  check(c.entry_ns >= c.request_ns && c.exit_ns >= c.entry_ns &&
        c.returned_ns >= c.exit_ns, "timestamp order");
  return c;
}
static void print_call(const Call& c, const char *prefix) {
  std::printf("\"%s_request_ns\":%llu,\"%s_request_tsc\":%llu,"
              "\"%s_entry_ns\":%llu,\"%s_entry_tsc\":%llu,"
              "\"%s_callback_end_marker_ns\":%llu,\"%s_return_ns\":%llu,"
              "\"%s_executor_cpu\":%d,\"%s_result\":%llu",
              prefix, (unsigned long long)c.request_ns, prefix, (unsigned long long)c.request_tsc,
              prefix, (unsigned long long)c.entry_ns, prefix, (unsigned long long)c.entry_tsc,
              prefix, (unsigned long long)c.exit_ns, prefix, (unsigned long long)c.returned_ns,
              prefix, c.executor_cpu, prefix, (unsigned long long)c.result);
}
static void print_state(const h2_state& s, const char *prefix) {
  std::printf(",\"%s_observed\":%d,\"%s_slice_valid\":%d,\"%s_slice_tsc\":%llu,"
              "\"%s_own_slice_tsc\":%llu,\"%s_banned_until_tsc\":%llu,"
              "\"%s_banned\":%d,\"%s_total_weight\":%llu",
              prefix, s.available, prefix, s.valid, prefix, (unsigned long long)s.slice,
              prefix, (unsigned long long)s.own_slice, prefix, (unsigned long long)s.banned_until,
              prefix, s.banned, prefix, (unsigned long long)s.total_weight);
}
int main(int argc, char **argv) {
  check(argc == 4, "usage: BINARY backend outside_us samples");
  int kind = -1;
  const char *names[] = {"bridge_mutex", "fc", "fc_pq", "uscl"};
  for (int i = 0; i < 4; ++i) if (!std::strcmp(argv[1], names[i])) kind = i;
#ifdef H2_DIRECT
  check(!std::strcmp(argv[1], "uscl_local_observed"), "direct backend label");
#else
  check(kind >= 0, "backend label");
#endif
  int pause_us = std::atoi(argv[2]), samples = std::atoi(argv[3]);
  check(pause_us == 0 || pause_us == 50 || pause_us == 500 || pause_us == 5000, "pause");
  check(samples > 0 && samples <= 32, "samples must be 1..32");
  alarm(10); // secondary kill guard even if an inherited lock wait never returns
  deadline = ns() + 8000000000ULL;
  int setup_cpu = pin(16);
  unsigned long node_mask = 1, observed_mask = 0;
  int policy = -1;
  check(syscall(SYS_set_mempolicy, MPOL_BIND, &node_mask, 64) == 0, "bind memory node0");
  check(syscall(SYS_get_mempolicy, &policy, &observed_mask, 64, nullptr, 0) == 0 &&
        policy == MPOL_BIND && observed_mask == 1, "verify memory policy");
  std::printf("{\"type\":\"setup\",\"backend\":\"%s\",\"setup_cpu\":%d,"
              "\"memory_policy\":\"bind\",\"memory_node_mask\":%lu,\"samples\":%d,"
              "\"pause_us\":%d,\"prime_requested_ns\":20000000,\"alarm_s\":10}\n",
              argv[1], setup_cpu, observed_mask, samples, pause_us);
  for (int i = 0; i < 3; ++i) {
    auto n0 = ns(), t0 = ticks(); sleep_ns(20000000); auto t1 = ticks(), n1 = ns();
    std::printf("{\"type\":\"calibration\",\"index\":%d,\"elapsed_ns\":%llu,"
                "\"tsc_ticks\":%llu}\n", i, (unsigned long long)(n1-n0),
                (unsigned long long)(t1-t0));
  }
  uint64_t overhead_sum = 0, overhead_min = UINT64_MAX, overhead_max = 0;
  for (int i = 0; i < 1000; ++i) {
    auto begin = ns(), end = ns(), delta = end - begin;
    overhead_sum += delta; overhead_min = std::min(overhead_min, delta);
    overhead_max = std::max(overhead_max, delta);
  }
  std::printf("{\"type\":\"clock_overhead\",\"pairs\":1000,\"sum_ns\":%llu,"
              "\"min_ns\":%llu,\"max_ns\":%llu}\n", (unsigned long long)overhead_sum,
              (unsigned long long)overhead_min, (unsigned long long)overhead_max);
  std::fflush(stdout);
  Lock lock(kind);
  Oracle oracle;
  std::atomic<int> a_ready{0}, b_done{0};
  Call a{&oracle, 0};
  h2_state a_state{};
  uint64_t prime_start = 0, prime_end = 0;
  int a_cpu = -1, b_cpu = -1;
  std::thread actor_a([&] {
    a_cpu = pin(16);
    a = call(lock, oracle, 1);
    a_ready.store(1, std::memory_order_release);
    wait_for(b_done, 1, true);
    for (int i = 0; i < samples; ++i) {
      // Both actors have already registered. Give BOTH equal-weight workers
      // elapsed-time credit; fixed sleep, no retry-until-valid selection.
      prime_start = ns(); sleep_ns(20000000); prime_end = ns();
      a = call(lock, oracle, uint64_t(3 + 2*i));
      a_state = lock.snapshot();
      check(oracle.active.load() == 0, "A callback still active after return");
      a_ready.store(i + 2, std::memory_order_release);
      // A stays alive OUTSIDE the lock until B has returned. No further API
      // access (including snapshots/TLS cleanup) during B's acquisition.
      wait_for(b_done, i + 2, true);
    }
  });
  std::thread actor_b([&] {
    b_cpu = pin(17);
    wait_for(a_ready, 1, false);
    auto first = call(lock, oracle, 2);
    std::printf("{\"type\":\"first_use\","); print_call(a, "a");
    std::printf(","); print_call(first, "b"); std::printf("}\n");
    b_done.store(1, std::memory_order_release);
    for (int i = 0; i < samples; ++i) {
      wait_for(a_ready, i + 2, false);
      auto notice_ns = ns();
      uint64_t loops = 0;
      const auto target = a.returned_ns + uint64_t(pause_us) * 1000;
      while (ns() < target) { ++loops; _mm_pause(); bounded(); }
      // A's release happens-before this read; A is waiting only on b_done.
      // B has not entered USCL yet. No other actor can touch lock fields.
      auto b_state = lock.snapshot();
      auto b = call(lock, oracle, uint64_t(4 + 2*i));
      check(b.request_ns >= a.returned_ns, "request precedes completed release");
      std::printf("{\"type\":\"sample\",\"index\":%d,\"notice_ns\":%llu,"
                  "\"prime_start_ns\":%llu,\"prime_end_ns\":%llu,\"pause_loops\":%llu,",
                  i, (unsigned long long)notice_ns, (unsigned long long)prime_start,
                  (unsigned long long)prime_end, (unsigned long long)loops);
      print_call(a, "a"); std::printf(","); print_call(b, "b");
      print_state(a_state, "a"); print_state(b_state, "b");
      std::printf("}\n"); std::fflush(stdout);
      b_done.store(i + 2, std::memory_order_release);
    }
  });
  actor_a.join(); actor_b.join(); // includes local USCL pthread TLS destructors
  uint64_t expected = 2 + 2 * uint64_t(samples);
  check(oracle.active.load() == 0 && oracle.calls.load() == expected &&
        oracle.sum.load() == expected*(expected+1)/2, "final callback oracle");
  lock.destroy();
  std::printf("{\"type\":\"complete\",\"status\":\"ok\",\"calls\":%llu,"
              "\"token_sum\":%llu,\"actor_cpus\":[%d,%d],\"joined_before_destroy\":true}\n",
              (unsigned long long)oracle.calls.load(), (unsigned long long)oracle.sum.load(),
              a_cpu, b_cpu);
  return 0;
}
#endif
