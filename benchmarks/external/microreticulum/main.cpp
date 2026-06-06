// Drive microReticulum (C++) over the shared announce-256 corpus through Packet::unpack +
// Identity::validate_announce (the Ed25519 verify + store), best-of-N min wall time.
// Conformance is the count of announces that validate (validate_announce == true), the
// same metric as the RNS reference's `resolved` (not known_destinations, which the library
// LRU-caps). Prints a `RESULT resolved=<n> per_sec=<f>` line for run.sh to file.

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
#include <vector>

static const int WARMUP = 5;
static const int ITERS = 50;

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return 0;
}

static std::vector<uint8_t> from_hex(const std::string& s) {
    std::vector<uint8_t> out;
    out.reserve(s.size() / 2);
    for (size_t i = 0; i + 1 < s.size(); i += 2) {
        out.push_back(static_cast<uint8_t>((hexval(s[i]) << 4) | hexval(s[i + 1])));
    }
    return out;
}

static std::vector<std::vector<uint8_t>> load_corpus(const char* path) {
    std::vector<std::vector<uint8_t>> corpus;
    std::ifstream f(path);
    std::string line;
    while (std::getline(f, line)) {
        while (!line.empty() && (line.back() == '\n' || line.back() == '\r' ||
                                 line.back() == ' ' || line.back() == '\t')) {
            line.pop_back();
        }
        if (line.empty()) continue;
        corpus.push_back(from_hex(line));
    }
    return corpus;
}

static int ingest_all(const std::vector<std::vector<uint8_t>>& corpus) {
    int valid = 0;
    for (const auto& raw : corpus) {
        RNS::Packet packet(RNS::Bytes(raw.data(), raw.size()));
        if (packet.unpack()) {
            if (RNS::Identity::validate_announce(packet)) valid++;
        }
    }
    return valid;
}

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: mr_announce_bench <corpus.hex>\n");
        return 1;
    }
    microStore::FileSystem filesystem{microStore::Adapters::UniversalFileSystem()};
    filesystem.init();
    RNS::Utilities::OS::register_filesystem(filesystem);

    auto corpus = load_corpus(argv[1]);
    const int count = static_cast<int>(corpus.size());

    const int resolved = ingest_all(corpus);

    double best = std::numeric_limits<double>::infinity();
    for (int i = 0; i < WARMUP + ITERS; i++) {
        auto start = std::chrono::steady_clock::now();
        ingest_all(corpus);
        double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
        if (i >= WARMUP) best = std::min(best, secs);
    }
    const double per_sec = count / best;

    std::printf("microReticulum / announce-256: resolved %d/%d, %.0f announce/s\n",
                resolved, count, per_sec);
    std::printf("RESULT resolved=%d per_sec=%.3f\n", resolved, per_sec);
    return 0;
}
