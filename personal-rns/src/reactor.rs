pub use prns_runtime::reactor::{
    airtime, announce_pacer, decline_all, duty_gate, grant, interface_seam, kernel, reconnect,
    throughput, timers, AppDeciders, Host,
};

#[cfg(feature = "tokio-host")]
pub mod tokio {
    pub use prns_runtime_tokio::reactor::compression::{
        compress_if_smaller, decompress_bounded, DecompressError, SAMPLE_GATE_LEN,
    };
    pub use prns_runtime_tokio::reactor::driver::{
        run, run_with_deciders, run_with_store, run_with_store_and_deciders, tokio_grant_lane,
        AddInterfaceCommand, CryptoPoolConfig, Egress, HeapFrameSlot, HostCommand,
        HostResourceMetadata, HostResourcePayload, HostResourcePayloadError, PoolWorkers,
        ProvideDecompressedHostCommand, ReactorWiring, RequestAnyHostCommand, ResourceInbound,
        RespondAnyHostCommand, SendResourceHostCommand, SendResourceSegmentHostCommand,
        StreamInbound, TokioGrantConsumer, TokioGrantProducer, TokioHost, TokioInterfaceSeam,
        TokioInterfaceStatus,
    };
}

#[cfg(feature = "embassy-host")]
pub mod embassy {
    pub use prns_runtime_embassy::reactor::driver::{
        embassy_grant_lane, run, run_with_deciders, run_with_store, EmbassyEgress,
        EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost, EmbassyInterfaceSeam,
        EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress, PooledWiring, ReactorEgress,
        ReactorWiring,
    };
    pub use prns_runtime_embassy::reactor::timebase::EmbassyTimebase;
}
