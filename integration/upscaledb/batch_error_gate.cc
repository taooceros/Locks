// Focused nontransactional batch error gate. Reuse the production Store and
// identical private ABI without changing the settled timed harness.
#include <exception>
#define main heterogeneous_clients_unused_main
#include "heterogeneous_clients.cc"
#undef main

int main(int argc, char **argv) {
  try {
    int cpu = 0;
    if (argc != 1) {
      if (argc != 3 || std::strcmp(argv[1], "--cpu"))
        throw std::runtime_error("expected --cpu N");
      const uint64_t selected = positive(argv[2], "CPU");
      if (selected >= CPU_SETSIZE) throw std::runtime_error("CPU out of range");
      cpu = int(selected);
    }
    constexpr uint64_t seed = 101;
    constexpr uint64_t preload = 64;
    cpu_set_t setup;
    CPU_ZERO(&setup);
    CPU_SET(cpu, &setup);
    if (sched_setaffinity(0, sizeof(setup), &setup))
      throw std::runtime_error("setup CPU affinity failed");

    Store db(0, preload, seed);
    struct Outcome {
      int status = UPS_INTERNAL_ERROR;
      // Native has no bridge return code; bridge variants overwrite this.
      int32_t bridge_status = DLOCK_BRIDGE_OK;
      int affinity_error = 0;
      std::array<int, batch_size> statuses{};
      std::array<uint64_t, batch_size> keys{}, values{};
      std::exception_ptr exception;
    } outcome;
    std::thread requester([&] {
      try {
        cpu_set_t mask;
        CPU_ZERO(&mask);
        CPU_SET(cpu, &mask);
        outcome.affinity_error = pthread_setaffinity_np(pthread_self(), sizeof(mask), &mask);
        if (outcome.affinity_error) return;
        std::array<uint64_t, batch_size> keys{}, values{};
        std::array<int, batch_size> statuses{};
        for (unsigned i = 0; i < batch_size; ++i) {
          keys[i] = insert_key(i * cohort_size, seed); // writer 4's prefix
          values[i] = value_of(keys[i], seed);
        }
        // The fourth insert duplicates the first. Later unique inserts must
        // remain unattempted, not silently committed after this failure.
        keys[3] = keys[0];
        values[3] = values[0];
        outcome.status = db.batch(keys, values, statuses, nullptr, outcome.bridge_status);
        outcome.keys = keys;
        outcome.values = values;
        outcome.statuses = statuses;
      } catch (...) { outcome.exception = std::current_exception(); }
    });
    requester.join(); // include USCL TLS destructor before bridge destruction
    if (outcome.exception) std::rethrow_exception(outcome.exception);
    if (outcome.affinity_error || outcome.bridge_status != DLOCK_BRIDGE_OK ||
        outcome.status != UPS_DUPLICATE_KEY)
      throw std::runtime_error("unexpected affinity, bridge or batch error status");
    for (unsigned i = 0; i < batch_size; ++i) {
      const uint64_t expected_key = insert_key((i == 3 ? 0 : i) * cohort_size, seed);
      if (outcome.keys[i] != expected_key ||
          outcome.values[i] != value_of(expected_key, seed) ||
          outcome.statuses[i] != (i < 3 ? UPS_SUCCESS :
                                  i == 3 ? UPS_DUPLICATE_KEY : UPS_INTERNAL_ERROR))
        throw std::runtime_error("batch record statuses or caller-owned buffers changed");
    }
    std::array<uint64_t, workers> written{};
    written[4] = 3;
    const auto oracle = db.verify(0, preload, seed, written, 4, 8);
    if (oracle.integrity || oracle.count_status || oracle.cursor_status || oracle.bad ||
        oracle.expected != preload + 3 || oracle.count != preload + 3 ||
        oracle.seen != preload + 3)
      throw std::runtime_error("partial success violated exact membership/value/count/integrity oracle");
    db.close(); // requester and its TLS teardown finished; no active ABI caller
    std::cout << "{\"status\":\"ok\",\"variant\":\"" << variant
              << "\",\"batch_status\":" << outcome.status
              << ",\"successful_inserts\":3,\"duplicate_index\":3"
              << ",\"unexecuted_suffix\":4,\"db_count\":" << oracle.count
              << ",\"oracle_bad\":" << oracle.bad << "}" << std::endl;
    return 0;
  } catch (const std::exception &ex) {
    std::cerr << "batch error gate: " << ex.what() << '\n';
    return 2;
  }
}
