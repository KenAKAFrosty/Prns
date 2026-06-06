# rns-cr's announce-parallel harness. Crystal's runtime is cooperative single-thread by
# default; with -Dpreview_mt + CRYSTAL_WORKERS its fibers run across OS threads. Shard the
# corpus across fibers, each verify-only (validate_announce only_validate_signature: true —
# no class-var store write, which isn't thread-safe), best-of-N min wall. Conformance is the
# verified count from a single pass. Swept single-thread vs cpu_count; prints the parallel
# RESULT line for run-mt.sh.
#
# Compile/run: crystal run --release -Dpreview_mt bench_mt.cr -- <corpus.hex>

require "./src/rns"

WARMUP =  5
ITERS  = 30

def from_hex(s : String) : Bytes
  out = Bytes.new(s.size // 2)
  i = 0
  while i + 1 < s.size
    out[i // 2] = s[i, 2].to_u8(16)
    i += 2
  end
  out
end

def verify_chunk(chunk : Array(Bytes)) : Int32
  ok = 0
  chunk.each do |raw|
    pkt = RNS::Packet.new(nil, raw)
    next unless pkt.unpack
    ok += 1 if RNS::Identity.validate_announce(pkt, only_validate_signature: true)
  end
  ok
end

def shard(corpus : Array(Bytes), t : Int32) : Array(Array(Bytes))
  total = corpus.size
  chunk = (total + t - 1) // t
  chunks = [] of Array(Bytes)
  (0...total).step(chunk) do |lo|
    hi = Math.min(total, lo + chunk)
    chunks << corpus[lo...hi]
  end
  chunks
end

def throughput_at(corpus : Array(Bytes), t : Int32) : Float64
  total = corpus.size
  chunks = shard(corpus, t)
  best = Float64::INFINITY
  (WARMUP + ITERS).times do |i|
    start = Time.monotonic
    done = Channel(Nil).new
    chunks.each do |c|
      spawn do
        verify_chunk(c)
        done.send(nil)
      end
    end
    chunks.size.times { done.receive }
    secs = (Time.monotonic - start).total_seconds
    best = Math.min(best, secs) if i >= WARMUP
  end
  total / best
end

path = ARGV[0]? || abort("usage: bench_mt <corpus.hex>")
corpus = [] of Bytes
File.each_line(path) do |line|
  line = line.strip
  corpus << from_hex(line) unless line.empty?
end

resolved = verify_chunk(corpus)
lo = 1
hi = System.cpu_count.to_i
hi = 1 if hi < 1
lo_ps = throughput_at(corpus, lo)
hi_ps = hi == lo ? lo_ps : throughput_at(corpus, hi)

puts "rns-cr / announce-parallel: verified #{resolved}/#{corpus.size}, #{lo}t #{lo_ps.round(0)}/s, #{hi}t #{hi_ps.round(0)}/s"
puts "RESULT resolved=#{resolved} lo=#{lo} lo_per_sec=#{lo_ps.round(3)} hi=#{hi} hi_per_sec=#{hi_ps.round(3)}"
