package prns

import "context"

func (host *Host) executeSettlement(
	ctx context.Context,
	value HostCommand,
) (CommandSettlement, error) {
	command, err := host.Execute(value)
	if err != nil {
		return nil, err
	}
	defer command.Close()
	return command.Wait(ctx)
}

func (host *Host) Announce(
	ctx context.Context,
	destination DestinationHash,
	interfaceID *InterfaceId,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandAnnounce{
		Destination: destination,
		Interface:   interfaceID,
	})
}

func (host *Host) SendSinglePacket(
	ctx context.Context,
	destination DestinationHash,
	payload []byte,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandSendSinglePacket{
		Destination: destination,
		Payload:     payload,
	})
}

func (host *Host) CloseLink(
	ctx context.Context,
	linkID LinkId,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandCloseLink{LinkId: linkID})
}

func (host *Host) AttachTCPServer(
	ctx context.Context,
	bind string,
	bitrate Bitrate,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandAttachTcpServer{
		Bind:    bind,
		Bitrate: bitrate,
	})
}

func (host *Host) AttachTCPClient(
	ctx context.Context,
	target string,
	bitrate Bitrate,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandAttachTcpClient{
		Target:  target,
		Bitrate: bitrate,
	})
}

func (host *Host) AttachUDP(
	ctx context.Context,
	local string,
	peer string,
	bitrate Bitrate,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandAttachUdp{
		Local:   local,
		Peer:    peer,
		Bitrate: bitrate,
	})
}

func (host *Host) AttachInterface(
	ctx context.Context,
	config InterfaceConfig,
) (CommandSettlement, error) {
	return host.AttachInterfaceWithRouting(ctx, config, nil)
}

func (host *Host) AttachInterfaceWithRouting(
	ctx context.Context,
	config InterfaceConfig,
	routing *InterfaceRoutingPolicy,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandAttachInterface{
		Config:  config,
		Routing: routing,
	})
}

func (host *Host) DetachInterface(
	ctx context.Context,
	interfaceID InterfaceId,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandDetachInterface{
		Interface: interfaceID,
	})
}

func (host *Host) EstablishLink(
	ctx context.Context,
	destination DestinationHash,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandEstablishLink{
		Destination: destination,
	})
}

func (host *Host) RequestPath(
	ctx context.Context,
	destination DestinationHash,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandRequestPath{
		Destination: destination,
	})
}

func (host *Host) Identify(
	ctx context.Context,
	linkID LinkId,
	identity IdentityHash,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandIdentify{
		LinkId:   linkID,
		Identity: identity,
	})
}

func (host *Host) SendLinkPacket(
	ctx context.Context,
	linkID LinkId,
	payload []byte,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandSendLinkPacket{
		LinkId:  linkID,
		Payload: payload,
	})
}

func (host *Host) Request(
	ctx context.Context,
	linkID LinkId,
	pathHash RequestPathHash,
	payload []byte,
	timeout ResponseTimeout,
) (CommandSettlement, error) {
	return host.RequestWithMaximumResponseBytes(
		ctx,
		linkID,
		pathHash,
		payload,
		timeout,
		nil,
	)
}

func (host *Host) RequestWithMaximumResponseBytes(
	ctx context.Context,
	linkID LinkId,
	pathHash RequestPathHash,
	payload []byte,
	timeout ResponseTimeout,
	maximumResponseBytes *uint64,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandRequest{
		LinkId:               linkID,
		PathHash:             pathHash,
		Payload:              payload,
		Timeout:              timeout,
		MaximumResponseBytes: maximumResponseBytes,
	})
}

func (host *Host) Respond(
	ctx context.Context,
	linkID LinkId,
	requestID RequestId,
	requestRTTMillis uint64,
	payload []byte,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandRespond{
		LinkId:           linkID,
		RequestId:        requestID,
		RequestRttMillis: requestRTTMillis,
		Payload:          payload,
	})
}

func (host *Host) SendResource(
	ctx context.Context,
	linkID LinkId,
	payload []byte,
	packedMetadata *[]byte,
	compression ResourceCompression,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandSendResource{
		LinkId:         linkID,
		Payload:        payload,
		PackedMetadata: packedMetadata,
		Compression:    compression,
	})
}

func (host *Host) SetLinkResourceStrategy(
	ctx context.Context,
	linkID LinkId,
	strategy ResourceStrategy,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandSetLinkResourceStrategy{
		LinkId:   linkID,
		Strategy: strategy,
	})
}

func (host *Host) SetDestinationResourceStrategy(
	ctx context.Context,
	destination DestinationHash,
	strategy ResourceStrategy,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandSetDestinationResourceStrategy{
		Destination: destination,
		Strategy:    strategy,
	})
}

func (host *Host) SendChannelMessage(
	ctx context.Context,
	linkID LinkId,
	messageType uint16,
	payload []byte,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandSendChannelMessage{
		LinkId:      linkID,
		MessageType: messageType,
		Payload:     payload,
	})
}

func (host *Host) AllowRequester(
	ctx context.Context,
	destination DestinationHash,
	pathHash RequestPathHash,
	identity IdentityHash,
) (CommandSettlement, error) {
	return host.executeSettlement(ctx, HostCommandAllowRequester{
		Destination: destination,
		PathHash:    pathHash,
		Identity:    identity,
	})
}
