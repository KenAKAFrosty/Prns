// microReticulum's announce-parallel harness: shard the corpus across std::threads, each
// parse + verify (validate_announce with only_validate_signature=true — verify only, so no
// write to the static known_destinations map, which isn't thread-safe), best-of-N min wall.
// Conformance is the verified count from a single pass. Swept single-thread vs
// hardware_concurrency; prints the parallel RESULT line for run-mt.sh.

#include <microStore/FileSystem.h>
#include <microStore/Adapters/UniversalFileSystem.h>
#include <microReticulum.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <fstream>
#include <limits>
#include <string>
#include <thread>
#include <vector>

static const int WARMUP = 5;
static const int ITERS = 30;

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return 0;
}
static std::vector<uint8_t> from_hex(const std::string& s) {
    std::vector<uint8_t> out;
    out.reserve(s.size() / 2);
    for (size_t i = 0; i + 1 < s.size(); i += 2)
        out.push_back(static_cast<uint8_t>((hexval(s[i]) << 4) | hexval(s[i + 1])));
    return out;
}
static std::vector<std::vector<uint8_t>> load_corpus(const char* path) {
    std::vector<std::vector<uint8_t>> corpus;
    std::ifstream f(path);
    std::string line;
    while (std::getline(f, line)) {
        while (!line.empty() && (line.back() == '\n' || line.back() == '\r' || line.back() == ' '))
            line.pop_back();
        if (!line.empty()) corpus.push_back(from_hex(line));
    }
    return corpus;
}

static int verify_range(const std::vector<std::vector<uint8_t>>& corpus, size_t lo, size_t hi) {
    int ok = 0;
    for (size_t i = lo; i < hi; i++) {
        RNS::Packet p(RNS::Bytes(corpus[i].data(), corpus[i].size()));
        if (p.unpack() && RNS::Identity::validate_announce(p, true)) ok++;
    }
    return ok;
}

static double throughput_at(const std::vector<std::vector<uint8_t>>& corpus, int t) {
    const size_t total = corpus.size();
    double best = std::numeric_limits<double>::infinity();
    for (int it = 0; it < WARMUP + ITERS; it++) {
        auto start = std::chrono::steady_clock::now();
        std::vector<std::thread> ths;
        size_t chunk = (total + t - 1) / t;
        for (int k = 0; k < t; k++) {
            size_t lo = static_cast<size_t>(k) * chunk;
            size_t hi = std::min(total, lo + chunk);
            if (lo >= hi) break;
            ths.emplace_back(verify_range, std::cref(corpus), lo, hi);
        }
        for (auto& th : ths) th.join();
        double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
        if (it >= WARMUP) best = std::min(best, secs);
    }
    return static_cast<double>(total) / best;
}

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: mr_announce_mt <corpus.hex>\n");
        return 1;
    }
    microStore::FileSystem filesystem{microStore::Adapters::UniversalFileSystem()};
    filesystem.init();
    RNS::Utilities::OS::register_filesystem(filesystem);

    auto corpus = load_corpus(argv[1]);
    const size_t total = corpus.size();

    int resolved = verify_range(corpus, 0, total);

    int hw = static_cast<int>(std::thread::hardware_concurrency());
    int lo = 1, hi = hw > 0 ? hw : 1;
    double lo_ps = throughput_at(corpus, lo);
    double hi_ps = (hi == lo) ? lo_ps : throughput_at(corpus, hi);

    std::printf("microReticulum / announce-parallel: verified %d/%zu, %dt %.0f/s, %dt %.0f/s\n",
                resolved, total, lo, lo_ps, hi, hi_ps);
    std::printf("RESULT resolved=%d lo=%d lo_per_sec=%.3f hi=%d hi_per_sec=%.3f\n",
                resolved, lo, lo_ps, hi, hi_ps);
    return 0;
}
