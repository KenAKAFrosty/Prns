package prns

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

type journeyFixture struct {
	SchemaVersion uint32 `json:"schemaVersion"`
	Destination   struct {
		AppName            string   `json:"appName"`
		Aspects            []string `json:"aspects"`
		AnnounceAppDataHex string   `json:"announceAppDataHex"`
	} `json:"destination"`
	Request struct {
		Path          string `json:"path"`
		PathHashHex   string `json:"pathHashHex"`
		PayloadHex    string `json:"payloadHex"`
		ResponseHex   string `json:"responseHex"`
		TimeoutMillis uint64 `json:"timeoutMillis"`
	} `json:"request"`
	Resource struct {
		ChunksHex                []string `json:"chunksHex"`
		MetadataHex              string   `json:"metadataHex"`
		MaximumUncompressedBytes uint64   `json:"maximumUncompressedBytes"`
		AcceptCompressed         bool     `json:"acceptCompressed"`
		Compression              string   `json:"compression"`
	} `json:"resource"`
}

type interfaceFixture struct {
	SchemaVersion uint32 `json:"schemaVersion"`
	Interfaces    []struct {
		Kind string `json:"kind"`
	} `json:"interfaces"`
}

func fixtureBytes(t *testing.T, value string) []byte {
	t.Helper()
	decoded, err := hex.DecodeString(value)
	if err != nil {
		t.Fatal(err)
	}
	return decoded
}

func loadJourneyFixture(t *testing.T) journeyFixture {
	t.Helper()
	value, err := os.ReadFile(filepath.Join(
		"..", "..", "conformance", "persistent-two-node-v1.json",
	))
	if err != nil {
		t.Fatal(err)
	}
	var fixture journeyFixture
	if err := json.Unmarshal(value, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.SchemaVersion != HostSchemaVersion {
		t.Fatalf("journey schema %d does not match host schema", fixture.SchemaVersion)
	}
	return fixture
}

func TestMarshalEveryInterfaceFixture(t *testing.T) {
	value, err := os.ReadFile(filepath.Join(
		"..", "..", "conformance", "interface-configs-v1.json",
	))
	if err != nil {
		t.Fatal(err)
	}
	var fixture interfaceFixture
	if err := json.Unmarshal(value, &fixture); err != nil {
		t.Fatal(err)
	}
	line := SerialLineConfig{115200, SerialDataBitsEight, SerialParityNone, SerialStopBitsOne}
	ax25Line := SerialLineConfig{9600, SerialDataBitsEight, SerialParityNone, SerialStopBitsOne}
	radio := RNodeRadioConfig{915000000, 125000, 14, 8, 5}
	groupID := "sdk-fixture"
	discoveryScope := DiscoveryScopeOrganization
	discoveryPort := uint16(29710)
	dataPort := uint16(42444)
	multicastAddressType := MulticastAddressTypePermanent
	stationCallsign := "PRNS"
	stationInterval := uint64(300)
	shortAirtime := uint16(1000)
	longAirtime := uint16(500)
	configs := []InterfaceConfig{
		InterfaceConfigAutoLan{&groupID, &discoveryScope, &discoveryPort, &dataPort, []string{"eth0"}, []string{"lo"}, &multicastAddressType},
		InterfaceConfigTcpClient{"127.0.0.1:4242", BitrateBitsPerSecond{1000000}},
		InterfaceConfigTcpServer{"127.0.0.1:4242", BitrateAuto{}},
		InterfaceConfigUdp{"127.0.0.1:4242", "127.0.0.1:4243", BitrateBitsPerSecond{2000000}},
		InterfaceConfigSerial{"/dev/ttyUSB0", line},
		InterfaceConfigKiss{"/dev/ttyUSB1", line, true, 150, 50, 64, 20, &stationCallsign, &stationInterval},
		InterfaceConfigAx25Kiss{"/dev/ttyUSB2", ax25Line, false, 100, 25, 32, 10, "PRNS", 1},
		InterfaceConfigRNode{"/dev/ttyACM0", radio, true, &stationCallsign, &stationInterval, &shortAirtime, &longAirtime},
		InterfaceConfigMultiRNode{"/dev/ttyACM1", &stationCallsign, &stationInterval, []MultiRNodeMemberConfig{{"primary", 1, radio, true, true}}},
		InterfaceConfigPipe{[]string{"fixture-command", "--fixture"}, 1000},
		InterfaceConfigBackboneClient{"127.0.0.1:4244", BitrateAuto{}},
		InterfaceConfigBackboneServer{"127.0.0.1:4245", BitrateBitsPerSecond{4000000}},
		InterfaceConfigI2p{[]string{"fixture.b32.i2p"}, true},
		InterfaceConfigWeave{"/dev/ttyWEAVE0"},
		InterfaceConfigAutomaticUsb{},
		InterfaceConfigAutomaticBluetoothLe{},
		InterfaceConfigWebSocketClient{"ws://fixture.invalid/client", WebSocketFramingSelectionAuto},
		InterfaceConfigWebSocketServer{"127.0.0.1:4246", WebSocketFramingSelectionHdlc},
		InterfaceConfigBrowserRendezvous{"ws://fixture.invalid/rendezvous"},
	}
	if fixture.SchemaVersion != HostSchemaVersion || len(fixture.Interfaces) != len(configs) {
		t.Fatal("shared interface fixture does not match the generated host contract")
	}
	var arena nativeArena
	defer arena.close()
	for index, config := range configs {
		name := strings.TrimPrefix(reflect.TypeOf(config).Name(), "InterfaceConfig")
		if fixture.Interfaces[index].Kind != name {
			t.Fatalf("fixture kind %s does not match %s", fixture.Interfaces[index].Kind, name)
		}
		native, err := marshalInterface(&arena, config)
		if err != nil {
			t.Fatalf("%s: %v", fixture.Interfaces[index].Kind, err)
		}
		if uint32(native.kind) != uint32(index+1) {
			t.Fatalf("%s marshalled as kind %d", fixture.Interfaces[index].Kind, native.kind)
		}
	}
}

func TestRequestByteLimitsRejectValuesOutsideTheSafeIntegerRange(t *testing.T) {
	oversized := SafeUintMax + 1
	var arena nativeArena
	defer arena.close()
	_, err := marshalDestination(&arena, DestinationConfigSingle{
		Name:                DestinationName{AppName: "limits", Aspects: []string{"request"}},
		Identity:            DestinationIdentityConfigHostIdentity{},
		MaximumRequestBytes: &oversized,
	})
	var configError ConfigError
	if !errors.As(err, &configError) || configError.Kind != ConfigInvalidLimits {
		t.Fatalf("oversized request limit returned %v", err)
	}

	_, status, err := ffiExecute(nativeHost{}, HostCommandRequest{
		Timeout:              ResponseTimeoutLinkDefault{},
		MaximumResponseBytes: &oversized,
	})
	if status != StatusInvalidArgument || !errors.As(err, &configError) {
		t.Fatalf("oversized response limit returned status %d and error %v", status, err)
	}
}

func settledCommand(t *testing.T, host *Host, value HostCommand) CommandSettlement {
	t.Helper()
	command, err := host.Execute(value)
	if err != nil {
		t.Fatal(err)
	}
	defer command.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	settlement, err := command.Wait(ctx)
	if err != nil {
		t.Fatal(err)
	}
	return settlement
}

func successfulOutcome(t *testing.T, settlement CommandSettlement) CommandOutcome {
	t.Helper()
	succeeded, ok := settlement.(CommandSucceeded)
	if !ok {
		t.Fatalf("command returned %T", settlement)
	}
	return succeeded.Outcome
}

func TestNativeHostContract(t *testing.T) {
	host, err := NewHost(EphemeralEndpoint(nil, []Capability{
		CapabilityTcpClient,
	}))
	if err != nil {
		t.Fatal(err)
	}
	defer host.Close()

	if host.IdentityHash() == (IdentityHash{}) {
		t.Fatal("native host returned an empty identity hash")
	}
	backend, err := host.BackendInfo()
	if err != nil {
		t.Fatal(err)
	}
	if backend.Backend != BackendKindNative {
		t.Fatalf("native backend reported %v", backend.Backend)
	}
	initialSnapshot, err := host.Snapshot(2 * time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if !initialSnapshot.Runtime.Running || initialSnapshot.Runtime.InterfaceCount != 0 {
		t.Fatalf("unexpected initial snapshot: %+v", initialSnapshot.Runtime)
	}

	firstClaim, err := host.ClaimApplicationEvents()
	if err != nil {
		t.Fatal(err)
	}
	claimed, ok := firstClaim.(StreamClaimed[*ApplicationEventStream])
	if !ok {
		t.Fatal("first application stream claim was rejected")
	}
	defer claimed.Stream.Close()

	secondClaim, err := host.ClaimApplicationEvents()
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := secondClaim.(StreamAlreadyClaimed[*ApplicationEventStream]); !ok {
		t.Fatal("second application stream claim was accepted")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	_, err = claimed.Stream.Next(ctx)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("event wait cancellation returned %v", err)
	}

	waitCtx, waitCancel := context.WithTimeout(
		context.Background(),
		2*time.Second,
	)
	defer waitCancel()
	mode := InterfaceModeBoundary
	gravity := int64(-73)
	recursive := true
	fromInternal := false
	toInternal := true
	settlement, err := host.AttachInterfaceWithRouting(
		waitCtx,
		InterfaceConfigTcpClient{
			Target:  "127.0.0.1:9",
			Bitrate: BitrateAuto{},
		},
		&InterfaceRoutingPolicy{
			Mode:                  &mode,
			Gravity:               &gravity,
			RecursivePathRequests: &recursive,
			AnnouncesFromInternal: &fromInternal,
			AnnouncesToInternal:   &toInternal,
		},
	)
	if err != nil {
		t.Fatal(err)
	}
	resource, err := host.SendResourceStream(
		waitCtx,
		LinkId{},
		uint64(len("bounded upload")),
		bytes.NewBufferString("bounded upload"),
		nil,
		ResourceCompressionNever{},
	)
	if err != nil {
		t.Fatal(err)
	}
	failed, ok := resource.(CommandFailed)
	if !ok {
		t.Fatalf("resource upload returned %T", resource)
	}
	if _, ok := failed.Failure.(CommandFailureUnknownLink); !ok {
		t.Fatalf("resource upload failed with %T", failed.Failure)
	}
	resource, err = host.SendResource(
		waitCtx,
		LinkId{},
		[]byte("bounded upload"),
		nil,
		ResourceCompressionNever{},
	)
	if err != nil {
		t.Fatal(err)
	}
	failed, ok = resource.(CommandFailed)
	if !ok {
		t.Fatalf("resource command returned %T", resource)
	}
	if _, ok := failed.Failure.(CommandFailureUnknownLink); !ok {
		t.Fatalf("resource command failed with %T", failed.Failure)
	}
	succeeded, ok := settlement.(CommandSucceeded)
	if !ok {
		t.Fatalf("attach command returned %T", settlement)
	}
	outcome, ok := succeeded.Outcome.(CommandOutcomeInterfaceAttached)
	if !ok {
		t.Fatalf("attach command produced %T", succeeded.Outcome)
	}
	attachedSnapshot, err := host.Snapshot(2 * time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if attachedSnapshot.Runtime.InterfaceCount != 1 ||
		len(attachedSnapshot.Interfaces) != 1 ||
		attachedSnapshot.Interfaces[0].InterfaceId != outcome.Interface {
		t.Fatalf("attached interface missing from snapshot: %+v", attachedSnapshot)
	}

	settlement, err = host.DetachInterface(waitCtx, outcome.Interface)
	if err != nil {
		t.Fatal(err)
	}
	succeeded, ok = settlement.(CommandSucceeded)
	if !ok {
		t.Fatalf("detach command returned %T", settlement)
	}
	if _, ok := succeeded.Outcome.(CommandOutcomeInterfaceDetached); !ok {
		t.Fatalf("detach command produced %T", succeeded.Outcome)
	}
}

func TestPersistentTwoNodeJourney(t *testing.T) {
	fixture := loadJourneyFixture(t)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	if err := listener.Close(); err != nil {
		t.Fatal(err)
	}
	announceData := fixtureBytes(t, fixture.Destination.AnnounceAppDataHex)
	maximumRequestBytes := uint64(1_048_576)
	destination := DestinationConfigSingle{
		Name: DestinationName{
			AppName: fixture.Destination.AppName,
			Aspects: fixture.Destination.Aspects,
		},
		Identity:            DestinationIdentityConfigHostIdentity{},
		AnnounceAppData:     &announceData,
		MaximumRequestBytes: &maximumRequestBytes,
		RequestHandlers: []RequestHandlerConfig{{
			Path:   fixture.Request.Path,
			Policy: RequestPolicyAllowAll,
		}},
	}
	root := t.TempDir()
	serverOptions := PersistentEndpoint(
		filepath.Join(root, "server"),
		[]DestinationConfig{destination},
		[]Capability{CapabilityTcpServer},
	)
	clientOptions := PersistentEndpoint(
		filepath.Join(root, "client"),
		nil,
		[]Capability{CapabilityTcpClient},
	)
	server, err := NewHost(serverOptions)
	if err != nil {
		t.Fatal(err)
	}
	client, err := NewHost(clientOptions)
	if err != nil {
		server.Close()
		t.Fatal(err)
	}
	serverIdentity := server.IdentityHash()
	clientIdentity := client.IdentityHash()
	destinationHash := server.DestinationHashes()[0]
	claim, err := server.ClaimApplicationEvents()
	if err != nil {
		t.Fatal(err)
	}
	events := claim.(StreamClaimed[*ApplicationEventStream]).Stream
	serverAttach := successfulOutcome(t, settledCommand(t, server, HostCommandAttachInterface{
		Config: InterfaceConfigTcpServer{
			Bind:    net.JoinHostPort("127.0.0.1", fmt.Sprint(port)),
			Bitrate: BitrateAuto{},
		},
	}))
	if _, ok := serverAttach.(CommandOutcomeInterfaceAttached); !ok {
		t.Fatalf("server attach produced %T", serverAttach)
	}
	clientAttach := successfulOutcome(t, settledCommand(t, client, HostCommandAttachInterface{
		Config: InterfaceConfigTcpClient{
			Target:  net.JoinHostPort("127.0.0.1", fmt.Sprint(port)),
			Bitrate: BitrateAuto{},
		},
	}))
	if _, ok := clientAttach.(CommandOutcomeInterfaceAttached); !ok {
		t.Fatalf("client attach produced %T", clientAttach)
	}
	routed := false
	for attempt := 0; attempt < 50; attempt++ {
		snapshot, snapshotErr := client.Snapshot(2 * time.Second)
		if snapshotErr != nil {
			t.Fatal(snapshotErr)
		}
		for _, route := range snapshot.Routes {
			if route.Destination == destinationHash {
				routed = true
				break
			}
		}
		if routed {
			break
		}
		successfulOutcome(t, settledCommand(t, server, HostCommandAnnounce{
			Destination: destinationHash,
		}))
		time.Sleep(50 * time.Millisecond)
	}
	if !routed {
		t.Fatal("announced destination did not become routable")
	}
	link := successfulOutcome(t, settledCommand(t, client, HostCommandEstablishLink{
		Destination: destinationHash,
	})).(CommandOutcomeLinkEstablished)
	var pathHash RequestPathHash
	copy(pathHash[:], fixtureBytes(t, fixture.Request.PathHashHex))
	requestPayload := fixtureBytes(t, fixture.Request.PayloadHex)
	responsePayload := fixtureBytes(t, fixture.Request.ResponseHex)
	maximumResponseBytes := uint64(1_048_576)
	requestCommand, err := client.Execute(HostCommandRequest{
		LinkId:               link.LinkId,
		PathHash:             pathHash,
		Payload:              requestPayload,
		Timeout:              ResponseTimeoutExact{Millis: fixture.Request.TimeoutMillis},
		MaximumResponseBytes: &maximumResponseBytes,
	})
	if err != nil {
		t.Fatal(err)
	}
	requestResult := make(chan CommandSettlement, 1)
	requestFailure := make(chan error, 1)
	requestContext, cancelRequest := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancelRequest()
	go func() {
		settlement, waitError := requestCommand.Wait(requestContext)
		if waitError != nil {
			requestFailure <- waitError
			return
		}
		requestResult <- settlement
	}()
	eventContext, cancelEvent := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancelEvent()
	var request ApplicationEventRequest
	for {
		event, nextError := events.Next(eventContext)
		if nextError != nil {
			t.Fatal(nextError)
		}
		if value, ok := event.(ApplicationEventRequest); ok {
			request = value
			break
		}
	}
	cancelEvent()
	if !bytes.Equal(request.Data, requestPayload) {
		t.Fatalf("request payload was %q", request.Data)
	}
	successfulOutcome(t, settledCommand(t, server, HostCommandRespond{
		LinkId:           request.LinkId,
		RequestId:        request.RequestId,
		RequestRttMillis: request.RttMillis,
		Payload:          responsePayload,
	}))
	select {
	case failure := <-requestFailure:
		t.Fatal(failure)
	case settlement := <-requestResult:
		response := successfulOutcome(t, settlement).(CommandOutcomeResponseReceived)
		if !bytes.Equal(response.Data, responsePayload) {
			t.Fatalf("response payload was %q", response.Data)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("request did not settle")
	}
	requestCommand.Close()
	successfulOutcome(t, settledCommand(t, server, HostCommandSetLinkResourceStrategy{
		LinkId: request.LinkId,
		Strategy: ResourceStrategyAccept{
			MaximumUncompressedBytes: fixture.Resource.MaximumUncompressedBytes,
			AcceptCompressed:         fixture.Resource.AcceptCompressed,
		},
	}))
	readers := make([]io.Reader, 0, len(fixture.Resource.ChunksHex))
	resourcePayload := make([]byte, 0)
	for _, encoded := range fixture.Resource.ChunksHex {
		chunk := fixtureBytes(t, encoded)
		resourcePayload = append(resourcePayload, chunk...)
		readers = append(readers, bytes.NewReader(chunk))
	}
	metadata := fixtureBytes(t, fixture.Resource.MetadataHex)
	resourceContext, cancelResource := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancelResource()
	resourceSettlement, err := client.SendResourceStream(
		resourceContext,
		link.LinkId,
		uint64(len(resourcePayload)),
		io.MultiReader(readers...),
		&metadata,
		ResourceCompressionNever{},
	)
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := successfulOutcome(t, resourceSettlement).(CommandOutcomeResourceSent); !ok {
		t.Fatalf("resource send produced %T", resourceSettlement)
	}
	resourceEventContext, cancelResourceEvent := context.WithTimeout(
		context.Background(),
		5*time.Second,
	)
	defer cancelResourceEvent()
	var resource ApplicationEventResourceAvailable
	for {
		event, nextError := events.Next(resourceEventContext)
		if nextError != nil {
			t.Fatal(nextError)
		}
		if value, ok := event.(ApplicationEventResourceAvailable); ok {
			resource = value
			break
		}
	}
	if resource.Metadata == nil || !bytes.Equal(*resource.Metadata, metadata) {
		t.Fatalf("resource metadata was %v", resource.Metadata)
	}
	received := make([]byte, 0, int(resource.Resource.TotalBytes()))
	for {
		chunk, finished, nextError := resource.Resource.Next(4)
		if nextError != nil {
			t.Fatal(nextError)
		}
		if finished {
			break
		}
		received = append(received, chunk...)
	}
	resource.Resource.Close()
	if !bytes.Equal(received, resourcePayload) {
		t.Fatalf("resource payload was %q", received)
	}
	events.Close()
	if err := client.Stop(); err != nil {
		t.Fatal(err)
	}
	if err := server.Stop(); err != nil {
		t.Fatal(err)
	}
	client.Close()
	server.Close()

	restoredServer, err := NewHost(serverOptions)
	if err != nil {
		t.Fatal(err)
	}
	defer restoredServer.Close()
	restoredClient, err := NewHost(clientOptions)
	if err != nil {
		t.Fatal(err)
	}
	defer restoredClient.Close()
	if restoredServer.IdentityHash() != serverIdentity || restoredClient.IdentityHash() != clientIdentity {
		t.Fatal("persistent identities changed across restart")
	}
	if restoredServer.DestinationHashes()[0] != destinationHash {
		t.Fatal("persistent destination changed across restart")
	}
	serverSnapshot, err := restoredServer.Snapshot(2 * time.Second)
	if err != nil {
		t.Fatal(err)
	}
	clientSnapshot, err := restoredClient.Snapshot(2 * time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if !serverSnapshot.Persistence.Restored || !clientSnapshot.Persistence.Restored {
		t.Fatal("persistence did not report restoration")
	}
	restoredRoute := false
	for _, route := range clientSnapshot.Routes {
		if route.Destination == destinationHash {
			restoredRoute = true
		}
	}
	if !restoredRoute {
		t.Fatal("client route was not restored")
	}
}
