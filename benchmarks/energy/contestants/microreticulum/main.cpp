// microReticulum's sustained energy harness: sustained verify-only across all logical cores
// (std::threads) for a fixed wall-time. usage: <corpus.hex> <secs> [working_set]

#include <microStore/FileSystem.h>
#include <microStore/Adapters/UniversalFileSystem.h>
#include <microReticulum.h>

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <string>
#include <thread>
#include <vector>

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

static void verify_until(const std::vector<std::vector<uint8_t>>& corpus, size_t lo, size_t hi,
                         std::chrono::steady_clock::time_point deadline, long long* count) {
    long long n = 0;
    while (std::chrono::steady_clock::now() < deadline) {
        for (size_t i = lo; i < hi; i++) {
            RNS::Packet p(RNS::Bytes(corpus[i].data(), corpus[i].size()));
            if (p.unpack()) RNS::Identity::validate_announce(p, true);
        }
        n += static_cast<long long>(hi - lo);
    }
    *count = n;
}

int main(int argc, char** argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage: mr_sustained <corpus.hex> <secs> [working_set]\n");
        return 1;
    }
    double secs = std::atof(argv[2]);
    size_t ws = (argc > 3) ? static_cast<size_t>(std::atoll(argv[3])) : 50000;

    microStore::FileSystem filesystem{microStore::Adapters::UniversalFileSystem()};
    filesystem.init();
    RNS::Utilities::OS::register_filesystem(filesystem);

    std::vector<std::vector<uint8_t>> base;
    {
        std::ifstream f(argv[1]);
        std::string line;
        while (std::getline(f, line)) {
            while (!line.empty() && (line.back() == '\n' || line.back() == '\r' || line.back() == ' '))
                line.pop_back();
            if (!line.empty()) base.push_back(from_hex(line));
        }
    }
    std::vector<std::vector<uint8_t>> corpus;
    while (corpus.size() < ws) corpus.insert(corpus.end(), base.begin(), base.end());
    const size_t total = corpus.size();

    int threads = static_cast<int>(std::thread::hardware_concurrency());
    if (threads < 1) threads = 1;
    size_t chunk = (total + threads - 1) / threads;

    auto deadline = std::chrono::steady_clock::now() + std::chrono::duration_cast<std::chrono::steady_clock::duration>(std::chrono::duration<double>(secs));
    auto start = std::chrono::steady_clock::now();
    std::vector<std::thread> ths;
    std::vector<long long> counts(threads, 0);
    for (int k = 0; k < threads; k++) {
        size_t lo = static_cast<size_t>(k) * chunk;
        size_t hi = std::min(total, lo + chunk);
        if (lo >= hi) break;
        ths.emplace_back(verify_until, std::cref(corpus), lo, hi, deadline, &counts[k]);
    }
    for (auto& th : ths) th.join();
    double elapsed = std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
    long long total_done = 0;
    for (auto c : counts) total_done += c;

    std::printf("THROUGHPUT announces_per_sec=%.1f total=%lld secs=%.2f\n",
                static_cast<double>(total_done) / elapsed, total_done, elapsed);
    return 0;
}
