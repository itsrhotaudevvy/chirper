//go:build !(cgo && !nocgomedia && (android || darwin))

package media

import (
	"bytes"
	"fmt"
	"image"
	"image/jpeg"
)

// decodeJPEG decodes with the standard library; without DCT scaling the full image is returned.
func decodeJPEG(data []byte, _ int) (*image.NRGBA, error) {
	img, err := jpeg.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("media: decode jpeg: %w", err)
	}
	return toNRGBA(img), nil
}

func encodeJPEG(img *image.NRGBA, quality int) ([]byte, error) {
	var buf bytes.Buffer
	if err := jpeg.Encode(&buf, img, &jpeg.Options{Quality: quality}); err != nil {
		return nil, fmt.Errorf("media: encode jpeg: %w", err)
	}
	return buf.Bytes(), nil
}

func encodeWebP(_ *image.NRGBA, _ float32) ([]byte, error) {
	return nil, fmt.Errorf("%w: WebP encoding needs the native build", ErrUnsupported)
}

func newAnimEncoder(_, _ int) (animEncoder, error) {
	return nil, fmt.Errorf("%w: animated WebP encoding needs the native build", ErrUnsupported)
}
