//go:build cgo && !nocgomedia && (android || darwin)

package media

/*
#cgo android CFLAGS: -I${SRCDIR}/../../native/out/arm64-v8a/include
#cgo android LDFLAGS: -L${SRCDIR}/../../native/out/arm64-v8a/lib -lwebpmux -lwebpdemux -lwebp -lsharpyuv -ljpeg
#cgo darwin CFLAGS: -I/opt/homebrew/opt/webp/include -I/opt/homebrew/opt/jpeg-turbo/include
#cgo darwin LDFLAGS: -L/opt/homebrew/opt/webp/lib -L/opt/homebrew/opt/jpeg-turbo/lib -lwebpmux -lwebpdemux -lwebp -lsharpyuv -ljpeg

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <jpeglib.h>
#include <webp/encode.h>
#include <webp/mux.h>

// libjpeg reports errors through error_exit, which must not return. The jump target is kept per call.
struct oc_jpeg_error {
	struct jpeg_error_mgr pub;
	jmp_buf jump;
};

static void oc_jpeg_error_exit(j_common_ptr cinfo) {
	struct oc_jpeg_error *e = (struct oc_jpeg_error *)cinfo->err;
	longjmp(e->jump, 1);
}

static void oc_jpeg_silent(j_common_ptr cinfo) {
	(void)cinfo;
}

// oc_jpeg_decode decodes JPEG bytes to packed RGB, using the largest DCT scale (1/1 .. 1/8) that keeps
// the longer side at or above maxSide. On success *out must be freed with free().
static int oc_jpeg_decode(const unsigned char *in, size_t inlen, int maxSide, unsigned char **out, int *w, int *h) {
	struct jpeg_decompress_struct cinfo;
	struct oc_jpeg_error jerr;
	*out = NULL;
	cinfo.err = jpeg_std_error(&jerr.pub);
	jerr.pub.error_exit = oc_jpeg_error_exit;
	jerr.pub.output_message = oc_jpeg_silent;
	if (setjmp(jerr.jump)) {
		jpeg_destroy_decompress(&cinfo);
		free(*out);
		*out = NULL;
		return -1;
	}
	jpeg_create_decompress(&cinfo);
	jpeg_mem_src(&cinfo, in, inlen);
	jpeg_read_header(&cinfo, TRUE);

	int longest = cinfo.image_width > cinfo.image_height ? (int)cinfo.image_width : (int)cinfo.image_height;
	int denom = 1;
	while (denom < 8 && longest / (denom * 2) >= maxSide) {
		denom *= 2;
	}
	cinfo.out_color_space = JCS_RGB;
	cinfo.scale_num = 1;
	cinfo.scale_denom = denom;
	jpeg_start_decompress(&cinfo);

	*w = (int)cinfo.output_width;
	*h = (int)cinfo.output_height;
	size_t stride = (size_t)*w * 3;
	*out = malloc(stride * (size_t)*h);
	if (*out == NULL) {
		jpeg_destroy_decompress(&cinfo);
		return -1;
	}
	while (cinfo.output_scanline < cinfo.output_height) {
		JSAMPROW row = *out + cinfo.output_scanline * stride;
		jpeg_read_scanlines(&cinfo, &row, 1);
	}
	jpeg_finish_decompress(&cinfo);
	jpeg_destroy_decompress(&cinfo);
	return 0;
}

// oc_jpeg_encode encodes packed RGB as baseline JPEG. On success *out must be freed with free().
static int oc_jpeg_encode(const unsigned char *rgb, int w, int h, int quality, unsigned char **out, unsigned long *outlen) {
	struct jpeg_compress_struct cinfo;
	struct oc_jpeg_error jerr;
	*out = NULL;
	*outlen = 0;
	cinfo.err = jpeg_std_error(&jerr.pub);
	jerr.pub.error_exit = oc_jpeg_error_exit;
	jerr.pub.output_message = oc_jpeg_silent;
	if (setjmp(jerr.jump)) {
		jpeg_destroy_compress(&cinfo);
		free(*out);
		*out = NULL;
		return -1;
	}
	jpeg_create_compress(&cinfo);
	jpeg_mem_dest(&cinfo, out, outlen);
	cinfo.image_width = (JDIMENSION)w;
	cinfo.image_height = (JDIMENSION)h;
	cinfo.input_components = 3;
	cinfo.in_color_space = JCS_RGB;
	jpeg_set_defaults(&cinfo);
	jpeg_set_quality(&cinfo, quality, TRUE);
	jpeg_start_compress(&cinfo, TRUE);
	while (cinfo.next_scanline < cinfo.image_height) {
		JSAMPROW row = (JSAMPROW)(rgb + cinfo.next_scanline * (size_t)w * 3);
		jpeg_write_scanlines(&cinfo, &row, 1);
	}
	jpeg_finish_compress(&cinfo);
	jpeg_destroy_compress(&cinfo);
	return 0;
}

// oc_webp_encode_rgba encodes packed non-premultiplied RGBA as lossy WebP. Returns the size, or 0 on
// failure. On success *out must be freed with WebPFree().
static size_t oc_webp_encode_rgba(const uint8_t *rgba, int w, int h, float quality, uint8_t **out) {
	return WebPEncodeRGBA(rgba, w, h, w * 4, quality, out);
}

static WebPAnimEncoder *oc_anim_new(int w, int h) {
	WebPAnimEncoderOptions opts;
	if (!WebPAnimEncoderOptionsInit(&opts)) {
		return NULL;
	}
	return WebPAnimEncoderNew(w, h, &opts);
}

static int oc_anim_add(WebPAnimEncoder *enc, const uint8_t *rgba, int w, int h, int tsMs, float quality) {
	WebPConfig cfg;
	WebPPicture pic;
	int ok = 0;
	if (!WebPConfigInit(&cfg) || !WebPPictureInit(&pic)) {
		return 0;
	}
	cfg.quality = quality;
	pic.width = w;
	pic.height = h;
	if (WebPPictureImportRGBA(&pic, rgba, w * 4)) {
		ok = WebPAnimEncoderAdd(enc, &pic, tsMs, &cfg);
	}
	WebPPictureFree(&pic);
	return ok;
}

// oc_anim_finish closes the animation at endMs and returns the WebP bytes (free with WebPFree).
static int oc_anim_finish(WebPAnimEncoder *enc, int endMs, uint8_t **out, size_t *outlen) {
	WebPData data;
	WebPDataInit(&data);
	if (!WebPAnimEncoderAdd(enc, NULL, endMs, NULL) || !WebPAnimEncoderAssemble(enc, &data)) {
		WebPDataClear(&data);
		return 0;
	}
	*out = (uint8_t *)data.bytes;
	*outlen = data.size;
	return 1;
}
*/
import "C"

import (
	"errors"
	"fmt"
	"image"
	"unsafe"
)

const animQuality = 80

// contiguousRGB returns img's pixels as packed RGB, dropping alpha.
func contiguousRGB(img *image.NRGBA) []byte {
	w, h := img.Bounds().Dx(), img.Bounds().Dy()
	rgb := make([]byte, w*h*3)
	for y := 0; y < h; y++ {
		row := img.Pix[y*img.Stride : y*img.Stride+w*4]
		out := rgb[y*w*3 : (y+1)*w*3]
		for x := 0; x < w; x++ {
			copy(out[x*3:x*3+3], row[x*4:x*4+3])
		}
	}
	return rgb
}

// contiguousRGBA returns img's pixels as packed non-premultiplied RGBA.
func contiguousRGBA(img *image.NRGBA) []byte {
	w, h := img.Bounds().Dx(), img.Bounds().Dy()
	out := make([]byte, w*h*4)
	for y := 0; y < h; y++ {
		copy(out[y*w*4:(y+1)*w*4], img.Pix[y*img.Stride:y*img.Stride+w*4])
	}
	return out
}

func decodeJPEG(data []byte, maxSide int) (*image.NRGBA, error) {
	if len(data) == 0 {
		return nil, errors.New("media: empty jpeg")
	}
	var out *C.uchar
	var w, h C.int
	if rc := C.oc_jpeg_decode((*C.uchar)(unsafe.Pointer(&data[0])), C.size_t(len(data)), C.int(maxSide), &out, &w, &h); rc != 0 {
		return nil, errors.New("media: jpeg decode failed")
	}
	defer C.free(unsafe.Pointer(out))
	rgb := unsafe.Slice((*byte)(unsafe.Pointer(out)), int(w)*int(h)*3)
	img := image.NewNRGBA(image.Rect(0, 0, int(w), int(h)))
	for i, j := 0, 0; i < len(rgb); i, j = i+3, j+4 {
		img.Pix[j], img.Pix[j+1], img.Pix[j+2], img.Pix[j+3] = rgb[i], rgb[i+1], rgb[i+2], 0xFF
	}
	return img, nil
}

func encodeJPEG(img *image.NRGBA, quality int) ([]byte, error) {
	w, h := img.Bounds().Dx(), img.Bounds().Dy()
	rgb := contiguousRGB(img)
	var out *C.uchar
	var outLen C.ulong
	if rc := C.oc_jpeg_encode((*C.uchar)(unsafe.Pointer(&rgb[0])), C.int(w), C.int(h), C.int(quality), &out, &outLen); rc != 0 {
		return nil, errors.New("media: jpeg encode failed")
	}
	defer C.free(unsafe.Pointer(out))
	return C.GoBytes(unsafe.Pointer(out), C.int(outLen)), nil
}

func encodeWebP(img *image.NRGBA, quality float32) ([]byte, error) {
	w, h := img.Bounds().Dx(), img.Bounds().Dy()
	rgba := contiguousRGBA(img)
	var out *C.uint8_t
	n := C.oc_webp_encode_rgba((*C.uint8_t)(unsafe.Pointer(&rgba[0])), C.int(w), C.int(h), C.float(quality), &out)
	if n == 0 {
		return nil, errors.New("media: webp encode failed")
	}
	defer C.WebPFree(unsafe.Pointer(out))
	return C.GoBytes(unsafe.Pointer(out), C.int(n)), nil
}

// nativeAnimEncoder wraps libwebpmux's WebPAnimEncoder.
type nativeAnimEncoder struct {
	enc  *C.WebPAnimEncoder
	w, h int
}

func newAnimEncoder(w, h int) (animEncoder, error) {
	enc := C.oc_anim_new(C.int(w), C.int(h))
	if enc == nil {
		return nil, fmt.Errorf("media: create animated webp encoder for %dx%d", w, h)
	}
	return &nativeAnimEncoder{enc: enc, w: w, h: h}, nil
}

func (e *nativeAnimEncoder) add(rgba []byte, tsMs int) error {
	if len(rgba) != e.w*e.h*4 {
		return fmt.Errorf("media: frame has %d bytes, want %d", len(rgba), e.w*e.h*4)
	}
	if C.oc_anim_add(e.enc, (*C.uint8_t)(unsafe.Pointer(&rgba[0])), C.int(e.w), C.int(e.h), C.int(tsMs), animQuality) == 0 {
		return errors.New("media: add animated webp frame")
	}
	return nil
}

func (e *nativeAnimEncoder) finish(endMs int) ([]byte, error) {
	defer C.WebPAnimEncoderDelete(e.enc)
	var out *C.uint8_t
	var n C.size_t
	if C.oc_anim_finish(e.enc, C.int(endMs), &out, &n) == 0 {
		return nil, errors.New("media: assemble animated webp")
	}
	defer C.WebPFree(unsafe.Pointer(out))
	return C.GoBytes(unsafe.Pointer(out), C.int(n)), nil
}
