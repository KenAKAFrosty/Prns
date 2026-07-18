use super::*;

pub(super) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(super) const TEST_TRANSPORT_IDENTITY_HASH: IdentityHash = IdentityHash::new([0xA5; 16]);

pub(super) async fn reply_for(
    request: &[u8],
    node: &(impl NodeIntrospection
          + RoutingControl
          + DestinationIdentityRetentionControl
          + IdentityBlackholeSource
          + IdentityBlackholeControl),
) -> Vec<u8> {
    reply_for_with_control(request, node, node).await
}

pub(super) async fn reply_for_with_control(
    request: &[u8],
    query: &(impl NodeIntrospection
          + DestinationIdentityRetentionControl
          + IdentityBlackholeSource
          + IdentityBlackholeControl),
    control: &impl RoutingControl,
) -> Vec<u8> {
    let Ok(request) = RpcRequest::decode(request) else {
        return Vec::new();
    };
    reply_for_decoded(
        &request,
        query,
        control,
        query,
        query,
        TEST_TRANSPORT_IDENTITY_HASH,
    )
    .await
    .unwrap_or_default()
}

pub(super) async fn reply_for_with_blackholes<B>(
    request: &[u8],
    query: &(impl NodeIntrospection + RoutingControl + DestinationIdentityRetentionControl),
    blackholes: &B,
) -> Vec<u8>
where
    B: IdentityBlackholeSource + IdentityBlackholeControl,
{
    let Ok(request) = RpcRequest::decode(request) else {
        return Vec::new();
    };
    reply_for_decoded(
        &request,
        query,
        query,
        query,
        blackholes,
        TEST_TRANSPORT_IDENTITY_HASH,
    )
    .await
    .unwrap_or_default()
}

pub(super) async fn reply_for_with_retention(
    request: &[u8],
    query: &(impl NodeIntrospection
          + RoutingControl
          + IdentityBlackholeSource
          + IdentityBlackholeControl),
    retention: &impl DestinationIdentityRetentionControl,
) -> Vec<u8> {
    let Ok(request) = RpcRequest::decode(request) else {
        return Vec::new();
    };
    reply_for_decoded(
        &request,
        query,
        query,
        retention,
        query,
        TEST_TRANSPORT_IDENTITY_HASH,
    )
    .await
    .unwrap_or_default()
}

pub(super) fn test_credentials(rpc_key: [u8; 32]) -> SharedInstanceCredentials {
    SharedInstanceCredentials::new(
        RpcAuthenticationKey::new(rpc_key.to_vec()),
        TEST_TRANSPORT_IDENTITY_HASH,
    )
}

pub(super) struct EnvVarRestore {
    key: &'static str,
    value: Option<std::ffi::OsString>,
}

impl EnvVarRestore {
    pub(super) fn capture(key: &'static str) -> Self {
        Self {
            key,
            value: std::env::var_os(key),
        }
    }
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        match &self.value {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

pub(super) async fn read_test_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    tokio::time::timeout(std::time::Duration::from_secs(1), read_frame(stream))
        .await
        .expect("test RPC frame arrives before the timeout")
        .unwrap()
}

pub(super) async fn read_frame_dup(c: &mut tokio::io::DuplexStream) -> Vec<u8> {
    read_test_frame(c).await
}

pub(super) async fn write_frame_dup(c: &mut tokio::io::DuplexStream, payload: &[u8]) {
    c.write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .unwrap();
    c.write_all(payload).await.unwrap();
    c.flush().await.unwrap();
}

pub(super) async fn authenticate_modern_client(
    client: &mut tokio::io::DuplexStream,
    rpc_key: &[u8; 32],
) {
    let server_challenge = read_frame_dup(client).await;
    let server_message = server_challenge.strip_prefix(b"#CHALLENGE#").unwrap();
    let mut response = b"{sha256}".to_vec();
    response.extend_from_slice(&hmac_sha256(rpc_key, server_message));
    write_frame_dup(client, &response).await;
    assert_eq!(
        read_frame_dup(client).await,
        RpcAuthenticationControlMessage::Welcome.wire_payload()
    );

    let mut our_message = b"{sha256}".to_vec();
    our_message.extend_from_slice(&[0x11u8; RpcChallengeNonce::LENGTH]);
    let mut our_challenge = b"#CHALLENGE#".to_vec();
    our_challenge.extend_from_slice(&our_message);
    write_frame_dup(client, &our_challenge).await;
    let server_reply = read_frame_dup(client).await;
    let server_mac = server_reply.strip_prefix(b"{sha256}").unwrap();
    assert!(hmac_sha256_verify(rpc_key, &our_message, server_mac).is_ok());
    write_frame_dup(
        client,
        RpcAuthenticationControlMessage::Welcome.wire_payload(),
    )
    .await;
}

pub(super) fn msgpack_request(entries: Vec<(&str, Value)>) -> Vec<u8> {
    let value = Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (Value::from(key), value))
            .collect(),
    );
    encode_msgpack(value).unwrap()
}

pub(super) fn value_field<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    value
        .as_map()?
        .iter()
        .find_map(|(key, value)| (key.as_str() == Some(field)).then_some(value))
}

#[derive(Clone)]
pub(super) struct StubQuery {
    pub(super) links: u32,
    pub(super) packet_phy: Option<(PacketHash, PacketPhyStats)>,
    pub(super) rates: Vec<AnnounceRateSnapshot>,
    pub(super) routes: Vec<RouteSnapshot>,
    pub(super) interfaces: Vec<InterfaceInventoryEntry>,
}

impl NodeIntrospection for StubQuery {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry> {
        self.interfaces.clone()
    }

    async fn link_count(&self) -> u32 {
        self.links
    }

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        self.packet_phy
            .and_then(|(retained_hash, stats)| (retained_hash == packet_hash).then_some(stats))
    }

    async fn announce_rates(&self) -> Vec<AnnounceRateSnapshot> {
        self.rates.clone()
    }

    async fn routes(&self) -> Vec<RouteSnapshot> {
        self.routes.clone()
    }

    async fn route(&self, destination: DestinationHash) -> Option<RouteSnapshot> {
        self.routes
            .iter()
            .find(|entry| entry.destination == destination)
            .cloned()
    }
}

impl RoutingControl for StubQuery {
    fn drop_route(
        &self,
        _destination: DestinationHash,
    ) -> impl std::future::Future<Output = Result<DropRouteOutcome, RoutingControlError>> + Send
    {
        std::future::ready(Ok(DropRouteOutcome::NotFound))
    }

    fn drop_routes_via(
        &self,
        _transport: TransportId,
    ) -> impl std::future::Future<Output = Result<DropRoutesViaOutcome, RoutingControlError>> + Send
    {
        std::future::ready(Ok(DropRoutesViaOutcome { dropped_routes: 0 }))
    }

    fn clear_announce_queues(
        &self,
    ) -> impl std::future::Future<Output = Result<ClearAnnounceQueuesOutcome, RoutingControlError>> + Send
    {
        std::future::ready(Ok(ClearAnnounceQueuesOutcome {
            dropped_announces: 0,
        }))
    }
}

impl DestinationIdentityRetentionControl for StubQuery {
    fn mark_destination_used(
        &self,
        _destination: DestinationHash,
    ) -> impl std::future::Future<
        Output = Result<MarkDestinationUsedOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        std::future::ready(Ok(MarkDestinationUsedOutcome::NotFound))
    }

    fn retain_destination(
        &self,
        _destination: DestinationHash,
    ) -> impl std::future::Future<
        Output = Result<RetainDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        std::future::ready(Ok(RetainDestinationOutcome::NotFound))
    }

    fn release_destination(
        &self,
        _destination: DestinationHash,
    ) -> impl std::future::Future<
        Output = Result<ReleaseDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        std::future::ready(Ok(ReleaseDestinationOutcome::NotFound))
    }

    fn retain_identity(
        &self,
        _identity: IdentityHash,
    ) -> impl std::future::Future<
        Output = Result<RetainIdentityOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        std::future::ready(Ok(RetainIdentityOutcome {
            newly_retained_destination_count: 0,
            already_retained_destination_count: 0,
        }))
    }
}

impl IdentityBlackholeSource for StubQuery {
    type Reason = String;
    type Entries = Vec<BlackholedIdentity<String>>;

    fn blackholed_identities(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send
    {
        std::future::ready(Ok(Vec::new()))
    }

    fn is_blackholed(
        &self,
        _identity: IdentityHash,
    ) -> impl std::future::Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send {
        std::future::ready(Ok(false))
    }
}

impl IdentityBlackholeControl for StubQuery {
    fn blackhole_identity<'a>(
        &'a self,
        _entry: BlackholedIdentity<&'a str>,
    ) -> impl std::future::Future<
        Output = Result<BlackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send
           + 'a {
        std::future::ready(Err(IdentityBlackholeControlError::NodeStopped))
    }

    fn unblackhole_identity(
        &self,
        _identity: IdentityHash,
    ) -> impl std::future::Future<
        Output = Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send {
        std::future::ready(Err(IdentityBlackholeControlError::NodeStopped))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoutingControlCall {
    DropRoute(DestinationHash),
    DropRoutesVia(TransportId),
    ClearAnnounceQueues,
}

pub(super) struct StubRoutingControl {
    pub(super) calls: tokio::sync::mpsc::UnboundedSender<RoutingControlCall>,
    pub(super) drop_route: Result<DropRouteOutcome, RoutingControlError>,
    pub(super) drop_routes_via: Result<DropRoutesViaOutcome, RoutingControlError>,
    pub(super) clear_announce_queues: Result<ClearAnnounceQueuesOutcome, RoutingControlError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetentionCapabilityCall {
    MarkUsed(DestinationHash),
    RetainDestination(DestinationHash),
    ReleaseDestination(DestinationHash),
    RetainIdentity(IdentityHash),
}

pub(super) struct StubRetention {
    pub(super) calls: tokio::sync::mpsc::UnboundedSender<RetentionCapabilityCall>,
    pub(super) mark_used:
        Result<MarkDestinationUsedOutcome, DestinationIdentityRetentionControlError>,
    pub(super) retain_destination:
        Result<RetainDestinationOutcome, DestinationIdentityRetentionControlError>,
    pub(super) release_destination:
        Result<ReleaseDestinationOutcome, DestinationIdentityRetentionControlError>,
    pub(super) retain_identity:
        Result<RetainIdentityOutcome, DestinationIdentityRetentionControlError>,
}

impl DestinationIdentityRetentionControl for StubRetention {
    fn mark_destination_used(
        &self,
        destination: DestinationHash,
    ) -> impl std::future::Future<
        Output = Result<MarkDestinationUsedOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        let _ = self
            .calls
            .send(RetentionCapabilityCall::MarkUsed(destination));
        std::future::ready(self.mark_used)
    }

    fn retain_destination(
        &self,
        destination: DestinationHash,
    ) -> impl std::future::Future<
        Output = Result<RetainDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        let _ = self
            .calls
            .send(RetentionCapabilityCall::RetainDestination(destination));
        std::future::ready(self.retain_destination)
    }

    fn release_destination(
        &self,
        destination: DestinationHash,
    ) -> impl std::future::Future<
        Output = Result<ReleaseDestinationOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        let _ = self
            .calls
            .send(RetentionCapabilityCall::ReleaseDestination(destination));
        std::future::ready(self.release_destination)
    }

    fn retain_identity(
        &self,
        identity: IdentityHash,
    ) -> impl std::future::Future<
        Output = Result<RetainIdentityOutcome, DestinationIdentityRetentionControlError>,
    > + Send {
        let _ = self
            .calls
            .send(RetentionCapabilityCall::RetainIdentity(identity));
        std::future::ready(self.retain_identity)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BlackholeCapabilityCall {
    ReadAll,
    IsBlackholed(IdentityHash),
    Blackhole(BlackholedIdentity<String>),
    Unblackhole(IdentityHash),
}

#[derive(Clone)]
pub(super) struct StubBlackholes {
    pub(super) calls: tokio::sync::mpsc::UnboundedSender<BlackholeCapabilityCall>,
    pub(super) entries: Result<Vec<BlackholedIdentity<String>>, IdentityBlackholeSourceError>,
    pub(super) is_blackholed: Result<bool, IdentityBlackholeSourceError>,
    pub(super) blackhole: Result<BlackholeIdentityOutcome, IdentityBlackholeControlError>,
    pub(super) unblackhole: Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
}

impl IdentityBlackholeSource for StubBlackholes {
    type Reason = String;
    type Entries = Vec<BlackholedIdentity<String>>;

    fn blackholed_identities(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send
    {
        let _ = self.calls.send(BlackholeCapabilityCall::ReadAll);
        std::future::ready(self.entries.clone())
    }

    fn is_blackholed(
        &self,
        identity: IdentityHash,
    ) -> impl std::future::Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send {
        let _ = self
            .calls
            .send(BlackholeCapabilityCall::IsBlackholed(identity));
        std::future::ready(self.is_blackholed)
    }
}

impl IdentityBlackholeControl for StubBlackholes {
    fn blackhole_identity<'a>(
        &'a self,
        entry: BlackholedIdentity<&'a str>,
    ) -> impl std::future::Future<
        Output = Result<BlackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send
           + 'a {
        let entry = BlackholedIdentity {
            identity: entry.identity,
            source: entry.source,
            expiry: entry.expiry,
            reason: entry.reason.map(String::from),
        };
        let _ = self.calls.send(BlackholeCapabilityCall::Blackhole(entry));
        std::future::ready(self.blackhole)
    }

    fn unblackhole_identity(
        &self,
        identity: IdentityHash,
    ) -> impl std::future::Future<
        Output = Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send {
        let _ = self
            .calls
            .send(BlackholeCapabilityCall::Unblackhole(identity));
        std::future::ready(self.unblackhole)
    }
}

impl RoutingControl for StubRoutingControl {
    fn drop_route(
        &self,
        destination: DestinationHash,
    ) -> impl std::future::Future<Output = Result<DropRouteOutcome, RoutingControlError>> + Send
    {
        let _ = self.calls.send(RoutingControlCall::DropRoute(destination));
        std::future::ready(self.drop_route)
    }

    fn drop_routes_via(
        &self,
        transport: TransportId,
    ) -> impl std::future::Future<Output = Result<DropRoutesViaOutcome, RoutingControlError>> + Send
    {
        let _ = self
            .calls
            .send(RoutingControlCall::DropRoutesVia(transport));
        std::future::ready(self.drop_routes_via)
    }

    fn clear_announce_queues(
        &self,
    ) -> impl std::future::Future<Output = Result<ClearAnnounceQueuesOutcome, RoutingControlError>> + Send
    {
        let _ = self.calls.send(RoutingControlCall::ClearAnnounceQueues);
        std::future::ready(self.clear_announce_queues)
    }
}
