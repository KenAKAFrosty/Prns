use core::future::Future;

#[cfg(test)]
mod tests;

pub struct BleEventSources<B, H, C, D, G> {
    pub backend: B,
    pub handshake: H,
    pub closed: C,
    pub disabled: D,
    pub groups: G,
}

#[allow(async_fn_in_trait)]
pub trait BleEventSelector {
    async fn select<T>(
        &mut self,
        sources: BleEventSources<
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
        >,
    ) -> T;
}

/// The ordinary Tokio fair selector, with no owned seed or additional state.
pub struct TokioFairBleEvents;

impl BleEventSelector for TokioFairBleEvents {
    async fn select<T>(
        &mut self,
        sources: BleEventSources<
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
        >,
    ) -> T {
        tokio::select! {
            event = sources.backend => event,
            event = sources.handshake => event,
            event = sources.closed => event,
            event = sources.disabled => event,
            event = sources.groups => event,
        }
    }
}
