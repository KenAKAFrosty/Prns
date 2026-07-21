use super::protocol::Channel;

#[allow(async_fn_in_trait)]
pub trait EspNowRadio {
    /// Park the radio on `channel`. Meaningful only for a [`ChannelPolicy::Fixed`](super::protocol::ChannelPolicy::Fixed) node; a station associated to an access point follows the association, not this.
    fn set_channel(&mut self, channel: Channel);

    /// Broadcast one frame; `true` if the radio accepted it for transmission.
    async fn broadcast(&mut self, frame: &[u8]) -> bool;

    /// Await the next inbound frame, copying it into `buf` and returning the byte length written. A frame larger than `buf` is truncated to its capacity.
    async fn receive(&mut self, buf: &mut [u8]) -> usize;
}
