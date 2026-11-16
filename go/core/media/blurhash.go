package media

import (
	"image"
	"math"
	"strings"
)

// base83Alphabet is the digit set of the BlurHash specification.
const base83Alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~"

// blurhash encodes img as a BlurHash with cx x cy DCT components (1..9 each), following the
// reference algorithm at https://github.com/woltapp/blurhash.
func blurhash(img *image.NRGBA, cx, cy int) string {
	factors := dctFactors(img, cx, cy)

	var sb strings.Builder
	sb.WriteString(base83(cx-1+(cy-1)*9, 1))

	maxAC := 1.0
	quantMax := 0
	if len(factors) > 1 {
		actual := 0.0
		for _, f := range factors[1:] {
			for _, c := range f {
				actual = math.Max(actual, math.Abs(c))
			}
		}
		quantMax = clampInt(int(math.Floor(actual*166-0.5)), 0, 82)
		maxAC = float64(quantMax+1) / 166
	}
	sb.WriteString(base83(quantMax, 1))

	dc := factors[0]
	sb.WriteString(base83(linearToSRGB(dc[0])<<16+linearToSRGB(dc[1])<<8+linearToSRGB(dc[2]), 4))

	for _, f := range factors[1:] {
		sb.WriteString(base83(encodeAC(f, maxAC), 2))
	}
	return sb.String()
}

// dctFactors returns the cx*cy averaged linear-light cosine coefficients of img, row-major by y.
func dctFactors(img *image.NRGBA, cx, cy int) [][3]float64 {
	b := img.Bounds()
	w, h := b.Dx(), b.Dy()
	factors := make([][3]float64, 0, cx*cy)
	for j := 0; j < cy; j++ {
		for i := 0; i < cx; i++ {
			norm := 2.0
			if i == 0 && j == 0 {
				norm = 1
			}
			var r, g, bl float64
			for y := 0; y < h; y++ {
				ry := math.Cos(math.Pi * float64(j) * float64(y) / float64(h))
				for x := 0; x < w; x++ {
					basis := math.Cos(math.Pi*float64(i)*float64(x)/float64(w)) * ry
					off := img.PixOffset(b.Min.X+x, b.Min.Y+y)
					p := img.Pix[off : off+3 : off+3]
					r += basis * srgbToLinear(p[0])
					g += basis * srgbToLinear(p[1])
					bl += basis * srgbToLinear(p[2])
				}
			}
			scale := norm / float64(w*h)
			factors = append(factors, [3]float64{r * scale, g * scale, bl * scale})
		}
	}
	return factors
}

func encodeAC(f [3]float64, maxAC float64) int {
	quant := func(v float64) int {
		return clampInt(int(math.Floor(signPow(v/maxAC, 0.5)*9+9.5)), 0, 18)
	}
	return quant(f[0])*19*19 + quant(f[1])*19 + quant(f[2])
}

func base83(v, length int) string {
	out := make([]byte, length)
	for i := length - 1; i >= 0; i-- {
		out[i] = base83Alphabet[v%83]
		v /= 83
	}
	return string(out)
}

var srgbLinearTable = func() [256]float64 {
	var t [256]float64
	for i := range t {
		v := float64(i) / 255
		if v <= 0.04045 {
			t[i] = v / 12.92
		} else {
			t[i] = math.Pow((v+0.055)/1.055, 2.4)
		}
	}
	return t
}()

func srgbToLinear(c uint8) float64 { return srgbLinearTable[c] }

func linearToSRGB(v float64) int {
	v = math.Max(0, math.Min(1, v))
	if v <= 0.0031308 {
		return int(v*12.92*255 + 0.5)
	}
	return int((1.055*math.Pow(v, 1/2.4)-0.055)*255 + 0.5)
}

func signPow(v, e float64) float64 {
	return math.Copysign(math.Pow(math.Abs(v), e), v)
}

func clampInt(v, lo, hi int) int {
	return max(lo, min(hi, v))
}
