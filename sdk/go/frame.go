package hotmint

import (
	"encoding/binary"
	"fmt"
	"io"
)

const maxFrameSize = 64 * 1024 * 1024 // 64 MB

// WriteFrame writes a length-prefixed frame: 4-byte little-endian u32 length + payload.
func WriteFrame(w io.Writer, payload []byte) error {
	var lenBuf [4]byte
	binary.LittleEndian.PutUint32(lenBuf[:], uint32(len(payload)))
	if _, err := w.Write(lenBuf[:]); err != nil {
		return err
	}
	if _, err := w.Write(payload); err != nil {
		return err
	}
	return nil
}

// ReadFrame reads a length-prefixed frame: 4-byte little-endian u32 length + payload.
func ReadFrame(r io.Reader) ([]byte, error) {
	var lenBuf [4]byte
	if _, err := io.ReadFull(r, lenBuf[:]); err != nil {
		return nil, err
	}
	length := binary.LittleEndian.Uint32(lenBuf[:])
	if int(length) > maxFrameSize {
		return nil, fmt.Errorf("frame size %d exceeds max %d", length, maxFrameSize)
	}
	buf := make([]byte, length)
	if _, err := io.ReadFull(r, buf); err != nil {
		return nil, err
	}
	return buf, nil
}
