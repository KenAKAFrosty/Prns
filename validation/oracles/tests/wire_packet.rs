#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use prns_core::wire::{
    wire_hop_count_is_valid, ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType,
    WirePacketHeader, BROADCAST_MTU, HEADER_MAX_LEN, HEADER_MIN_LEN,
};

mod support;

const SEED: u64 = 0x77a1_5eed_1357_2468;

struct Case {
    label: String,
    raw: Vec<u8>,
}

struct Generator {
    state: u64,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn byte(&mut self) -> u8 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state.to_be_bytes()[0]
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.byte()).collect()
    }
}

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("python/wire_oracle.py")
}

fn packet(flags: u8, hops: u8, context: u8, payload: &[u8], generator: &mut Generator) -> Vec<u8> {
    let type_2 = flags & 0x40 != 0;
    let mut raw = Vec::with_capacity(
        if type_2 {
            HEADER_MAX_LEN
        } else {
            HEADER_MIN_LEN
        } + payload.len(),
    );
    raw.push(flags);
    raw.push(hops);
    if type_2 {
        raw.extend_from_slice(&generator.bytes(16));
    }
    raw.extend_from_slice(&generator.bytes(16));
    raw.push(context);
    raw.extend_from_slice(payload);
    raw
}

fn corpus() -> Vec<Case> {
    let mut generator = Generator::new(SEED);
    let contexts = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x80, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff,
    ];
    let hops = [0, 1, 126, 127, 128, u8::MAX];
    let mut cases = (0u8..=u8::MAX)
        .map(|flags| {
            let payload = generator.bytes(usize::from(flags % 5));
            Case {
                label: format!("flags-{flags:02x}"),
                raw: packet(
                    flags,
                    hops[usize::from(flags) % hops.len()],
                    contexts[usize::from(flags) % contexts.len()],
                    &payload,
                    &mut generator,
                ),
            }
        })
        .collect::<Vec<_>>();

    for type_2 in [false, true] {
        let flags = if type_2 { 0x40 } else { 0x00 };
        for context in contexts {
            cases.push(Case {
                label: format!("context-{type_2}-{context:02x}"),
                raw: packet(flags, 0, context, &[0xa5], &mut generator),
            });
        }
        for hops in hops {
            cases.push(Case {
                label: format!("hops-{type_2}-{hops}"),
                raw: packet(flags, hops, 0, &[], &mut generator),
            });
        }
        let complete = packet(flags, 0, 0, &[0x11, 0x22], &mut generator);
        let header_length = if type_2 {
            HEADER_MAX_LEN
        } else {
            HEADER_MIN_LEN
        };
        for length in 0..header_length {
            cases.push(Case {
                label: format!("truncated-{type_2}-{length}"),
                raw: complete[..length].to_vec(),
            });
        }
        for total_length in [header_length, BROADCAST_MTU, BROADCAST_MTU + 1] {
            let payload = generator.bytes(total_length - header_length);
            cases.push(Case {
                label: format!("length-{type_2}-{total_length}"),
                raw: packet(flags, 0, 0, &payload, &mut generator),
            });
        }
    }
    cases
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalized(raw: &[u8]) -> serde_json::Value {
    let Ok((header, payload)) = WirePacketHeader::parse(raw) else {
        return serde_json::json!({ "error": "rejected" });
    };
    if !wire_hop_count_is_valid(header.hops) {
        return serde_json::json!({ "error": "rejected" });
    }
    let mut encoded = [0u8; HEADER_MAX_LEN];
    let written = header.write(&mut encoded).expect("parsed header writes");
    let mut reencoded = encoded[..written].to_vec();
    reencoded.extend_from_slice(payload);
    serde_json::json!({
        "ok": {
            "ifac_flag": match header.ifac_flag { IfacFlag::Open => 0, IfacFlag::Authenticated => 1 },
            "header_type": usize::from(header.transport_id.is_some()),
            "context_flag": match header.context_flag { ContextFlag::Unset => 0, ContextFlag::Set => 1 },
            "propagation": match header.propagation { PropagationType::Broadcast => 0, PropagationType::Transport => 1 },
            "destination_type": match header.destination_type { DestinationType::Single => 0, DestinationType::Group => 1, DestinationType::Plain => 2, DestinationType::Link => 3 },
            "packet_type": match header.packet_type { PacketType::Data => 0, PacketType::Announce => 1, PacketType::LinkRequest => 2, PacketType::Proof => 3 },
            "hops": header.hops,
            "transport_id": header.transport_id.map(|id| hex(id.as_bytes())),
            "address": hex(header.address.as_bytes()),
            "context": header.context.to_byte(),
            "payload": hex(payload),
        },
        "reencoded": hex(&reencoded),
    })
}

#[test]
fn stock_rns_and_prns_agree_on_adversarial_packet_boundaries() {
    let python = support::required_python("SMOKE_PYTHON");
    let cases = corpus();
    let input = serde_json::Value::Array(
        cases
            .iter()
            .map(|case| serde_json::Value::String(hex(&case.raw)))
            .collect(),
    );
    let oracle = support::run_json_oracle(&python, &oracle_script(), &input);
    let oracle = oracle.as_array().expect("wire oracle emits an array");
    assert_eq!(oracle.len(), cases.len());
    for (index, (case, expected)) in cases.iter().zip(oracle).enumerate() {
        assert_eq!(
            normalized(&case.raw),
            *expected,
            "seed {SEED:#018x}, case {index} ({}), input {}",
            case.label,
            hex(&case.raw)
        );
    }
}
