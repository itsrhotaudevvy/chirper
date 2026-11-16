// Package media converts and inspects media on the device: thumbnails, blurhash placeholders,
// metadata stripping, WebP stickers and Lottie (.tgs) stickers.
//
// Two implementations sit behind the same API. The native build (cgo, Android arm64 or macOS with
// Homebrew libwebp/jpeg-turbo) uses libjpeg-turbo for DCT-scaled JPEG decode and encode, libwebp for
// WebP and animated WebP encoding, and rlottie for Lottie rendering on Android. The pure-Go build
// (CGO_ENABLED=0, the nocgomedia tag, or other platforms) decodes and encodes JPEG with the standard
// library and returns ErrUnsupported for WebP encoding and Lottie rendering.
package media

import (
	"bytes"
	"compress/gzip"
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/draw"
	"io"

	xdraw "golang.org/x/image/draw"
	_ "golang.org/x/image/webp" // registers the WebP decoder with image.Decode
)

// ErrUnsupported is returned when the current build lacks the codec an operation needs.
var ErrUnsupported = errors.New("media: not supported by this build")

const (
	// maxPixels bounds decoded image size so hostile headers cannot exhaust memory.
	maxPixels = 40 << 20
	// maxLottieJSON bounds the decompressed .tgs payload.
	maxLottieJSON = 4 << 20
	// stickerSide is the side length Telegram requires for static and animated stickers.
	stickerSide = 512
	// blurhashSide is the working size for blurhash: the hash only encodes low frequencies.
	blurhashSide = 64
	thumbQuality = 85
	stickerWebP  = 90
)

// animEncoder accumulates RGBA frames into an animated WebP.
type animEncoder interface {
	// add appends one frame of w*h non-premultiplied RGBA bytes shown from tsMs.
	add(rgba []byte, tsMs int) error
	// finish returns the assembled animation; the encoder must not be used afterwards.
	finish(endMs int) ([]byte, error)
}

// Thumbnail returns a JPEG thumbnail of data that fits within maxSide x maxSide, preserving the
// aspect ratio. Images already small enough are not upscaled. Transparent pixels are flattened onto
// white. It returns the JPEG bytes and the thumbnail dimensions.
func Thumbnail(data []byte, maxSide int) ([]byte, int, int, error) {
	if maxSide <= 0 {
		return nil, 0, 0, errors.New("media: maxSide must be positive")
	}
	if err := checkDimensions(data); err != nil {
		return nil, 0, 0, err
	}
	src, err := decodeJPEGOrAny(data, maxSide)
	if err != nil {
		return nil, 0, 0, err
	}
	w, h := src.Bounds().Dx(), src.Bounds().Dy()
	if max(w, h) > maxSide {
		w, h = fitSize(w, h, maxSide)
		src = resize(src, w, h)
	}
	jpegBytes, err := encodeJPEG(flatten(src), thumbQuality)
	if err != nil {
		return nil, 0, 0, err
	}
	return jpegBytes, w, h, nil
}

// Blurhash returns the BlurHash (4x3 components) placeholder for an image.
func Blurhash(data []byte) (string, error) {
	if err := checkDimensions(data); err != nil {
		return "", err
	}
	src, err := decodeAny(data)
	if err != nil {
		return "", err
	}
	if w, h := src.Bounds().Dx(), src.Bounds().Dy(); max(w, h) > blurhashSide {
		nw, nh := fitSize(w, h, blurhashSide)
		src = resize(src, nw, nh)
	}
	return blurhash(src, 4, 3), nil
}

// ToWebPSticker converts a static image (PNG, JPEG, GIF's first frame or WebP) to a 512px WebP
// sticker that keeps transparency. The longer side is scaled to 512 pixels.
func ToWebPSticker(data []byte) ([]byte, error) {
	if err := checkDimensions(data); err != nil {
		return nil, err
	}
	src, err := decodeAny(data)
	if err != nil {
		return nil, err
	}
	w, h := fitSize(src.Bounds().Dx(), src.Bounds().Dy(), stickerSide)
	return encodeWebP(resize(src, w, h), stickerWebP)
}

// TGSToAnimatedWebP renders a gzip-compressed Lottie animation (Telegram .tgs) to an animated WebP
// sticker at 512px, at most 300 frames. Rendering needs the native build on Android.
func TGSToAnimatedWebP(tgs []byte) ([]byte, error) {
	zr, err := gzip.NewReader(bytes.NewReader(tgs))
	if err != nil {
		return nil, fmt.Errorf("media: tgs is not gzip: %w", err)
	}
	js, err := io.ReadAll(io.LimitReader(zr, maxLottieJSON+1))
	if err != nil {
		return nil, fmt.Errorf("media: read tgs: %w", err)
	}
	if len(js) > maxLottieJSON {
		return nil, errors.New("media: tgs animation too large")
	}
	return renderLottieToWebP(js)
}

// checkDimensions rejects images whose declared size exceeds maxPixels before any pixel is decoded.
func checkDimensions(data []byte) error {
	cfg, _, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return fmt.Errorf("media: unrecognised image: %w", err)
	}
	if cfg.Width <= 0 || cfg.Height <= 0 || cfg.Width*cfg.Height > maxPixels {
		return fmt.Errorf("media: image dimensions %dx%d out of range", cfg.Width, cfg.Height)
	}
	return nil
}

// decodeJPEGOrAny uses the DCT-scaled JPEG path when the data is JPEG and the generic decoder otherwise.
func decodeJPEGOrAny(data []byte, maxSide int) (*image.NRGBA, error) {
	if isJPEG(data) {
		return decodeJPEG(data, maxSide)
	}
	return decodeAny(data)
}

func isJPEG(data []byte) bool {
	return len(data) > 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

// decodeAny decodes the first frame of any registered format into non-premultiplied RGBA.
func decodeAny(data []byte) (*image.NRGBA, error) {
	img, _, err := image.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("media: decode image: %w", err)
	}
	return toNRGBA(img), nil
}

func toNRGBA(img image.Image) *image.NRGBA {
	if n, ok := img.(*image.NRGBA); ok {
		return n
	}
	dst := image.NewNRGBA(img.Bounds())
	draw.Draw(dst, dst.Bounds(), img, img.Bounds().Min, draw.Src)
	return dst
}

// flatten composites an image onto white, producing an opaque image for JPEG output.
func flatten(src *image.NRGBA) *image.NRGBA {
	dst := image.NewNRGBA(src.Bounds())
	draw.Draw(dst, dst.Bounds(), image.NewUniform(color.White), image.Point{}, draw.Src)
	draw.Draw(dst, dst.Bounds(), src, src.Bounds().Min, draw.Over)
	return dst
}

// resize scales src to w x h with Catmull-Rom filtering.
func resize(src *image.NRGBA, w, h int) *image.NRGBA {
	dst := image.NewNRGBA(image.Rect(0, 0, w, h))
	xdraw.CatmullRom.Scale(dst, dst.Bounds(), src, src.Bounds(), draw.Src, nil)
	return dst
}

// fitSize returns the dimensions of a w x h image scaled so its longer side equals side.
func fitSize(w, h, side int) (int, int) {
	if w >= h {
		return side, max(1, h*side/w)
	}
	return max(1, w*side/h), side
}
