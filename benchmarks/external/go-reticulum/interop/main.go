// A go-reticulum participation node speaking the benchmark harness's scenario_node contract:
//
//	go-node <manifest.json> <responder|initiator> <addr> [duration-ms]
//
// then the stdout line protocol — `READY role=…` once it is bound/dialed, and one final
// `RESULT k=v …`. It fields both interop mechanisms: `single` (one-shot packets proven by the
// destination's PROVE_ALL strategy) and `link` (a session the initiator establishes first).
// Built against the pinned upstream cloned into ../.upstream by build.sh.
package main

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"sort"
	"strconv"
	"sync"
	"sync/atomic"
	"time"

	rns "github.com/svanichkin/go-reticulum/rns"
)

type profile struct {
	Mechanism  string `json:"mechanism"`
	PayloadLen int    `json:"payload_len"`
	Window     int    `json:"window"`
	DurationMs uint64 `json:"duration_ms"`
}
type manifest struct {
	Name    string  `json:"name"`
	Profile profile `json:"profile"`
}

func freePort() int {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 45000
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port
}

func writeConfig(dir, ifaceBlock string) {
	cfg := "[reticulum]\n  enable_transport = No\n  share_instance = No\n  shared_instance_port = 37428\n  instance_control_port = 37429\n  panic_on_interface_error = No\n\n[logging]\n  loglevel = 1\n\n[interfaces]\n" + ifaceBlock
	_ = os.MkdirAll(dir, 0o755)
	_ = os.WriteFile(dir+"/config", []byte(cfg), 0o644)
}

func percentile(sorted []uint64, p float64) uint64 {
	if len(sorted) == 0 {
		return 0
	}
	rank := int((float64(len(sorted)-1) * p) + 0.5)
	if rank >= len(sorted) {
		rank = len(sorted) - 1
	}
	return sorted[rank]
}

type heardHandler struct {
	once sync.Once
	ch   chan *rns.Identity
}

func (h *heardHandler) AspectFilter() any { return nil }
func (h *heardHandler) ReceivedAnnounce(_ []byte, id *rns.Identity, _ []byte) {
	if id == nil {
		return
	}
	h.once.Do(func() { h.ch <- id })
}

func main() {
	args := os.Args[1:]
	manifestPath, role, addr := args[0], args[1], args[2]
	var durationOverride uint64
	if len(args) > 3 {
		durationOverride, _ = strconv.ParseUint(args[3], 10, 64)
	}
	raw, _ := os.ReadFile(manifestPath)
	var m manifest
	_ = json.Unmarshal(raw, &m)
	if m.Profile.Mechanism != "single" && m.Profile.Mechanism != "link" {
		fmt.Printf("RESULT error=unsupported-mechanism:%s\n", m.Profile.Mechanism)
		return
	}
	durationMs := m.Profile.DurationMs
	if durationOverride > 0 {
		durationMs = durationOverride
	}
	duration := time.Duration(durationMs) * time.Millisecond
	cfgDir, _ := os.MkdirTemp("", "goret-")

	switch role {
	case "responder":
		responder(cfgDir, m)
	case "initiator":
		initiator(cfgDir, m, addr, duration)
	default:
		fmt.Println("RESULT error=unknown-role")
	}
}

func responder(cfgDir string, m manifest) {
	port := freePort()
	writeConfig(cfgDir, fmt.Sprintf("  [[bench-server]]\n    type = TCPServerInterface\n    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = %d\n", port))
	if _, err := rns.NewReticulum(&cfgDir, nil, nil, nil, false, nil); err != nil {
		fmt.Printf("RESULT error=reticulum:%v\n", err)
		return
	}
	id, _ := rns.NewIdentity()
	dest, err := rns.NewDestination(id, rns.DestinationIN, rns.DestinationSINGLE, "bench", m.Name)
	if err != nil {
		fmt.Printf("RESULT error=destination:%v\n", err)
		return
	}
	dest.SetProofStrategy(rns.DestinationPROVE_ALL)

	var delivered, payloadBytes uint64
	var lastDelivery atomic.Int64
	done := make(chan struct{})
	var closeOnce sync.Once
	count := func(data []byte) {
		atomic.AddUint64(&delivered, 1)
		atomic.AddUint64(&payloadBytes, uint64(len(data)))
		lastDelivery.Store(time.Now().UnixMilli())
	}

	if m.Profile.Mechanism == "link" {
		dest.AcceptsLinks(true)
		dest.SetLinkEstablishedCallback(func(link *rns.Link) {
			link.SetPacketCallback(func(data []byte, _ *rns.Packet) { count(data) })
			link.SetLinkClosedCallback(func(_ *rns.Link) { closeOnce.Do(func() { close(done) }) })
		})
	} else {
		dest.SetPacketCallback(func(data []byte, _ *rns.Packet) { count(data) })
	}

	fmt.Printf("READY role=responder addr=127.0.0.1:%d\n", port)
	report := func() {
		fmt.Printf("RESULT delivered=%d payload_bytes=%d\n", atomic.LoadUint64(&delivered), atomic.LoadUint64(&payloadBytes))
		os.Exit(0)
	}

	// Announce on its own goroutine, and only until the first delivery lands — once a packet
	// arrives the initiator has clearly heard us. Keeping it off the report loop is what makes
	// the single mechanism terminate: `dest.Announce` blocks once the peer disconnects and its
	// send buffer fills, and if that ran on the select loop it would stall the idle check that
	// detects the quiet and reports. (Link reports on close, so it never hit this.)
	go func() {
		t := time.NewTicker(500 * time.Millisecond)
		defer t.Stop()
		for range t.C {
			if atomic.LoadUint64(&delivered) > 0 {
				return
			}
			dest.Announce(nil, false, nil, nil, true)
		}
	}()

	idle := time.NewTicker(200 * time.Millisecond)
	for {
		select {
		case <-idle.C:
			last := lastDelivery.Load()
			if last > 0 && time.Now().UnixMilli()-last > 1500 {
				report()
			}
		case <-done:
			report()
		}
	}
}

func initiator(cfgDir string, m manifest, addr string, duration time.Duration) {
	host, portStr, _ := net.SplitHostPort(addr)
	port, _ := strconv.Atoi(portStr)
	writeConfig(cfgDir, fmt.Sprintf("  [[bench-client]]\n    type = TCPClientInterface\n    enabled = yes\n    target_host = %s\n    target_port = %d\n", host, port))
	if _, err := rns.NewReticulum(&cfgDir, nil, nil, nil, false, nil); err != nil {
		fmt.Printf("RESULT error=reticulum:%v\n", err)
		return
	}
	heard := make(chan *rns.Identity, 1)
	rns.RegisterAnnounceHandler(&heardHandler{ch: heard})
	fmt.Println("READY role=initiator")

	id := <-heard
	outDest, err := rns.NewDestination(id, rns.DestinationOUT, rns.DestinationSINGLE, "bench", m.Name)
	if err != nil {
		fmt.Printf("RESULT error=out-destination:%v\n", err)
		return
	}

	emptyResult := "RESULT sent=0 delivered=0 timeouts=0 payload_bytes=0 elapsed_ms=0 delivered_per_sec=0.0 goodput_bytes_per_sec=0 rtt_p50_ms=0 rtt_p99_ms=0"
	var target interface{} = outDest
	var link *rns.Link
	if m.Profile.Mechanism == "link" {
		established := make(chan struct{}, 1)
		var estOnce sync.Once
		link, err = rns.NewLink(outDest, nil, -1, func(_ *rns.Link) { estOnce.Do(func() { established <- struct{}{} }) }, nil)
		if err != nil {
			fmt.Printf("RESULT error=link:%v\n", err)
			return
		}
		select {
		case <-established:
		case <-time.After(10 * time.Second):
			fmt.Println(emptyResult)
			return
		}
		target = link
	}

	payload := make([]byte, m.Profile.PayloadLen)
	for i := range payload {
		payload[i] = 0xAB
	}
	type res struct {
		delivered bool
		rttMs     uint64
	}
	resolved := make(chan res, m.Profile.Window*8)
	var sent, delivered, timeouts uint64
	var rtts []uint64

	sendOne := func() {
		p := rns.NewPacket(target, payload, rns.PacketTypeData, rns.PacketCtxNone, rns.Broadcast, rns.HeaderType1, nil, nil, true, rns.FlagUnset)
		r := p.Send()
		if r == nil {
			return
		}
		sent++
		r.Callbacks.Delivery = func(rc *rns.PacketReceipt) {
			resolved <- res{true, uint64(rc.GetRTT() * 1000.0)}
		}
		r.Callbacks.Timeout = func(_ *rns.PacketReceipt) { resolved <- res{false, 0} }
	}

	started := time.Now()
	deadline := started.Add(duration)
	inFlight := 0
	for i := 0; i < m.Profile.Window; i++ {
		sendOne()
		inFlight++
	}
	drainDeadline := deadline.Add(5 * time.Second)
	for inFlight > 0 {
		wait := time.Until(drainDeadline)
		if wait <= 0 {
			break
		}
		select {
		case r := <-resolved:
			inFlight--
			if r.delivered {
				delivered++
				rtts = append(rtts, r.rttMs)
			} else {
				timeouts++
			}
			if time.Now().Before(deadline) {
				sendOne()
				inFlight++
			}
		case <-time.After(wait):
			inFlight = 0
		}
	}
	elapsedMs := uint64(time.Since(started).Milliseconds())
	if link != nil {
		link.Teardown()
	}

	sort.Slice(rtts, func(i, j int) bool { return rtts[i] < rtts[j] })
	payloadBytes := delivered * uint64(m.Profile.PayloadLen)
	seconds := float64(elapsedMs) / 1000.0
	if seconds <= 0 {
		seconds = 0.001
	}
	fmt.Printf("RESULT sent=%d delivered=%d timeouts=%d payload_bytes=%d elapsed_ms=%d delivered_per_sec=%.1f goodput_bytes_per_sec=%.0f rtt_p50_ms=%d rtt_p99_ms=%d\n",
		sent, delivered, timeouts, payloadBytes, elapsedMs,
		float64(delivered)/seconds, float64(payloadBytes)/seconds,
		percentile(rtts, 0.50), percentile(rtts, 0.99))
}
