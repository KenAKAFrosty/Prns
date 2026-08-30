use js_sys::{Array, Object, Reflect, Uint8Array};
use personal_rns::interfaces::websocket::{
    WebSocketFramingSelection, WebSocketSessionFrameDecodeOutcome, WebSocketSessionFraming,
    WebSocketSessionOutboundAction, WebSocketWireFraming, AUTO_DETECTION_GRACE_PERIOD_MILLIS,
    FRAME_CAP,
};
use personal_rns::interfaces::{FrameSink, FrameSinkError};
use wasm_bindgen::prelude::*;

const WEBSOCKET_DECODE_BATCH_MAGIC: u32 = u32::from_le_bytes(*b"PWSD");
const WEBSOCKET_DECODE_BATCH_VERSION: u16 = 1;
const WEBSOCKET_DECODE_BATCH_HEADER_BYTES: usize = 12;
const WEBSOCKET_DECODE_BATCH_RESOLVED_OUTBOUND: u16 = 1;

#[wasm_bindgen]
pub struct WebSocketFramingCodec {
    session: WebSocketSessionFraming,
    frame: WasmFrame,
    message_cap: usize,
}

#[wasm_bindgen]
impl WebSocketFramingCodec {
    #[wasm_bindgen(constructor)]
    pub fn new(selection: &str) -> Result<Self, JsValue> {
        let selection = WebSocketFramingSelection::from_name(selection)
            .map_err(|_| JsValue::from_str("unknown WebSocket framing selection"))?;
        Ok(Self {
            session: WebSocketSessionFraming::new(selection),
            frame: WasmFrame::new(),
            message_cap: selection.message_cap(),
        })
    }

    #[wasm_bindgen(js_name = messageCap)]
    pub fn message_cap(&self) -> usize {
        self.message_cap
    }

    #[wasm_bindgen(js_name = canReadOutbound)]
    pub fn can_read_outbound(&self) -> bool {
        self.session.can_read_outbound()
    }

    #[wasm_bindgen(js_name = canStageMultipleOutbound)]
    pub fn can_stage_multiple_outbound(&self) -> bool {
        self.session.can_stage_multiple_outbound()
    }

    #[wasm_bindgen(js_name = rawFallbackIsArmed)]
    pub fn raw_fallback_is_armed(&self) -> bool {
        self.session.raw_fallback_is_armed()
    }

    #[wasm_bindgen(js_name = isDetecting)]
    pub fn is_detecting(&self) -> bool {
        self.session.is_detecting()
    }

    #[wasm_bindgen(js_name = rawFallbackDelayMillis)]
    pub fn raw_fallback_delay_millis(&self) -> u32 {
        u32::try_from(AUTO_DETECTION_GRACE_PERIOD_MILLIS).unwrap_or(u32::MAX)
    }

    pub fn decode(&mut self, message: Vec<u8>) -> Result<JsValue, JsValue> {
        let packets = Array::new();
        let resolved_outbound = self.decode_with(message, |frame| {
            packets.push(&Uint8Array::from(frame));
            Ok(())
        })?;
        let batch = Object::new();
        Reflect::set(
            batch.as_ref(),
            &JsValue::from_str("packets"),
            packets.as_ref(),
        )?;
        if let Some(outbound) = resolved_outbound {
            Reflect::set(
                batch.as_ref(),
                &JsValue::from_str("resolvedOutbound"),
                Uint8Array::from(outbound.as_slice()).as_ref(),
            )?;
        }
        Ok(batch.into())
    }

    #[wasm_bindgen(js_name = decodePacked)]
    pub fn decode_packed(&mut self, message: Vec<u8>) -> Result<Vec<u8>, JsValue> {
        let mut writer = WebSocketDecodeBatchWriter::new(message.len())?;
        let resolved_outbound = self.decode_with(message, |frame| writer.frame(frame))?;
        writer.finish(resolved_outbound.as_deref())
    }

    fn decode_with(
        &mut self,
        message: Vec<u8>,
        mut emit: impl FnMut(&[u8]) -> Result<(), JsValue>,
    ) -> Result<Option<Vec<u8>>, JsValue> {
        let mut resolved_outbound = None;
        let mut offset = 0;
        while offset < message.len() {
            let outcome = self
                .session
                .next_frame_into(&message, &mut offset, &mut self.frame);
            match outcome {
                Ok(WebSocketSessionFrameDecodeOutcome::Frame) => {
                    emit(self.frame.as_slice())?;
                }
                Ok(WebSocketSessionFrameDecodeOutcome::ResolvedFrame(resolution)) => {
                    emit(self.frame.as_slice())?;
                    resolved_outbound = resolution
                        .pending_packet()
                        .map(|packet| {
                            encode_packet(resolution.framing(), packet).ok_or_else(|| {
                                JsValue::from_str("WebSocket packet encoding failed")
                            })
                        })
                        .transpose()?;
                }
                Ok(
                    WebSocketSessionFrameDecodeOutcome::Incomplete
                    | WebSocketSessionFrameDecodeOutcome::AmbiguousFraming,
                )
                | Err(_) => break,
            }
        }
        Ok(resolved_outbound)
    }

    #[wasm_bindgen(js_name = stageOutbound)]
    pub fn stage_outbound(&mut self, packet: Vec<u8>) -> Result<Option<Vec<u8>>, JsValue> {
        match self.session.stage_outbound(&packet) {
            WebSocketSessionOutboundAction::Queued => Ok(None),
            WebSocketSessionOutboundAction::Send(framing) => encode_packet(framing, &packet)
                .map(Some)
                .ok_or_else(|| JsValue::from_str("WebSocket packet encoding failed")),
            WebSocketSessionOutboundAction::Rejected => {
                Err(JsValue::from_str("WebSocket packet length is invalid"))
            }
            WebSocketSessionOutboundAction::Backpressured => {
                Err(JsValue::from_str("WebSocket framing is awaiting evidence"))
            }
        }
    }

    #[wasm_bindgen(js_name = releaseRawFallback)]
    pub fn release_raw_fallback(&mut self) -> Option<Vec<u8>> {
        let released = self.session.release_raw_fallback()?;
        released
            .pending_packet()
            .and_then(|packet| encode_packet(released.framing(), packet))
    }
}

struct WebSocketDecodeBatchWriter {
    bytes: Vec<u8>,
    packet_count: u32,
}

impl WebSocketDecodeBatchWriter {
    fn new(message_bytes: usize) -> Result<Self, JsValue> {
        let capacity = WEBSOCKET_DECODE_BATCH_HEADER_BYTES
            .checked_add(message_bytes)
            .ok_or_else(|| JsValue::from_str("WebSocket decode batch capacity overflowed"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| JsValue::from_str("WebSocket decode batch allocation failed"))?;
        bytes.extend_from_slice(&WEBSOCKET_DECODE_BATCH_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&WEBSOCKET_DECODE_BATCH_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        Ok(Self {
            bytes,
            packet_count: 0,
        })
    }

    fn frame(&mut self, frame: &[u8]) -> Result<(), JsValue> {
        let length = u32::try_from(frame.len())
            .map_err(|_| JsValue::from_str("WebSocket decoded frame is too large"))?;
        self.packet_count = self
            .packet_count
            .checked_add(1)
            .ok_or_else(|| JsValue::from_str("WebSocket decode batch has too many frames"))?;
        self.bytes.extend_from_slice(&length.to_le_bytes());
        self.bytes.extend_from_slice(frame);
        Ok(())
    }

    fn finish(mut self, resolved_outbound: Option<&[u8]>) -> Result<Vec<u8>, JsValue> {
        let flags = if let Some(outbound) = resolved_outbound {
            let length = u32::try_from(outbound.len())
                .map_err(|_| JsValue::from_str("WebSocket resolved frame is too large"))?;
            self.bytes.extend_from_slice(&length.to_le_bytes());
            self.bytes.extend_from_slice(outbound);
            WEBSOCKET_DECODE_BATCH_RESOLVED_OUTBOUND
        } else {
            0
        };
        self.bytes[6..8].copy_from_slice(&flags.to_le_bytes());
        self.bytes[8..12].copy_from_slice(&self.packet_count.to_le_bytes());
        Ok(self.bytes)
    }
}

struct WasmFrame {
    bytes: Vec<u8>,
}

impl WasmFrame {
    const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl FrameSink for WasmFrame {
    fn clear(&mut self) {
        self.bytes.clear();
    }

    fn frame_len(&self) -> usize {
        self.bytes.len()
    }

    fn free_capacity(&self) -> usize {
        FRAME_CAP.saturating_sub(self.bytes.len())
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        if self.bytes.len() >= FRAME_CAP {
            return Err(FrameSinkError::Full);
        }
        self.bytes.push(byte);
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        if run.len() > self.free_capacity() {
            return Err(FrameSinkError::Full);
        }
        self.bytes.extend_from_slice(run);
        Ok(())
    }
}

fn encode_packet(framing: WebSocketWireFraming, packet: &[u8]) -> Option<Vec<u8>> {
    if packet.is_empty() || packet.len() > FRAME_CAP {
        return None;
    }
    if framing == WebSocketWireFraming::RawPacket {
        return Some(packet.to_vec());
    }
    let mut encoded = vec![0; framing.maximum_encoded_len(packet.len())];
    let encoded_len = framing.encode(packet, &mut encoded).ok()?;
    encoded.truncate(encoded_len);
    Some(encoded)
}

#[cfg(test)]
mod tests {
    use super::{WebSocketFramingCodec, WEBSOCKET_DECODE_BATCH_MAGIC};

    #[test]
    fn packed_decode_preserves_packet_bytes_without_object_keys() -> Result<(), &'static str> {
        let mut codec = WebSocketFramingCodec::new("kiss").map_err(|_| "codec creation failed")?;
        let encoded = codec
            .stage_outbound(vec![0x21, 0x22, 0x23])
            .map_err(|_| "packet encoding failed")?
            .ok_or("packet was unexpectedly queued")?;
        let batch = codec
            .decode_packed(encoded)
            .map_err(|_| "packet decoding failed")?;

        assert_eq!(
            batch,
            [
                WEBSOCKET_DECODE_BATCH_MAGIC.to_le_bytes().as_slice(),
                1u16.to_le_bytes().as_slice(),
                0u16.to_le_bytes().as_slice(),
                1u32.to_le_bytes().as_slice(),
                3u32.to_le_bytes().as_slice(),
                &[0x21, 0x22, 0x23],
            ]
            .concat(),
        );
        Ok(())
    }
}
