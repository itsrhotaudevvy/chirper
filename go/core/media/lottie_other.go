//go:build !(cgo && !nocgomedia && android)

package media

import "fmt"

// renderLottieToWebP is unavailable without the Android native build (rlottie).
func renderLottieToWebP(_ []byte) ([]byte, error) {
	return nil, fmt.Errorf("%w: Lottie rendering needs the Android native build", ErrUnsupported)
}
