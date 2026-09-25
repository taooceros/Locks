// Actual UpScaleDB: one gate per environment, never one gate per shared-env table.
#include <stddef.h>
#include <ups/upscaledb.h>
#include <ups/upscaledb_int.h>
#include "bridge.h"
#ifdef UPS_BRIDGE_KIND
#include "private_ops.h"
#endif

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <exception>
#include <fstream>
#include <functional>
#include <iomanip>
#include <iostream>
#include <linux/mempolicy.h>
#include <mutex>
#include <pthread.h>
#include <sched.h>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <thread>
#include <time.h>
#include <unistd.h>
#include <vector>

namespace {
constexpr int kMaxWorkers = 32, kMaxTables = 32;
constexpr uint64_t kInitial = 32768, kInsertLimitPerWorker = 4000000;
constexpr uint64_t kHigh = 1ULL << 63, kMask = kHigh - 1;
constexpr uint64_t kMultiplier = 0x5851f42d4c957f2dULL;
constexpr uint64_t kCanary = 0xacc0ffee12345678ULL;
const char *backend() {
#ifndef UPS_BRIDGE_KIND
  return "native";
#else
  switch (UPS_BRIDGE_KIND) {
    case DLOCK_BRIDGE_MUTEX: return "bridge_mutex";
    case DLOCK_BRIDGE_FC: return "fc";
    case DLOCK_BRIDGE_FC_PQ: return "fc_pq";
    case DLOCK_BRIDGE_USCL: return "uscl";
    case DLOCK_BRIDGE_CFL_LOCAL: return "cfl_local";
    case DLOCK_BRIDGE_SPINLOCK: return "spinlock";
    case DLOCK_BRIDGE_MCS: return "mcs";
    case DLOCK_BRIDGE_TICKET: return "ticket";
    case DLOCK_BRIDGE_CLH: return "clh";
    default: return "unsupported";
  }
#endif
}
uint64_t now(clockid_t clock = CLOCK_MONOTONIC) {
  timespec t{};
  if (clock_gettime(clock, &t)) throw std::runtime_error("clock_gettime failed");
  return uint64_t(t.tv_sec) * 1000000000ULL + uint64_t(t.tv_nsec);
}
uint64_t mix(uint64_t x) {
  x += 0x9e3779b97f4a7c15ULL;
  x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
  x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
  return x ^ (x >> 31);
}
constexpr uint64_t inverse() {
  uint64_t x = 1;
  for (int i = 0; i < 6; ++i) x *= 2 - kMultiplier * x;
  return x;
}
uint64_t table_seed(uint64_t seed, int table) { return mix(seed ^ mix(uint64_t(table))); }
uint64_t value_for(uint64_t key, uint64_t seed, int table) {
  return mix(key ^ table_seed(seed, table) ^ 0xa531ea3dd5bd7121ULL);
}
uint64_t insert_key(uint64_t ordinal, uint64_t seed, int table) {
  return kHigh | ((ordinal * kMultiplier + table_seed(seed, table)) & kMask);
}
uint64_t decode_key(uint64_t key, uint64_t seed, int table) {
  return (((key & kMask) - table_seed(seed, table)) * inverse()) & kMask;
}
void require(bool ok, const char *message) {
  if (!ok) throw std::runtime_error(message);
}
void check(int status, const char *where) {
  if (status) throw std::runtime_error(std::string(where) + ": " + ups_strerror(status));
}
std::string quote(const std::string &text) {
  std::ostringstream out;
  out << '"';
  for (unsigned char c : text) {
    if (c == '"' || c == '\\') out << '\\' << c;
    else if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(c) << std::dec;
    else out << c;
  }
  out << '"';
  return out.str();
}
struct Options {
  std::string layout = "coarse", routing = "uniform";
  int tables = 1, workers = 8;
  uint64_t seed = 1;
  double seconds = 2;
  bool errors = false;
  int first_cpu() const { return 64 - workers; }
};
Options parse(int argc, char **argv) {
  Options o;
  for (int i = 1; i < argc; ++i) {
    std::string arg = argv[i];
    require(i + 1 < argc, "option requires a value");
    std::string value = argv[++i];
    size_t used = 0;
    if (arg == "--layout") o.layout = value;
    else if (arg == "--tables") {
      o.tables = std::stoi(value, &used); require(used == value.size(), "invalid tables");
    } else if (arg == "--seed") {
      require(!value.empty() && value[0] != '-', "invalid seed");
      o.seed = std::stoull(value, &used); require(used == value.size(), "invalid seed");
    } else if (arg == "--seconds") {
      o.seconds = std::stod(value, &used); require(used == value.size(), "invalid seconds");
    } else if (arg == "--workers") {
      o.workers = std::stoi(value, &used); require(used == value.size(), "invalid workers");
    } else if (arg == "--routing") {
      o.routing = value;
    } else if (arg == "--self-test") {
      require(value == "errors", "only errors self-test is supported"); o.errors = true;
    } else throw std::runtime_error("unknown option " + arg);
  }
  require(o.layout == "coarse" || o.layout == "split", "layout must be coarse or split");
  require(o.tables == 1 || o.tables == 8 || o.tables == 32, "tables must be 1, 8 or 32");
  require(o.workers == 8 || o.workers == 16 || o.workers == 32, "workers must be 8, 16 or 32");
  require(o.routing == "uniform" || o.routing == "hot90" || o.routing == "hot100", "invalid routing");
  require(o.routing == "uniform" || (o.layout == "split" && o.tables == 32), "hot routing requires 32 independent environments");
  require(std::isfinite(o.seconds) && o.seconds >= .01 && o.seconds <= 2, "seconds must be in [0.01,2]");
  require(std::string(backend()) != "unsupported", "unsupported bridge kind");
  return o;
}
bool singleton(int cpu) {
  cpu_set_t actual;
  CPU_ZERO(&actual);
  return pthread_getaffinity_np(pthread_self(), sizeof(actual), &actual) == 0 &&
      CPU_COUNT(&actual) == 1 && CPU_ISSET(cpu, &actual) && sched_getcpu() == cpu;
}
void pin(int cpu) {
  cpu_set_t selected;
  CPU_ZERO(&selected); CPU_SET(cpu, &selected);
  require(!pthread_setaffinity_np(pthread_self(), sizeof(selected), &selected) && singleton(cpu), "singleton CPU affinity failed");
}
void resources(const Options &o) {
  cpu_set_t allowed;
  CPU_ZERO(&allowed);
  require(!sched_getaffinity(0, sizeof(allowed), &allowed), "cannot read inherited affinity");
  for (int cpu = o.first_cpu(); cpu < o.first_cpu() + o.workers; ++cpu)
    require(CPU_ISSET(cpu, &allowed), "worker CPU unavailable in inherited affinity");
  pin(o.first_cpu());
  rlimit limit{};
  require(!getrlimit(RLIMIT_AS, &limit), "getrlimit failed");
  const rlim_t cap = 32ULL << 30;
  require(limit.rlim_max == RLIM_INFINITY || limit.rlim_max >= cap, "inherited hard RLIMIT_AS below 32GiB");
  limit.rlim_cur = cap;
  require(!setrlimit(RLIMIT_AS, &limit), "setrlimit failed");
  unsigned long mask = 1UL << 1;
  require(!syscall(SYS_set_mempolicy, MPOL_BIND, &mask, 3UL), "set_mempolicy bind node1 failed");
  std::array<unsigned long, 16> actual{};
  int mode = -1;
  require(!syscall(SYS_get_mempolicy, &mode, actual.data(), 1025UL, nullptr, 0), "get_mempolicy failed");
  require(mode == MPOL_BIND && actual[0] == mask &&
      std::all_of(actual.begin() + 1, actual.end(), [](unsigned long n) { return n == 0; }), "effective NUMA policy differs");
}
struct Guarded {
  uint64_t left = kCanary, bytes = 0, right = kCanary;
  bool intact() const { return left == kCanary && right == kCanary; }
};
using Written = std::array<std::array<uint64_t, kMaxWorkers>, kMaxTables>;
struct Verification {
  uint64_t seen = 0, expected = 0, count = 0, bad = 0;
  int integrity = 0, cursor = 0, count_status = 0;
  bool ok() const { return !bad && !integrity && !cursor && !count_status && seen == expected && count == expected; }
};
struct Shutdown { int bridge = 0, db = 0, env = 0; };

// Own every handle and gate. Only find/insert are exposed during the worker phase.
// Coarse tables intentionally share the SAME gate because Env owns mutable state.
class Store {
  struct Environment {
    ups_env_t *env = nullptr;
#ifdef UPS_BRIDGE_KIND
    dlock_bridge *gate = nullptr;
#endif
  };
  Options options_;
  std::array<Environment, kMaxTables> environments_{};
  std::array<ups_db_t *, kMaxTables> tables_{};
  int env_count_;
  int environment(int table) const { return options_.layout == "coarse" ? 0 : table; }
public:
  explicit Store(const Options &o) : options_(o), env_count_(o.layout == "coarse" ? 1 : o.tables) {
    try {
      for (int e = 0; e < env_count_; ++e)
        check(ups_env_create(&environments_[e].env, nullptr, UPS_IN_MEMORY, 0, nullptr), "env_create");
      const ups_parameter_t params[] = {{UPS_PARAM_KEY_TYPE, UPS_TYPE_UINT64},
          {UPS_PARAM_KEY_SIZE, 8}, {UPS_PARAM_RECORD_SIZE, 8}, {0, 0}};
      for (int t = 0; t < o.tables; ++t) {
        check(ups_env_create_db(environments_[environment(t)].env, &tables_[t],
                               uint16_t(o.layout == "coarse" ? t + 1 : 1), 0, params), "create_db");
        for (uint64_t key_bytes = 0; key_bytes < kInitial / o.tables; ++key_bytes) {
          uint64_t value = value_for(key_bytes, o.seed, t);
          ups_key_t key = {8, &key_bytes, 0, 0};
          ups_record_t record = {8, &value, 0};
          check(ups_db_insert(tables_[t], nullptr, &key, &record, 0), "preload");
        }
      }
#ifdef UPS_BRIDGE_KIND
      for (int e = 0; e < env_count_; ++e) {
        void *mutex = ups_dlock_private_native_mutex(tables_[e]);
        require(mutex != nullptr, "missing original environment mutex");
        // Check the coarse alias invariant quiescently, before any worker exists.
        if (o.layout == "coarse")
          for (int t = 0; t < o.tables; ++t)
            require(ups_dlock_private_native_mutex(tables_[t]) == mutex, "coarse tables do not share an environment mutex");
        require(dlock_bridge_create(UPS_BRIDGE_KIND,
                    UPS_BRIDGE_KIND == DLOCK_BRIDGE_MUTEX ? mutex : nullptr,
                    &environments_[e].gate) == DLOCK_BRIDGE_OK, "bridge_create failed");
      }
#endif
    } catch (...) { close(); throw; }
  }
  Store(const Store &) = delete;
  Store &operator=(const Store &) = delete;
  ~Store() { close(); }
  int operation(int table, bool insert, uint64_t &key_bytes, uint64_t &value,
                uint16_t &key_size, uint32_t &record_size, int32_t &bridge_status) {
    ups_key_t key = {8, &key_bytes, insert ? 0u : UPS_KEY_USER_ALLOC, 0};
    ups_record_t record = {8, &value, insert ? 0u : UPS_RECORD_USER_ALLOC};
#ifdef UPS_BRIDGE_KIND
    dlock_bridge *gate = environments_[environment(table)].gate;
    int status = insert ? ups_dlock_private_insert(tables_[table], &key, &record, gate,
                             dlock_bridge_execute, nullptr, &bridge_status)
                        : ups_dlock_private_find(tables_[table], &key, &record, gate,
                             dlock_bridge_execute, nullptr, &bridge_status);
#else
    bridge_status = DLOCK_BRIDGE_OK;
    int status = insert ? ups_db_insert(tables_[table], nullptr, &key, &record, 0)
                        : ups_db_find(tables_[table], nullptr, &key, &record, 0);
#endif
    key_size = key.size; record_size = record.size;
    if (key.data != &key_bytes || record.data != &value) return UPS_INTERNAL_ERROR;
    return status;
  }
  std::array<Verification, kMaxTables> verify(const Written &written) {
    std::array<Verification, kMaxTables> result{};
    // No cursor/count/integrity API is called until ALL workers (and TLS destructors) joined.
    for (int t = 0; t < options_.tables; ++t) {
      Verification &v = result[t];
      v.expected = kInitial / options_.tables;
      for (auto count : written[t]) v.expected += count;
      v.integrity = ups_db_check_integrity(tables_[t], 0);
      v.count_status = ups_db_count(tables_[t], nullptr, 0, &v.count);
      ups_cursor_t *cursor = nullptr;
      v.cursor = ups_cursor_create(&cursor, tables_[t], nullptr, 0);
      if (v.cursor) continue;
      uint64_t previous = 0;
      bool first = true;
      while (true) {
        Guarded key_bytes, value;
        ups_key_t key = {8, &key_bytes.bytes, UPS_KEY_USER_ALLOC, 0};
        ups_record_t record = {8, &value.bytes, UPS_RECORD_USER_ALLOC};
        int st = ups_cursor_move(cursor, &key, &record, first ? UPS_CURSOR_FIRST : UPS_CURSOR_NEXT);
        if (st == UPS_KEY_NOT_FOUND) break;
        if (st) { v.cursor = st; break; }
        bool valid = key.size == 8 && record.size == 8 && key.data == &key_bytes.bytes &&
            record.data == &value.bytes && key_bytes.intact() && value.intact();
        const uint64_t k = key_bytes.bytes;
        if (k >= kInitial / options_.tables) {
          uint64_t decoded = decode_key(k, options_.seed, t);
          valid = valid && (k & kHigh) && decoded / options_.workers < written[t][decoded % options_.workers];
        }
        valid = valid && (first || k > previous) && value.bytes == value_for(k, options_.seed, t);
        if (!valid) ++v.bad;
        previous = k; first = false; ++v.seen;
        if (v.seen > kInitial / options_.tables + uint64_t(options_.workers) * kInsertLimitPerWorker) { ++v.bad; break; }
      }
      int close_status = ups_cursor_close(cursor);
      if (!v.cursor) v.cursor = close_status;
      // Membership, strictly increasing keys and exact count imply the exact final set.
    }
    return result;
  }
  Shutdown close() noexcept {
    Shutdown result;
#ifdef UPS_BRIDGE_KIND
    for (int e = 0; e < env_count_; ++e) if (environments_[e].gate) {
      int st = dlock_bridge_destroy(environments_[e].gate);
      if (st != DLOCK_BRIDGE_OK) std::terminate(); // never free a reachable DB after failed destroy
      environments_[e].gate = nullptr;
    }
#endif
    for (int t = 0; t < options_.tables; ++t) if (tables_[t]) {
      int st = ups_db_close(tables_[t], 0);
      if (st && !result.db) result.db = st;
      if (!st) tables_[t] = nullptr;
    }
    for (int e = 0; e < env_count_; ++e) if (environments_[e].env) {
      bool remaining = false;
      for (int t = 0; t < options_.tables; ++t)
        remaining = remaining || (environment(t) == e && tables_[t] != nullptr);
      int st = ups_env_close(environments_[e].env, remaining ? UPS_AUTO_CLEANUP : 0);
      if (st && !result.env) result.env = st;
      environments_[e].env = nullptr;
    }
    tables_.fill(nullptr);
    return result;
  }
};
struct Histogram {
  std::array<uint64_t, 64> bins{};
  uint64_t samples = 0, sum_ns = 0, max_ns = 0;
  void add(uint64_t ns) {
    ++bins[std::min(ns ? 64 - __builtin_clzll(ns) : 0, 63)];
    ++samples; sum_ns += ns; max_ns = std::max(max_ns, ns);
  }
};
struct Progress {
  std::array<uint64_t, 2> done{}, on_time{};
  uint64_t first_ns = 0, last_ns = 0;
};
struct alignas(64) Worker {
  int id = 0, cpu_start = -1, cpu_end = -1, db_error = 0, bridge_error = 0;
  bool affinity_start = false, affinity_end = false;
  uint64_t warmup_reads = 0, inserts = 0, bad_buffers = 0, cpu_ns = 0, end_ns = 0;
  std::string error;
  std::array<Histogram, 2> latency;
  std::array<Progress, kMaxTables> tables;
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
  std::array<std::array<uint64_t, kMaxWorkers + 1>, kMaxTables> overlap{};
#endif
};
struct Gate {
  std::mutex mutex;
  std::condition_variable cv;
  int ready = 0;
  bool release = false, abort = false;
  uint64_t start_ns = 0, deadline_ns = 0;
};
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
struct ApiOverlap {
  std::array<std::atomic<unsigned>, kMaxTables> outstanding{};
};
#endif
void checked_read(Store &db, const Options &o, int table, uint64_t input) {
  Guarded key, value;
  key.bytes = input;
  uint16_t key_size = 8; uint32_t size = 8; int32_t bridge = DLOCK_BRIDGE_INVALID;
  check(db.operation(table, false, key.bytes, value.bytes, key_size, size, bridge), "warmup find");
  require(bridge == DLOCK_BRIDGE_OK && key_size == 8 && size == 8 && key.bytes == input &&
      value.bytes == value_for(input, o.seed, table) && key.intact() && value.intact(), "warmup output/canary failure");
}
void worker(Store &db, const Options &o, Worker &w, Gate &gate
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
            , ApiOverlap &overlap
#endif
            ) noexcept {
  try {
    pin(o.first_cpu() + w.id);
    w.cpu_start = sched_getcpu(); w.affinity_start = singleton(o.first_cpu() + w.id);
    // These same live threads persist through the measurement. All locks are touched
    // before the common gate, stabilizing per-lock USCL registration/publication nodes.
    for (int round = 0; round < 8; ++round)
      for (int table = 0; table < o.tables; ++table) {
        checked_read(db, o, table, uint64_t(round * o.workers + w.id) % (kInitial / o.tables));
        ++w.warmup_reads;
      }
  } catch (const std::exception &ex) { w.error = ex.what(); }
  catch (...) { w.error = "unexpected warmup exception"; }
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    ++gate.ready; gate.cv.notify_all();
    gate.cv.wait(lock, [&] { return gate.release; });
    if (gate.abort || !w.error.empty()) return;
  }
  try {
    const uint64_t cpu_start = now(CLOCK_THREAD_CPUTIME_ID);
    const uint64_t stream_seed = mix(o.seed ^ mix(uint64_t(w.id) + 0x714acbd1ULL));
    const bool hot100 = o.routing == "hot100", hot90 = o.routing == "hot90";
    for (uint64_t sequence = 0; now() < gate.deadline_ns; ++sequence) {
      const bool insertion = (sequence & 1) != 0;
      const int type = insertion ? 1 : 0;
      const uint64_t random = mix(stream_seed + sequence);
      // Routing is a pure function of saved seed, worker and sequence, not completion timing.
      int table = int(random & uint64_t(o.tables - 1));
      if (hot100) table = 0;
      else if (hot90)
        table = (random >> 32) < 3865470566ULL ? 0 : 1 + int(mix(random ^ 0x72e015237ca8b9d1ULL) % 31);
      Progress &p = w.tables[table];
      if (insertion && w.inserts >= kInsertLimitPerWorker) {
        w.error = "unique insert resource cap reached (trial fails, never truncates)"; break;
      }
      const uint64_t input = insertion ? insert_key(p.done[1] * o.workers + uint64_t(w.id), o.seed, table)
          : mix(random ^ 0xc5017eacULL) % (kInitial / o.tables);
      Guarded key, value;
      key.bytes = input; value.bytes = insertion ? value_for(input, o.seed, table) : 0;
      uint16_t key_size = 8; uint32_t size = 8; int32_t bridge = DLOCK_BRIDGE_INVALID;
      const uint64_t before = now();
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
      // Counts requester API lifetimes, including execution and return, NOT lock wait depth.
      const unsigned outstanding = overlap.outstanding[table].fetch_add(1, std::memory_order_relaxed) + 1;
      ++w.overlap[table][outstanding];
#endif
      int status = db.operation(table, insertion, key.bytes, value.bytes, key_size, size, bridge);
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
      overlap.outstanding[table].fetch_sub(1, std::memory_order_relaxed);
#endif
      const uint64_t after = now();
      w.latency[type].add(after - before);
      if (bridge != DLOCK_BRIDGE_OK) w.bridge_error = bridge;
      if (status) { w.db_error = status; break; }
      ++p.done[type];
      if (insertion) ++w.inserts;
      if (after <= gate.deadline_ns) ++p.on_time[type];
      if (!p.first_ns) p.first_ns = after - gate.start_ns;
      p.last_ns = after - gate.start_ns;
      if (!key.intact() || !value.intact() || key.bytes != input || key_size != 8 || size != 8 ||
          value.bytes != value_for(input, o.seed, table)) { ++w.bad_buffers; break; }
      if (w.bridge_error) break;
    }
    w.cpu_ns = now(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
    w.end_ns = now(); w.cpu_end = sched_getcpu(); w.affinity_end = singleton(o.first_cpu() + w.id);
  } catch (const std::exception &ex) { w.error = ex.what(); }
  catch (...) { w.error = "unexpected worker exception"; }
}
void hist_json(const Histogram &h) {
  std::cout << "{\"samples\":" << h.samples << ",\"sum_ns\":" << h.sum_ns
            << ",\"max_ns\":" << h.max_ns << ",\"bins\":[";
  for (size_t i = 0; i < h.bins.size(); ++i) std::cout << (i ? "," : "") << h.bins[i];
  std::cout << "]}";
}
void verification_json(const std::array<Verification, kMaxTables> &v, int tables) {
  std::cout << '[';
  for (int t = 0; t < tables; ++t) {
    if (t) std::cout << ',';
    std::cout << "{\"table\":" << t << ",\"seen\":" << v[t].seen << ",\"expected\":" << v[t].expected
              << ",\"db_count\":" << v[t].count << ",\"bad\":" << v[t].bad << ",\"integrity_status\":" << v[t].integrity
              << ",\"count_status\":" << v[t].count_status << ",\"cursor_status\":" << v[t].cursor << '}';
  }
  std::cout << ']';
}
std::string numa_snapshot() {
  std::ifstream input("/proc/self/numa_maps");
  require(bool(input), "cannot read numa_maps");
  std::ostringstream out; out << input.rdbuf();
  require(!input.bad(), "numa_maps read failed");
  return out.str();
}
int run(const Options &o) {
  Store db(o);
  std::vector<Worker> workers(o.workers);
  Gate gate;
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
  ApiOverlap overlap;
#endif
  std::vector<std::thread> callers;
  callers.reserve(o.workers);
  uint64_t process_start = 0, process_cpu = 0, end = 0;
  rusage usage_start{}, usage_end{};
  try {
    for (int id = 0; id < o.workers; ++id) {
      workers[id].id = id;
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
      callers.emplace_back(worker, std::ref(db), std::cref(o), std::ref(workers[id]), std::ref(gate), std::ref(overlap));
#else
      callers.emplace_back(worker, std::ref(db), std::cref(o), std::ref(workers[id]), std::ref(gate));
#endif
    }
    {
      std::unique_lock<std::mutex> lock(gate.mutex);
      gate.cv.wait(lock, [&] { return gate.ready == o.workers; });
      for (const auto &w : workers) if (!w.error.empty()) gate.abort = true;
      require(!getrusage(RUSAGE_SELF, &usage_start), "initial getrusage failed");
      process_start = now(CLOCK_PROCESS_CPUTIME_ID);
      gate.start_ns = now(); gate.deadline_ns = gate.start_ns + uint64_t(o.seconds * 1e9);
      gate.release = true;
    }
    gate.cv.notify_all();
    for (auto &thread : callers) thread.join();
    end = now(); process_cpu = now(CLOCK_PROCESS_CPUTIME_ID) - process_start;
    require(!getrusage(RUSAGE_SELF, &usage_end), "final getrusage failed");
  } catch (...) {
    { std::lock_guard<std::mutex> lock(gate.mutex); gate.abort = true; gate.release = true; }
    gate.cv.notify_all();
    for (auto &thread : callers) if (thread.joinable()) thread.join();
    throw; // Store destruction happens only after joins, even partial thread creation.
  }
  bool failed = gate.abort;
  Written written{};
  for (const auto &w : workers) {
    failed = failed || !w.error.empty() || w.db_error || w.bridge_error || w.bad_buffers ||
        !w.affinity_start || !w.affinity_end;
    for (int t = 0; t < o.tables; ++t) written[t][w.id] = w.tables[t].done[1];
  }
  std::string residency, residency_error;
  try { residency = numa_snapshot(); }
  catch (const std::exception &ex) { failed = true; residency_error = ex.what(); }
  auto verification = db.verify(written);
  for (int t = 0; t < o.tables; ++t) failed = failed || !verification[t].ok();
  Shutdown shutdown = db.close();
  failed = failed || shutdown.bridge || shutdown.db || shutdown.env;
  std::cout << "{\"schema\":\"boundary-tables-v1\",\"backend\":" << quote(backend())
            << ",\"layout\":" << quote(o.layout) << ",\"tables\":" << o.tables << ",\"seed\":" << o.seed
            << ",\"failed\":" << (failed ? "true" : "false") << ",\"seconds\":" << o.seconds
            << ",\"initial_records\":" << kInitial << ",\"max_inserts_total\":" << uint64_t(o.workers) * kInsertLimitPerWorker
            << ",\"workers_count\":" << o.workers << ",\"routing\":" << quote(o.routing)
            << ",\"api_overlap_diagnostic\":"
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
            << "true"
#else
            << "false"
#endif
            << ",\"environments\":" << (o.layout == "coarse" ? 1 : o.tables)
            << ",\"warmup\":{\"kind\":\"same live workers, 8 find-only sweeps of every table before common gate\",\"inserts\":0}"
            << ",\"start_ns\":" << gate.start_ns << ",\"deadline_ns\":" << gate.deadline_ns << ",\"join_end_ns\":" << end
            << ",\"drain_ns\":" << (end > gate.deadline_ns ? end - gate.deadline_ns : 0)
            << ",\"process_cpu_ns\":" << process_cpu
            << ",\"cpu_scope\":\"common release gate through all worker joins including drain and TLS teardown; excludes setup,warmup,oracle\""
            << ",\"worker_cpu_scope\":\"post-gate loop including drained operation; excludes TLS teardown\""
            << ",\"resources\":{\"address_space_cap_bytes\":" << (32ULL << 30)
            << ",\"peak_rss_kib_setup_warmup_measurement\":" << usage_end.ru_maxrss
            << ",\"minor_faults\":" << usage_end.ru_minflt - usage_start.ru_minflt
            << ",\"major_faults\":" << usage_end.ru_majflt - usage_start.ru_majflt
            << ",\"voluntary_context_switches\":" << usage_end.ru_nvcsw - usage_start.ru_nvcsw
            << ",\"involuntary_context_switches\":" << usage_end.ru_nivcsw - usage_start.ru_nivcsw << '}'
            << ",\"memory_policy\":{\"requested\":\"bind\",\"effective\":\"bind\",\"nodes\":[1],\"init_cpu\":" << o.first_cpu() << "}"
            << ",\"numa_maps_after_join_before_oracle\":" << quote(residency)
            << ",\"numa_snapshot_error\":" << quote(residency_error) << ",\"workers\":[";
  for (int id = 0; id < o.workers; ++id) {
    const auto &w = workers[id];
    if (id) std::cout << ',';
    std::cout << "{\"id\":" << id << ",\"requested_cpu\":" << o.first_cpu() + id
              << ",\"cpu_start\":" << w.cpu_start << ",\"cpu_end\":" << w.cpu_end
              << ",\"singleton_start\":" << (w.affinity_start ? "true" : "false")
              << ",\"singleton_end\":" << (w.affinity_end ? "true" : "false")
              << ",\"cpu_ns\":" << w.cpu_ns << ",\"end_ns\":" << w.end_ns << ",\"warmup_reads\":" << w.warmup_reads
              << ",\"db_error\":" << w.db_error << ",\"bridge_error\":" << w.bridge_error
              << ",\"bad_buffers\":" << w.bad_buffers << ",\"error\":" << quote(w.error) << ",\"latency\":[";
    hist_json(w.latency[0]); std::cout << ','; hist_json(w.latency[1]); std::cout << "],\"tables\":[";
    for (int t = 0; t < o.tables; ++t) {
      const auto &p = w.tables[t];
      if (t) std::cout << ',';
      std::cout << "{\"table\":" << t << ",\"finds\":" << p.done[0] << ",\"inserts\":" << p.done[1]
                << ",\"finds_on_time\":" << p.on_time[0] << ",\"inserts_on_time\":" << p.on_time[1]
                << ",\"first_completion_offset_ns\":" << p.first_ns << ",\"last_completion_offset_ns\":" << p.last_ns << '}';
    }
    std::cout << ']';
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
    std::cout << ",\"api_overlap_counts\":[";
    for (int t = 0; t < o.tables; ++t) {
      if (t) std::cout << ',';
      std::cout << '[';
      for (int n = 1; n <= o.workers; ++n) std::cout << (n == 1 ? "" : ",") << w.overlap[t][n];
      std::cout << ']';
    }
    std::cout << ']';
#endif
    std::cout << '}';
  }
  std::cout << "],\"verification\":"; verification_json(verification, o.tables);
  std::cout << ",\"shutdown\":{\"bridge_status\":" << shutdown.bridge << ",\"db_status\":" << shutdown.db
            << ",\"env_status\":" << shutdown.env << "}}\n";
  return failed ? 2 : 0;
}
int error_cases(const Options &o) {
  Store db(o);
  Written written{};
  std::string error;
  uint64_t checks = 0;
  // Dedicated joined caller avoids leaving main-thread USCL TLS registered at destroy.
  std::thread caller([&] {
    try {
      pin(o.first_cpu());
      for (int table = 0; table < o.tables; ++table) {
        Guarded key, value;
        key.bytes = kInitial / o.tables + 1; value.bytes = kCanary;
        uint16_t key_size = 8; uint32_t size = 8; int32_t bridge = DLOCK_BRIDGE_INVALID;
        int status = db.operation(table, false, key.bytes, value.bytes, key_size, size, bridge);
        require(status == UPS_KEY_NOT_FOUND && bridge == DLOCK_BRIDGE_OK && key.intact() && value.intact() &&
            key.bytes == kInitial / o.tables + 1 && value.bytes == kCanary, "missing status/canary failed"); ++checks;
        key.bytes = insert_key(0, o.seed, table); value.bytes = value_for(key.bytes, o.seed, table);
        status = db.operation(table, true, key.bytes, value.bytes, key_size, size, bridge);
        if (status == UPS_SUCCESS) written[table][0] = 1;
        require(status == UPS_SUCCESS && bridge == DLOCK_BRIDGE_OK && key.intact() && value.intact() &&
            key.bytes == insert_key(0, o.seed, table) && value.bytes == value_for(key.bytes, o.seed, table),
            "insert status/canary failed"); ++checks;
        status = db.operation(table, true, key.bytes, value.bytes, key_size, size, bridge);
        require(status == UPS_DUPLICATE_KEY && bridge == DLOCK_BRIDGE_OK && key.intact() && value.intact() &&
            key.bytes == insert_key(0, o.seed, table) && value.bytes == value_for(key.bytes, o.seed, table),
            "duplicate status/canary failed"); ++checks;
        checked_read(db, o, table, key.bytes); ++checks;
      }
    } catch (const std::exception &ex) { error = ex.what(); }
    catch (...) { error = "unexpected error-probe exception"; }
  });
  caller.join();
  auto verification = db.verify(written);
  bool failed = !error.empty();
  for (int t = 0; t < o.tables; ++t) failed = failed || !verification[t].ok();
  auto shutdown = db.close();
  failed = failed || shutdown.bridge || shutdown.db || shutdown.env;
  std::cout << "{\"schema\":\"boundary-tables-v1\",\"self_test\":\"errors\",\"backend\":" << quote(backend())
            << ",\"layout\":" << quote(o.layout) << ",\"tables\":" << o.tables << ",\"seed\":" << o.seed
            << ",\"workers_count\":" << o.workers << ",\"routing\":" << quote(o.routing)
            << ",\"api_overlap_diagnostic\":"
#ifdef UPS_API_OVERLAP_DIAGNOSTIC
            << "true"
#else
            << "false"
#endif
            << ",\"failed\":" << (failed ? "true" : "false") << ",\"checks\":" << checks
            << ",\"probe_inserts\":" << o.tables << ",\"error\":" << quote(error) << ",\"verification\":";
  verification_json(verification, o.tables);
  std::cout << ",\"shutdown\":{\"bridge_status\":" << shutdown.bridge << ",\"db_status\":" << shutdown.db
            << ",\"env_status\":" << shutdown.env << "}}\n";
  return failed ? 2 : 0;
}
} // namespace
int main(int argc, char **argv) {
  try {
    Options options = parse(argc, argv);
    resources(options);
    return options.errors ? error_cases(options) : run(options);
  } catch (const std::exception &ex) {
    std::cout << "{\"schema\":\"boundary-tables-v1\",\"failed\":true,\"fatal\":" << quote(ex.what()) << "}\n";
    return 2;
  }
}
