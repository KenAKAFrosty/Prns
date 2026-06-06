// go-reticulum's sustained energy harness: sustained parse + ValidateAnnounce across all
// logical cores for a fixed wall-time. usage: <corpus.hex> <secs> [working_set]

package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"os"
	"runtime"
	"strconv"
	"sync"
	"time"

	"github.com/svanichkin/go-reticulum/rns"
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

func work(chunk [][]byte) {
	for _, raw := range chunk {
		p := rns.NewPacket(nil, raw, 0, 0, 0, 0, nil, nil, false, 0)
		if p.Unpack() {
			rns.ValidateAnnounce(p, false)
		}
	}
}

func main() {
	if len(os.Args) < 3 {
		panic("usage: sustained-go <corpus.hex> <secs> [working_set]")
	}
	base := loadCorpus(os.Args[1])
	secs, _ := strconv.ParseFloat(os.Args[2], 64)
	ws := 50000
	if len(os.Args) > 3 {
		ws, _ = strconv.Atoi(os.Args[3])
	}
	corpus := make([][]byte, 0, ws)
	for len(corpus) < ws {
		corpus = append(corpus, base...)
	}

	threads := runtime.NumCPU()
	chunkSize := (len(corpus) + threads - 1) / threads
	var shards [][][]byte
	for i := 0; i < len(corpus); i += chunkSize {
		end := i + chunkSize
		if end > len(corpus) {
			end = len(corpus)
		}
		shards = append(shards, corpus[i:end])
	}

	deadline := time.Now().Add(time.Duration(secs * float64(time.Second)))
	start := time.Now()
	counts := make([]int, len(shards))
	var wg sync.WaitGroup
	for i, shard := range shards {
		wg.Add(1)
		go func(idx int, c [][]byte) {
			defer wg.Done()
			n := 0
			for time.Now().Before(deadline) {
				work(c)
				n += len(c)
			}
			counts[idx] = n
		}(i, shard)
	}
	wg.Wait()
	elapsed := time.Since(start).Seconds()
	total := 0
	for _, n := range counts {
		total += n
	}
	fmt.Printf("THROUGHPUT announces_per_sec=%.1f total=%d secs=%.2f\n", float64(total)/elapsed, total, elapsed)
}
