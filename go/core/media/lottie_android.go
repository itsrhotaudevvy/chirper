//go:build cgo && !nocgomedia && android

package media

/*
#cgo android CFLAGS: -I${SRCDIR}/../../native/out/arm64-v8a/include
#cgo android LDFLAGS: -L${SRCDIR}/../../native/out/arm64-v8a/lib -lrlottie -lc++_static -lc++abi

#include <stdlib.h>
#include <stdint.h>
#include <rlottie_capi.h>
*/
import "C"

import (
	"errors"
	"fmt"
	"math"
	"unsafe"
)

const (
	maxLottieFrames = 300 // 5 seconds at 60 fps, the longest Telegram allows for animated stickers
	maxLottieSide   = 512
)

// renderLottieToWebP renders decompressed Lottie JSON with rlottie and encodes every frame into an
// animated WebP.
func renderLottieToWebP(js []byte) ([]byte, error) {
	cjs := C.CString(string(js))
	defer C.free(unsafe.Pointer(cjs))
	empty := C.CString("")
	defer C.free(unsafe.Pointer(empty))

	anim := C.lottie_animation_from_data(cjs, empty, empty)
	if anim == nil {
		return nil, errors.New("media: invalid lottie animation")
	}
	defer C.lottie_animation_destroy(anim)

	var cw, ch C.size_t
	C.lottie_animation_get_size(anim, &cw, &ch)
	w, h := int(cw), int(ch)
	frames := int(C.lottie_animation_get_totalframe(anim))
	fps := float64(C.lottie_animation_get_framerate(anim))
	if w <= 0 || h <= 0 || w > maxLottieSide || h > maxLottieSide {
		return nil, fmt.Errorf("media: lottie size %dx%d out of range", w, h)
	}
	if frames <= 0 || frames > maxLottieFrames || fps <= 0 {
		return nil, fmt.Errorf("media: lottie has %d frames at %.1f fps", frames, fps)
	}

	enc, err := newAnimEncoder(w, h)
	if err != nil {
		return nil, err
	}
	pixels := make([]uint32, w*h)
	rgba := make([]byte, w*h*4)
	for f := 0; f < frames; f++ {
		C.lottie_animation_render(anim, C.size_t(f), (*C.uint32_t)(unsafe.Pointer(&pixels[0])), cw, ch, C.size_t(w*4))
		unpremultiplyARGB(pixels, rgba)
		if err := enc.add(rgba, int(math.Round(float64(f)*1000/fps))); err != nil {
			return nil, err
		}
	}
	return enc.finish(int(math.Round(float64(frames) * 1000 / fps)))
}

// unpremultiplyARGB converts rlottie's premultiplied 0xAARRGGBB pixels to non-premultiplied RGBA bytes.
func unpremultiplyARGB(pixels []uint32, rgba []byte) {
	for i, px := range pixels {
		a := px >> 24
		r, g, b := (px>>16)&0xFF, (px>>8)&0xFF, px&0xFF
		if a != 0 && a != 0xFF {
			r = min(255, (r*255+a/2)/a)
			g = min(255, (g*255+a/2)/a)
			b = min(255, (b*255+a/2)/a)
		}
		if a == 0 {
			r, g, b = 0, 0, 0
		}
		rgba[i*4], rgba[i*4+1], rgba[i*4+2], rgba[i*4+3] = byte(r), byte(g), byte(b), byte(a)
	}
}
