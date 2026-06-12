# An rns-cr (Crystal) participation node speaking the benchmark harness's scenario_node
# contract:
#
#   rnscr-node <manifest.json> <responder|initiator> <addr> [duration-ms]
#
# then the stdout line protocol: `READY role=...` once bound/dialed, and one final
# `RESULT k=v ...`. It fields both interop mechanisms: `single` (one-shot packets proven
# by the destination's PROVE_ALL strategy) and `link` (a session the initiator establishes
# first). Built against the pinned upstream cloned into ../.upstream by build.sh; compiled
# from the upstream root so its shard dependencies resolve from ../.upstream/lib.
#
# rns-cr runs its transport, interface readers, and receipt-timeout jobs on cooperative
# fibers; this node stays single-threaded so the counters mutated from those callbacks need
# no locking, and bridges callback -> main-loop settlement over a buffered Channel.

require "socket"
require "json"
require "../.upstream/src/rns"

EMPTY_RESULT = "RESULT sent=0 delivered=0 timeouts=0 payload_bytes=0 elapsed_ms=0 " \
               "delivered_per_sec=0.0 goodput_bytes_per_sec=0 rtt_p50_ms=0 rtt_p99_ms=0"

# The varied-size law every node speaks identically: a seeded xorshift64 draws each
# message's size in [min, max] — the same sequence the Go, Rust, and Python nodes draw,
# so byte totals stay comparable without exchanging anything.
class SizeSequence
  def initialize(@state : UInt64, @min : Int32, @max : Int32)
  end

  def next_len : Int32
    @state ^= @state << 13
    @state ^= @state >> 7
    @state ^= @state << 17
    span = (@max - @min + 1).to_u64
    @min + (@state % span).to_i32
  end
end

record Resolution, delivered : Bool, rtt_ms : UInt64, size : UInt64

# Catches the first announce that carries an identity and hands it to the initiator.
class HeardHandler
  include RNS::Transport::AnnounceHandler

  getter aspect_filter : String?

  def initialize(@channel : Channel(RNS::Identity), @aspect_filter : String? = nil)
    @fired = false
  end

  def received_announce(destination_hash : Bytes, announced_identity : RNS::Identity?,
                        app_data : Bytes?, announce_packet_hash : Bytes?,
                        is_path_response : Bool = false)
    return if @fired
    id = announced_identity
    return unless id
    @fired = true
    @channel.send(id)
  end
end

def free_port : Int32
  server = TCPServer.new("127.0.0.1", 0)
  port = server.local_address.port
  server.close
  port
end

def write_config(dir : String, iface_block : String)
  cfg = String.build do |s|
    s << "[reticulum]\n"
    s << "  enable_transport = No\n"
    s << "  share_instance = No\n"
    s << "  panic_on_interface_error = No\n\n"
    s << "[logging]\n  loglevel = 1\n\n"
    s << "[interfaces]\n"
    s << iface_block
  end
  Dir.mkdir_p(dir)
  File.write(File.join(dir, "config"), cfg)
end

def percentile(sorted : Array(UInt64), p : Float64) : UInt64
  return 0_u64 if sorted.empty?
  rank = ((sorted.size - 1) * p + 0.5).to_i
  rank = sorted.size - 1 if rank >= sorted.size
  sorted[rank]
end

def responder(cfg_dir : String, name : String, mechanism : String)
  port = free_port
  write_config(cfg_dir,
    "  [[bench-server]]\n" \
    "    type = TCPServerInterface\n" \
    "    enabled = yes\n" \
    "    listen_ip = 127.0.0.1\n" \
    "    listen_port = #{port}\n")

  RNS::ReticulumInstance.new(cfg_dir)
  id = RNS::Identity.new
  dest = RNS::Destination.new(id, RNS::Destination::IN, RNS::Destination::SINGLE, "bench", [name])
  dest.set_proof_strategy(RNS::Destination::PROVE_ALL)

  delivered = 0
  payload_bytes = 0_u64
  last_delivery : Time::Instant? = nil
  closed = false

  count = ->(data : Bytes) {
    delivered += 1
    payload_bytes += data.size
    last_delivery = Time.instant
    nil
  }

  if mechanism == "link"
    dest.set_link_established_callback(->(link : RNS::Link) {
      link.set_packet_callback(->(msg : Bytes, _pkt : RNS::Packet) { count.call(msg); nil })
      link.set_link_closed_callback(->(_l : RNS::Link) { closed = true; nil })
      nil
    })
  else
    dest.set_packet_callback(->(msg : Bytes, _pkt : RNS::Packet) { count.call(msg); nil })
  end

  puts "READY role=responder addr=127.0.0.1:#{port}"
  STDOUT.flush

  # rns-cr's TCPServerInterface accepts client connections and reads them (each spawned
  # connection wires the inbound callback), but it never registers those spawned interfaces
  # into Transport's outbound set and its own process_outgoing is a no-op — so a server-side
  # node can receive but never announce or send proofs. Register each spawned connection
  # ourselves (the property is public) so outbound broadcasts reach the connected client.
  registered_spawns = Set(UInt64).new
  spawn do
    loop do
      pending = [] of RNS::Interface
      RNS::Transport.interface_objects.each do |iface|
        children = iface.spawned_interfaces
        next unless children
        children.each { |c| pending << c unless registered_spawns.includes?(c.object_id) }
      end
      pending.each do |c|
        registered_spawns.add(c.object_id)
        RNS::Transport.register_interface(c)
      end
      sleep 50.milliseconds
    end
  end

  # Announce until the first delivery lands — once a packet arrives the initiator has
  # clearly heard us. On its own fiber so it never stalls the idle check that reports.
  spawn do
    loop do
      break if delivered > 0
      dest.announce
      sleep 500.milliseconds
    end
  end

  # Report on link close, or on 1500 ms of quiet after the last delivery (the single
  # mechanism never closes; the idle check is also the link's safety if a teardown is lost).
  loop do
    sleep 200.milliseconds
    break if closed
    if (ld = last_delivery) && (Time.instant - ld) > 1500.milliseconds
      break
    end
  end

  puts "RESULT delivered=#{delivered} payload_bytes=#{payload_bytes}"
  STDOUT.flush
end

def initiator(cfg_dir : String, name : String, mechanism : String, addr : String,
              duration : Time::Span, window : Int32, seed : UInt64,
              payload_min : Int32, payload_max : Int32, payload_len : Int32)
  idx = addr.rindex(":") || abort("bad addr #{addr}")
  host = addr[0...idx]
  port = addr[(idx + 1)..].to_i
  write_config(cfg_dir,
    "  [[bench-client]]\n" \
    "    type = TCPClientInterface\n" \
    "    enabled = yes\n" \
    "    target_host = #{host}\n" \
    "    target_port = #{port}\n")

  RNS::ReticulumInstance.new(cfg_dir)

  heard = Channel(RNS::Identity).new(1)
  RNS::Transport.register_announce_handler(HeardHandler.new(heard, nil))
  puts "READY role=initiator"
  STDOUT.flush

  id = heard.receive
  out_dest = RNS::Destination.new(id, RNS::Destination::OUT, RNS::Destination::SINGLE, "bench", [name])

  # rns-cr records an announced destination into its path table with a nil interface
  # (announce_handler stores `receiving_interface: nil` — an unfinished feature), and that
  # unusable entry shadows the broadcast fallback in Transport.outbound: every send then fails
  # with "No interfaces could process the outbound packet". Dropping the entry lets outbound
  # broadcast on our one interface, which for this point-to-point pair is a direct send. The
  # responder stops announcing at its first delivery, so the path is never re-added.
  RNS::Transport.remove_path(out_dest.hash)

  link : RNS::Link? = nil
  if mechanism == "link"
    established = Channel(Nil).new(1)
    l = RNS::Link.new(out_dest)
    l.set_link_established_callback(->(_l : RNS::Link) {
      established.send(nil) unless established.closed?
      nil
    })
    select
    when established.receive
    when timeout(10.seconds)
      puts EMPTY_RESULT
      STDOUT.flush
      return
    end
    link = l
  end

  size_seq = SizeSequence.new(seed, payload_min, payload_max)
  scratch_len = {payload_max, payload_len, 1}.max
  scratch = Bytes.new(scratch_len, 0xAB_u8)
  resolved = Channel(Resolution).new(window * 8)

  sent = 0
  delivered = 0
  timeouts = 0
  delivered_bytes = 0_u64
  rtts = [] of UInt64

  send_one = ->{
    size = size_seq.next_len
    data = scratch[0, size]
    lnk = link
    pkt = if lnk
            lnk.send(data)
          else
            RNS::Packet.new(out_dest, data).tap(&.send)
          end
    registered = false
    if pkt && (receipt = pkt.receipt)
      sent += 1
      registered = true
      sent_size = size.to_u64
      receipt.set_delivery_callback(->(r : RNS::PacketReceipt) {
        rtt = r.get_rtt
        rtt = 0.0 if rtt < 0
        resolved.send(Resolution.new(true, (rtt * 1000.0).to_u64, sent_size))
        nil
      })
      receipt.set_timeout_callback(->(_r : RNS::PacketReceipt) {
        resolved.send(Resolution.new(false, 0_u64, 0_u64))
        nil
      })
    end
    registered
  }

  started = Time.instant
  deadline = started + duration
  in_flight = 0
  window.times { in_flight += 1 if send_one.call }

  drain_deadline = deadline + 5.seconds
  while in_flight > 0
    wait = drain_deadline - Time.instant
    break if wait <= Time::Span.zero
    select
    when r = resolved.receive
      in_flight -= 1
      if r.delivered
        delivered += 1
        delivered_bytes += r.size
        rtts << r.rtt_ms
      else
        timeouts += 1
      end
      in_flight += 1 if Time.instant < deadline && send_one.call
    when timeout(wait)
      in_flight = 0
    end
  end

  elapsed = Time.instant - started
  link.try(&.teardown)

  rtts.sort!
  seconds = elapsed.total_seconds
  seconds = 0.001 if seconds <= 0
  elapsed_ms = elapsed.total_milliseconds.to_i64
  dps = "%.1f" % (delivered / seconds)
  gp = "%.0f" % (delivered_bytes.to_f64 / seconds)
  p50 = percentile(rtts, 0.50)
  p99 = percentile(rtts, 0.99)

  puts "RESULT sent=#{sent} delivered=#{delivered} timeouts=#{timeouts} " \
       "payload_bytes=#{delivered_bytes} elapsed_ms=#{elapsed_ms} " \
       "delivered_per_sec=#{dps} goodput_bytes_per_sec=#{gp} " \
       "rtt_p50_ms=#{p50} rtt_p99_ms=#{p99}"
  STDOUT.flush
end

STDOUT.sync = true

args = ARGV
abort("usage: rnscr-node <manifest.json> <responder|initiator> <addr> [duration-ms]") if args.size < 3
manifest_path = args[0]
role = args[1]
addr = args[2]
duration_override = (args.size > 3 ? args[3].to_u64? : nil) || 0_u64

json = JSON.parse(File.read(manifest_path))
name = json["name"].as_s
prof = json["profile"]
mechanism = prof["mechanism"].as_s
unless mechanism == "single" || mechanism == "link"
  puts "RESULT error=unsupported-mechanism:#{mechanism}"
  exit 0
end

window = prof["window"]?.try(&.as_i) || 16
duration_ms = prof["duration_ms"]?.try(&.as_i64.to_u64) || 0_u64
duration_ms = duration_override if duration_override > 0
duration = duration_ms.to_i64.milliseconds

payload_min = prof["payload_min"]?.try(&.as_i) || 0
payload_max = prof["payload_max"]?.try(&.as_i) || 0
payload_len = prof["payload_len"]?.try(&.as_i) || 0
seed_field = prof["size_seed"]?.try(&.as_i64).try(&.to_u64) || 0_u64
seed = seed_field == 0 ? 0x5EEDCAFEF00D0001_u64 : seed_field
if payload_max == 0
  payload_min = payload_len
  payload_max = payload_len
end

cfg_dir = File.join(Dir.tempdir, "rnscr-#{Process.pid}-#{role}")

case role
when "responder"
  responder(cfg_dir, name, mechanism)
when "initiator"
  initiator(cfg_dir, name, mechanism, addr, duration, window, seed, payload_min, payload_max, payload_len)
else
  puts "RESULT error=unknown-role:#{role}"
end
