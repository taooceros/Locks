// SYNTHETIC open-loop arrivals on the real restricted UpScaleDB Store.
// Reuse the frozen Store, operation result checks, key bijection and exact oracle.
// No changes to lock/bridge implementations or supported database semantics.
#define main arrival_reused_native_main
#include "native_harness.cc"
#undef main

namespace {
constexpr uint64_t arrival_cap = 2000000;
constexpr uint64_t drain_limit_ns = 5000000000ULL;
constexpr uint64_t hard_grace_ns = 1000000000ULL;
const std::vector<int> arrival_cpus{40, 41, 42, 43, 44, 45, 46, 47};

std::vector<int> arrival_affinity() {
  cpu_set_t mask;
  CPU_ZERO(&mask);
  if (sched_getaffinity(0, sizeof(mask), &mask)) throw std::runtime_error("get affinity failed");
  std::vector<int> result;
  for (int cpu = 0; cpu < CPU_SETSIZE; ++cpu)
    if (CPU_ISSET(cpu, &mask)) result.push_back(cpu);
  return result;
}
void arrival_ints(const std::vector<int> &values) {
  std::cout << '[';
  for (size_t i = 0; i < values.size(); ++i) std::cout << (i ? "," : "") << values[i];
  std::cout << ']';
}
void sleep_until(uint64_t ns) {
  const timespec ts{time_t(ns / 1000000000ULL), long(ns % 1000000000ULL)};
  int status;
  do { status = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, nullptr); }
  while (status == EINTR);
  if (status) throw std::runtime_error("absolute arrival sleep failed");
}
uint64_t read_u64(std::ifstream &input) {
  unsigned char bytes[8];
  if (!input.read(reinterpret_cast<char *>(bytes), 8)) throw std::runtime_error("truncated schedule");
  uint64_t n = 0;
  for (int i = 0; i < 8; ++i) n |= uint64_t(bytes[i]) << (8 * i);
  return n;
}
struct alignas(64) ArrivalWorker {
  Worker base;
  std::vector<uint64_t> arrivals;
  // Histograms are outside callbacks. Phase 0 = wrapper return before deadline;
  // phase 1 = wrapper return during drain. Includes failed returned calls.
  Histogram scheduled[2][2], response[2][2], lag[2][2], censored;
  uint64_t submitted_before = 0, returned_before = 0, successful_before = 0;
  uint64_t returned = 0, successful = 0, stop_ns = 0;
  std::vector<int> received_affinity;
  std::string error;
  std::atomic<uint64_t> live_submitted{0}, live_returned{0}, live_successful{0};
};
using ArrivalWorkers = std::array<ArrivalWorker, 8>;
uint64_t load_schedule(const std::string &path, ArrivalWorkers &workers,
                       uint64_t seed, uint64_t window_ns, unsigned find_percent) {
  std::ifstream input(path, std::ios::binary);
  char magic[8];
  if (!input.read(magic, 8) || std::memcmp(magic, "ARRIVAL1", 8))
    throw std::runtime_error("wrong schedule magic");
  if (read_u64(input) != seed || read_u64(input) != window_ns || read_u64(input) != find_percent)
    throw std::runtime_error("schedule parameters disagree");
  std::array<uint64_t, 8> counts{};
  uint64_t total = 0;
  for (auto &n : counts) {
    n = read_u64(input);
    if (n > arrival_cap || total > arrival_cap - n) throw std::runtime_error("arrival cap exceeded");
    total += n;
  }
  for (size_t id = 0; id < workers.size(); ++id) {
    auto &events = workers[id].arrivals;
    events.resize(counts[id]);
    uint64_t previous = 0;
    for (auto &offset : events) {
      offset = read_u64(input);
      if (offset < previous || offset >= window_ns) throw std::runtime_error("invalid arrival offset");
      previous = offset;
    }
  }
  if (input.peek() != std::char_traits<char>::eof()) throw std::runtime_error("schedule trailing data");
  return total;
}
bool arrival_bad(const Worker &w) {
  return w.first_error || w.missing || w.duplicate || w.other_error || w.bad_size ||
         w.bad_value || w.bridge_error || w.affinity_error;
}
void arrival_worker(Store &db, const Options &o, ArrivalWorker &a, Gate &gate,
                    bool calibration, unsigned find_percent) noexcept {
  auto &w = a.base;
  cpu_set_t mask;
  CPU_ZERO(&mask);
  // Workers inherit only the initializer CPU after parse(). This pin is bounded
  // by the original process mask, checked before parse, not a broadening of it.
  CPU_SET(w.cpu_requested, &mask);
  w.affinity_error = pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask);
  try {
    a.received_affinity = arrival_affinity();
    if (a.received_affinity != std::vector<int>{w.cpu_requested}) w.affinity_error = EINVAL;
  } catch (...) { w.affinity_error = EINVAL; }
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    ++gate.ready;
    gate.cv.notify_all();
    gate.cv.wait(lock, [&] { return gate.release; });
    if (gate.abort || w.affinity_error) { w.finished.store(true, std::memory_order_release); return; }
  }
  try {
    const uint64_t cpu_start = clock_ns(CLOCK_THREAD_CPUTIME_ID);
    const uint64_t deadline = gate.start_ns + uint64_t(o.run_seconds * 1e9);
    const uint64_t stop_submit = calibration ? deadline : deadline + drain_limit_ns;
    w.cpu = sched_getcpu();
    for (uint64_t index = 0; calibration || index < a.arrivals.size(); ++index) {
      // Arrival times exist before release even while this worker is blocked.
      // Calibration alone runs continuously, with the same mixed operation path.
      const uint64_t scheduled = calibration ? clock_ns() : gate.start_ns + a.arrivals[index];
      if (!calibration && clock_ns() < scheduled) sleep_until(scheduled);
      if (clock_ns() >= stop_submit) break;
      const bool insertion = mix(o.seed ^ mix(index * 8 + w.id) ^ 0xc6bc279692b5cc83ULL) % 100 >= find_percent;
      if (insertion && w.writes >= o.max_inserts / 8) {
        a.error = "calibration insert guard reached";
        break;
      }
      const uint64_t key = insertion ? insert_key(uint64_t(w.id) + w.writes * 8, o.seed)
                                     : read_key(o, index * 8 + w.id);
      const uint64_t value = expected(key, o.seed);
      const uint64_t submit = clock_ns();
      if (submit >= stop_submit) break;
      a.live_submitted.store(index + 1, std::memory_order_release);
      if (submit <= deadline) ++a.submitted_before;
      operation(db, o, w, key, value, insertion, deadline);
      const uint64_t returned = clock_ns();
      const unsigned phase = returned <= deadline ? 0 : 1;
      a.scheduled[phase][insertion].add(returned - scheduled);
      a.response[phase][insertion].add(returned - submit);
      a.lag[phase][insertion].add(submit - scheduled);
      ++a.returned;
      if (!phase) ++a.returned_before;
      if (!arrival_bad(w)) {
        ++a.successful;
        if (!phase) ++a.successful_before;
      }
      a.live_returned.store(a.returned, std::memory_order_release);
      a.live_successful.store(a.successful, std::memory_order_release);
      if (arrival_bad(w)) break; // the inserted set stays a deterministic prefix per requester
    }
    a.stop_ns = clock_ns();
    w.cpu_time_ns = clock_ns(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
    w.cpu_end = sched_getcpu();
    if (arrival_affinity() != a.received_affinity || w.cpu != w.cpu_requested ||
        w.cpu_end != w.cpu_requested) w.affinity_error = EINVAL;
  } catch (const std::exception &ex) { a.error = ex.what(); }
  w.finished.store(true, std::memory_order_release);
}
void watchdog_exit(const ArrivalWorkers &workers, uint64_t generated, uint64_t start,
                   uint64_t process_cpu_start, bool calibration) {
  // Never destroy the Store or inspect non-atomic worker state with callbacks
  // in flight. A process exit is the only safe bound on an opaque stuck call.
  std::cout << "{\"schema\":\"boundary-arrival-v1\",\"status\":\"hard_timeout\","
               "\"verification\":null,\"histograms_available\":false,\"generated\":";
  if (calibration) std::cout << "null"; else std::cout << generated;
  std::cout << ",\"snapshot_semantics\":\"nontransactional atomic progress sampled after hard drain bound;"
               " not exact deadline counts; all missing scheduled outcomes retained\",\"workers\":[";
  for (size_t id = 0; id < workers.size(); ++id) {
    const auto &a = workers[id];
    const uint64_t success = a.live_successful.load(std::memory_order_acquire);
    const uint64_t returned = a.live_returned.load(std::memory_order_acquire);
    const uint64_t submitted = a.live_submitted.load(std::memory_order_acquire);
    std::cout << (id ? "," : "") << "{\"id\":" << id << ",\"submitted\":" << submitted
              << ",\"returned\":" << returned << ",\"successful\":" << success
              << ",\"inflight\":" << submitted - returned;
    if (!calibration) std::cout << ",\"unsubmitted\":" << a.arrivals.size() - submitted;
    std::cout << '}';
  }
  std::cout << "],\"observed_elapsed_ns\":" << clock_ns() - start
            << ",\"process_cpu_ns\":" << clock_ns(CLOCK_PROCESS_CPUTIME_ID) - process_cpu_start
            << "}\n" << std::flush;
  std::_Exit(124);
}
void arrival_histograms(const ArrivalWorker &a) {
  const char *names[] = {"scheduled_to_return_ns", "submit_to_return_ns", "submit_lag_ns"};
  for (int metric = 0; metric < 3; ++metric) {
    std::cout << ",\"" << names[metric] << "\":{\"before_deadline\":[";
    for (int phase = 0; phase < 2; ++phase) {
      if (phase) std::cout << "],\"drain\":[";
      for (int op = 0; op < 2; ++op) {
        if (op) std::cout << ',';
        hist_json(metric == 0 ? a.scheduled[phase][op] : metric == 1 ? a.response[phase][op] : a.lag[phase][op]);
      }
    }
    std::cout << "]}";
  }
  std::cout << ",\"unreturned_censor_age_ns\":";
  hist_json(a.censored);
}
} // namespace

int main(int argc, char **argv) {
  try {
    const auto inherited = arrival_affinity();
    std::string schedule_path, mode = "open", smoke;
    unsigned find_percent = 95;
    std::vector<std::string> args{argv[0], "--cpus", "40,41,42,43,44,45,46,47",
        "--mode", "duration", "--seconds", "2", "--warmup", "0", "--init-cpu", "40",
        "--init-node", "1", "--memory-policy", "bind", "--memory-nodes", "1", "--memory-limit-gib", "32"};
    for (int i = 1; i < argc; ++i) {
      const std::string arg = argv[i];
      if (arg == "--arrival-mode" || arg == "--schedule" || arg == "--find-percent" || arg == "--self-test") {
        if (++i == argc) throw std::runtime_error("missing value for " + arg);
        if (arg == "--arrival-mode") mode = argv[i];
        else if (arg == "--schedule") schedule_path = argv[i];
        else if (arg == "--self-test") smoke = argv[i];
        else { const uint64_t value = number(argv[i], "--find-percent");
          if (value != 95 && value != 50) throw std::runtime_error("mix must be 95/5 or 50/50");
          find_percent = unsigned(value); }
      } else args.push_back(arg);
    }
    std::vector<char *> pointers;
    for (auto &arg : args) pointers.push_back(&arg[0]);
    Options o = parse(int(pointers.size()), pointers.data());
    if (o.cpus != arrival_cpus || o.init_cpu != 40 || o.init_node != 1 ||
        o.requested_memory_policy != "bind" || o.memory_nodes != std::vector<int>{1} ||
        o.memory_limit_gib != 32 || o.warmup != 0 || o.preload != 100000 ||
        o.max_inserts != 200000000 || !o.duration ||
        (o.run_seconds != 2.0 && o.run_seconds != 0.2) || (mode != "open" && mode != "calibration"))
      throw std::runtime_error("fixed arrival configuration violated");
    const MemoryPlacement placement = setup_memory("bind", {1}, true);
    if (!smoke.empty()) {
      if (smoke != "errors") throw std::runtime_error("unsupported canary");
      return self_test(smoke);
    }
    const bool calibration = mode == "calibration";
    // verify() uses inserters only to decode the eight interleaved insert prefixes.
    // We do not call the native role-based worker constructor or its JSON printer.
    o.finders = 0;
    o.inserters = 8;
    ArrivalWorkers workers;
    const uint64_t window_ns = uint64_t(o.run_seconds * 1e9);
    const uint64_t generated = calibration ? 0 : load_schedule(schedule_path, workers, o.seed, window_ns, find_percent);
    Store db(o);
    const auto setup_residency = resident_pages();
    Gate gate;
    std::vector<std::thread> threads;
    threads.reserve(8);
    auto abort_join = [&] {
      { std::lock_guard<std::mutex> lock(gate.mutex); gate.abort = true; gate.release = true; }
      gate.cv.notify_all();
      for (auto &t : threads) t.join();
    };
    try {
      for (size_t id = 0; id < workers.size(); ++id) {
        auto &w = workers[id].base;
        w.id = int(id); w.cpu_requested = arrival_cpus[id]; w.finder = w.writer = true;
        threads.emplace_back(arrival_worker, std::ref(db), std::cref(o), std::ref(workers[id]),
                             std::ref(gate), calibration, find_percent);
      }
    } catch (...) { abort_join(); throw; }
    uint64_t process_start = 0, start = 0;
    const auto usage_start = process_usage();
    {
      std::unique_lock<std::mutex> lock(gate.mutex);
      gate.cv.wait(lock, [&] { return gate.ready == 8; });
      for (const auto &a : workers) if (a.base.affinity_error) gate.abort = true;
      process_start = clock_ns(CLOCK_PROCESS_CPUTIME_ID);
      start = gate.start_ns = clock_ns();
      gate.release = true;
    }
    gate.cv.notify_all();
    uint64_t window_cpu = 0, window_observed_ns = 0;
    while (true) {
      const uint64_t now = clock_ns();
      if (!window_observed_ns && now >= start + window_ns) {
        window_cpu = clock_ns(CLOCK_PROCESS_CPUTIME_ID) - process_start;
        window_observed_ns = now - start;
      }
      bool finished = true;
      for (const auto &a : workers) finished &= a.base.finished.load(std::memory_order_acquire);
      if (finished && window_observed_ns) break;
      if (now >= start + window_ns + drain_limit_ns + hard_grace_ns)
        watchdog_exit(workers, generated, start, process_start, calibration);
      sleep_until(now + 1000000); // observer only, never a worker vacation
    }
    for (auto &thread : threads) thread.join(); // includes requester TLS destruction
    const uint64_t end = clock_ns();
    const uint64_t process_cpu = clock_ns(CLOCK_PROCESS_CPUTIME_ID) - process_start;
    const auto usage = usage_delta(usage_start, process_usage());
    const auto work_residency = resident_pages();
    bool failed = gate.abort;
    uint64_t submitted = 0, returned = 0, successful = 0, submitted_before = 0;
    uint64_t returned_before = 0, successful_before = 0, last_return = start;
    std::vector<uint64_t> prefixes;
    for (auto &a : workers) {
      const auto &w = a.base;
      const auto count = a.live_submitted.load();
      submitted += count; returned += a.returned; successful += a.successful;
      submitted_before += a.submitted_before; returned_before += a.returned_before;
      successful_before += a.successful_before;
      last_return = std::max(last_return, a.stop_ns);
      prefixes.push_back(w.writes);
      failed |= arrival_bad(w) || !a.error.empty();
      if (!calibration) for (size_t index = a.returned; index < a.arrivals.size(); ++index)
        a.censored.add(end - start - a.arrivals[index]);
    }
    const auto v = db.verify(o, prefixes, true);
    failed |= v.integrity || v.count_status || v.cursor_status || v.bad ||
              v.seen != v.expected_count || v.db_count != v.expected_count;
    const auto shutdown = db.close();
    failed |= shutdown[0] || shutdown[1];
    const char *status = failed ? "failed" : !calibration && successful != generated ? "incomplete" : "ok";
    std::cout << "{\"schema\":\"boundary-arrival-v1\",\"experiment\":\"synthetic_poisson_upscaledb\",\"status\":\""
              << status << "\",\"mode\":\"" << (calibration ? "closed_loop_calibration" : "synthetic_open_loop")
              << "\",\"find_percent\":" << find_percent << ",\"seed\":" << o.seed
              << ",\"window_ns\":" << window_ns << ",\"drain_limit_ns\":" << drain_limit_ns
              << ",\"hard_grace_ns\":" << hard_grace_ns << ",\"generated\":";
    if (calibration) std::cout << "null"; else std::cout << generated;
    std::cout << ",\"submitted\":" << submitted << ",\"returned\":" << returned
              << ",\"successful\":" << successful << ",\"submitted_before_deadline\":" << submitted_before
              << ",\"returned_before_deadline\":" << returned_before
              << ",\"successful_before_deadline\":" << successful_before
              << ",\"successful_during_drain\":" << successful - successful_before
              << ",\"failed_returns\":" << returned - successful;
    if (!calibration) std::cout << ",\"unsubmitted_at_deadline\":" << generated - submitted_before
              << ",\"inflight_at_deadline\":" << submitted_before - returned_before
              << ",\"backlog_at_deadline\":" << generated - returned_before
              << ",\"unsatisfied_at_deadline\":" << generated - successful_before
              << ",\"unsubmitted_final\":" << generated - submitted
              << ",\"unsatisfied_final\":" << generated - successful;
    std::cout << ",\"inflight_final\":" << submitted - returned
              << ",\"worker_drain_ns\":" << (last_return > start + window_ns ? last_return - start - window_ns : 0)
              << ",\"join_drain_ns\":" << end - start - window_ns
              << ",\"process_cpu_ns\":" << process_cpu
              << ",\"window_process_cpu_ns\":" << window_cpu
              << ",\"window_cpu_observation_ns\":" << window_observed_ns
              << ",\"cpu_scope\":\"release-through-joins excluding oracle; window sample may overshoot, offset recorded\""
              << ",\"peak_rss_kib\":" << process_usage().ru_maxrss
              << ",\"voluntary_context_switches\":" << usage.ru_nvcsw
              << ",\"involuntary_context_switches\":" << usage.ru_nivcsw
              << ",\"inherited_affinity\":";
    arrival_ints(inherited);
    std::cout << ",\"memory_policy\":\"" << placement.effective << "\",\"memory_nodes\":";
    arrival_ints(placement.effective_nodes);
    std::cout << ",\"residency_before\":"; residency_json(setup_residency);
    std::cout << ",\"residency_after\":"; residency_json(work_residency);
    std::cout << ",\"verification\":{\"expected_count\":" << v.expected_count
              << ",\"seen\":" << v.seen << ",\"db_count\":" << v.db_count << ",\"bad_entries\":" << v.bad
              << ",\"integrity_status\":" << v.integrity << ",\"count_status\":" << v.count_status
              << ",\"cursor_status\":" << v.cursor_status << "},\"shutdown\":[" << shutdown[0] << ',' << shutdown[1]
              << "],\"workers\":[";
    for (size_t id = 0; id < workers.size(); ++id) {
      const auto &a = workers[id]; const auto &w = a.base;
      std::cout << (id ? "," : "") << "{\"id\":" << id << ",\"cpu_requested\":" << w.cpu_requested
                << ",\"cpu_start\":" << w.cpu << ",\"cpu_end\":" << w.cpu_end
                << ",\"affinity_error\":" << w.affinity_error << ",\"received_affinity\":";
      arrival_ints(a.received_affinity);
      std::cout << ",\"cpu_time_ns\":" << w.cpu_time_ns << ",\"generated\":";
      if (calibration) std::cout << "null"; else std::cout << a.arrivals.size();
      std::cout << ",\"submitted\":" << a.live_submitted.load() << ",\"returned\":" << a.returned
                << ",\"successful\":" << a.successful << ",\"successful_before_deadline\":" << a.successful_before
                << ",\"submitted_before_deadline\":" << a.submitted_before << ",\"returned_before_deadline\":" << a.returned_before
                << ",\"successful_by_operation\":[" << w.done[0] << ',' << w.done[1]
                << "],\"insert_prefix\":" << w.writes << ",\"first_db_status\":" << w.first_error
                << ",\"bridge_status\":" << w.bridge_error << ",\"bad_value\":" << w.bad_value
                << ",\"bad_size\":" << w.bad_size << ",\"error\":";
      json_string(a.error.c_str());
      arrival_histograms(a);
      std::cout << '}';
    }
    std::cout << "]}\n";
    return failed ? 1 : 0; // bounded incomplete drain is an observed outcome, not an omitted trial
  } catch (const std::exception &ex) {
    std::cout << "{\"schema\":\"boundary-arrival-v1\",\"status\":\"failed\",\"error\":";
    json_string(ex.what()); std::cout << "}\n";
    return 2;
  }
}
