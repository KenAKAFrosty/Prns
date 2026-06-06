# Drive rns-cr (Crystal) over the shared announce-256 corpus through Packet#unpack +
# Identity.validate_announce (the Ed25519 verify + store), best-of-N min wall time.
# Reset known_destinations each pass so every pass does the full verify work. run.sh
# copies this into the cloned repo root (so `require "./src/rns"` resolves) and runs it.
# Prints a `RESULT resolved=<n> per_sec=<f>` line for run.sh to file.

require "./src/rns"

WARMUP =  5
ITERS  = 50

def from_hex(s : String) : Bytes
  out = Bytes.new(s.size // 2)
  i = 0
  while i + 1 < s.size
    out[i // 2] = s[i, 2].to_u8(16)
    i += 2
  end
  out
end

def load_corpus(path : String) : Array(Bytes)
  corpus = [] of Bytes
  File.each_line(path) do |line|
    line = line.strip
    next if line.empty?
    corpus << from_hex(line)
  end
  corpus
end

def ingest_all(corpus : Array(Bytes)) : Int32
  RNS::Identity.known_destinations.clear
  resolved = 0
  corpus.each do |raw|
    pkt = RNS::Packet.new(nil, raw)
    next unless pkt.unpack
    resolved += 1 if RNS::Identity.validate_announce(pkt)
  end
  resolved
end

abort "usage: bench.cr <corpus.hex>" if ARGV.empty?
corpus = load_corpus(ARGV[0])
count = corpus.size

resolved = ingest_all(corpus)

best = Float64::INFINITY
(WARMUP + ITERS).times do |i|
  start = Time.monotonic
  ingest_all(corpus)
  secs = (Time.monotonic - start).total_seconds
  best = Math.min(best, secs) if i >= WARMUP
end
per_sec = count / best

puts "rns-cr / announce-256: resolved #{resolved}/#{count}, #{per_sec.round(0)} announce/s"
puts "RESULT resolved=#{resolved} per_sec=#{per_sec.round(3)}"
