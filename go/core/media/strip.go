package media

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
)

// StripMetadata removes privacy-sensitive metadata (EXIF with GPS, XMP, IPTC, comments, text and
// timestamp chunks) from an image while keeping everything needed to render it. The mime argument
// must match the content, which is verified from its magic bytes.
//
//   - JPEG: APP1..APP15 and COM segments are removed except the ICC_PROFILE segment (APP2).
//     APP0 (JFIF) is kept because it carries no personal data.
//   - PNG: ancillary chunks are removed except iCCP, sRGB, gAMA and cHRM (colour), tRNS
//     (transparency) and acTL, fcTL, fdAT (animation).
//   - WebP: EXIF and XMP chunks are removed and their VP8X flags cleared. ICCP, ALPH and ANIM are kept.
//     Metadata inside animation frames (ANMF) is not inspected.
func StripMetadata(data []byte, mime string) ([]byte, error) {
	switch {
	case mime == "image/jpeg" && isJPEG(data):
		return stripJPEG(data)
	case mime == "image/png" && bytes.HasPrefix(data, pngSignature):
		return stripPNG(data)
	case mime == "image/webp" && isWebP(data):
		return stripWebP(data)
	}
	return nil, fmt.Errorf("%w: metadata stripping for %q", ErrUnsupported, mime)
}

var errMalformed = errors.New("media: malformed image container")

const iccProfileTag = "ICC_PROFILE\x00"

func stripJPEG(data []byte) ([]byte, error) {
	out := []byte{0xFF, 0xD8}
	p := 2
	for p < len(data) {
		if data[p] != 0xFF {
			return nil, errMalformed
		}
		if p+1 >= len(data) {
			return nil, errMalformed
		}
		m := data[p+1]
		switch {
		case m == 0xFF: // fill byte before a marker
			p++
			continue
		case m == 0xD9: // EOI ends the image; anything after it is dropped
			return append(out, 0xFF, 0xD9), nil
		case m == 0x01 || (m >= 0xD0 && m <= 0xD7): // standalone markers have no length
			out = append(out, 0xFF, m)
			p += 2
			continue
		}
		if p+4 > len(data) {
			return nil, errMalformed
		}
		segEnd := p + 2 + int(binary.BigEndian.Uint16(data[p+2:]))
		if segEnd > len(data) || segEnd < p+4 {
			return nil, errMalformed
		}
		if keepJPEGSegment(m, data[p+4:segEnd]) {
			out = append(out, data[p:segEnd]...)
		}
		p = segEnd
		if m == 0xDA { // start of scan: entropy-coded data follows, up to the next real marker
			end := entropyEnd(data, p)
			out = append(out, data[p:end]...)
			p = end
		}
	}
	return nil, errMalformed
}

// keepJPEGSegment reports whether a segment with marker m and payload payload survives stripping.
func keepJPEGSegment(m byte, payload []byte) bool {
	switch {
	case m == 0xE2: // APP2 is ICC_PROFILE when tagged, and is kept for colour management
		return bytes.HasPrefix(payload, []byte(iccProfileTag))
	case m >= 0xE1 && m <= 0xEF: // APP1..APP15 (EXIF, XMP, IPTC, vendor data); APP0 (JFIF) is kept
		return false
	case m == 0xFE: // COM
		return false
	}
	return true
}

// entropyEnd returns the index of the first marker at or after p, skipping stuffed zero bytes and
// restart markers that belong to the entropy-coded segment.
func entropyEnd(data []byte, p int) int {
	for i := p; i+1 < len(data); i++ {
		if data[i] != 0xFF {
			continue
		}
		switch n := data[i+1]; {
		case n == 0x00 || (n >= 0xD0 && n <= 0xD7):
			i++
		case n == 0xFF:
			// fill byte: the next iteration sees the marker
		default:
			return i
		}
	}
	return len(data)
}

var pngSignature = []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}

// pngKeepAncillary lists the ancillary chunks that affect rendering or colour but carry no personal data.
var pngKeepAncillary = map[string]bool{
	"iCCP": true, "sRGB": true, "gAMA": true, "cHRM": true, "tRNS": true,
	"acTL": true, "fcTL": true, "fdAT": true,
}

func stripPNG(data []byte) ([]byte, error) {
	out := append([]byte(nil), pngSignature...)
	for p := len(pngSignature); p+8 <= len(data); {
		n := int(binary.BigEndian.Uint32(data[p:]))
		typ := string(data[p+4 : p+8])
		end := p + 12 + n
		if n < 0 || end > len(data) {
			return nil, errMalformed
		}
		critical := typ[0]&0x20 == 0 // uppercase first letter marks a critical chunk
		if critical || pngKeepAncillary[typ] {
			out = append(out, data[p:end]...)
		}
		p = end
		if typ == "IEND" {
			return out, nil
		}
	}
	return nil, errMalformed
}

func isWebP(data []byte) bool {
	return len(data) >= 12 && string(data[0:4]) == "RIFF" && string(data[8:12]) == "WEBP"
}

const (
	webpFlagXMP  = 0x04
	webpFlagEXIF = 0x08
)

func stripWebP(data []byte) ([]byte, error) {
	out := append([]byte(nil), data[:12]...)
	for p := 12; p+8 <= len(data); {
		typ := string(data[p : p+4])
		n := int(binary.LittleEndian.Uint32(data[p+4:]))
		payloadEnd := p + 8 + n
		if n < 0 || payloadEnd > len(data) {
			return nil, errMalformed
		}
		padded := min(payloadEnd+(n&1), len(data)) // chunks are padded to even length
		switch typ {
		case "EXIF", "XMP ":
			// dropped
		case "VP8X":
			chunk := append([]byte(nil), data[p:padded]...)
			chunk[8] &^= webpFlagEXIF | webpFlagXMP
			out = append(out, chunk...)
		default:
			out = append(out, data[p:padded]...)
		}
		p = padded
	}
	binary.LittleEndian.PutUint32(out[4:8], uint32(len(out)-8))
	return out, nil
}
