// Independent calibration evidence, not part of any benchmark hot path.
#include <x86intrin.h>
#include <sched.h>
#include <time.h>
#include <unistd.h>
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <iostream>

static uint64_t ns() {
  timespec value{};
  if (clock_gettime(CLOCK_MONOTONIC_RAW, &value)) std::abort();
  return uint64_t(value.tv_sec) * 1000000000ULL + value.tv_nsec;
}

int main(int argc, char **argv) {
  if (argc < 2) return 2;
  std::cout << "{\"clock\":\"CLOCK_MONOTONIC_RAW\",\"meaning\":"
               "\"elapsed TSC ticks, not CPU cycles; per-CPU calibration does not prove cross-socket synchronization\","
               "\"samples\":[";
  bool comma = false;
  for (int arg = 1; arg < argc; ++arg) {
    char *end = nullptr;
    errno = 0;
    long cpu = std::strtol(argv[arg], &end, 10);
    if (errno || !end || *end || end == argv[arg] || cpu < 0 || cpu >= CPU_SETSIZE) return 2;
    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET(cpu, &set);
    if (sched_setaffinity(0, sizeof(set), &set)) return 3;
    for (int sample = 0; sample < 5; ++sample) {
      unsigned aux_begin = 0, aux_end = 0;
      const uint64_t begin_ns = ns();
      const uint64_t begin_ticks = __rdtscp(&aux_begin);
      timespec delay{0, 100000000};
      while (nanosleep(&delay, &delay)) if (errno != EINTR) return 4;
      const uint64_t end_ticks = __rdtscp(&aux_end);
      const uint64_t end_ns = ns();
      if (comma) std::cout << ',';
      comma = true;
      std::cout << "{\"cpu\":" << cpu << ",\"sample\":" << sample
                << ",\"elapsed_ns\":" << end_ns - begin_ns
                << ",\"elapsed_tsc_ticks\":" << end_ticks - begin_ticks
                << ",\"aux_begin\":" << aux_begin << ",\"aux_end\":" << aux_end << '}';
      if (aux_begin != aux_end || end_ticks < begin_ticks || end_ns <= begin_ns) return 5;
    }
  }
  std::cout << "]}\n";
}
