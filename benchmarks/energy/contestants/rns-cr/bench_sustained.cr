# rns-cr's sustained energy harness: sustained verify-only across all logical cores (fibers on
# OS threads via -Dpreview_mt + CRYSTAL_WORKERS) for a fixed wall-time.
# usage: bench_sustained <corpus.hex> <secs> [working_set]

require "./src/rns"

def from_hex(s : String) : Bytes
  out = Bytes.new(s.size // 2)
  i = 0
  while i + 1 < s.size
    out[i // 2] = s[i, 2].to_u8(16)
    i += 2
  end
  out
end

def verify_chunk(chunk : Array(Bytes))
  chunk.each do |raw|
    pkt = RNS::Packet.new(nil, raw)
    next unless pkt.unpack
    RNS::Identity.validate_announce(pkt, only_validate_signature: true)
  end
end

path = ARGV[0]? || abort("usage: bench_sustained <corpus.hex> <secs> [working_set]")
secs = (ARGV[1]? || "60").to_f
ws = (ARGV[2]? || "50000").to_i

base = [] of Bytes
File.each_line(path) do |line|
  line = line.strip
  base << from_hex(line) unless line.empty?
end
corpus = [] of Bytes
while corpus.size < ws
  corpus.concat(base)
end

threads = System.cpu_count.to_i
threads = 1 if threads < 1
chunk = (corpus.size + threads - 1) // threads
shards = [] of Array(Bytes)
(0...corpus.size).step(chunk) do |lo|
  hi = Math.min(corpus.size, lo + chunk)
  shards << corpus[lo...hi]
end

deadline = Time.monotonic + secs.seconds
start = Time.monotonic
counts = Array(Int32).new(shards.size, 0)
done = Channel(Nil).new
shards.each_with_index do |c, i|
  spawn do
    n = 0
    while Time.monotonic < deadline
      verify_chunk(c)
      n += c.size
    end
    counts[i] = n
    done.send(nil)
  end
end
shards.size.times { done.receive }
elapsed = (Time.monotonic - start).total_seconds
total = counts.sum
puts "THROUGHPUT announces_per_sec=#{(total / elapsed).round(1)} total=#{total} secs=#{elapsed.round(2)}"
