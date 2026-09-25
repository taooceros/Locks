// Synthetic cost-asymmetry boundary, not database throughput. Frozen bridge only.
#include "bridge.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cinttypes>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <linux/mempolicy.h>
#include <new>
#include <pthread.h>
#include <sched.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <thread>
#include <time.h>
#include <unistd.h>
#include <x86intrin.h>

namespace {
constexpr unsigned max_workers = 8, bins = 64, progress_bins = 20;
constexpr uint64_t salt = 0x9e3779b97f4a7c15ULL;
constexpr uint64_t canary = 0x6789abcdef012345ULL;
constexpr unsigned smoke_calls = 32;
[[noreturn]] void fail(const char *message) {
  std::fprintf(stderr, "cost-boundary: %s (errno=%d)\n", message, errno);
  std::fflush(stderr);
  std::_Exit(2); // Fail-stop: never destroy while any requester may still run.
}
void check(bool okay, const char *message) { if (!okay) fail(message); }
uint64_t clock_ns(clockid_t id = CLOCK_MONOTONIC) {
  timespec t{};
  check(clock_gettime(id, &t) == 0, "clock_gettime");
  return uint64_t(t.tv_sec) * 1000000000ULL + uint64_t(t.tv_nsec);
}
void pin(unsigned cpu) {
  cpu_set_t received, mask, actual;
  CPU_ZERO(&received);
  check(pthread_getaffinity_np(pthread_self(), sizeof(received), &received) == 0,
        "received affinity");
  check(CPU_ISSET(cpu, &received), "requested CPU outside inherited affinity");
  CPU_ZERO(&mask); CPU_SET(cpu, &mask);
  check(pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask) == 0, "pin worker");
  CPU_ZERO(&actual);
  check(pthread_getaffinity_np(pthread_self(), sizeof(actual), &actual) == 0 &&
        CPU_COUNT(&actual) == 1 && CPU_ISSET(cpu, &actual) && sched_getcpu() == int(cpu),
        "actual worker affinity mismatch");
}
void memory_policy() {
  unsigned long observed = 0;
  int policy = -1;
  check(syscall(SYS_get_mempolicy, &policy, &observed, 64, nullptr, 0) == 0 &&
        policy == MPOL_BIND && observed == 1, "memory policy must bind node0");
}
int page_node(void *address) {
  int node = -1;
  check(syscall(SYS_get_mempolicy, &node, nullptr, 0, address,
                MPOL_F_NODE | MPOL_F_ADDR) == 0, "observe protected page NUMA node");
  return node;
}
struct State {
  std::array<uint64_t, 64> words{};
  std::array<uint64_t, max_workers> calls{}, token_sums{}, result_sums{};
  uint64_t total = 0;
  std::atomic<unsigned> active{0}; // ONLY touched by the separate smoke callback.
};
static_assert(sizeof(State) <= 4096, "fixed protected state must fit one page");

// Dependent integer/memory work: every index depends on the previous result.
// The shared lookup table is immutable, while protected counters/checksums are
// mutated on every call. No data-dependent allocation, sleep, or service clock.
__attribute__((noinline)) uint64_t compute(const uint64_t *words, unsigned id,
                                          unsigned iterations) noexcept {
  uint64_t value = salt ^ (uint64_t(id + 1) * 0xd6e8feb86659fd93ULL);
  for (unsigned i = 0; i < iterations; ++i) {
    value ^= words[(value ^ (value >> 29)) & 63];
    value = (value << 13) | (value >> 51);
    value = value * 0xd1342543de82ef95ULL + uint64_t(i) + salt;
  }
  return value;
}
uint64_t reference(const State &state, unsigned id, unsigned iterations) {
  uint64_t value = salt ^ (uint64_t(id + 1) * 0xd6e8feb86659fd93ULL);
  for (unsigned i = 0; i != iterations; ++i) {
    unsigned slot = unsigned((value ^ (value >> 29)) % 64);
    uint64_t mixed = value ^ state.words[slot];
    uint64_t rotated = (mixed << 13) | (mixed >> (64 - 13));
    value = rotated * 0xd1342543de82ef95ULL + i + salt;
  }
  return value;
}
struct Call {
  uint64_t head = canary;
  State *state;
  unsigned id, iterations;
  uint64_t sequence;
  bool injected_error;
  uint64_t result = 0;
  unsigned invocations = 0;
  int status = -1;
  uint64_t tail = canary;
};
template<bool oracle> void callback(void *raw) noexcept {
  auto &c = *static_cast<Call *>(raw);
  auto &s = *c.state;
  if constexpr (oracle) {
    check(s.active.fetch_add(1, std::memory_order_acq_rel) == 0,
          "overlapping protected callbacks");
  }
  check(c.head == canary && c.tail == canary && c.id < max_workers,
        "callback input canaries/identity");
  check(c.sequence == s.calls[c.id] + 1, "duplicate/missing requester sequence");
  ++c.invocations;
  c.result = compute(s.words.data(), c.id, c.iterations);
  c.status = c.injected_error ? 17 : 0;
  ++s.calls[c.id]; ++s.total;
  s.token_sums[c.id] += c.sequence;
  s.result_sums[c.id] += c.result;
  if constexpr (oracle)
    check(s.active.fetch_sub(1, std::memory_order_release) == 1, "active oracle");
}
struct Histogram {
  std::array<uint64_t, bins> counts{};
  uint64_t maximum = 0, sum = 0;
  void add(uint64_t elapsed) {
    unsigned index = elapsed <= 1 ? 0 : 64 - unsigned(__builtin_clzll(elapsed - 1));
    check(index < bins, "response latency overflow");
    ++counts[index]; sum += elapsed; maximum = std::max(maximum, elapsed);
  }
};
struct alignas(64) Worker {
  unsigned id = 0, cpu = 0, iterations = 0;
  int actual_start = -1, actual_end = -1;
  uint64_t expected = 0, calls = 0, before = 0, drain = 0, errors = 0;
  uint64_t service_before = 0, service_drain = 0, ticks_before = 0, ticks_drain = 0;
  uint64_t started = 0, finished = 0, first_return = 0, last_return = 0;
  uint64_t max_completion_gap = 0, cpu_ns = 0;
  std::array<uint64_t, progress_bins> progress{};
  Histogram latency, drain_latency;
  dlock_thread_metrics executor{};
};
struct Gate {
  std::atomic<unsigned> ready{0}, finished{0};
  std::atomic<bool> go{false};
  uint64_t start = 0, deadline = 0;
};
void worker(dlock_bridge *lock, State &state, Worker &w, Gate &gate, bool smoke,
            uint64_t duration) noexcept {
  pin(w.cpu); memory_policy();
  w.actual_start = sched_getcpu();
  gate.ready.fetch_add(1, std::memory_order_release);
  while (!gate.go.load(std::memory_order_acquire)) _mm_pause();
  w.started = clock_ns();
  uint64_t cpu_start = clock_ns(CLOCK_THREAD_CPUTIME_ID);
  uint64_t previous_return = gate.start;
  auto cb = smoke ? callback<true> : callback<false>;
  for (;;) {
    uint64_t request = clock_ns();
    if (smoke ? w.calls == smoke_calls : request >= gate.deadline) break;
    Call c{canary, &state, w.id, w.iterations, w.calls + 1,
           smoke && (w.calls + 1) % 11 == 0};
#ifdef UPS_BRIDGE_PROFILE
    dlock_request_metrics metrics{};
    int status = dlock_bridge_execute(lock, cb, &c, &metrics);
#else
    int status = dlock_bridge_execute(lock, cb, &c, nullptr);
#endif
    uint64_t returned = clock_ns();
    // No extra publication flag: acquire/release in the ACTUAL synchronous
    // bridge publishes this requester-owned stack context before return.
    check(status == DLOCK_BRIDGE_OK, "bridge execute status");
    check(c.head == canary && c.tail == canary && c.invocations == 1 &&
          c.result == w.expected && c.status == (c.injected_error ? 17 : 0),
          "published callback result/status/count/canaries");
    ++w.calls;
    w.errors += c.injected_error;
    if (!w.first_return) w.first_return = returned;
    w.last_return = returned;
    w.max_completion_gap = std::max(w.max_completion_gap, returned - previous_return);
    previous_return = returned;
    bool on_time = smoke || returned <= gate.deadline;
    if (on_time) {
      ++w.before; w.latency.add(returned - request);
      if (!smoke) {
        unsigned bin = unsigned((returned - gate.start) * progress_bins / duration);
        ++w.progress[std::min(bin, progress_bins - 1)];
      }
    } else {
      ++w.drain; w.drain_latency.add(returned - request);
    }
#ifdef UPS_BRIDGE_PROFILE
    check(metrics.enabled == 1, "profile archive did not return service metrics");
    (on_time ? w.service_before : w.service_drain) += metrics.service_ns;
    (on_time ? w.ticks_before : w.ticks_drain) += metrics.service_tsc_ticks;
#endif
  }
  w.finished = clock_ns();
  w.cpu_ns = clock_ns(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
  if (!smoke && previous_return < gate.deadline)
    w.max_completion_gap = std::max(w.max_completion_gap, gate.deadline - previous_return);
  check(dlock_bridge_get_thread_metrics(lock, &w.executor) == DLOCK_BRIDGE_OK,
        "executor metrics status");
#ifdef UPS_BRIDGE_PROFILE
  check(w.executor.enabled == 1, "executor profiling disabled");
#else
  check(w.executor.enabled == 0, "primary unexpectedly instrumented");
#endif
  w.actual_end = sched_getcpu();
  check(w.actual_end == int(w.cpu), "worker migrated outside pin");
  gate.finished.fetch_add(1, std::memory_order_release);
}
void array_json(const uint64_t *values, unsigned count) {
  std::printf("[");
  for (unsigned i = 0; i < count; ++i) std::printf("%s%" PRIu64, i ? "," : "", values[i]);
  std::printf("]");
}
void histogram_json(const Histogram &h) {
  std::printf("{\"upper_bound_log2_counts\":"); array_json(h.counts.data(), bins);
  std::printf(",\"sum_ns\":%" PRIu64 ",\"max_ns\":%" PRIu64 "}", h.sum, h.maximum);
}
} // namespace

int main(int argc, char **argv) {
  check(argc == 5, "usage: BINARY backend case smoke|timed duration_ms");
  const char *names[] = {"bridge_mutex", "fc", "fc_pq", "uscl"};
  int kind = -1;
  for (int i = 0; i < 4; ++i) if (!std::strcmp(argv[1], names[i])) kind = i;
  check(kind >= 0 && kind == UPS_BRIDGE_KIND, "binary/backend mismatch");
  unsigned count = 8, cheap = 8;
  if (!std::strcmp(argv[2], "one_equal")) count = cheap = 1;
  else if (!std::strcmp(argv[2], "eight_equal")) {}
  else if (!std::strcmp(argv[2], "four_four")) cheap = 4;
  else if (!std::strcmp(argv[2], "seven_one")) cheap = 7;
  else fail("unknown case");
  bool smoke = !std::strcmp(argv[3], "smoke");
  check(smoke || !std::strcmp(argv[3], "timed"), "mode");
  char *end = nullptr;
  unsigned long ms = std::strtoul(argv[4], &end, 10);
  check(end && !*end && (ms == 1000 || ms == 2000), "duration must be 1000 or 2000 ms");
#ifdef UPS_BRIDGE_PROFILE
  constexpr bool profile = true;
#else
  constexpr bool profile = false;
#endif
  check(smoke || ms == (profile ? 1000UL : 2000UL), "primary/profile duration");
  alarm(12); // Process timeout never invokes unsafe concurrent destruction.
  rlimit limit{32ULL << 30, 32ULL << 30};
  check(setrlimit(RLIMIT_AS, &limit) == 0, "address-space cap");
  cpu_set_t inherited;
  CPU_ZERO(&inherited);
  check(pthread_getaffinity_np(pthread_self(), sizeof(inherited), &inherited) == 0,
        "inherited affinity");
  check(CPU_COUNT(&inherited) == 8, "expected exactly CPUs20-27");
  for (unsigned cpu = 20; cpu < 28; ++cpu)
    check(CPU_ISSET(cpu, &inherited), "slot affinity mismatch");
  unsigned long node_mask = 1;
  check(syscall(SYS_set_mempolicy, MPOL_BIND, &node_mask, 64) == 0, "bind node0");
  memory_policy();
  int setup_cpu_begin = sched_getcpu();
  long page_size = sysconf(_SC_PAGESIZE);
  check(page_size >= long(sizeof(State)), "protected page size");
  void *allocation = mmap(nullptr, size_t(page_size), PROT_READ | PROT_WRITE,
                          MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  check(allocation != MAP_FAILED, "protected state mmap");
  auto &state = *new (allocation) State{};
  for (unsigned i = 0; i < state.words.size(); ++i)
    state.words[i] = salt * (i + 1) ^ (uint64_t(i) << 37);
  int initial_node = page_node(allocation);
  check(initial_node == 0, "protected page not on node0");
  int setup_cpu_end = sched_getcpu();
  pthread_mutex_t mutex{};
  check(pthread_mutex_init(&mutex, nullptr) == 0, "borrowed mutex init");
  dlock_bridge *lock = nullptr;
  check(dlock_bridge_create(unsigned(kind), &mutex, &lock) == DLOCK_BRIDGE_OK && lock,
        "bridge create");
  std::array<Worker, max_workers> workers{};
  std::array<std::thread, max_workers> threads;
  Gate gate;
  uint64_t duration = uint64_t(ms) * 1000000ULL;
  std::printf("{\"type\":\"setup\",\"backend\":\"%s\",\"case\":\"%s\","
              "\"profile\":%s,\"smoke\":%s,\"workers\":%u,\"cheap_workers\":%u,"
              "\"inherited_affinity\":[20,21,22,23,24,25,26,27],\"memory_policy\":\"bind\","
              "\"setup_cpu_begin\":%d,\"setup_cpu_end\":%d,\"coordinator_gate_cpu\":20,"
              "\"memory_node_mask\":1,\"protected_page_initial_node\":%d,"
              "\"state_bytes\":%zu,\"state_allocation_bytes\":%ld,\"cheap_iterations\":64,"
              "\"expensive_iterations\":1024,\"address_space_limit_bytes\":34359738368,"
              "\"alarm_s\":12,\"nonoverlap_guard\":%s}\n",
              argv[1], argv[2], profile ? "true" : "false", smoke ? "true" : "false",
              count, cheap, setup_cpu_begin, setup_cpu_end, initial_node, sizeof(State), page_size,
              smoke ? "true" : "false");
  std::fflush(stdout);
  for (unsigned i = 0; i < count; ++i) {
    auto &w = workers[i]; w.id = i; w.cpu = 20 + i; w.iterations = i < cheap ? 64 : 1024;
    w.expected = reference(state, i, w.iterations);
    threads[i] = std::thread([&, i] { worker(lock, state, workers[i], gate, smoke, duration); });
  }
  while (gate.ready.load(std::memory_order_acquire) != count) _mm_pause();
  // Workers inherited the complete slot BEFORE the coordinator narrows itself.
  pin(20);
  uint64_t process_cpu = clock_ns(CLOCK_PROCESS_CPUTIME_ID);
  gate.start = clock_ns(); gate.deadline = gate.start + duration;
  gate.go.store(true, std::memory_order_release);
  for (unsigned i = 0; i < count; ++i) threads[i].join(); // Includes USCL TLS destructors.
  uint64_t joined = clock_ns();
  process_cpu = clock_ns(CLOCK_PROCESS_CPUTIME_ID) - process_cpu;
  check(gate.finished.load(std::memory_order_acquire) == count && state.active.load() == 0,
        "joined/active oracle");
  uint64_t calls = 0, before = 0, drain = 0, service = 0, executed = 0, executor_service = 0;
  uint64_t latest = 0;
  for (unsigned i = 0; i < max_workers; ++i) {
    auto &w = workers[i];
    check(state.calls[i] == w.calls && state.token_sums[i] == w.calls * (w.calls + 1) / 2 &&
          state.result_sums[i] == w.expected * w.calls, "final count/token/result checksum oracle");
    if (i >= count) continue;
    check(w.calls == w.before + w.drain && (smoke ? w.calls == smoke_calls : w.drain <= 1),
          "deadline/drain count oracle");
    check(w.errors == (smoke ? smoke_calls / 11 : 0), "application error oracle");
    calls += w.calls; before += w.before; drain += w.drain;
    service += w.service_before + w.service_drain;
    executed += w.executor.executed_callbacks;
    executor_service += w.executor.executed_service_ns;
    latest = std::max(latest, w.finished);
  }
  check(state.total == calls, "global callback count");
  for (unsigned i = 0; i < state.words.size(); ++i)
    check(state.words[i] == (salt * (i + 1) ^ (uint64_t(i) << 37)), "shared lookup-table integrity");
  dlock_global_metrics global{};
  check(dlock_bridge_get_global_metrics(lock, &global) == DLOCK_BRIDGE_OK, "global metrics status");
  if (profile)
    check(global.enabled == 1 && global.accepted_calls == calls && global.completed_calls == calls &&
          global.active_calls == 0 && global.rejected_reentrant == 0 && executed == calls &&
          executor_service == service, "profile global/requester/executor conservation");
  else check(global.enabled == 0, "unexpected primary counters");
  int final_node = page_node(allocation);
  check(final_node == 0, "protected page migrated from node0");
  check(dlock_bridge_destroy(lock) == DLOCK_BRIDGE_OK, "exclusive destroy after joins");
  check(pthread_mutex_destroy(&mutex) == 0, "borrowed mutex destroy after bridge");
  state.~State();
  check(munmap(allocation, size_t(page_size)) == 0, "protected state munmap");
  for (unsigned i = 0; i < count; ++i) {
    const auto &w = workers[i];
    std::printf("{\"type\":\"worker\",\"id\":%u,\"role\":\"%s\",\"iterations\":%u,"
                "\"actual_cpu_start\":%d,\"actual_cpu_end\":%d,\"actual_affinity\":[%u],"
                "\"calls\":%" PRIu64 ",\"before_deadline\":%" PRIu64 ",\"drain\":%" PRIu64 ","
                "\"application_errors\":%" PRIu64 ",\"expected_result\":%" PRIu64 ","
                "\"cpu_ns_active_including_drain\":%" PRIu64 ",\"started_ns\":%" PRIu64 ","
                "\"finished_ns\":%" PRIu64 ",\"first_return_ns\":%" PRIu64 ",\"last_return_ns\":%" PRIu64 ","
                "\"max_completion_gap_ns_including_drain\":%" PRIu64 ","
                "\"service_before_deadline_ns\":%" PRIu64 ",\"service_drain_ns\":%" PRIu64 ","
                "\"service_before_deadline_tsc_ticks\":%" PRIu64 ",\"service_drain_tsc_ticks\":%" PRIu64 ","
                "\"executor_callbacks\":%" PRIu64 ",\"executor_service_ns\":%" PRIu64 ","
                "\"has_combiner_pass_ticks\":%s,\"combiner_pass_ticks\":%" PRIu64 ",\"progress_bins\":",
                i, i < cheap ? "cheap" : "expensive", w.iterations, w.actual_start, w.actual_end, w.cpu,
                w.calls, w.before, w.drain, w.errors, w.expected, w.cpu_ns, w.started, w.finished,
                w.first_return, w.last_return, w.max_completion_gap, w.service_before, w.service_drain,
                w.ticks_before, w.ticks_drain, w.executor.executed_callbacks, w.executor.executed_service_ns,
                w.executor.has_combiner_pass_ticks ? "true" : "false", w.executor.combiner_pass_tsc_ticks);
    array_json(w.progress.data(), progress_bins);
    std::printf(",\"response_latency_before_deadline\":"); histogram_json(w.latency);
    std::printf(",\"response_latency_drain\":"); histogram_json(w.drain_latency);
    std::printf("}\n");
  }
  std::printf("{\"type\":\"complete\",\"status\":\"ok\",\"calls\":%" PRIu64 ","
              "\"before_deadline\":%" PRIu64 ",\"drain\":%" PRIu64 ",\"start_ns\":%" PRIu64 ","
              "\"deadline_ns\":%" PRIu64 ",\"last_worker_finished_ns\":%" PRIu64 ",\"joined_ns\":%" PRIu64 ","
              "\"window_ns\":%" PRIu64 ",\"drain_wall_ns\":%" PRIu64 ","
              "\"process_cpu_ns_gate_to_join\":%" PRIu64 ",\"protected_page_final_node\":%d,"
              "\"joined_before_destroy\":true,\"oracle\":\"result/status/canary/sequence/count/checksum\","
              "\"profile_conservation_checked\":%s}\n",
              calls, before, drain, gate.start, gate.deadline, latest, joined, duration,
              latest > gate.deadline ? latest - gate.deadline : 0, process_cpu, final_node,
              profile ? "true" : "false");
}
