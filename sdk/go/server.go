package hotmint

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"sync"

	pb "github.com/rust-util-collections/hotmint/sdk/go/proto/abci"
	"google.golang.org/protobuf/proto"
)

// Server listens on a Unix domain socket and dispatches incoming ABCI
// requests to an Application implementation.
type Server struct {
	socketPath string
	app        Application
	listener   net.Listener
	mu         sync.Mutex
}

// NewServer creates a new IPC server bound to the given Unix socket path.
func NewServer(socketPath string, app Application) *Server {
	return &Server{
		socketPath: socketPath,
		app:        app,
	}
}

// Run starts the server. It blocks until the context is cancelled or an
// unrecoverable error occurs. The server handles one connection at a time,
// matching the single-threaded consensus engine model.
func (s *Server) Run(ctx context.Context) error {
	// Remove stale socket file if present.
	_ = os.Remove(s.socketPath)

	ln, err := net.Listen("unix", s.socketPath)
	if err != nil {
		return fmt.Errorf("listen %s: %w", s.socketPath, err)
	}
	s.mu.Lock()
	s.listener = ln
	s.mu.Unlock()

	log.Printf("IPC server listening on %s", s.socketPath)

	// Close listener when context is cancelled.
	go func() {
		<-ctx.Done()
		ln.Close()
	}()

	for {
		conn, err := ln.Accept()
		if err != nil {
			if ctx.Err() != nil {
				return nil // Graceful shutdown.
			}
			return fmt.Errorf("accept: %w", err)
		}
		s.handleConn(conn)
	}
}

// Stop gracefully shuts down the server by closing the listener.
func (s *Server) Stop() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.listener != nil {
		s.listener.Close()
	}
}

func (s *Server) handleConn(conn net.Conn) {
	defer conn.Close()

	for {
		frame, err := ReadFrame(conn)
		if err != nil {
			if errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) || errors.Is(err, net.ErrClosed) {
				return // Client disconnected.
			}
			log.Printf("read_frame error: %v", err)
			return
		}

		var req pb.Request
		if err := proto.Unmarshal(frame, &req); err != nil {
			log.Printf("failed to decode request: %v", err)
			return
		}

		resp := s.dispatch(&req)
		respBytes, err := proto.Marshal(resp)
		if err != nil {
			log.Printf("failed to encode response: %v", err)
			return
		}

		if err := WriteFrame(conn, respBytes); err != nil {
			log.Printf("write_frame error: %v", err)
			return
		}
	}
}

func (s *Server) dispatch(req *pb.Request) *pb.Response {
	switch r := req.Request.(type) {
	case *pb.Request_CreatePayload:
		payload := s.app.CreatePayload(r.CreatePayload)
		return &pb.Response{
			Response: &pb.Response_CreatePayload{
				CreatePayload: &pb.CreatePayloadResponse{Payload: payload},
			},
		}

	case *pb.Request_ValidateBlock:
		ok := s.app.ValidateBlock(r.ValidateBlock.Block, r.ValidateBlock.Ctx)
		return &pb.Response{
			Response: &pb.Response_ValidateBlock{
				ValidateBlock: &pb.ValidateBlockResponse{Ok: ok},
			},
		}

	case *pb.Request_ValidateTx:
		ok := s.app.ValidateTx(r.ValidateTx.Tx, r.ValidateTx.Ctx)
		return &pb.Response{
			Response: &pb.Response_ValidateTx{
				ValidateTx: &pb.ValidateTxResponse{Ok: ok},
			},
		}

	case *pb.Request_ExecuteBlock:
		result, err := s.app.ExecuteBlock(r.ExecuteBlock.Txs, r.ExecuteBlock.Ctx)
		resp := &pb.ExecuteBlockResponse{}
		if err != nil {
			resp.Error = err.Error()
		} else {
			resp.Result = result
		}
		return &pb.Response{
			Response: &pb.Response_ExecuteBlock{ExecuteBlock: resp},
		}

	case *pb.Request_OnCommit:
		err := s.app.OnCommit(r.OnCommit.Block, r.OnCommit.Ctx)
		resp := &pb.OnCommitResponse{}
		if err != nil {
			resp.Error = err.Error()
		}
		return &pb.Response{
			Response: &pb.Response_OnCommit{OnCommit: resp},
		}

	case *pb.Request_OnEvidence:
		err := s.app.OnEvidence(r.OnEvidence)
		resp := &pb.OnEvidenceResponse{}
		if err != nil {
			resp.Error = err.Error()
		}
		return &pb.Response{
			Response: &pb.Response_OnEvidence{OnEvidence: resp},
		}

	case *pb.Request_Query:
		data, err := s.app.Query(r.Query.Path, r.Query.Data)
		resp := &pb.QueryResponse{}
		if err != nil {
			resp.Error = err.Error()
		} else {
			resp.Data = data
		}
		return &pb.Response{
			Response: &pb.Response_Query{Query: resp},
		}

	default:
		log.Printf("unknown request type: %T", req.Request)
		return &pb.Response{}
	}
}
