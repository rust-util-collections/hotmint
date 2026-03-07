// testserver is a minimal ABCI application for cross-language integration testing.
//
// Behavior:
//   - CreatePayload returns a configurable payload (default: "test")
//   - ExecuteBlock returns an empty EndBlockResponse
//   - Query("commits", _) returns the commit count as 8-byte LE uint64
//   - Query(_, data) echoes back data
//   - ValidateBlock and ValidateTx return true
//
// Usage: go run ./cmd/testserver <socket-path> [payload-size]
//
//   payload-size: number of bytes for CreatePayload (default: 4 = "test")
package main

import (
	"context"
	"encoding/binary"
	"fmt"
	"os"
	"os/signal"
	"strconv"
	"sync/atomic"
	"syscall"

	hotmint "github.com/rust-util-collections/hotmint/sdk/go"
	pb "github.com/rust-util-collections/hotmint/sdk/go/proto/abci"
)

type testApp struct {
	hotmint.BaseApplication
	payloadSize int
	commitCount atomic.Uint64
}

func (a *testApp) CreatePayload(_ *pb.BlockContext) []byte {
	if a.payloadSize <= 4 {
		return []byte("test")
	}
	return make([]byte, a.payloadSize)
}

func (a *testApp) ExecuteBlock(_ [][]byte, _ *pb.BlockContext) (*pb.EndBlockResponse, error) {
	return &pb.EndBlockResponse{}, nil
}

func (a *testApp) OnCommit(_ *pb.Block, _ *pb.BlockContext) error {
	a.commitCount.Add(1)
	return nil
}

func (a *testApp) Query(path string, data []byte) ([]byte, error) {
	if path == "commits" {
		var buf [8]byte
		binary.LittleEndian.PutUint64(buf[:], a.commitCount.Load())
		return buf[:], nil
	}
	return data, nil // echo
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintf(os.Stderr, "usage: %s <socket-path> [payload-size]\n", os.Args[0])
		os.Exit(1)
	}
	socketPath := os.Args[1]

	payloadSize := 4
	if len(os.Args) >= 3 {
		n, err := strconv.Atoi(os.Args[2])
		if err == nil && n > 0 {
			payloadSize = n
		}
	}

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer cancel()

	srv := hotmint.NewServer(socketPath, &testApp{payloadSize: payloadSize})
	if err := srv.Run(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "server error: %v\n", err)
		os.Exit(1)
	}
}
