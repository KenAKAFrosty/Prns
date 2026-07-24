package prns

import (
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

	attach, err := host.Execute(HostCommandAttachTcpClient{
		Target:  "127.0.0.1:9",
		Bitrate: BitrateAuto{},
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
