package prns

import (
	"bytes"
	"context"
	"errors"
	"testing"
	"time"
)

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

	attach, err := host.Execute(HostCommandAttachInterface{
		Config: InterfaceConfigTcpClient{
			Target:  "127.0.0.1:9",
			Bitrate: BitrateAuto{},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	defer attach.Close()

	waitCtx, waitCancel := context.WithTimeout(
		context.Background(),
		2*time.Second,
	)
	defer waitCancel()
	resource, err := host.SendResource(
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
	settlement, err := attach.Wait(waitCtx)
	if err != nil {
		t.Fatal(err)
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

	detach, err := host.Execute(HostCommandDetachInterface{
		Interface: outcome.Interface,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer detach.Close()
	settlement, err = detach.Wait(waitCtx)
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
