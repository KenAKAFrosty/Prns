use js_sys::{Array, Object, Reflect, Uint8Array};
use personal_rns::interfaces::websocket::{
    WebSocketFramingSelection, WebSocketSessionFrameDecodeOutcome, WebSocketSessionFraming,
    WebSocketSessionOutboundAction, WebSocketWireFraming, AUTO_DETECTION_GRACE_PERIOD_MILLIS,
    FRAME_CAP,
};
use personal_rns::interfaces::{FrameSink, FrameSinkError};
use wasm_bindgen::prelude::*;

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
        let mut resolved_outbound = None;
        let mut offset = 0;
        while offset < message.len() {
            let outcome = self
                .session
                .next_frame_into(&message, &mut offset, &mut self.frame);
            match outcome {
                Ok(WebSocketSessionFrameDecodeOutcome::Frame) => {
                    packets.push(&Uint8Array::from(self.frame.as_slice()));
                }
                Ok(WebSocketSessionFrameDecodeOutcome::ResolvedFrame(resolution)) => {
                    packets.push(&Uint8Array::from(self.frame.as_slice()));
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
    let mut encoded = vec![0; framing.message_cap()];
    let encoded_len = framing.encode(packet, &mut encoded).ok()?;
    encoded.truncate(encoded_len);
    Some(encoded)
}
