// H3 exploration, NOT the original UpScaleDB benchmark.
// Reuse dependency: native_harness.cc is compiled into this translation unit.
// Original SHA-256: 87305b0c4a7e23bfab450ea9f5f024f02918fa88c713f7007cac8995030a00b0
// Store, Options/parse/setup, operation (including result checks/timestamps),
// worker/key initialization, histograms, final exact-set/integrity oracle,
// canary self-test and exclusive close are unchanged. Only arrival scheduling
// and outside-callback measurement/lifecycle orchestration below are new.
#define main h3_reused_native_main
#include "native_harness.cc"
#undef main

namespace {
constexpr uint64_t h3_sleep_ns = 5000000;
constexpr uint64_t h3_bin_ns = 100000000;
constexpr uint64_t h3_burst = 64;

std::vector<int> h3_affinity() {
  cpu_set_t mask;
  CPU_ZERO(&mask);
  if (pthread_getaffinity_np(pthread_self(), sizeof(mask), &mask))
    throw std::runtime_error("cannot read received affinity");
  std::vector<int> result;
  for (int cpu = 0; cpu < CPU_SETSIZE; ++cpu)
    if (CPU_ISSET(cpu, &mask)) result.push_back(cpu);
  return result;
}
void h3_ints(const std::vector<int> &values) {
  std::cout << '[';
  for (size_t i = 0; i < values.size(); ++i)
    std::cout << (i ? "," : "") << values[i];
  std::cout << ']';
}
struct alignas(64) H3Schedule {
  bool bursty = false;
  std::vector<int> received_affinity;
  std::vector<std::array<uint64_t, 2>> sleep_intervals;
  std::vector<uint64_t> completion_bins;
  uint64_t voluntary_sleep_ns = 0, longest_completion_gap_ns = 0;
};
bool h3_bursty(const std::string &pattern, int id) {
  return pattern == "one_per_role" ? id % 4 == 0 :
         pattern == "half_per_role" ? id % 4 < 2 : false;
}

void h3_worker(Store &db, const Options &o, Worker &w, Gate &gate,
               H3Schedule &schedule) noexcept {
  cpu_set_t mask;
  CPU_ZERO(&mask);
  CPU_SET(w.cpu_requested, &mask);
  w.affinity_error = pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask);
  try {
    schedule.received_affinity = h3_affinity();
    if (schedule.received_affinity != std::vector<int>{w.cpu_requested})
      w.affinity_error = EINVAL;
  } catch (...) { w.affinity_error = EINVAL; }
  {
    std::unique_lock<std::mutex> lock(gate.mutex);
    ++gate.ready;
    gate.cv.notify_all();
    gate.cv.wait(lock, [&] { return gate.release; });
    if (gate.abort || w.affinity_error) {
      w.finished.store(true, std::memory_order_release);
      return;
    }
  }
  // As in the reused worker, an unexpected callback exception fail-stops.
  // No sleep, bookkeeping, or changed code is placed inside the DB callback.
  try {
    w.cpu = sched_getcpu();
    const uint64_t cpu_start = clock_ns(CLOCK_THREAD_CPUTIME_ID);
    const uint64_t deadline = gate.start_ns + uint64_t(o.run_seconds * 1e9);
    uint64_t read_ordinal = w.read_slot, write_ordinal = 0;
    uint64_t requests = 0, previous_completion = gate.start_ns;
    while (clock_ns() < deadline) {
      if (w.writer && w.writes >= o.max_inserts / w.write_stride +
          (w.write_slot < o.max_inserts % w.write_stride)) {
        ++w.other_error;
        w.first_error = UPS_LIMITS_REACHED;
        break;
      }
      const uint64_t key = w.writer
          ? insert_key(w.write_slot + write_ordinal * w.write_stride, o.seed)
          : read_key(o, read_ordinal);
      const int type = w.writer ? 1 : 0;
      const uint64_t prior_on_time = w.before_deadline[type];
      operation(db, o, w, key, expected(key, o.seed), w.writer, deadline);
      const uint64_t returned = clock_ns();
      if (w.writer) ++write_ordinal; else read_ordinal += w.read_stride;
      if (w.first_error || w.bad_size || w.bad_value || w.bridge_error) break;
      ++requests;
      if (w.before_deadline[type] != prior_on_time) {
        const uint64_t end = std::min(returned, deadline);
        schedule.longest_completion_gap_ns = std::max(
            schedule.longest_completion_gap_ns, end - previous_completion);
        previous_completion = end;
        const size_t bin = std::min(size_t((returned - gate.start_ns) / h3_bin_ns),
                                   schedule.completion_bins.size() - 1);
        ++schedule.completion_bins[bin];
      }
      if (schedule.bursty && requests % h3_burst == 0 && returned < deadline) {
        const uint64_t begin = clock_ns();
        if (begin >= deadline) break;
        const uint64_t wake = std::min(begin + h3_sleep_ns, deadline);
        const timespec ts{time_t(wake / 1000000000ULL), long(wake % 1000000000ULL)};
        int status;
        do { status = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, nullptr); }
        while (status == EINTR);
        if (status) throw std::runtime_error("voluntary sleep failed");
        const uint64_t end = clock_ns();
        schedule.voluntary_sleep_ns += end - begin;
        schedule.sleep_intervals.push_back({{begin - gate.start_ns, end - gate.start_ns}});
      }
    }
    schedule.longest_completion_gap_ns = std::max(
        schedule.longest_completion_gap_ns, deadline - previous_completion);
#ifdef UPS_BRIDGE_KIND
    w.executor = db.thread_metrics();
#endif
    w.cpu_time_ns = clock_ns(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
    w.cpu_end = sched_getcpu();
    if (h3_affinity() != schedule.received_affinity || w.cpu != w.cpu_requested ||
        w.cpu_end != w.cpu_requested) w.affinity_error = EINVAL;
  } catch (...) { std::terminate(); }
  w.finished.store(true, std::memory_order_release);
}

Run h3_trial(Store &db, const Options &o, const std::string &pattern,
             std::array<H3Schedule, 8> &schedules) {
  Run r;
  make_workers(r, o, true);
  Gate gate;
  std::vector<std::thread> threads;
  threads.reserve(8);
  for (size_t id = 0; id < schedules.size(); ++id) {
    auto &s = schedules[id];
    s.bursty = h3_bursty(pattern, int(id));
    s.completion_bins.resize(size_t(std::ceil(o.run_seconds * 1e9 / h3_bin_ns)), 0);
    s.sleep_intervals.reserve(size_t(std::ceil(o.run_seconds * 1e9 / h3_sleep_ns)) + 2);
  }
  auto abort_and_join = [&] {
    { std::lock_guard<std::mutex> lock(gate.mutex); gate.abort = true; gate.release = true; }
    gate.cv.notify_all();
    for (auto &t : threads) t.join();
  };
  try {
    for (size_t id = 0; id < schedules.size(); ++id)
      threads.emplace_back(h3_worker, std::ref(db), std::cref(o),
          std::ref(*r.workers[id]), std::ref(gate), std::ref(schedules[id]));
  } catch (...) { abort_and_join(); throw; }
  rusage usage_start{};
  uint64_t process_cpu_start = 0;
  try {
    std::unique_lock<std::mutex> lock(gate.mutex);
    gate.cv.wait(lock, [&] { return gate.ready == 8; });
    for (const auto &w : r.workers) if (w->affinity_error) {
      gate.abort = true;
      r.failed = true;
      r.error = "worker affinity failed";
    }
    usage_start = process_usage();
    process_cpu_start = clock_ns(CLOCK_PROCESS_CPUTIME_ID);
    r.start_ns = clock_ns();
    r.deadline_ns = r.start_ns + uint64_t(o.run_seconds * 1e9);
    gate.start_ns = r.start_ns;
    gate.release = true;
  } catch (...) { abort_and_join(); throw; }
  gate.cv.notify_all();
  for (auto &t : threads) t.join(); // includes USCL pthread TLS teardown
  r.end_ns = clock_ns();
  r.process_cpu_time_ns = clock_ns(CLOCK_PROCESS_CPUTIME_ID) - process_cpu_start;
  r.phase_usage = usage_delta(usage_start, process_usage());
  r.drain_ns = r.end_ns > r.deadline_ns ? r.end_ns - r.deadline_ns : 0;
  r.progress.push_back(snapshot(r, r.end_ns)); // detailed progress is worker-local bins
  try { r.work_residency = resident_pages(); }
  catch (const std::exception &ex) { r.failed = true; r.error = ex.what(); }
#ifdef UPS_BRIDGE_KIND
  r.bridge_metrics = db.global_metrics();
#endif
  std::vector<uint64_t> written(4, 0);
  for (const auto &w : r.workers) {
    if (w->writer) written[w->write_slot] = w->writes;
    if (w->first_error || w->missing || w->duplicate || w->other_error ||
        w->bad_size || w->bad_value || w->profile_missing ||
        w->affinity_error || w->bridge_error) r.failed = true;
  }
  r.verification = db.verify(o, written, true);
  if (r.verification.integrity || r.verification.count_status || r.verification.cursor_status ||
      r.verification.bad || r.verification.seen != r.verification.expected_count ||
      r.verification.db_count != r.verification.expected_count) r.failed = true;
  return r;
}

void h3_json(const Options &o, const Run &r, const std::string &pattern,
             const std::array<H3Schedule, 8> &schedules,
             const std::vector<int> &inherited, const std::vector<int> &setup,
             const MemoryPlacement &placement, const Residency &setup_residency) {
  std::cout << "{\"schema\":\"h3-bursts-v1\",\"experiment\":\"h3_intermittent_arrivals_exploration\","
      "\"reuse_source\":\"native_harness.cc\",\"reuse_source_sha256\":"
      "\"87305b0c4a7e23bfab450ea9f5f024f02918fa88c713f7007cac8995030a00b0\","
      "\"pattern\":\"" << pattern << "\",\"burst_operations\":" << h3_burst
      << ",\"sleep_requested_ns\":" << h3_sleep_ns << ",\"bin_width_ns\":" << h3_bin_ns
      << ",\"schedule_semantics\":\"worker-local 64 requests then sleep; starts requesting; "
         "not synchronized across workers; sleep clipped at deadline; actual intervals include oversleep\","
         "\"mapping\":\"id0-3=find/CPU32-35; id4-7=insert/CPU36-39; continuous:none bursty; "
         "one_per_role:0,4 bursty; half_per_role:0,1,4,5 bursty; others always requesting\","
         "\"measurement_notes\":\"base latency is request response only, includes drain; "
         "all completion gaps may include voluntary sleep and are NOT starvation evidence; "
         "bins count on-time successful operations at wrapper return with last-bin clamping; "
         "no backlog/idle instrumentation, no usage-fairness inference from counts; "
         "H3 process CPU covers gate release through worker joins, no observer thread; "
         "base writer_eligible_end is only a legacy deadline field, NOT request eligibility\","
         "\"inherited_affinity\":";
  h3_ints(inherited);
  std::cout << ",\"setup_affinity\":";
  h3_ints(setup);
  std::cout << ",\"error\":";
  json_string(r.error.c_str());
  std::cout << ",\"worker_schedule\":[";
  for (size_t id = 0; id < schedules.size(); ++id) {
    const auto &s = schedules[id];
    std::cout << (id ? "," : "") << "{\"id\":" << id << ",\"group\":\""
        << (s.bursty ? "bursty" : "always") << "\",\"role\":\""
        << (id < 4 ? "find" : "insert") << "\",\"received_affinity\":";
    h3_ints(s.received_affinity);
    std::cout << ",\"voluntary_sleep_ns\":" << s.voluntary_sleep_ns
        << ",\"longest_completion_gap_including_sleep_ns\":" << s.longest_completion_gap_ns
        << ",\"sleep_intervals_ns\":[";
    for (size_t i = 0; i < s.sleep_intervals.size(); ++i)
      std::cout << (i ? "," : "") << '[' << s.sleep_intervals[i][0] << ','
          << s.sleep_intervals[i][1] << ']';
    std::cout << "],\"completion_bins\":[";
    for (size_t i = 0; i < s.completion_bins.size(); ++i)
      std::cout << (i ? "," : "") << s.completion_bins[i];
    std::cout << "]}";
  }
  std::cout << "],\"base\":";
  print_json(o, r, "skipped_h3_no_warmup", placement, setup_residency, r.work_residency);
  std::cout << "}\n";
}
} // namespace

int main(int argc, char **argv) {
  try {
    const auto inherited = h3_affinity();
    // Defaults are also used by --self-test errors, ensuring the same placement.
    std::vector<std::string> args{argv[0], "--cpus", "32,33,34,35,36,37,38,39",
        "--mode", "duration", "--seconds", "4", "--warmup", "0",
        "--init-cpu", "32", "--init-node", "1", "--memory-policy", "bind",
        "--memory-nodes", "1", "--memory-limit-gib", "32"};
    std::string pattern = "continuous", smoke;
    for (int i = 1; i < argc; ++i) {
      const std::string arg = argv[i];
      if (arg == "--pattern" || arg == "--self-test") {
        if (++i == argc) throw std::runtime_error("missing value for " + arg);
        (arg == "--pattern" ? pattern : smoke) = argv[i];
      } else args.push_back(arg);
    }
    if (pattern != "continuous" && pattern != "one_per_role" && pattern != "half_per_role")
      throw std::runtime_error("unknown H3 pattern");
    if (!smoke.empty() && smoke != "errors") throw std::runtime_error("only --self-test errors supported");
    std::vector<char *> pointers;
    for (auto &arg : args) pointers.push_back(&arg[0]);
    const Options o = parse(int(pointers.size()), pointers.data());
    const std::vector<int> cpus{32, 33, 34, 35, 36, 37, 38, 39};
    if (!o.duration || o.finders != 4 || o.inserters != 4 || o.cpus != cpus ||
        o.init_cpu != 32 || o.init_node != 1 || o.memory_limit_gib != 32 ||
        o.requested_memory_policy != "bind" || o.memory_nodes != std::vector<int>{1} ||
        !o.memory_policy_explicit || o.warmup != 0 || o.preload != 100000 ||
        o.max_inserts != 200000000 || (o.run_seconds != 0.2 && o.run_seconds != 4.0))
      throw std::runtime_error("H3 fixed configuration violated");
    const auto setup = h3_affinity();
    const auto placement = setup_memory(o.requested_memory_policy, o.memory_nodes, true);
    if (!smoke.empty()) return self_test(smoke); // real production-path canaries and oracle
    Store db(o);
    const Residency setup_residency = resident_pages();
    std::array<H3Schedule, 8> schedules;
    Run r = h3_trial(db, o, pattern, schedules);
    r.shutdown = db.close(); // quiescent verification, then exclusive destroy
    if (r.shutdown[0] || r.shutdown[1]) r.failed = true;
    r.peak_rss_kib = uint64_t(process_usage().ru_maxrss);
    h3_json(o, r, pattern, schedules, inherited, setup, placement, setup_residency);
    return r.failed ? 1 : 0;
  } catch (const std::exception &ex) {
    std::cout << "{\"schema\":\"h3-bursts-v1\",\"status\":\"failed\",\"error\":";
    json_string(ex.what());
    std::cout << "}\n";
    return 2;
  }
}
