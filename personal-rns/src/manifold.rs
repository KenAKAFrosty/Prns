pub use prns_runtime::manifold::{
    airtime, announce_pacer, decline_all, duty_gate, grant, interface_seam, reaction_routing,
    reconnect, throughput, timers, wake_schedule, AppDeciders, Host,
};

#[cfg(feature = "tokio-host")]
pub mod tokio {
    pub use prns_runtime_tokio::manifold::compression::{
        compress_if_smaller, decompress_bounded, DecompressError, SAMPLE_GATE_LEN,
    };
    pub use prns_runtime_tokio::manifold::driver::{
        run, run_with_deciders, run_with_store, run_with_store_and_deciders, tokio_grant_lane,
        AddInterfaceCommand, CryptoPoolConfig, Egress, HeapFrameSlot, HostCommand,
        HostResourceMetadata, HostResourcePayload, HostResourcePayloadError, ManifoldWiring,
        PoolWorkers, ProvideDecompressedHostCommand, RequestAnyHostCommand, ResourceInbound,
        RespondAnyHostCommand, SendResourceHostCommand, SendResourceSegmentHostCommand,
        StreamInbound, TokioClock, TokioGrantConsumer, TokioGrantProducer, TokioHost,
        TokioInterfaceSeam, TokioInterfaceStatus,
    };
}

#[cfg(feature = "embassy-host")]
pub mod embassy {
    pub use prns_runtime_embassy::manifold::driver::{
        embassy_grant_lane, run, run_with_deciders, run_with_store, EmbassyEgress,
        EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost, EmbassyInterfaceSeam,
        EmbassyInterfaceStatus, InterfaceLifecycle, ManifoldEgress, ManifoldWiring, PooledEgress,
        PooledWiring,
    };
    pub use prns_runtime_embassy::manifold::timebase::EmbassyTimebase;
}
