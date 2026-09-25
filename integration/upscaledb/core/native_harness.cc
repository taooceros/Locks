// Restricted in-memory UpScaleDB experiment; one private Store owns the gate.
#include <stddef.h> // upscaledb_int.h uses size_t without including its prerequisite.
#include <ups/upscaledb.h>
#include <ups/upscaledb_int.h>
#include "bridge.h"
#if defined(UPS_REFACTORED) || defined(UPS_BRIDGE_KIND)
#include "private_ops.h"
#endif

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cctype>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <chrono>
#include <functional>
#include <cstring>
#include <exception>
#include <iomanip>
#include <iostream>
#include <limits>
#include <memory>
#include <mutex>
#include <sched.h>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <time.h>
#include <vector>
#include <sys/resource.h>
#include <pthread.h>
#include <fstream>
#include <linux/mempolicy.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifdef UPS_NATIVE_PROFILE
extern "C" void ups_native_lock_sample(uint64_t *, uint64_t *, uint32_t *);
#endif

namespace {
uint64_t clock_ns(clockid_t clock = CLOCK_MONOTONIC) {
  timespec t;
  if (clock_gettime(clock, &t)) throw std::runtime_error("clock_gettime failed");
  return uint64_t(t.tv_sec) * 1000000000ULL + t.tv_nsec;
}
rusage process_usage() {
  rusage value{};
  if (getrusage(RUSAGE_SELF, &value)) throw std::runtime_error("getrusage(RUSAGE_SELF) failed");
  return value;
}
rusage usage_delta(const rusage &before, const rusage &after) {
  rusage delta{};
  delta.ru_nvcsw = after.ru_nvcsw - before.ru_nvcsw;
  delta.ru_nivcsw = after.ru_nivcsw - before.ru_nivcsw;
  delta.ru_minflt = after.ru_minflt - before.ru_minflt;
  delta.ru_majflt = after.ru_majflt - before.ru_majflt;
  delta.ru_inblock = after.ru_inblock - before.ru_inblock;
  delta.ru_oublock = after.ru_oublock - before.ru_oublock;
  return delta;
}
uint64_t mix(uint64_t x) {
  x += 0x9e3779b97f4a7c15ULL;
  x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
  x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
  return x ^ (x >> 31);
}
uint64_t expected(uint64_t key, uint64_t seed) { return mix(key ^ seed ^ 0xa531ea3dd5bd7121ULL); }
constexpr uint64_t kInsertHighBit = 1ULL << 63;
constexpr uint64_t kInsertMask = kInsertHighBit - 1;
constexpr uint64_t kInsertMultiplier = 0x5851f42d4c957f2dULL;
constexpr uint64_t insert_inverse() {
  uint64_t inverse = 1;
  // Odd a is invertible modulo 2^64. Each Newton step doubles the
  // correct low bits: 1 -> 2 -> 4 -> 8 -> 16 -> 32 -> 64 (six steps).
  for (int step = 0; step < 6; ++step)
    inverse *= 2 - kInsertMultiplier * inverse;
  return inverse;
}
constexpr uint64_t kInsertInverse = insert_inverse();
uint64_t insert_key(uint64_t ordinal, uint64_t seed) {
  return kInsertHighBit | ((ordinal * kInsertMultiplier + (mix(seed) & kInsertMask)) & kInsertMask);
}
uint64_t insert_ordinal(uint64_t key, uint64_t seed) {
  return (((key & kInsertMask) - (mix(seed) & kInsertMask)) * kInsertInverse) & kInsertMask;
}
// Reduction modulo 2^63 preserves the inverse because 2^63 divides 2^64.
// Thus decode(encode(n)) == n for every n < 2^63, in particular for all
// allowed n < 200 million. The high bit excludes the preload domain, so
// distinct ordinals yield distinct inserted keys without count collisions.
void check(ups_status_t st, const char *where) {
  if (st) throw std::runtime_error(std::string(where) + ": " + ups_strerror(st) + " (" + std::to_string(st) + ")");
}
uint64_t number(const char *s, const char *option) {
  if (!*s) throw std::runtime_error(std::string("invalid ") + option);
  for (const char *p = s; *p; ++p)
    if (*p < '0' || *p > '9') throw std::runtime_error(std::string("invalid ") + option);
  char *end = nullptr; errno = 0;
  unsigned long long n = std::strtoull(s, &end, 10);
  if (errno || end == s || *end) throw std::runtime_error(std::string("invalid ") + option);
  return n;
}
double seconds(const char *s, const char *option) {
  char *end = nullptr; errno = 0;
  const double n = std::strtod(s, &end);
  if (errno || end == s || *end || !std::isfinite(n) || n < 0)
    throw std::runtime_error(std::string("invalid ") + option);
  return n;
}
std::vector<int> cpu_list(const std::string &s) {
  std::vector<int> result;
  std::istringstream input(s);
  std::string token;
  while (std::getline(input, token, ',')) {
    const uint64_t n = number(token.c_str(), "--cpus");
    if (n >= CPU_SETSIZE || std::find(result.begin(), result.end(), int(n)) != result.end())
      throw std::runtime_error("invalid or repeated CPU in --cpus");
    result.push_back(int(n));
  }
  if (result.empty() || s.back() == ',') throw std::runtime_error("--cpus needs a CPU list");
  return result;
}
std::vector<int> node_list(const std::string &s) {
  std::vector<int> nodes;
  std::istringstream input(s);
  std::string token;
  while (std::getline(input, token, ',')) {
    const uint64_t n = number(token.c_str(), "--memory-nodes");
    if (n >= 1024 || std::find(nodes.begin(), nodes.end(), int(n)) != nodes.end())
      throw std::runtime_error("invalid or repeated node in --memory-nodes");
    nodes.push_back(int(n));
  }
  if (nodes.empty() || s.back() == ',') throw std::runtime_error("--memory-nodes needs a node list");
  return nodes;
}
bool listed_cpu(const std::string &text, int cpu) {
  std::istringstream input(text);
  std::string token;
  while (std::getline(input, token, ',')) {
    const size_t dash = token.find('-');
    const uint64_t first = number(token.substr(0, dash).c_str(), "node cpulist");
    const uint64_t last = dash == std::string::npos ? first :
                          number(token.substr(dash + 1).c_str(), "node cpulist");
    if (first > last) throw std::runtime_error("invalid node cpulist range");
    if (first <= unsigned(cpu) && unsigned(cpu) <= last) return true;
  }
  return false;
}
struct MemoryPlacement {
  std::string requested = "first-touch", effective = "not_queried";
  std::vector<int> nodes, effective_nodes;
  bool explicit_policy = false;
};
MemoryPlacement setup_memory(const std::string &policy, const std::vector<int> &nodes, bool explicit_policy) {
  MemoryPlacement placement{policy, "not_queried", nodes, {}, explicit_policy};
  if (!explicit_policy) return placement; // legacy runs retain their inherited policy.
  const int requested = policy == "first-touch" ? MPOL_DEFAULT :
                        policy == "bind" ? MPOL_BIND : MPOL_INTERLEAVE;
  const size_t bits = sizeof(unsigned long) * 8;
  // Linux get_nodes() imports maxnode-1 bits: preserve the highest selected bit,
  // including the one-bit node-0 mask (maxnode=1 would import an empty mask).
  const unsigned long maxnode = nodes.empty() ? 0 : static_cast<unsigned long>(nodes.back() + 2);
  std::vector<unsigned long> mask((maxnode + bits - 1) / bits, 0);
  for (int node : nodes) mask[node / bits] |= 1UL << (node % bits);
  if (syscall(SYS_set_mempolicy, requested, mask.empty() ? nullptr : mask.data(), maxnode))
    throw std::runtime_error("set_mempolicy(" + policy + ") failed: " + std::strerror(errno));
  std::vector<unsigned long> actual((1024 + bits - 1) / bits, 0);
  int mode = -1;
  if (syscall(SYS_get_mempolicy, &mode, actual.data(), 1025UL, nullptr, 0))
    throw std::runtime_error("get_mempolicy failed: " + std::string(std::strerror(errno)));
  if (mode != requested) throw std::runtime_error("effective NUMA policy differs from request");
  for (int node = 0; node < 1024; ++node) {
    if (actual[node / bits] & (1UL << (node % bits)))
      placement.effective_nodes.push_back(node);
  }
  if (placement.effective_nodes != nodes)
    throw std::runtime_error("effective NUMA nodemask differs from request");
  placement.effective = policy;
  return placement;
}
struct Residency {
  std::array<uint64_t, 1024> pages{};
  uint64_t mapped_regions = 0, resident_pages = 0;
};
Residency resident_pages() {
  std::ifstream input("/proc/self/numa_maps");
  if (!input) throw std::runtime_error("cannot inspect /proc/self/numa_maps");
  Residency result;
  std::string line, token;
  while (std::getline(input, line)) {
    ++result.mapped_regions;
    std::istringstream words(line);
    while (words >> token) {
      if (token.size() < 4 || token[0] != 'N') continue;
      size_t eq = token.find('=');
      if (eq == std::string::npos) continue;
      const std::string index = token.substr(1, eq - 1);
      if (index.empty() || !std::all_of(index.begin(), index.end(), ::isdigit)) continue;
      const uint64_t node = number(index.c_str(), "numa_maps node");
      if (node >= result.pages.size()) throw std::runtime_error("numa_maps node exceeds 1023");
      const uint64_t count = number(token.c_str() + eq + 1, "numa_maps pages");
      result.pages[node] += count;
      result.resident_pages += count;
    }
  }
  if (input.bad() || !result.mapped_regions) throw std::runtime_error("numa_maps snapshot failed");
  return result;
}
void residency_json(const Residency &value) {
  std::cout << "{\"scope\":\"whole_process_including_libraries_stacks_and_db\","
            << "\"unit\":\"kernel_pages\",\"mapped_regions\":" << value.mapped_regions
            << ",\"resident_pages\":" << value.resident_pages << ",\"nodes\":{";
  bool first = true;
  for (size_t node = 0; node < value.pages.size(); ++node) {
    if (!value.pages[node]) continue;
    std::cout << (first ? "" : ",") << '"' << node << "\":" << value.pages[node];
    first = false;
  }
  std::cout << "}}";
}
struct Options {
  bool duration = false;
  uint64_t preload = 100000, reads = 400000, inserts = 400000;
  uint64_t max_inserts = 200000000, seed = 1, wait_proxy_ns = 1000;
  uint64_t memory_limit_gib = 8;
  int finders = 4, inserters = 4;
  double run_seconds = 120, warmup = 5;
  std::vector<int> cpus;
  std::string cpu_text;
  int init_cpu = -1, init_node = -1;
  std::string requested_memory_policy = "first-touch";
  std::vector<int> memory_nodes;
  bool memory_policy_explicit = false;
};
void usage() {
  std::cout << "Usage: upscaledb-{native,profile} --cpus 0,1,2,3 [options]\n"
    "  --mode fixed|duration    fixed: exact total reads/inserts; duration: deadline\n"
    "  --finders N --inserters N  default 4+4; total 1..128 (duration dedicated roles)\n"
    "  --workers 1             single worker alternates find/insert\n"
    "  --preload N             default 100000, bounded 1..1000000\n"
    "  --reads N --inserts N   fixed totals, default 400000 each\n"
    "  --seconds N             duration seconds, default 120 (0.01..600)\n"
    "  --warmup N              disposable warmup seconds, default 5 (0..60)\n"
    "  --max-inserts N         fail (never truncate) at bound, default/max 200000000\n"
    "  --memory-limit-gib N    process address-space ceiling, default 8 GiB\n"
    "  --seed N                deterministic key/value seed, default 1\n"
    "  --wait-proxy-ns N       profile timestamp threshold, default 1000 ns\n"
    "  --init-cpu N --init-node N    initialization CPU/node; CPU must belong to node\n"
    "  --memory-policy first-touch|bind|interleave  explicit Linux process policy\n"
    "  --memory-nodes N[,N]   required for bind/interleave; forbidden for first-touch\n"
    "All times use CLOCK_MONOTONIC; one JSON trial to stdout. No durable storage.\n";
}
Options parse(int argc, char **argv) {
  Options o;
  for (int i = 1; i < argc; ++i) {
    const std::string a(argv[i]);
    if (a == "--help") { usage(); std::exit(0); }
    if (i + 1 == argc) throw std::runtime_error("missing value for " + a);
    const char *v = argv[++i];
    if (a == "--mode") {
      if (!std::strcmp(v, "duration")) o.duration = true;
      else if (!std::strcmp(v, "fixed")) o.duration = false;
      else throw std::runtime_error("--mode must be fixed or duration");
    } else if (a == "--cpus") { o.cpu_text = v; o.cpus = cpu_list(v); }
    else if (a == "--finders" || a == "--inserters") {
      const uint64_t n = number(v, a.c_str());
      if (n > 128) throw std::runtime_error(a + " out of range (0..128)");
      if (a == "--finders") o.finders = int(n); else o.inserters = int(n);
    }
    else if (a == "--workers") {
      if (number(v, a.c_str()) != 1) throw std::runtime_error("only --workers 1 supported; use --finders/--inserters otherwise");
      o.finders = 1; o.inserters = 0;
    } else if (a == "--preload") o.preload = number(v, argv[i-1]);
    else if (a == "--reads") o.reads = number(v, argv[i-1]);
    else if (a == "--inserts") o.inserts = number(v, argv[i-1]);
    else if (a == "--max-inserts") o.max_inserts = number(v, argv[i-1]);
    else if (a == "--memory-limit-gib") o.memory_limit_gib = number(v, argv[i-1]);
    else if (a == "--seed") o.seed = number(v, argv[i-1]);
    else if (a == "--wait-proxy-ns") o.wait_proxy_ns = number(v, argv[i-1]);
    else if (a == "--init-cpu" || a == "--init-node") {
      const uint64_t n = number(v, a.c_str());
      if (n >= (a == "--init-cpu" ? CPU_SETSIZE : 1024))
        throw std::runtime_error(a + " out of range");
      if (a == "--init-cpu") o.init_cpu = int(n); else o.init_node = int(n);
    } else if (a == "--memory-policy") {
      o.requested_memory_policy = v;
      o.memory_policy_explicit = true;
      if (o.requested_memory_policy != "first-touch" &&
          o.requested_memory_policy != "bind" &&
          o.requested_memory_policy != "interleave")
        throw std::runtime_error("invalid --memory-policy");
    } else if (a == "--memory-nodes") o.memory_nodes = node_list(v);
    else if (a == "--seconds") o.run_seconds = seconds(v, argv[i-1]);
    else if (a == "--warmup") o.warmup = seconds(v, argv[i-1]);
    else throw std::runtime_error("unknown option: " + a);
  }
  if (o.cpus.empty()) throw std::runtime_error("--cpus required (parent chooses eligible CPUs)");
  if (o.init_cpu < 0) o.init_cpu = o.cpus.front();
  if (o.init_node >= 0) {
    std::ifstream node_cpus("/sys/devices/system/node/node" + std::to_string(o.init_node) + "/cpulist");
    std::string text;
    if (!node_cpus || !std::getline(node_cpus, text) || !listed_cpu(text, o.init_cpu))
      throw std::runtime_error("initialization CPU is not on requested initialization node");
  }
  if (o.memory_policy_explicit &&
      ((o.requested_memory_policy == "first-touch") != o.memory_nodes.empty()))
    throw std::runtime_error("first-touch forbids nodes; bind/interleave requires nodes");
  if (!o.memory_policy_explicit && !o.memory_nodes.empty())
    throw std::runtime_error("--memory-nodes requires --memory-policy");
  std::sort(o.memory_nodes.begin(), o.memory_nodes.end());
  if (o.finders < 1 || o.finders + o.inserters > 128)
    throw std::runtime_error("workers must total 1..128 with at least one finder");
  if (o.inserters == 0 && o.finders != 1) throw std::runtime_error("zero inserters only valid for --workers 1");
  if (o.preload < 1 || o.preload > 1000000 || o.max_inserts < 1 || o.max_inserts > 200000000 ||
      o.inserts > o.max_inserts || o.reads > 2000000 || o.memory_limit_gib < 1 || o.memory_limit_gib > 64)
    throw std::runtime_error("dataset exceeds configured or hard memory cap");
  if (o.duration && (o.run_seconds < 0.01 || o.run_seconds > 600)) throw std::runtime_error("--seconds out of range");
  if (o.warmup > 60) throw std::runtime_error("--warmup out of range");
  if (o.preload + o.max_inserts < o.preload) throw std::runtime_error("key range overflow");
  cpu_set_t allowed;
  CPU_ZERO(&allowed);
  if (sched_getaffinity(0, sizeof(allowed), &allowed)) throw std::runtime_error("sched_getaffinity failed");
  cpu_set_t selected;
  CPU_ZERO(&selected);
  for (int cpu : o.cpus) {
    if (!CPU_ISSET(cpu, &allowed)) throw std::runtime_error("requested CPU is not in parent affinity mask: " + std::to_string(cpu));
  }
  // Parent/setup/first-touch stays on the selected initialization CPU.
  CPU_SET(o.init_cpu, &selected);
  if (!CPU_ISSET(o.init_cpu, &allowed) ||
      sched_setaffinity(0, sizeof(selected), &selected))
    throw std::runtime_error("setup CPU affinity failed or CPU not allowed");
  rlimit limits;
  if (getrlimit(RLIMIT_AS, &limits)) throw std::runtime_error("getrlimit(RLIMIT_AS) failed");
  const uint64_t ceiling = o.memory_limit_gib * (1ULL << 30);
  if (limits.rlim_max != RLIM_INFINITY && limits.rlim_max < ceiling)
    throw std::runtime_error("requested memory cap exceeds inherited hard RLIMIT_AS");
  limits.rlim_cur = ceiling;
  if (setrlimit(RLIMIT_AS, &limits)) throw std::runtime_error("setrlimit(RLIMIT_AS) failed");
  return o;
}

// Opaque private owner: only find/insert are callable during the worker phase.
class Store {
  ups_env_t *env_ = nullptr;
  ups_db_t *db_ = nullptr;
#ifdef UPS_BRIDGE_KIND
  dlock_bridge *bridge_ = nullptr;
#endif
public:
  explicit Store(const Options &o) {
    try {
      check(ups_env_create(&env_, nullptr, UPS_IN_MEMORY, 0, nullptr), "env_create");
      const ups_parameter_t params[] = {{UPS_PARAM_KEY_TYPE, UPS_TYPE_UINT64},
          {UPS_PARAM_KEY_SIZE, 8}, {UPS_PARAM_RECORD_SIZE, 8}, {0, 0}};
      check(ups_env_create_db(env_, &db_, 1, 0, params), "env_create_db");
      for (uint64_t k = 0; k < o.preload; ++k) {
        uint64_t value = expected(k, o.seed);
        ups_key_t key = {8, &k, 0, 0};
        ups_record_t record = {8, &value, 0};
        check(ups_db_insert(db_, nullptr, &key, &record, 0), "preload insert");
      }
#ifdef UPS_BRIDGE_KIND
      // The handle exists only after preload; no setup API runs concurrently
      // with a submitting worker, and the mutex-control borrows the real Env.
      void *native_mutex = UPS_BRIDGE_KIND == DLOCK_BRIDGE_MUTEX
          ? ups_dlock_private_native_mutex(db_) : nullptr;
      if (dlock_bridge_create(UPS_BRIDGE_KIND, native_mutex, &bridge_) != DLOCK_BRIDGE_OK)
        throw std::runtime_error("bridge create failed");
#endif
    } catch (...) { close(); throw; }
  }
  Store(const Store &) = delete;
  Store &operator=(const Store &) = delete;
  ~Store() { close(); }
  ups_status_t find(uint64_t &k, uint64_t &value, uint32_t &returned_size,
                    uint16_t &returned_key_size, dlock_request_metrics *metrics = nullptr,
                    int32_t *bridge_status = nullptr) {
    ups_key_t key = {8, &k, UPS_KEY_USER_ALLOC, 0};
    ups_record_t record = {8, &value, UPS_RECORD_USER_ALLOC};
#ifdef UPS_BRIDGE_KIND
    int32_t local_bridge_status = DLOCK_BRIDGE_INVALID;
    if (!bridge_status) bridge_status = &local_bridge_status;
    const ups_status_t st = ups_dlock_private_find(db_, &key, &record, bridge_,
                                                   dlock_bridge_execute, metrics, bridge_status);
#else
    const ups_status_t st = ups_db_find(db_, nullptr, &key, &record, 0);
    if (bridge_status) *bridge_status = DLOCK_BRIDGE_OK;
#endif
    returned_size = record.size;
    returned_key_size = key.size;
    if (!st && (key.data != &k || record.data != &value))
      return UPS_INTERNAL_ERROR; // never expose DB-owned output storage
    return st;
  }
  ups_status_t insert(uint64_t &k, uint64_t &value,
                      dlock_request_metrics *metrics = nullptr,
                      int32_t *bridge_status = nullptr) {
    ups_key_t key = {8, &k, 0, 0};
    ups_record_t record = {8, &value, 0};
#ifdef UPS_BRIDGE_KIND
    int32_t local_bridge_status = DLOCK_BRIDGE_INVALID;
    if (!bridge_status) bridge_status = &local_bridge_status;
    return ups_dlock_private_insert(db_, &key, &record, bridge_,
                                    dlock_bridge_execute, metrics, bridge_status);
#else
    if (bridge_status) *bridge_status = DLOCK_BRIDGE_OK;
    return ups_db_insert(db_, nullptr, &key, &record, 0);
#endif
  }
#ifdef UPS_BRIDGE_KIND
  dlock_thread_metrics thread_metrics() {
    dlock_thread_metrics m{};
    if (dlock_bridge_get_thread_metrics(bridge_, &m) != DLOCK_BRIDGE_OK)
      throw std::runtime_error("bridge thread metrics failed");
    return m;
  }
  dlock_global_metrics global_metrics() {
    dlock_global_metrics m{};
    if (dlock_bridge_get_global_metrics(bridge_, &m) != DLOCK_BRIDGE_OK)
      throw std::runtime_error("bridge global metrics failed");
    return m;
  }
#ifdef DLOCK_BRIDGE_TEST_HOOKS
  dlock_bridge *test_bridge() { return bridge_; }
#endif
#endif
  // Correctness-only probe; uses the SAME guarded production dispatch as find
  // and insert, with caller-owned buffers and deliberate malformed inputs.
  ups_status_t probe(bool insertion, ups_key_t *key, ups_record_t *record,
                     int32_t *bridge_status) {
#ifdef UPS_BRIDGE_KIND
    return insertion ? ups_dlock_private_insert(db_, key, record, bridge_,
                                                  dlock_bridge_execute, nullptr, bridge_status)
                     : ups_dlock_private_find(db_, key, record, bridge_,
                                                dlock_bridge_execute, nullptr, bridge_status);
#else
    *bridge_status = DLOCK_BRIDGE_OK;
    return insertion ? ups_db_insert(db_, nullptr, key, record, 0)
                     : ups_db_find(db_, nullptr, key, record, 0);
#endif
  }
  struct Verification { uint64_t seen = 0, bad = 0, expected_count = 0, db_count = 0; int integrity = 0, cursor_status = 0, count_status = 0; };
  Verification verify(const Options &o, const std::vector<uint64_t> &written, bool duration) {
    Verification v;
    v.expected_count = o.preload;
    for (auto n : written) v.expected_count += n;
    v.integrity = ups_db_check_integrity(db_, 0);
    v.count_status = ups_db_count(db_, nullptr, 0, &v.db_count);
    ups_cursor_t *cursor = nullptr;
    v.cursor_status = ups_cursor_create(&cursor, db_, nullptr, 0);
    if (v.cursor_status) return v;
    uint64_t previous = 0;
    bool first = true;
    while (true) {
      uint64_t key_bytes = 0, record_bytes = 0;
      ups_key_t key = {8, &key_bytes, UPS_KEY_USER_ALLOC, 0};
      ups_record_t rec = {8, &record_bytes, UPS_RECORD_USER_ALLOC};
      int st = ups_cursor_move(cursor, &key, &rec, first ? UPS_CURSOR_FIRST : UPS_CURSOR_NEXT);
      if (st == UPS_KEY_NOT_FOUND) break;
      if (st) { v.cursor_status = st; break; }
      bool valid = key.size == 8 && rec.size == 8;
      // Preload is [0, preload); only high-bit affine insert keys are valid otherwise.
      if (valid && key_bytes >= o.preload) {
        if (!(key_bytes & kInsertHighBit)) valid = false;
        else {
          const uint64_t decoded = insert_ordinal(key_bytes, o.seed);
          if (!duration) valid = decoded < o.inserts;
          else {
            const uint64_t writers = o.inserters ? o.inserters : 1;
            const uint64_t ordinal = decoded / writers;
            const uint64_t role = decoded % writers;
            valid = role < written.size() && ordinal < written[role];
          }
        }
      }
      if (!first && key_bytes <= previous) valid = false;
      if (record_bytes != expected(key_bytes, o.seed)) valid = false;
      if (!valid) ++v.bad;
      previous = key_bytes;
      first = false;
      ++v.seen;
      if (v.seen > o.preload + o.max_inserts) { ++v.bad; break; }
    }
    int close_st = ups_cursor_close(cursor);
    if (!v.cursor_status) v.cursor_status = close_st;
    // Membership + monotonic uniqueness + exact count imply complete expected set.
    return v;
  }
  std::array<int, 2> close() noexcept {
    int db_status = 0, env_status = 0;
#ifdef UPS_BRIDGE_KIND
    if (bridge_) {
      if (dlock_bridge_destroy(bridge_) != DLOCK_BRIDGE_OK)
        std::terminate(); // workers have joined; never close a DB reachable by a callback
      bridge_ = nullptr;
    }
#endif
    if (db_) { db_status = ups_db_close(db_, 0); if (!db_status) db_ = nullptr; }
    if (env_) { env_status = ups_env_close(env_, db_ ? UPS_AUTO_CLEANUP : 0); env_ = nullptr; db_ = nullptr; }
    return {{db_status, env_status}};
  }
};

struct Histogram {
  std::array<uint64_t, 64> bins{};
  uint64_t samples = 0, sum_ns = 0, min_ns = 0, max_ns = 0;
  void add(uint64_t ns) {
    const int bin = ns ? 64 - __builtin_clzll(ns) : 0;
    ++bins[std::min(bin, 63)];
    if (!samples || ns < min_ns) min_ns = ns;
    if (!samples || ns > max_ns) max_ns = ns;
    ++samples;
    sum_ns += ns;
  }
};
struct Op { uint64_t key, value; bool insert; };
struct alignas(64) Worker {
  int id = 0, cpu_requested = -1, cpu = -1, cpu_end = -1;
  bool finder = false, writer = false;
  uint64_t read_slot = 0, write_slot = 0, read_stride = 1, write_stride = 1;
  std::vector<Op> trace;
  Histogram latency[2], wait[2], hold[2];
  uint64_t write_quota = 0; // fixed mode only; no eligibility after this quota
  uint64_t attempted[2] = {0, 0}, done[2] = {0, 0}, before_deadline[2] = {0, 0}, late[2] = {0, 0};
  Histogram requester_service[2];
  uint64_t service_tsc_ticks[2] = {0, 0};
  uint64_t service_before_deadline_ns[2] = {0, 0};
  uint64_t service_before_deadline_tsc_ticks[2] = {0, 0};
  dlock_thread_metrics executor{};
  uint64_t missing = 0, duplicate = 0, other_error = 0, bad_value = 0, bad_size = 0;
  uint64_t proxy[2] = {0, 0}, profile_missing = 0, max_writer_gap_ns = 0;
  uint64_t cpu_time_ns = 0, writes = 0, first_ns = 0, last_ns = 0;
  uint64_t max_gap_start_ns = 0, max_gap_end_ns = 0;
  int first_error = 0, affinity_error = 0, bridge_error = 0;
  std::atomic<uint64_t> public_done[2], last_write_ns, quota_done_ns;
  std::atomic<bool> finished{false};
  Worker() : public_done{}, last_write_ns(0), quota_done_ns(0) {}
};
struct Gate {
  std::mutex mutex;
  std::condition_variable cv;
  int ready = 0;
  bool release = false, abort = false;
  uint64_t start_ns = 0;
};
struct Progress {
  uint64_t offset_ns, completed_finds, completed_inserts;
  std::vector<uint64_t> worker_finds, worker_inserts, writer_since_last_ns;
};
struct Run {
  std::vector<std::unique_ptr<Worker>> workers;
  std::vector<Progress> progress;
  uint64_t start_ns = 0, end_ns = 0, drain_ns = 0, deadline_ns = 0;
  uint64_t process_cpu_time_ns = 0, peak_rss_kib = 0;
  rusage phase_usage{};
  Residency work_residency{};
  Store::Verification verification;
  std::array<int, 2> shutdown{{0, 0}};
  dlock_global_metrics bridge_metrics{};
  bool failed = false;
  std::string error;
};
uint64_t read_key(const Options &o, uint64_t ordinal) { return mix(o.seed ^ mix(ordinal)) % o.preload; }
void make_workers(Run &r, const Options &o, bool duration) {
  const bool both = o.inserters == 0;
  const int total = o.finders + o.inserters;
  for (int id = 0; id < total; ++id) {
    std::unique_ptr<Worker> w(new Worker);
    w->id = id; w->cpu_requested = w->cpu = o.cpus[id % o.cpus.size()];
    w->finder = id < o.finders; w->writer = both || id >= o.finders;
    w->read_slot = id; w->write_slot = both ? 0 : uint64_t(id - o.finders);
    w->read_stride = o.finders; w->write_stride = both ? 1 : o.inserters;
    if (!duration && w->writer)
      w->write_quota = both ? o.inserts : o.inserts / o.inserters +
          (w->write_slot < o.inserts % o.inserters);
    if (!duration) {
      if (both) {
        const uint64_t count = o.reads + o.inserts;
        w->trace.reserve(count);
        uint64_t rd = 0, wr = 0;
        while (rd < o.reads || wr < o.inserts) {
          if (rd < o.reads) {
            uint64_t key = read_key(o, rd++);
            w->trace.push_back({key, expected(key, o.seed), false});
          }
          if (wr < o.inserts) {
            uint64_t key = insert_key(wr++, o.seed);
            w->trace.push_back({key, expected(key, o.seed), true});
          }
        }
      } else if (w->finder) {
        const uint64_t n = o.reads / o.finders + (uint64_t(id) < o.reads % o.finders);
        w->trace.reserve(n);
        for (uint64_t k = id; k < o.reads; k += o.finders) {
          uint64_t key = read_key(o, k);
          w->trace.push_back({key, expected(key, o.seed), false});
        }
      } else {
        const uint64_t slot = w->write_slot;
        const uint64_t n = o.inserts / o.inserters + (slot < o.inserts % o.inserters);
        w->trace.reserve(n);
        for (uint64_t k = slot; k < o.inserts; k += o.inserters) {
          uint64_t key = insert_key(k, o.seed);
          w->trace.push_back({key, expected(key, o.seed), true});
        }
      }
    }
    r.workers.push_back(std::move(w));
  }
}
void operation(Store &db, const Options &o, Worker &w, uint64_t key_input,
               uint64_t expected_value, bool insert, uint64_t deadline) {
  const int type = insert ? 1 : 0;
  uint64_t key = key_input, value = insert ? expected_value : 0;
  uint32_t size = 8;
  uint16_t key_size = 8;
  ++w.attempted[type];
  const uint64_t before = clock_ns();
#ifdef UPS_BRIDGE_PROFILE
  dlock_request_metrics service{};
  dlock_request_metrics *service_output = &service;
#else
  dlock_request_metrics *service_output = nullptr;
#endif
  int32_t bridge_status = DLOCK_BRIDGE_OK;
  const int st = insert ? db.insert(key, value, service_output, &bridge_status)
                        : db.find(key, value, size, key_size, service_output, &bridge_status);
  const uint64_t after = clock_ns();
  if (bridge_status != DLOCK_BRIDGE_OK) w.bridge_error = bridge_status;
#ifdef UPS_BRIDGE_PROFILE
  if (service.enabled) {
    w.requester_service[type].add(service.service_ns);
    w.service_tsc_ticks[type] += service.service_tsc_ticks;
    if (after <= deadline) {
      w.service_before_deadline_ns[type] += service.service_ns;
      w.service_before_deadline_tsc_ticks[type] += service.service_tsc_ticks;
    }
  }
#endif
  w.latency[type].add(after - before);
#ifdef UPS_NATIVE_PROFILE
  uint64_t wait_ns = 0, hold_ns = 0; uint32_t sampled_op = 0;
  ups_native_lock_sample(&wait_ns, &hold_ns, &sampled_op);
  if (sampled_op == uint32_t(type + 1)) {
    w.wait[type].add(wait_ns); w.hold[type].add(hold_ns);
    w.proxy[type] += wait_ns > o.wait_proxy_ns;
  } else ++w.profile_missing;
#endif
  if (st) {
    if (!w.first_error) w.first_error = st;
    if (!insert && st == UPS_KEY_NOT_FOUND) ++w.missing;
    else if (insert && st == UPS_DUPLICATE_KEY) ++w.duplicate;
    else ++w.other_error;
    return;
  }
  if (!insert && (size != 8 || key_size != 8 || key != key_input)) ++w.bad_size;
  if (!insert && value != expected_value) ++w.bad_value;
  if (insert && key != key_input) ++w.bad_value;
  ++w.done[type];
  if (after <= deadline) ++w.before_deadline[type]; else ++w.late[type];
  if (insert) {
    ++w.writes;
    if (w.last_ns) {
      const uint64_t capped_after = std::min(after, deadline);
      const uint64_t capped_previous = std::min(w.last_ns, deadline);
      if (capped_after - capped_previous > w.max_writer_gap_ns) {
        w.max_writer_gap_ns = capped_after - capped_previous;
        w.max_gap_start_ns = capped_previous; w.max_gap_end_ns = capped_after;
      }
    } else w.first_ns = after;
    w.last_ns = after;
    w.last_write_ns.store(after, std::memory_order_relaxed);
    if (w.write_quota && w.writes == w.write_quota)
      w.quota_done_ns.store(after, std::memory_order_release);
  }
  w.public_done[type].store(w.done[type], std::memory_order_relaxed);
}
void worker_thread(Store &db, const Options &o, Worker &w, Gate &gate,
                   bool duration, double run_seconds) noexcept {
  cpu_set_t mask; CPU_ZERO(&mask); CPU_SET(w.cpu, &mask);
  w.affinity_error = pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask);
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    ++gate.ready; gate.cv.notify_all();
    gate.cv.wait(lock, [&gate] { return gate.release; });
    if (gate.abort || w.affinity_error) { w.finished.store(true, std::memory_order_release); return; }
  }
  // noexcept fail-stops unexpected exceptions; the native DB state is unknown.
  try {
    w.cpu = sched_getcpu();
    const uint64_t cpu_start = clock_ns(CLOCK_THREAD_CPUTIME_ID);
    const uint64_t deadline = duration ? gate.start_ns + uint64_t(run_seconds * 1e9)
                                       : std::numeric_limits<uint64_t>::max();
    if (duration) {
      uint64_t read_ordinal = w.read_slot, write_ordinal = 0;
      bool next_insert = false;
      while (clock_ns() < deadline) {
        const bool insert = w.writer && (!w.finder || next_insert);
        if (w.finder && w.writer) next_insert = !next_insert;
        if (insert) {
          if (w.writes >= o.max_inserts / w.write_stride +
              (w.write_slot < o.max_inserts % w.write_stride)) {
            ++w.other_error; w.first_error = UPS_LIMITS_REACHED; break;
          }
          uint64_t key = insert_key(w.write_slot + write_ordinal * w.write_stride, o.seed);
          operation(db, o, w, key, expected(key, o.seed), true, deadline);
          ++write_ordinal;
        } else {
          uint64_t key = read_key(o, read_ordinal);
          operation(db, o, w, key, expected(key, o.seed), false, deadline);
          read_ordinal += w.read_stride;
        }
        if (w.first_error || w.bad_size || w.bad_value) break;
      }
    } else {
      for (const Op &op : w.trace) {
        operation(db, o, w, op.key, op.value, op.insert, deadline);
        if (w.first_error || w.bad_size || w.bad_value) break;
      }
    }
#ifdef UPS_BRIDGE_KIND
    w.executor = db.thread_metrics(); // physical executor, not requester
#endif
    w.cpu_time_ns = clock_ns(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
    w.cpu_end = sched_getcpu();
  } catch (...) { std::terminate(); }
  w.finished.store(true, std::memory_order_release);
}
Progress snapshot(const Run &r, uint64_t now) {
  Progress s{};
  s.offset_ns = now - r.start_ns;
  for (const auto &p : r.workers) {
    uint64_t finds = p->public_done[0].load(std::memory_order_relaxed);
    uint64_t inserts = p->public_done[1].load(std::memory_order_relaxed);
    s.completed_finds += finds; s.completed_inserts += inserts;
    s.worker_finds.push_back(finds); s.worker_inserts.push_back(inserts);
    const uint64_t exhausted = p->quota_done_ns.load(std::memory_order_acquire);
    const uint64_t last = p->last_write_ns.load(std::memory_order_relaxed);
    const uint64_t window_end = r.deadline_ns ? std::min(now, r.deadline_ns)
        : p->write_quota == 0 ? r.start_ns : exhausted ? std::min(now, exhausted) : now;
    const uint64_t reference = last ? last : r.start_ns;
    s.writer_since_last_ns.push_back(p->writer && window_end >= reference
                                         ? window_end - reference : 0);
  }
  return s;
}
Run trial(Store &db, const Options &o, bool duration, double run_seconds) {
  Run r;
  make_workers(r, o, duration); // all fixed inputs allocated before timing
  Gate gate;
  std::vector<std::thread> threads;
  threads.reserve(r.workers.size());
  try {
    for (auto &w : r.workers)
      threads.emplace_back(worker_thread, std::ref(db), std::cref(o), std::ref(*w),
                           std::ref(gate), duration, run_seconds);
  } catch (...) {
    { std::lock_guard<std::mutex> lock(gate.mutex); gate.abort = true; gate.release = true; }
    gate.cv.notify_all();
    for (auto &t : threads) t.join();
    throw;
  }
  rusage usage_start{};
  uint64_t process_cpu_start = 0;
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    gate.cv.wait(lock, [&] { return gate.ready == int(threads.size()); });
    for (const auto &w : r.workers) if (w->affinity_error) {
      gate.abort = true; r.failed = true; r.error = "worker CPU affinity failed";
    }
    try {
      usage_start = process_usage();
      process_cpu_start = clock_ns(CLOCK_PROCESS_CPUTIME_ID);
      r.start_ns = clock_ns();
      r.deadline_ns = duration ? r.start_ns + uint64_t(run_seconds * 1e9) : 0;
      gate.start_ns = r.start_ns;
    } catch (...) {
      gate.abort = true; gate.release = true;
      lock.unlock(); gate.cv.notify_all();
      for (auto &t : threads) t.join();
      throw;
    }
    gate.release = true;
  }
  gate.cv.notify_all();
  // Observer never touches handles; only worker-published atomics. Join timing
  // is taken immediately after workers finish, not after an observer sleep.
  std::atomic<bool> stop_observer{false};
  std::atomic<bool> observer_error{false};
  std::thread observer;
  try {
    observer = std::thread([&] {
      try {
        for (uint64_t tick = r.start_ns + 1000000000ULL;; tick += 1000000000ULL) {
          timespec ts{time_t(tick / 1000000000ULL), long(tick % 1000000000ULL)};
          clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, nullptr);
          if (stop_observer.load(std::memory_order_acquire)) break;
          if (r.progress.size() < 7200) r.progress.push_back(snapshot(r, clock_ns()));
        }
      } catch (...) { observer_error.store(true, std::memory_order_release); }
    });
  } catch (...) {
    for (auto &t : threads) t.join();
    throw;
  }
  for (auto &t : threads) t.join();
  r.end_ns = clock_ns();
  r.process_cpu_time_ns = clock_ns(CLOCK_PROCESS_CPUTIME_ID) - process_cpu_start;
  r.phase_usage = usage_delta(usage_start, process_usage());
  stop_observer.store(true, std::memory_order_release);
  observer.join();
  if (observer_error.load(std::memory_order_acquire)) r.failed = true;
  r.drain_ns = duration && r.end_ns > r.deadline_ns ? r.end_ns - r.deadline_ns : 0;
  r.progress.push_back(snapshot(r, r.end_ns));
  // Workers are joined and end time/resource deltas frozen. Inspect pages
  // before quiescent DB verification touches every key (which may migrate pages).
  try { r.work_residency = resident_pages(); }
  catch (const std::exception &ex) {
    r.failed = true;
    r.error = ex.what(); // still run the full integrity oracle below
  }
#ifdef UPS_BRIDGE_KIND
  r.bridge_metrics = db.global_metrics(); // after all submitters joined
#endif
  std::vector<uint64_t> written(o.inserters ? o.inserters : 1, 0);
  for (auto &w : r.workers) {
    if (w->writer) written[w->write_slot] = w->writes;
    if (w->first_error || w->missing || w->duplicate || w->other_error || w->bad_size || w->bad_value ||
        w->profile_missing || w->affinity_error || w->bridge_error) r.failed = true;
  }
  r.verification = db.verify(o, written, duration);
  if (r.verification.integrity || r.verification.count_status || r.verification.cursor_status ||
      r.verification.bad || r.verification.seen != r.verification.expected_count ||
      r.verification.db_count != r.verification.expected_count) r.failed = true;
  return r;
}
void array_json(const std::array<uint64_t, 64> &bins) {
  std::cout << '[';
  for (size_t i = 0; i < bins.size(); ++i) std::cout << (i ? "," : "") << bins[i];
  std::cout << ']';
}
void hist_json(const Histogram &h) {
  std::cout << "{\"samples\":" << h.samples << ",\"sum_ns\":" << h.sum_ns
    << ",\"min_ns\":";
  if (h.samples) std::cout << h.min_ns; else std::cout << "null";
  std::cout << ",\"max_ns\":";
  if (h.samples) std::cout << h.max_ns; else std::cout << "null";
  std::cout << ",\"bins\":";
  array_json(h.bins);
  std::cout << ",\"quantile_min_samples\":{\"p50\":200,\"p95\":2000,\"p99\":10000,\"p99_9\":100000}"
            << ",\"quantile_upper_bound_ns\":{";
  const int ps[] = {50, 95, 99, 999}, minimum[] = {200, 2000, 10000, 100000};
  const char *names[] = {"p50", "p95", "p99", "p99_9"};
  for (int p = 0; p < 4; ++p) {
    if (p) std::cout << ',';
    std::cout << '"' << names[p] << "\":";
    if (h.samples < unsigned(minimum[p])) { std::cout << "null"; continue; }
    const uint64_t rank = (h.samples * ps[p] + (p == 3 ? 999 : 99)) / (p == 3 ? 1000 : 100);
    uint64_t cumulative = 0;
    for (size_t bin = 0; bin < 64; ++bin) {
      cumulative += h.bins[bin];
      if (cumulative >= rank) {
        if (bin == 63) std::cout << "null"; // overflow bin, no finite bound
        else std::cout << (bin == 0 ? 0 : 1ULL << bin);
        break;
      }
    }
  }
  std::cout << "}}";
}
void worker_json(const Worker &w, const Run &r) {
  uint64_t longest = w.max_writer_gap_ns, gap_start = w.max_gap_start_ns, gap_end = w.max_gap_end_ns;
  if (w.writer) {
    const uint64_t exhausted = w.quota_done_ns.load(std::memory_order_acquire);
    const uint64_t finish = r.deadline_ns ? r.deadline_ns :
        w.write_quota == 0 ? r.start_ns : exhausted ? exhausted : r.end_ns;
    const uint64_t initial_end = w.first_ns ? std::min(w.first_ns, finish) : finish;
    if (initial_end - r.start_ns > longest) {
      longest = initial_end - r.start_ns; gap_start = r.start_ns; gap_end = initial_end;
    }
    const uint64_t last = w.last_ns ? std::min(w.last_ns, finish) : r.start_ns;
    if (finish - last > longest) {
      longest = finish - last; gap_start = last; gap_end = finish;
    }
  }
  std::cout << "{\"id\":" << w.id << ",\"cpu_requested\":" << w.cpu_requested
    << ",\"cpu_start\":" << w.cpu << ",\"cpu_end\":" << w.cpu_end
    << ",\"finder\":" << (w.finder ? "true" : "false") << ",\"inserter\":" << (w.writer ? "true" : "false")
    << ",\"cpu_time_ns\":" << w.cpu_time_ns << ",\"attempted\":[" << w.attempted[0] << ',' << w.attempted[1]
    << "],\"completed\":[" << w.done[0] << ',' << w.done[1] << "],\"completed_before_deadline\":["
    << w.before_deadline[0] << ',' << w.before_deadline[1] << "],\"completed_during_drain\":["
    << w.late[0] << ',' << w.late[1] << "],\"missing_reads\":" << w.missing << ",\"duplicate_inserts\":" << w.duplicate
    << ",\"other_errors\":" << w.other_error << ",\"bad_value\":" << w.bad_value
    << ",\"bad_size\":" << w.bad_size << ",\"first_db_status\":" << w.first_error
    << ",\"affinity_error\":" << w.affinity_error << ",\"longest_writer_no_progress_ns\":" << longest
    << ",\"longest_writer_no_progress_start_offset_ns\":" << (gap_start ? gap_start - r.start_ns : 0)
    << ",\"longest_writer_no_progress_end_offset_ns\":" << (gap_end ? gap_end - r.start_ns : 0)
    << ",\"last_write_offset_ns\":" << (w.last_ns ? w.last_ns - r.start_ns : 0)
    << ",\"latency_ns\":{";
  for (int op = 0; op < 2; ++op) { if (op) std::cout << ','; std::cout << '"' << (op ? "insert" : "find") << "\":"; hist_json(w.latency[op]); }
  std::cout << "},\"native_mutex_profile\":{";
  for (int op = 0; op < 2; ++op) {
    if (op) std::cout << ',';
    std::cout << '"' << (op ? "insert" : "find") << "\":{\"wait_ns\":";
    hist_json(w.wait[op]); std::cout << ",\"hold_pre_release_ns\":"; hist_json(w.hold[op]);
    std::cout << ",\"wait_over_threshold_proxy\":" << w.proxy[op] << '}';
  }
  std::cout << "},\"missing_profile_samples\":" << w.profile_missing
    << ",\"bridge_status\":" << w.bridge_error
    << ",\"writer_eligible_end_offset_ns\":"
    << (w.writer ? (r.deadline_ns ? r.deadline_ns : w.write_quota == 0 ? r.start_ns :
        w.quota_done_ns.load(std::memory_order_acquire) ?
        w.quota_done_ns.load(std::memory_order_acquire) : r.end_ns) - r.start_ns : 0)
    << ",\"bridge_profile\":{\"enabled\":" << (w.executor.enabled ? "true" : "false")
#ifdef UPS_BRIDGE_KIND
    << ",\"requested_calls\":[" << w.attempted[0] << ',' << w.attempted[1]
#else
    << ",\"requested_calls\":[0,0"
#endif
    << "],\"service_ns\":{\"find\":";
  hist_json(w.requester_service[0]);
  std::cout << ",\"insert\":";
  hist_json(w.requester_service[1]);
  std::cout << "},\"service_tsc_ticks\":[" << w.service_tsc_ticks[0] << ','
    << w.service_tsc_ticks[1] << "],\"service_before_deadline_ns\":["
    << w.service_before_deadline_ns[0] << ',' << w.service_before_deadline_ns[1]
    << "],\"service_before_deadline_tsc_ticks\":["
    << w.service_before_deadline_tsc_ticks[0] << ','
    << w.service_before_deadline_tsc_ticks[1] << "],\"executor\":{\"enabled\":"
    << (w.executor.enabled ? "true" : "false")
    << ",\"executed_callbacks\":" << w.executor.executed_callbacks
    << ",\"executed_service_tsc_ticks\":" << w.executor.executed_service_tsc_ticks
    << ",\"executed_service_ns\":" << w.executor.executed_service_ns
    << ",\"combiner_pass_tsc_ticks\":" << w.executor.combiner_pass_tsc_ticks
    << ",\"has_combiner_pass_ticks\":"
    << (w.executor.has_combiner_pass_ticks ? "true" : "false") << "}}}";
}
void print_json(const Options &o, const Run &r, const std::string &warmup_status,
                const MemoryPlacement &placement, const Residency &setup_residency,
                const Residency &work_residency) {
  std::cout << std::setprecision(17);
  uint64_t attempted[2] = {0, 0}, completed[2] = {0, 0};
  uint64_t on_time[2] = {0, 0}, drained[2] = {0, 0};
  uint64_t missing = 0, duplicate = 0, other = 0, bad_value = 0, bad_size = 0;
  for (const auto &w : r.workers) {
    for (int type = 0; type < 2; ++type) {
      attempted[type] += w->attempted[type]; completed[type] += w->done[type];
      on_time[type] += w->before_deadline[type]; drained[type] += w->late[type];
    }
    missing += w->missing; duplicate += w->duplicate; other += w->other_error;
    bad_value += w->bad_value; bad_size += w->bad_size;
  }
  std::cout << "{\"schema\":1,\"variant\":\""
#if defined(UPS_BRIDGE_KIND)
#if UPS_BRIDGE_KIND == 0
#ifdef UPS_BRIDGE_PROFILE
    "bridge_mutex_profile"
#else
    "bridge_mutex"
#endif
#elif UPS_BRIDGE_KIND == 1
#ifdef UPS_BRIDGE_PROFILE
    "fc_profile"
#else
    "fc"
#endif
#elif UPS_BRIDGE_KIND == 2
#ifdef DLOCK_BRIDGE_TEST_HOOKS
    "test_hooks"
#elif defined(UPS_BRIDGE_PROFILE)
    "fc_pq_profile"
#else
    "fc_pq"
#endif
#elif UPS_BRIDGE_KIND == 3
#ifdef UPS_BRIDGE_PROFILE
    "uscl_profile"
#else
    "uscl"
#endif
#elif UPS_BRIDGE_KIND == 4
#ifdef UPS_BRIDGE_PROFILE
    "cfl_local_profile"
#else
    "cfl_local"
#endif
#elif UPS_BRIDGE_KIND == 5
#ifdef UPS_BRIDGE_PROFILE
    "spinlock_profile"
#else
    "spinlock"
#endif
#elif UPS_BRIDGE_KIND == 6
#ifdef UPS_BRIDGE_PROFILE
    "mcs_profile"
#else
    "mcs"
#endif
#elif UPS_BRIDGE_KIND == 7
#ifdef UPS_BRIDGE_PROFILE
    "ticket_profile"
#else
    "ticket"
#endif
#elif UPS_BRIDGE_KIND == 8
#ifdef UPS_BRIDGE_PROFILE
    "clh_profile"
#else
    "clh"
#endif
#else
#error "unknown UPS_BRIDGE_KIND"
#endif
#elif defined(UPS_REFACTORED)
    "refactored"
#elif defined(UPS_NATIVE_PROFILE)
    "profile"
#else
    "native"
#endif
    "\",\"status\":\"" << (r.failed ? "failed" : "ok") << "\",\"mode\":\""
    << (o.duration ? "duration" : "fixed") << "\""
#ifdef UPS_BRIDGE_KIND
    << ",\"bridge_admission\":\"external\""
#endif
    << ",\"clock\":\"CLOCK_MONOTONIC\",\"seed\":" << o.seed
    << ",\"upstream_sha\":\"cb124e1f91601872a7b3bd4da10e5fa97a8da86b\""
    << ",\"environment\":\"UPS_IN_MEMORY\",\"db_flags\":0,\"key_type\":\"UPS_TYPE_UINT64\""
    << ",\"key_bytes\":8,\"record_bytes\":8,\"transactions\":false,\"recovery\":false"
    << ",\"duplicates\":false,\"compression\":false,\"direct_access\":false"
    << ",\"record_value_formula\":\"splitmix64(key_xor_seed_xor_a531ea3dd5bd7121)\""
    << ",\"insert_key_formula\":\"0x8000000000000000|((ordinal*0x5851f42d4c957f2d+(splitmix64(seed)&0x7fffffffffffffff))&0x7fffffffffffffff)\""
    << ",\"profile_clock\":\"std::chrono::steady_clock\",\"cpu_list\":[";
  for (size_t i = 0; i < o.cpus.size(); ++i) std::cout << (i ? "," : "") << o.cpus[i];
  std::cout << "],\"preload\":" << o.preload << ",\"fixed_requested\":{\"find\":" << o.reads
    << ",\"insert\":" << o.inserts << "},\"finders\":" << o.finders << ",\"inserters\":" << o.inserters
    << ",\"max_inserts\":" << o.max_inserts << ",\"memory_limit_gib\":" << o.memory_limit_gib
    << ",\"setup_cpu\":" << o.init_cpu << ",\"init_node\":" << o.init_node
    << ",\"warmup_seconds\":" << o.warmup
    << ",\"warmup_status\":\"" << warmup_status << "\",\"duration_requested_seconds\":" << o.run_seconds
    << ",\"elapsed_ns\":" << r.end_ns - r.start_ns << ",\"deadline_offset_ns\":"
    << (r.deadline_ns ? r.deadline_ns - r.start_ns : 0) << ",\"drain_ns\":" << r.drain_ns
    << ",\"wait_proxy_threshold_ns\":" << o.wait_proxy_ns
    << ",\"profile_contention_label\":\"wait_over_threshold_is_timestamp_proxy_not_exact_contention\""
    << ",\"histogram_label\":\"log2_ns_upper_bounds;bin0=zero;bin63=overflow;empty_min_max=null;profile_hold_excludes_unlock\""
    << ",\"memory_policy\":\"unique_insert_cap_is_not_memory_guarantee;RLIMIT_AS_GiB="
    << o.memory_limit_gib << ";limit_hit_invalidates_trial_never_truncates\""
    << ",\"numa_memory\":{\"requested\":\"" << placement.requested
    << "\",\"effective\":\"" << placement.effective
    << "\",\"explicit\":" << (placement.explicit_policy ? "true" : "false")
    << ",\"nodes\":[";
  for (size_t i = 0; i < placement.nodes.size(); ++i)
    std::cout << (i ? "," : "") << placement.nodes[i];
  std::cout << "],\"effective_nodes\":[";
  for (size_t i = 0; i < placement.effective_nodes.size(); ++i)
    std::cout << (i ? "," : "") << placement.effective_nodes[i];
  std::cout << "],\"after_setup\":";
  residency_json(setup_residency);
  std::cout << ",\"after_work\":";
  residency_json(work_residency);
  std::cout << "}"
    << ",\"process_resource_scope\":\"RUSAGE_SELF_and_CLOCK_PROCESS_CPUTIME_ID_worker_phase_including_observer_and_parent;peak_rss_process_lifetime_including_warmup\""
    << ",\"process_cpu_time_ns\":" << r.process_cpu_time_ns
    << ",\"phase_rusage\":{\"involuntary_context_switches\":" << r.phase_usage.ru_nivcsw
    << ",\"voluntary_context_switches\":" << r.phase_usage.ru_nvcsw
    << ",\"minor_faults\":" << r.phase_usage.ru_minflt
    << ",\"major_faults\":" << r.phase_usage.ru_majflt
    << ",\"block_input\":" << r.phase_usage.ru_inblock
    << ",\"block_output\":" << r.phase_usage.ru_oublock
    << "},\"peak_rss_process_lifetime_kib\":" << r.peak_rss_kib
    << ",\"totals\":{\"attempted\":[" << attempted[0] << ',' << attempted[1]
    << "],\"completed\":[" << completed[0] << ',' << completed[1]
    << "],\"completed_before_deadline\":[" << on_time[0] << ',' << on_time[1]
    << "],\"completed_during_drain\":[" << drained[0] << ',' << drained[1]
    << "],\"missing_reads\":" << missing << ",\"duplicate_inserts\":" << duplicate
    << ",\"other_errors\":" << other << ",\"bad_value\":" << bad_value
    << ",\"bad_size\":" << bad_size << "}"
    << ",\"verification\":{\"expected_count\":" << r.verification.expected_count
    << ",\"seen\":" << r.verification.seen << ",\"db_count\":" << r.verification.db_count
    << ",\"bad_entries\":" << r.verification.bad << ",\"integrity_status\":" << r.verification.integrity
    << ",\"cursor_status\":" << r.verification.cursor_status << ",\"count_status\":" << r.verification.count_status
    << "},\"shutdown\":{\"db_status\":" << r.shutdown[0] << ",\"env_status\":" << r.shutdown[1]
    << "},\"bridge_profile\":{\"enabled\":" << (r.bridge_metrics.enabled ? "true" : "false")
    << ",\"accepted_calls\":" << r.bridge_metrics.accepted_calls
    << ",\"completed_calls\":" << r.bridge_metrics.completed_calls
    << ",\"rejected_reentrant\":" << r.bridge_metrics.rejected_reentrant
    << ",\"peak_inflight_calls\":" << r.bridge_metrics.peak_inflight_calls
    << ",\"active_calls\":" << r.bridge_metrics.active_calls
    << "},\"bridge_profile_semantics\":\"requester callback service_ns is Rust Instant elapsed ns; "
       "service_tsc_ticks and combiner_pass_tsc_ticks are elapsed TSC ticks including preemption, "
       "not CPU cycles or CPU time; physical executor fields describe executing workers; "
       "global in-flight counters are profile-only instrumentation, not lifecycle admission; "
       "histograms include post-deadline work; disabled counters are unmeasured\""
    << ",\"workers\":[";
  for (size_t i = 0; i < r.workers.size(); ++i) { if (i) std::cout << ','; worker_json(*r.workers[i], r); }
  std::cout << "],\"progress\":[";
  for (size_t i = 0; i < r.progress.size(); ++i) {
    if (i) std::cout << ',';
    const Progress &p = r.progress[i];
    std::cout << "{\"offset_ns\":" << p.offset_ns << ",\"find\":" << p.completed_finds
      << ",\"insert\":" << p.completed_inserts << ",\"worker_find\":[";
    for (size_t j = 0; j < p.worker_finds.size(); ++j) std::cout << (j ? "," : "") << p.worker_finds[j];
    std::cout << "],\"worker_insert\":[";
    for (size_t j = 0; j < p.worker_inserts.size(); ++j) std::cout << (j ? "," : "") << p.worker_inserts[j];
    std::cout << "],\"writer_since_last_ns\":[";
    for (size_t j = 0; j < p.writer_since_last_ns.size(); ++j) std::cout << (j ? "," : "") << p.writer_since_last_ns[j];
    std::cout << "]}";
  }
  std::cout << "]}\n";
}
void expect_case(bool condition, const char *message) {
  if (!condition) throw std::runtime_error(std::string("self-test: ") + message);
}

// Deliberately uses the production Store methods and internal dispatch, never
// a copied/unlocked DB implementation. No test hooks run in primary builds.
int self_test(const std::string &which) {
  if (which == "memory-policy") {
    // Exercise the real syscall boundary, not only Python placement parsing.
    setup_memory("bind", {0}, true);
    setup_memory("interleave", {0}, true);
    setup_memory("first-touch", {}, true);
    std::cout << "{\"self_test\":\"memory-policy\",\"status\":\"ok\"}\n";
    return 0;
  }
  Options o;
  o.preload = 64;
  o.warmup = 0;
  Store db(o);
  if (which == "errors") {
    struct Guarded { uint64_t bytes, canary; };
    constexpr uint64_t sentinel = 0xacc0ffee12345678ULL;
    Guarded key_bytes{UINT64_MAX - 1, sentinel}, value_bytes{sentinel, sentinel};
    ups_key_t key = {8, &key_bytes.bytes, UPS_KEY_USER_ALLOC, 0};
    ups_record_t record = {8, &value_bytes.bytes, UPS_RECORD_USER_ALLOC};
    int32_t bridge_status = -1;
    expect_case(db.probe(false, &key, &record, &bridge_status) == UPS_KEY_NOT_FOUND &&
                    bridge_status == DLOCK_BRIDGE_OK, "missing-key status");
    expect_case(key_bytes.canary == sentinel && value_bytes.canary == sentinel &&
                    value_bytes.bytes == sentinel, "missing-key output canaries");

    key_bytes.bytes = insert_key(0, o.seed);
    value_bytes.bytes = expected(key_bytes.bytes, o.seed);
    key = {8, &key_bytes.bytes, 0, 0};
    record = {8, &value_bytes.bytes, 0};
    expect_case(db.probe(true, &key, &record, &bridge_status) == UPS_SUCCESS &&
                    bridge_status == DLOCK_BRIDGE_OK, "first insertion");
    expect_case(db.probe(true, &key, &record, &bridge_status) == UPS_DUPLICATE_KEY &&
                    bridge_status == DLOCK_BRIDGE_OK, "duplicate-key status");
    key.size = 7;
    expect_case(db.probe(true, &key, &record, &bridge_status) == UPS_INV_KEY_SIZE &&
                    bridge_status == DLOCK_BRIDGE_OK, "bad key size status");
    key.size = 8;
    record.size = 7; // input only: never offer a short output buffer
    expect_case(db.probe(true, &key, &record, &bridge_status) == UPS_INV_RECORD_SIZE &&
                    bridge_status == DLOCK_BRIDGE_OK, "bad record size status");
    expect_case(key_bytes.canary == sentinel && value_bytes.canary == sentinel,
                "insert buffer canaries");
    uint64_t found = 0; uint32_t size = 0; uint16_t key_size = 0;
    expect_case(db.find(key_bytes.bytes, found, size, key_size) == UPS_SUCCESS &&
                    size == 8 && key_size == 8 &&
                    found == expected(key_bytes.bytes, o.seed), "caller-owned find output");
    const auto v = db.verify(o, {1, 0, 0, 0}, true);
    expect_case(!v.integrity && !v.count_status && !v.cursor_status &&
                    !v.bad && v.seen == o.preload + 1 && v.db_count == v.seen,
                "exact final key set and integrity");
  } else if (which == "stress") {
    // Two waves force >64 concurrent callers and publication-node churn.
    o.inserters = 128;
    for (int wave = 0; wave != 2; ++wave) {
      std::atomic<int> ready{0};
      std::atomic<bool> go{false};
      std::atomic<int> bad{0};
      std::vector<std::thread> threads;
      threads.reserve(128);
      try {
      for (int id = 0; id != 128; ++id) {
        threads.emplace_back([&, id, wave] {
          ready.fetch_add(1, std::memory_order_release);
          while (!go.load(std::memory_order_acquire)) std::this_thread::yield();
          for (int n = 0; n != 16; ++n) {
            uint64_t k = uint64_t((id + n) % o.preload), v = 0;
            uint32_t size = 0; uint16_t ksize = 0;
            if (db.find(k, v, size, ksize) || size != 8 || ksize != 8 ||
                v != expected(k, o.seed)) ++bad;
            k = insert_key(uint64_t(id) + uint64_t(n + wave * 16) * 128, o.seed);
            v = expected(k, o.seed);
            if (db.insert(k, v)) ++bad;
          }
        });
      }
      } catch (...) {
        go.store(true, std::memory_order_release);
        for (auto &t : threads) t.join();
        throw; // Store closes only after every started worker has exited
      }
      while (ready.load(std::memory_order_acquire) != 128) std::this_thread::yield();
      go.store(true, std::memory_order_release);
      for (auto &t : threads) t.join();
      expect_case(bad.load() == 0, ">64 callers / churn status or data");
    }
    const auto v = db.verify(o, std::vector<uint64_t>(128, 32), true);
    expect_case(!v.integrity && !v.count_status && !v.cursor_status && !v.bad &&
                    v.seen == o.preload + 128 * 32 && v.db_count == v.seen,
                "stress quiescent integrity");
  }
#ifdef DLOCK_BRIDGE_TEST_HOOKS
  else if (which == "expected-exception") {
    expect_case(ups_dlock_private_test_exception(db.test_bridge(),
                                                 dlock_bridge_execute) == UPS_KEY_NOT_FOUND,
                "expected UpScaleDB exception maps to native DB status");
  }
  else if (which == "cpp-throw") {
    ups_dlock_private_test_throw(db.test_bridge(), dlock_bridge_execute);
    throw std::runtime_error("C++ exception escaped protected callback");
  } else if (which == "rust-panic") {
    (void)dlock_bridge_test_panic(db.test_bridge());
    throw std::runtime_error("Rust panic did not fail-stop");
  } else if (which == "external-lifecycle") {
    struct Nested { Store *db; int32_t submission, destruction; };
    Nested nested{&db, -1, -1};
    auto reenter = [](void *p) {
      auto *n = static_cast<Nested *>(p);
      uint64_t k = 0, v = 0; uint32_t size = 8; uint16_t ksize = 8;
      (void)n->db->find(k, v, size, ksize, nullptr, &n->submission);
      n->destruction = dlock_bridge_destroy(n->db->test_bridge());
    };
    expect_case(dlock_bridge_execute(db.test_bridge(), reenter, &nested, nullptr) == DLOCK_BRIDGE_OK &&
                    nested.submission == DLOCK_BRIDGE_REENTRANT &&
                    nested.destruction == DLOCK_BRIDGE_REENTRANT, "external callback reentry/destroy rejection");
    ups_dlock_private_test_pause(1);
    std::atomic<int> accepted{-1};
    std::thread caller;
    try {
      caller = std::thread([&] {
        uint64_t k = 0, v = 0; uint32_t size = 0; uint16_t ksize = 0;
        int32_t status = -1;
        const int result = db.find(k, v, size, ksize, nullptr, &status);
        accepted.store(result == UPS_SUCCESS && status == DLOCK_BRIDGE_OK &&
                       size == 8 && ksize == 8 && v == expected(k, o.seed) ? 1 : 0,
                       std::memory_order_release);
      });
    } catch (...) {
      ups_dlock_private_test_pause(0);
      throw;
    }
    const auto timeout = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (!ups_dlock_private_test_entered() && std::chrono::steady_clock::now() < timeout)
      std::this_thread::yield();
    const bool entered = ups_dlock_private_test_entered() != 0;
    // The Store and bridge stay live until the worker has returned and joined.
    // No destroy while a different worker is inside the callback: join first,
    // including its USCL pthread TLS cleanup, then verify and close the Store.
    ups_dlock_private_test_pause(0);
    caller.join();
    expect_case(entered && accepted.load(std::memory_order_acquire) == 1,
                "paused DB callback finished before caller join");
    uint64_t k = 0, v = 0; uint32_t size = 0; uint16_t ksize = 0;
    int32_t status = -1;
    expect_case(db.find(k, v, size, ksize, nullptr, &status) == UPS_SUCCESS &&
                    status == DLOCK_BRIDGE_OK && size == 8 && ksize == 8 &&
                    v == expected(k, o.seed), "bridge remains usable after join");
  }
#endif
  else {
    throw std::runtime_error("unknown self-test: " + which);
  }
  auto shutdown = db.close();
  expect_case(!shutdown[0] && !shutdown[1], "quiescent bridge/DB/Env shutdown");
  std::cout << "{\"self_test\":\"" << which << "\",\"status\":\"ok\"}\n";
  return 0;
}
void json_string(const char *text) {
  std::cout << '"';
  const char *hex = "0123456789abcdef";
  for (const unsigned char *p = reinterpret_cast<const unsigned char *>(text); *p; ++p) {
    if (*p == '"' || *p == '\\') std::cout << '\\' << char(*p);
    else if (*p < 32) std::cout << "\\u00" << hex[*p >> 4] << hex[*p & 15];
    else std::cout << char(*p);
  }
  std::cout << '"';
}
} // namespace

int main(int argc, char **argv) {
  try {
    if (argc >= 3 && std::strcmp(argv[1], "--self-test") == 0) {
      if (argc != 3) throw std::runtime_error("--self-test CASE");
      return self_test(argv[2]);
    }
    Options o = parse(argc, argv);
    const MemoryPlacement placement = setup_memory(
        o.requested_memory_policy, o.memory_nodes, o.memory_policy_explicit);
    std::string warmup_status = "skipped";
    if (o.warmup > 0) {
      Store disposable(o);
      Run warm = trial(disposable, o, true, o.warmup);
      const auto closed = disposable.close();
      if (warm.failed || closed[0] || closed[1]) throw std::runtime_error("disposable warmup failed verification or shutdown");
      warmup_status = "verified_and_destroyed";
    }
    Store db(o);
    const Residency setup_residency = resident_pages();
    Run r = trial(db, o, o.duration, o.run_seconds);
    const Residency work_residency = r.work_residency;
    r.shutdown = db.close(); // only after every worker joined and verification finished
    if (r.shutdown[0] || r.shutdown[1]) r.failed = true;
    r.peak_rss_kib = uint64_t(process_usage().ru_maxrss);
    print_json(o, r, warmup_status, placement, setup_residency, work_residency);
    return r.failed ? 1 : 0;
  } catch (const std::exception &ex) {
    std::cout << "{\"schema\":1,\"status\":\"failed\",\"phase\":\"setup_or_trial\","
                 "\"error\":";
    json_string(ex.what());
    std::cout << ",\"argv\":[";
    for (int i = 1; i < argc; ++i) {
      if (i != 1) std::cout << ',';
      json_string(argv[i]);
    }
    std::cout << "]}\n";
    std::cerr << "upscaledb trial failed: " << ex.what() << '\n';
    return 2;
  }
}
