// go-reticulum's announce-parallel harness: shard the corpus across goroutines, each
// parsing (NewPacket + Unpack) + ValidateAnnounce (Ed25519 verify; the store serializes
// behind go-reticulum's global mutex), best-of-N min wall. Conformance is the valid
// announce count from a single-threaded pass. Swept single-thread vs runtime.NumCPU();
// prints the parallel RESULT line for run-mt.sh.

package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"math"
	"os"
	"runtime"
	"sync"
	"time"

	"github.com/svanichkin/go-reticulum/rns"
)

const (
	warmup = 5
	iters  = 30
)

func loadCorpus(path string) [][]byte {
	f, err := os.Open(path)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	var out [][]byte
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64*1024), 64*1024)
	for sc.Scan() {
		line := sc.Text()
		if line == "" {
			continue
		}
		raw, err := hex.DecodeString(line)
		if err != nil {
			panic(err)
		}
		out = append(out, raw)
	}
	return out
}

func ingest(chunk [][]byte) int {
	valid := 0
	for _, raw := range chunk {
		p := rns.NewPacket(nil, raw, 0, 0, 0, 0, nil, nil, false, 0)
		if !p.Unpack() {
			continue
		}
		if rns.ValidateAnnounce(p, false) {
			valid++
		}
	}
	return valid
}

func split(all [][]byte, t int) [][][]byte {
	chunk := (len(all) + t - 1) / t
	var chunks [][][]byte
	for i := 0; i < len(all); i += chunk {
		end := i + chunk
		if end > len(all) {
			end = len(all)
		}
		chunks = append(chunks, all[i:end])
	}
	return chunks
}

func throughputAt(all [][]byte, t int) float64 {
	total := len(all)
	chunks := split(all, t)
	best := math.Inf(1)
	for i := 0; i < warmup+iters; i++ {
		start := time.Now()
		var wg sync.WaitGroup
		for _, ch := range chunks {
			wg.Add(1)
			go func(c [][]byte) { defer wg.Done(); ingest(c) }(ch)
		}
		wg.Wait()
		secs := time.Since(start).Seconds()
		if i >= warmup {
			best = math.Min(best, secs)
		}
	}
	return float64(total) / best
}

func main() {
	if len(os.Args) < 2 {
		panic("usage: announcebench-mt <corpus.hex>")
	}
	corpus := loadCorpus(os.Args[1])

	resolved := ingest(corpus)

	lo, hi := 1, runtime.NumCPU()
	if hi < 1 {
		hi = 1
	}
	loPS := throughputAt(corpus, lo)
	hiPS := loPS
	if hi != lo {
		hiPS = throughputAt(corpus, hi)
	}

	fmt.Printf("go-reticulum / announce-parallel: resolved %d/%d, %dt %.0f/s, %dt %.0f/s\n", resolved, len(corpus), lo, loPS, hi, hiPS)
	fmt.Printf("RESULT resolved=%d lo=%d lo_per_sec=%.3f hi=%d hi_per_sec=%.3f\n", resolved, lo, loPS, hi, hiPS)
}
