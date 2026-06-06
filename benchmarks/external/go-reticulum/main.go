// Drive go-reticulum over the shared announce-256 corpus through parse (NewPacket +
// Unpack) + ValidateAnnounce (the Ed25519 verify + store), best-of-N min wall time.
// run.sh copies this into the cloned module's announcebench/ and runs it. Prints a
// `RESULT resolved=<n> per_sec=<f>` line for run.sh to file.

package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"math"
	"os"
	"time"

	"github.com/svanichkin/go-reticulum/rns"
)

const (
	warmup = 5
	iters  = 50
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

func ingestAll(corpus [][]byte) int {
	valid := 0
	for _, raw := range corpus {
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

func main() {
	if len(os.Args) < 2 {
		panic("usage: announcebench <corpus.hex>")
	}
	corpus := loadCorpus(os.Args[1])
	count := len(corpus)

	resolved := ingestAll(corpus)

	best := math.Inf(1)
	for i := 0; i < warmup+iters; i++ {
		start := time.Now()
		ingestAll(corpus)
		secs := time.Since(start).Seconds()
		if i >= warmup {
			best = math.Min(best, secs)
		}
	}
	perSec := float64(count) / best

	fmt.Printf("go-reticulum / announce-256: resolved %d/%d, %.0f announce/s\n", resolved, count, perSec)
	fmt.Printf("RESULT resolved=%d per_sec=%.3f\n", resolved, perSec)
}
