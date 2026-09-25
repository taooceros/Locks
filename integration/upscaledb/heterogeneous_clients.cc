// Eight physical synchronous requesters. A batch is one nontransactional
// scheduling unit containing eight actual UpScaleDB insert bodies.
#include <stddef.h>
#include <ups/upscaledb.h>
#include <ups/upscaledb_int.h>
#include "bridge.h"
#include "private_ops.h"
#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <mutex>
#include <pthread.h>
#include <sys/resource.h>
#include <sched.h>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <time.h>
#include <vector>

namespace {
constexpr unsigned workers = 8, cohort_size = 4, batch_size = 8, windows = 8;
constexpr uint64_t high_bit = 1ULL << 63, mask = high_bit - 1;
constexpr uint64_t multiplier = 0x5851f42d4c957f2dULL;
uint64_t ns(clockid_t clock = CLOCK_MONOTONIC) {
  timespec t{};
  if (clock_gettime(clock, &t)) throw std::runtime_error("clock_gettime");
  return uint64_t(t.tv_sec) * 1000000000ULL + t.tv_nsec;
}
uint64_t mix(uint64_t x) {
  x += 0x9e3779b97f4a7c15ULL;
  x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
  x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
  return x ^ (x >> 31);
}
uint64_t value_of(uint64_t key, uint64_t seed) { return mix(key ^ seed ^ 0xa531ea3dd5bd7121ULL); }
uint64_t insert_key(uint64_t ordinal, uint64_t seed) {
  return high_bit | ((ordinal * multiplier + (mix(seed) & mask)) & mask);
}
uint64_t inverse() {
  uint64_t n = 1;
  for (int i = 0; i < 6; ++i) n *= 2 - multiplier * n;
  return n;
}
uint64_t inserted_ordinal(uint64_t key, uint64_t seed) {
  return (((key & mask) - (mix(seed) & mask)) * inverse()) & mask;
}
uint64_t positive(const char *text, const char *label) {
  if (!*text) throw std::runtime_error(std::string("missing ") + label);
  for (const char *p = text; *p; ++p)
    if (*p < '0' || *p > '9') throw std::runtime_error(std::string("invalid ") + label);
  errno = 0;
  char *end = nullptr;
  auto result = std::strtoull(text, &end, 10);
  if (errno || *end) throw std::runtime_error(std::string("invalid ") + label);
  return result;
}
void check(int status, const char *operation) {
  if (status) throw std::runtime_error(std::string(operation) + ": " + ups_strerror(status));
}
struct Options {
  std::string mix = "single", placement = "shared";
  std::array<int, workers> cpus{};
  uint64_t preload = 40000, seed = 1, max_records = 5000000;
  uint64_t duration_ns = 2000000000ULL;
};
Options parse(int argc, char **argv) {
  Options o;
  bool cpus_given = false;
  for (int i = 1; i < argc; ++i) {
    std::string arg(argv[i]);
    if (arg == "--help") {
      std::cout << "--mix reads|single|batch --placement shared|split --cpus 0,1,2,3,4,5,6,7 "
                   "--seed N --preload N --seconds N --max-records N\n";
      std::exit(0);
    }
    if (++i == argc) throw std::runtime_error("missing value for " + arg);
    const char *v = argv[i];
    if (arg == "--mix") o.mix = v;
    else if (arg == "--placement") o.placement = v;
    else if (arg == "--preload") o.preload = positive(v, argv[i-1]);
    else if (arg == "--seed") o.seed = positive(v, argv[i-1]);
    else if (arg == "--seconds") {
      const uint64_t s = positive(v, argv[i-1]);
      if (s < 1 || s > 10) throw std::runtime_error("seconds outside 1..10");
      o.duration_ns = s * 1000000000ULL;
    } else if (arg == "--max-records") o.max_records = positive(v, argv[i-1]);
    else if (arg == "--cpus") {
      std::istringstream input(v);
      std::string token;
      for (unsigned n = 0; n < workers; ++n) {
        if (!std::getline(input, token, ',')) throw std::runtime_error("exactly eight CPUs required");
        const auto cpu = positive(token.c_str(), "CPU");
        if (cpu >= CPU_SETSIZE) throw std::runtime_error("invalid CPU");
        o.cpus[n] = int(cpu);
      }
      if (std::getline(input, token, ',') || v[std::strlen(v)-1] == ',')
        throw std::runtime_error("exactly eight CPUs required");
      cpus_given = true;
    } else throw std::runtime_error("unknown option: " + arg);
  }
  if (!cpus_given || (o.mix != "reads" && o.mix != "single" && o.mix != "batch") ||
      (o.placement != "shared" && o.placement != "split")) throw std::runtime_error("invalid experiment configuration");
  if (o.preload < 2 || o.preload % 2 || o.preload > 1000000 ||
      o.max_records < 32 || o.max_records > 20000000 || o.max_records % 32)
    throw std::runtime_error("preload or record cap outside approved bounds");
  cpu_set_t allowed;
  if (sched_getaffinity(0, sizeof(allowed), &allowed)) throw std::runtime_error("sched_getaffinity failed");
  for (int cpu : o.cpus) if (!CPU_ISSET(cpu, &allowed)) throw std::runtime_error("CPU outside parent affinity");
  for (unsigned a = 0; a < workers; ++a)
    for (unsigned b = 0; b < a; ++b)
      if (o.cpus[a] == o.cpus[b]) throw std::runtime_error("physical worker CPU repeated");
  cpu_set_t setup;
  CPU_ZERO(&setup); CPU_SET(o.cpus[0], &setup);
  if (sched_setaffinity(0, sizeof(setup), &setup)) throw std::runtime_error("setup affinity failed");
  rlimit limit{};
  if (getrlimit(RLIMIT_AS, &limit)) throw std::runtime_error("getrlimit failed");
  constexpr rlim_t ceiling = 8ULL << 30;
  if (limit.rlim_max != RLIM_INFINITY && limit.rlim_max < ceiling)
    throw std::runtime_error("hard address-space limit below 8 GiB");
  limit.rlim_cur = ceiling;
  if (setrlimit(RLIMIT_AS, &limit)) throw std::runtime_error("setrlimit failed");
  return o;
}
constexpr const char *variant =
#if defined(UPS_BRIDGE_KIND)
#if UPS_BRIDGE_KIND == 0
#ifdef UPS_BRIDGE_PROFILE
  "bridge_mutex_profile";
#else
  "bridge_mutex";
#endif
#elif UPS_BRIDGE_KIND == 1
#ifdef UPS_BRIDGE_PROFILE
  "fc_profile";
#else
  "fc";
#endif
#elif UPS_BRIDGE_KIND == 2
#ifdef UPS_BRIDGE_PROFILE
  "fc_pq_profile";
#else
  "fc_pq";
#endif
#elif UPS_BRIDGE_KIND == 3
#ifdef UPS_BRIDGE_PROFILE
  "uscl_profile";
#else
  "uscl";
#endif
#elif UPS_BRIDGE_KIND == 4
#ifdef UPS_BRIDGE_PROFILE
  "cfl_local_profile";
#else
  "cfl_local";
#endif
#elif UPS_BRIDGE_KIND == 6
#ifdef UPS_BRIDGE_PROFILE
  "mcs_profile";
#else
  "mcs";
#endif
#else
#error Unsupported bridge kind
#endif
#elif defined(UPS_NATIVE_PROFILE)
  "profile";
#else
  "native";
#endif

class Store {
  ups_env_t *env_ = nullptr;
  ups_db_t *db_ = nullptr;
#ifdef UPS_BRIDGE_KIND
  dlock_bridge *bridge_ = nullptr;
#endif
public:
  Store(uint64_t first, uint64_t count, uint64_t seed) {
    try {
      check(ups_env_create(&env_, nullptr, UPS_IN_MEMORY, 0, nullptr), "env_create");
      const ups_parameter_t parameters[] = {{UPS_PARAM_KEY_TYPE, UPS_TYPE_UINT64},
        {UPS_PARAM_KEY_SIZE, 8}, {UPS_PARAM_RECORD_SIZE, 8}, {0, 0}};
      check(ups_env_create_db(env_, &db_, 1, 0, parameters), "create_db");
      for (uint64_t key_value = first; key_value < first + count; ++key_value) {
        uint64_t record_value = value_of(key_value, seed);
        ups_key_t key = {8, &key_value, 0, 0};
        ups_record_t record = {8, &record_value, 0};
        check(ups_db_insert(db_, nullptr, &key, &record, 0), "preload");
      }
#ifdef UPS_BRIDGE_KIND
      void *mutex = UPS_BRIDGE_KIND == 0 ? ups_dlock_private_native_mutex(db_) : nullptr;
      if (dlock_bridge_create(UPS_BRIDGE_KIND, mutex, &bridge_) != DLOCK_BRIDGE_OK)
        throw std::runtime_error("bridge create failed");
#endif
    } catch (...) { close(); throw; }
  }
  Store(const Store &) = delete;
  ~Store() { close(); }
  void close() noexcept {
#ifdef UPS_BRIDGE_KIND
    if (bridge_) {
      if (dlock_bridge_destroy(bridge_) != DLOCK_BRIDGE_OK) std::terminate();
      bridge_ = nullptr;
    }
#endif
    if (db_) { if (ups_db_close(db_, 0)) std::terminate(); db_ = nullptr; }
    if (env_) { if (ups_env_close(env_, 0)) std::terminate(); env_ = nullptr; }
  }
  int find(uint64_t &key_value, uint64_t &record_value,
           uint32_t &size, uint16_t &key_size, dlock_request_metrics *sample,
           int32_t &bridge_status) {
    ups_key_t key = {8, &key_value, UPS_KEY_USER_ALLOC, 0};
    ups_record_t record = {8, &record_value, UPS_RECORD_USER_ALLOC};
#ifdef UPS_BRIDGE_KIND
    const int status = ups_dlock_private_find(db_, &key, &record, bridge_,
                                               dlock_bridge_execute, sample, &bridge_status);
#elif defined(UPS_NATIVE_PROFILE)
    const int status = ups_dlock_private_native_find(db_, &key, &record, sample);
#else
    const int status = ups_db_find(db_, nullptr, &key, &record, 0);
#endif
    size = record.size; key_size = key.size;
    if (!status && (key.data != &key_value || record.data != &record_value)) return UPS_INTERNAL_ERROR;
    return status;
  }
  int insert(uint64_t &key_value, uint64_t &record_value,
             dlock_request_metrics *sample, int32_t &bridge_status) {
    ups_key_t key = {8, &key_value, 0, 0};
    ups_record_t record = {8, &record_value, 0};
#ifdef UPS_BRIDGE_KIND
    return ups_dlock_private_insert(db_, &key, &record, bridge_,
                                     dlock_bridge_execute, sample, &bridge_status);
#elif defined(UPS_NATIVE_PROFILE)
    return ups_dlock_private_native_insert(db_, &key, &record, sample);
#else
    return ups_db_insert(db_, nullptr, &key, &record, 0);
#endif
  }
  int batch(std::array<uint64_t, batch_size> &key_values,
            std::array<uint64_t, batch_size> &record_values,
            std::array<int, batch_size> &statuses, dlock_request_metrics *sample,
            int32_t &bridge_status) {
    std::array<ups_key_t, batch_size> keys{};
    std::array<ups_record_t, batch_size> records{};
    for (unsigned i = 0; i < batch_size; ++i) {
      keys[i] = {8, &key_values[i], 0, 0};
      records[i] = {8, &record_values[i], 0};
    }
#ifdef UPS_BRIDGE_KIND
    return ups_dlock_private_batch(db_, keys.data(), records.data(), statuses.data(),
                                   bridge_, dlock_bridge_execute, sample, &bridge_status);
#else
    return ups_dlock_private_native_batch(db_, keys.data(), records.data(),
                                          statuses.data(), sample);
#endif
  }
  struct Oracle { int integrity = 0, count_status = 0, cursor_status = 0;
                  uint64_t count = 0, seen = 0, bad = 0, expected = 0; };
  Oracle verify(uint64_t first, uint64_t preload, uint64_t seed,
                const std::array<uint64_t, workers> &written,
                unsigned writer_first, unsigned writer_last) {
    Oracle result;
    result.expected = preload;
    for (unsigned id = writer_first; id < writer_last; ++id) result.expected += written[id];
    result.integrity = ups_db_check_integrity(db_, 0);
    result.count_status = ups_db_count(db_, nullptr, 0, &result.count);
    ups_cursor_t *cursor = nullptr;
    result.cursor_status = ups_cursor_create(&cursor, db_, nullptr, 0);
    if (result.cursor_status) return result;
    bool initial = true;
    uint64_t previous = 0;
    for (;;) {
      uint64_t k = 0, v = 0;
      ups_key_t key = {8, &k, UPS_KEY_USER_ALLOC, 0};
      ups_record_t record = {8, &v, UPS_RECORD_USER_ALLOC};
      int status = ups_cursor_move(cursor, &key, &record,
                                   initial ? UPS_CURSOR_FIRST : UPS_CURSOR_NEXT);
      if (status == UPS_KEY_NOT_FOUND) break;
      if (status) { result.cursor_status = status; break; }
      bool valid = key.size == 8 && record.size == 8 && v == value_of(k, seed) &&
                   (initial || k > previous);
      if (k & high_bit) {
        const uint64_t ordinal = inserted_ordinal(k, seed);
        const unsigned writer = cohort_size + (ordinal % cohort_size);
        valid &= writer >= writer_first && writer < writer_last &&
                 ordinal / cohort_size < written[writer];
      } else valid &= k >= first && k - first < preload;
      if (!valid) ++result.bad;
      previous = k;
      initial = false;
      if (++result.seen > preload + 20000000ULL) { ++result.bad; break; }
    }
    const int closed = ups_cursor_close(cursor);
    if (!result.cursor_status) result.cursor_status = closed;
    return result;
  }
#ifdef UPS_BRIDGE_KIND
  dlock_thread_metrics thread_metrics() {
    dlock_thread_metrics result{};
    if (dlock_bridge_get_thread_metrics(bridge_, &result) != DLOCK_BRIDGE_OK)
      throw std::runtime_error("thread metrics failed");
    return result;
  }
#endif
};
struct Histogram {
  uint64_t count = 0, total = 0, minimum = 0, maximum = 0;
  std::array<uint64_t, 64> bins{};
  void add(uint64_t delta) {
    if (!count || delta < minimum) minimum = delta;
    if (delta > maximum) maximum = delta;
    ++count; total += delta;
    ++bins[delta ? std::min(63, 64 - __builtin_clzll(delta)) : 0];
  }
};
struct Window { uint64_t reads = 0, write_requests = 0, records = 0, service_ns = 0; };
struct alignas(64) Worker {
  unsigned id = 0; int cpu = -1, cpu_start = -1, cpu_end = -1, affinity_error = 0;
  uint64_t cpu_ns = 0, attempted = 0, completed = 0, on_time = 0, drained = 0;
  uint64_t read_requests = 0, write_requests = 0, records = 0, service_ns = 0;
  uint64_t first_completion_ns = 0, last_completion_ns = 0, max_gap_ns = 0;
  uint64_t last_return_ns = 0, pending_ns = 0, submission_gap_ns = 0, max_submission_gap_ns = 0;
  int first_db_status = 0, bridge_status = 0;
  uint64_t bad_values = 0, bad_sizes = 0, missing_samples = 0;
  Histogram latency;
  std::array<Window, windows> progress{};
#ifdef UPS_BRIDGE_KIND
  dlock_thread_metrics executor{};
#endif
};
struct Gate { std::mutex mutex; std::condition_variable cv; unsigned ready = 0;
              bool released = false, abort = false; uint64_t start = 0; };
void work(Store &db, const Options &opts, Worker &w, Gate &gate) noexcept {
  cpu_set_t mask; CPU_ZERO(&mask); CPU_SET(w.cpu, &mask);
  w.affinity_error = pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask);
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    ++gate.ready; gate.cv.notify_all();
    gate.cv.wait(lock, [&] { return gate.released; });
    if (gate.abort || w.affinity_error) return;
  }
  try {
    w.cpu_start = sched_getcpu();
    const uint64_t cpu_start = ns(CLOCK_THREAD_CPUTIME_ID);
    const uint64_t deadline = gate.start + opts.duration_ns;
    uint64_t ordinal = 0;
    while (ns() < deadline) {
      const bool write = opts.mix != "reads" && w.id >= cohort_size;
      uint64_t started = 0, finished = 0;
      dlock_request_metrics sample{};
      dlock_request_metrics *sample_ptr =
#if defined(UPS_BRIDGE_PROFILE) || defined(UPS_NATIVE_PROFILE)
          &sample;
#else
          nullptr;
#endif
      int32_t bridge = DLOCK_BRIDGE_OK;
      int status = 0;
      unsigned records = 0;
      uint64_t key_before = 0, value_before = 0, key = 0, value = 0;
      uint32_t size = 8; uint16_t key_size = 8;
      if (!write) {
        const uint64_t half = opts.preload / 2;
        key_before = (w.id >= cohort_size ? half : 0) +
                     (mix(opts.seed ^ mix(w.id + ordinal * workers)) % half);
        value_before = value_of(key_before, opts.seed);
        key = key_before;
        started = ns();
        status = db.find(key, value, size, key_size, sample_ptr, bridge);
        finished = ns();
        if (!status && value != value_before) ++w.bad_values;
        if (!status && (key != key_before || size != 8 || key_size != 8)) ++w.bad_sizes;
        ++ordinal;
      } else if (opts.mix == "single") {
        if (ordinal >= opts.max_records / cohort_size) {
          w.first_db_status = UPS_LIMITS_REACHED; break;
        }
        key_before = insert_key((w.id - cohort_size) + ordinal * cohort_size, opts.seed);
        value_before = value_of(key_before, opts.seed);
        key = key_before; value = value_before;
        started = ns();
        status = db.insert(key, value, sample_ptr, bridge);
        finished = ns();
        records = status ? 0 : 1;
        if (!status && (key != key_before || value != value_before)) ++w.bad_values;
        ++ordinal;
      } else {
        if (ordinal > opts.max_records / cohort_size / batch_size - 1) {
          w.first_db_status = UPS_LIMITS_REACHED; break;
        }
        std::array<uint64_t, batch_size> keys{}, values{};
        std::array<int, batch_size> statuses{};
        for (unsigned i = 0; i < batch_size; ++i) {
          keys[i] = insert_key((w.id - cohort_size) +
                               (ordinal * batch_size + i) * cohort_size, opts.seed);
          values[i] = value_of(keys[i], opts.seed);
        }
        started = ns();
        status = db.batch(keys, values, statuses, sample_ptr, bridge);
        finished = ns();
        for (unsigned i = 0; i < batch_size; ++i) {
          if (statuses[i]) break;
          if (keys[i] != insert_key((w.id - cohort_size) +
                                   (ordinal * batch_size + i) * cohort_size, opts.seed) ||
              values[i] != value_of(keys[i], opts.seed)) ++w.bad_values;
          ++records;
        }
        ++ordinal;
      }
      const uint64_t prior_return = w.last_return_ns ? w.last_return_ns : gate.start;
      const uint64_t gap = started - prior_return;
      w.submission_gap_ns += gap;
      w.max_submission_gap_ns = std::max(w.max_submission_gap_ns, gap);
      w.pending_ns += finished - started;
      w.last_return_ns = finished;
      ++w.attempted;
      w.records += records;
      if (bridge != DLOCK_BRIDGE_OK) w.bridge_status = bridge;
      if (status) { w.first_db_status = status; break; }
      if (w.bad_values || w.bad_sizes) break;
      ++w.completed;
      w.latency.add(finished - started);
      if (finished <= deadline) {
        ++w.on_time;
        const unsigned index = std::min<unsigned>(windows - 1,
            (finished - gate.start) * windows / opts.duration_ns);
        auto &p = w.progress[index];
        if (write) { ++p.write_requests; p.records += records; ++w.write_requests; }
        else { ++p.reads; ++w.read_requests; }
        if (sample.enabled) { p.service_ns += sample.service_ns; w.service_ns += sample.service_ns; }
        if (!w.first_completion_ns) w.first_completion_ns = finished;
        const uint64_t previous = w.last_completion_ns ? w.last_completion_ns : gate.start;
        w.max_gap_ns = std::max(w.max_gap_ns, finished - previous);
        w.last_completion_ns = finished;
      } else ++w.drained;
#if defined(UPS_BRIDGE_PROFILE) || defined(UPS_NATIVE_PROFILE)
      if (!sample.enabled) ++w.missing_samples;
#endif
    }
    w.max_gap_ns = std::max(w.max_gap_ns, deadline -
        std::min(deadline, w.last_completion_ns ? w.last_completion_ns : gate.start));
#ifdef UPS_BRIDGE_KIND
    w.executor = db.thread_metrics();
#endif
    w.cpu_ns = ns(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
    w.cpu_end = sched_getcpu();
  } catch (...) { std::terminate(); }
}
void histogram_json(const Histogram &h) {
  std::cout << "{\"count\":" << h.count << ",\"sum_ns\":" << h.total
    << ",\"min_ns\":" << h.minimum << ",\"max_ns\":" << h.maximum << ",\"log2_bins\":[";
  // The last bin is open-ended; analysis leaves that quantile unbounded.
  for (unsigned i = 0; i < h.bins.size(); ++i) std::cout << (i ? "," : "") << h.bins[i];
  std::cout << "]}";
}
void worker_json(const Worker &w) {
  std::cout << "{\"id\":" << w.id << ",\"cohort\":" << (w.id / cohort_size)
    << ",\"cpu\":" << w.cpu << ",\"cpu_start\":" << w.cpu_start
    << ",\"cpu_end\":" << w.cpu_end << ",\"cpu_ns\":" << w.cpu_ns
    << ",\"attempted_requests\":" << w.attempted
    << ",\"completed_requests\":" << w.completed
    << ",\"completed_before_deadline\":" << w.on_time
    << ",\"completed_during_drain\":" << w.drained
    << ",\"read_requests\":" << w.read_requests
    << ",\"write_requests\":" << w.write_requests
    << ",\"inserted_records\":" << w.records
    << ",\"service_before_deadline_ns\":" << w.service_ns
    << ",\"first_completion_offset_ns\":" << w.first_completion_ns
    << ",\"last_completion_offset_ns\":" << w.last_completion_ns
    << ",\"max_no_completion_gap_ns\":" << w.max_gap_ns
    << ",\"pending_request_ns\":" << w.pending_ns
    << ",\"submission_gap_ns\":" << w.submission_gap_ns
    << ",\"max_submission_gap_ns\":" << w.max_submission_gap_ns
    << ",\"first_db_status\":" << w.first_db_status
    << ",\"bridge_status\":" << w.bridge_status
    << ",\"affinity_error\":" << w.affinity_error
    << ",\"bad_values\":" << w.bad_values
    << ",\"bad_sizes\":" << w.bad_sizes
    << ",\"missing_service_samples\":" << w.missing_samples
    << ",\"latency_ns\":";
  histogram_json(w.latency);
  std::cout << ",\"windows\":[";
  for (unsigned i = 0; i < windows; ++i) {
    const auto &p = w.progress[i];
    std::cout << (i ? "," : "") << "{\"reads\":" << p.reads
      << ",\"write_requests\":" << p.write_requests << ",\"records\":" << p.records
      << ",\"service_ns\":" << p.service_ns << "}";
  }
  std::cout << "]";
#ifdef UPS_BRIDGE_KIND
  std::cout << ",\"executor\":{\"callbacks\":" << w.executor.executed_callbacks
    << ",\"service_ns\":" << w.executor.executed_service_ns
    << ",\"combiner_pass_ticks\":" << w.executor.combiner_pass_tsc_ticks << "}";
#endif
  std::cout << "}";
}
int run(const Options &opts) {
  std::vector<std::unique_ptr<Store>> stores;
  const uint64_t half = opts.preload / 2;
  if (opts.placement == "shared") stores.emplace_back(new Store(0, opts.preload, opts.seed));
  else {
    stores.emplace_back(new Store(0, half, opts.seed));
    stores.emplace_back(new Store(half, half, opts.seed));
  }
  std::array<Worker, workers> state{};
  Gate gate;
  std::vector<std::thread> threads;
  threads.reserve(workers);
  try {
    for (unsigned id = 0; id < workers; ++id) {
      state[id].id = id; state[id].cpu = opts.cpus[id];
      Store &db = *stores[opts.placement == "split" && id >= cohort_size ? 1 : 0];
      threads.emplace_back(work, std::ref(db), std::cref(opts), std::ref(state[id]), std::ref(gate));
    }
  } catch (...) {
    { std::lock_guard<std::mutex> lock(gate.mutex); gate.abort = gate.released = true; }
    gate.cv.notify_all();
    for (auto &thread : threads) thread.join();
    throw;
  }
  uint64_t process_start = 0;
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    gate.cv.wait(lock, [&] { return gate.ready == workers; });
    for (const auto &w : state) if (w.affinity_error) gate.abort = true;
    process_start = ns(CLOCK_PROCESS_CPUTIME_ID);
    gate.start = ns();
    gate.released = true;
  }
  gate.cv.notify_all();
  for (auto &thread : threads) thread.join(); // all callbacks and USCL TLS teardown finished
  const uint64_t end = ns();
  const uint64_t process_cpu = ns(CLOCK_PROCESS_CPUTIME_ID) - process_start;
  std::array<uint64_t, workers> written{};
  bool failed = false;
  for (const auto &w : state) {
    written[w.id] = w.records;
    failed |= w.first_db_status || w.bridge_status || w.affinity_error ||
              w.bad_values || w.bad_sizes || w.missing_samples;
  }
  std::vector<Store::Oracle> oracle;
  if (opts.placement == "shared")
    oracle.push_back(stores[0]->verify(0, opts.preload, opts.seed, written, 4, 8));
  else {
    oracle.push_back(stores[0]->verify(0, half, opts.seed, written, 0, 0));
    oracle.push_back(stores[1]->verify(half, half, opts.seed, written, 4, 8));
  }
  for (const auto &v : oracle)
    failed |= v.integrity || v.count_status || v.cursor_status || v.bad ||
              v.seen != v.expected || v.count != v.expected;
  std::cout << "{\"schema\":1,\"variant\":\"" << variant << "\",\"status\":\""
    << (failed ? "failed" : "ok") << "\",\"mix\":\"" << opts.mix
    << "\",\"placement\":\"" << opts.placement << "\",\"seed\":" << opts.seed
    << ",\"preload_total\":" << opts.preload << ",\"max_inserted_records\":" << opts.max_records
    << ",\"memory_limit_gib\":8"
    << ",\"duration_ns\":" << opts.duration_ns
    << ",\"elapsed_ns\":" << end - gate.start << ",\"process_cpu_ns\":" << process_cpu
    << ",\"drain_ns\":" << (end > gate.start + opts.duration_ns ? end - gate.start - opts.duration_ns : 0)
    << ",\"window_ns\":" << opts.duration_ns / windows
    << ",\"profile_enabled\":"
#if defined(UPS_BRIDGE_PROFILE) || defined(UPS_NATIVE_PROFILE)
    << "true"
#else
    << "false"
#endif
    << ",\"workers\":[";
  for (unsigned id = 0; id < workers; ++id) {
    if (id) std::cout << ',';
    // Offsets are monotonic relative to the common gate start.
    if (state[id].first_completion_ns) state[id].first_completion_ns -= gate.start;
    if (state[id].last_completion_ns) state[id].last_completion_ns -= gate.start;
    worker_json(state[id]);
  }
  std::cout << "],\"oracle\":[";
  for (size_t i = 0; i < oracle.size(); ++i) {
    if (i) std::cout << ',';
    const auto &v = oracle[i];
    std::cout << "{\"environment\":" << i << ",\"integrity_status\":" << v.integrity
      << ",\"count_status\":" << v.count_status << ",\"cursor_status\":" << v.cursor_status
      << ",\"expected\":" << v.expected << ",\"count\":" << v.count
      << ",\"seen\":" << v.seen << ",\"bad\":" << v.bad << "}";
  }
  std::cout << "]}" << std::endl;
  stores.clear(); // verified first, then all handles/DBs destroyed after joins
  return failed ? 2 : 0;
}
} // namespace
int main(int argc, char **argv) {
  try { return run(parse(argc, argv)); }
  catch (const std::exception &e) { std::cerr << "heterogeneous client failure: " << e.what() << '\n'; return 2; }
}
