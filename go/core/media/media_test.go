package media

import (
	"bytes"
	"compress/gzip"
	"encoding/binary"
	"errors"
	"hash/crc32"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"strings"
	"testing"

	_ "golang.org/x/image/webp"
)

// gradient returns a w x h image with a colour ramp, so resampling and encoding have real content.
func gradient(w, h int) *image.NRGBA {
	img := image.NewNRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, color.NRGBA{R: uint8(x * 255 / w), G: uint8(y * 255 / h), B: 128, A: 255})
		}
	}
	return img
}

func encodeStdJPEG(t *testing.T, img image.Image) []byte {
	t.Helper()
	var buf bytes.Buffer
	if err := jpeg.Encode(&buf, img, &jpeg.Options{Quality: 80}); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func encodeStdPNG(t *testing.T, img image.Image) []byte {
	t.Helper()
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

// jpegSegment returns a JPEG marker segment (marker byte m) with the given payload.
func jpegSegment(m byte, payload []byte) []byte {
	out := []byte{0xFF, m, 0, 0}
	binary.BigEndian.PutUint16(out[2:], uint16(len(payload)+2))
	return append(out, payload...)
}

// gpsExifPayload is an APP1 EXIF payload whose IFD0 points at a GPS IFD with a latitude reference.
func gpsExifPayload() []byte {
	tiff := []byte("II*\x00")
	tiff = binary.LittleEndian.AppendUint32(tiff, 8)        // IFD0 offset
	tiff = binary.LittleEndian.AppendUint16(tiff, 1)        // IFD0 entry count
	tiff = binary.LittleEndian.AppendUint16(tiff, 0x8825)   // GPSInfo tag
	tiff = binary.LittleEndian.AppendUint16(tiff, 4)        // LONG
	tiff = binary.LittleEndian.AppendUint32(tiff, 1)        // count
	tiff = binary.LittleEndian.AppendUint32(tiff, 8+2+12+4) // GPS IFD offset
	tiff = binary.LittleEndian.AppendUint32(tiff, 0)        // next IFD
	tiff = binary.LittleEndian.AppendUint16(tiff, 1)        // GPS entry count
	tiff = binary.LittleEndian.AppendUint16(tiff, 0x0001)   // GPSLatitudeRef
	tiff = binary.LittleEndian.AppendUint16(tiff, 2)        // ASCII
	tiff = binary.LittleEndian.AppendUint32(tiff, 2)        // "N\x00"
	tiff = append(tiff, 'N', 0, 0, 0)                       // inline value
	tiff = binary.LittleEndian.AppendUint32(tiff, 0)        // next IFD
	return append([]byte("Exif\x00\x00"), tiff...)
}

var iccPayload = append([]byte(iccProfileTag+"\x01\x01"), []byte("ICC-TEST-PROFILE-BYTES")...)

// jpegWithMetadata inserts GPS EXIF and ICC APP segments after the SOI marker of a standard JPEG.
func jpegWithMetadata(t *testing.T) (data, exif []byte) {
	t.Helper()
	std := encodeStdJPEG(t, gradient(64, 48))
	exif = jpegSegment(0xE1, gpsExifPayload())
	out := []byte{0xFF, 0xD8}
	out = append(out, exif...)
	out = append(out, jpegSegment(0xE2, iccPayload)...)
	out = append(out, std[2:]...)
	return out, exif
}

func TestStripJPEGRemovesGPSKeepsICC(t *testing.T) {
	data, exif := jpegWithMetadata(t)
	if !bytes.Contains(data, exif) {
		t.Fatal("fixture does not contain the EXIF segment")
	}

	out, err := StripMetadata(data, "image/jpeg")
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(out, exif) || bytes.Contains(out, []byte("Exif\x00\x00")) {
		t.Error("EXIF segment with GPS data survived stripping")
	}
	if !bytes.Contains(out, iccPayload) {
		t.Error("ICC profile segment was removed")
	}
	cfg, err := jpeg.DecodeConfig(bytes.NewReader(out))
	if err != nil {
		t.Fatalf("stripped JPEG does not decode: %v", err)
	}
	if cfg.Width != 64 || cfg.Height != 48 {
		t.Errorf("stripped JPEG is %dx%d, want 64x48", cfg.Width, cfg.Height)
	}
}

func TestStripJPEGDropsCommentsAndKeepsJFIF(t *testing.T) {
	std := encodeStdJPEG(t, gradient(16, 16))
	data := []byte{0xFF, 0xD8}
	data = append(data, jpegSegment(0xFE, []byte("author: private"))...)
	data = append(data, std[2:]...)

	out, err := StripMetadata(data, "image/jpeg")
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(out, []byte("author: private")) {
		t.Error("JPEG comment survived stripping")
	}
	if _, err := jpeg.Decode(bytes.NewReader(out)); err != nil {
		t.Errorf("stripped JPEG does not decode: %v", err)
	}
}

func pngChunk(typ string, data []byte) []byte {
	out := binary.BigEndian.AppendUint32(nil, uint32(len(data)))
	out = append(out, typ...)
	out = append(out, data...)
	return binary.BigEndian.AppendUint32(out, crc32.ChecksumIEEE(out[4:]))
}

func TestStripPNGRemovesTextAndEXIFKeepsICC(t *testing.T) {
	std := encodeStdPNG(t, gradient(20, 10))
	const ihdrEnd = 8 + 25 // signature plus the 25-byte IHDR chunk
	text := pngChunk("tEXt", []byte("Location\x00GPS 47.6062 N, 122.3321 W"))
	exif := pngChunk("eXIf", []byte("II*\x00private-exif"))
	icc := pngChunk("iCCP", []byte("sRGB-ish\x00\x00ICC-TEST-PROFILE-BYTES"))

	data := append([]byte(nil), std[:ihdrEnd]...)
	data = append(data, text...)
	data = append(data, exif...)
	data = append(data, icc...)
	data = append(data, std[ihdrEnd:]...)

	out, err := StripMetadata(data, "image/png")
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(out, []byte("GPS 47.6062")) || bytes.Contains(out, []byte("private-exif")) {
		t.Error("PNG text or eXIf chunk survived stripping")
	}
	if !bytes.Contains(out, icc) {
		t.Error("iCCP chunk was removed")
	}
	img, err := png.Decode(bytes.NewReader(out))
	if err != nil {
		t.Fatalf("stripped PNG does not decode: %v", err)
	}
	if img.Bounds().Dx() != 20 || img.Bounds().Dy() != 10 {
		t.Errorf("stripped PNG is %v, want 20x10", img.Bounds())
	}
}

func webpChunk(typ string, payload []byte) []byte {
	out := append([]byte(typ), 0, 0, 0, 0)
	binary.LittleEndian.PutUint32(out[4:], uint32(len(payload)))
	out = append(out, payload...)
	if len(payload)%2 == 1 {
		out = append(out, 0)
	}
	return out
}

func riffWebP(chunks ...[]byte) []byte {
	body := []byte("WEBP")
	for _, c := range chunks {
		body = append(body, c...)
	}
	out := append([]byte("RIFF"), 0, 0, 0, 0)
	binary.LittleEndian.PutUint32(out[4:], uint32(len(body)))
	return append(out, body...)
}

func TestStripWebPRemovesEXIFXMPKeepsICC(t *testing.T) {
	vp8x := []byte{0x2C, 0, 0, 0, 0, 0, 0, 0, 0, 0} // ICC, alpha, EXIF, XMP flags set
	icc := webpChunk("ICCP", []byte("ICC-TEST-PROFILE"))
	image := webpChunk("VP8L", []byte{0x2F, 0, 0, 0, 0})
	exif := webpChunk("EXIF", []byte("gps-exif-private"))
	xmp := webpChunk("XMP ", []byte("<x:xmpmeta>private</x:xmpmeta>"))
	data := riffWebP(webpChunk("VP8X", vp8x), icc, exif, image, xmp)

	out, err := StripMetadata(data, "image/webp")
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(out, []byte("gps-exif-private")) || bytes.Contains(out, []byte("private</x:xmpmeta>")) {
		t.Error("EXIF or XMP chunk survived stripping")
	}
	if !bytes.Contains(out, icc) || !bytes.Contains(out, image) {
		t.Error("ICCP or image chunk was removed")
	}
	if got := binary.LittleEndian.Uint32(out[4:8]); int(got) != len(out)-8 {
		t.Errorf("RIFF size %d, want %d", got, len(out)-8)
	}
	if flags := out[20]; flags&(webpFlagEXIF|webpFlagXMP) != 0 || flags&0x20 == 0 {
		t.Errorf("VP8X flags %#x: EXIF/XMP bits must clear and ICC must stay", flags)
	}
	if _, err := StripMetadata(out, "image/webp"); err != nil {
		t.Errorf("stripped WebP is not accepted again: %v", err)
	}
}

func TestStripMetadataRejectsMismatchedOrUnknownType(t *testing.T) {
	png := encodeStdPNG(t, gradient(4, 4))
	if _, err := StripMetadata(png, "image/jpeg"); !errors.Is(err, ErrUnsupported) {
		t.Errorf("PNG bytes labelled as JPEG: err = %v, want ErrUnsupported", err)
	}
	if _, err := StripMetadata(png, "video/mp4"); !errors.Is(err, ErrUnsupported) {
		t.Errorf("unknown type: err = %v, want ErrUnsupported", err)
	}
}

func TestThumbnailSizes(t *testing.T) {
	tests := []struct {
		name         string
		w, h, maxSde int
		wantW, wantH int
	}{
		{"landscape downscaled", 800, 400, 200, 200, 100},
		{"portrait downscaled", 100, 300, 200, 66, 200},
		{"small image not upscaled", 50, 40, 200, 50, 40},
		{"exact fit unchanged", 200, 120, 200, 200, 120},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			src := encodeStdJPEG(t, gradient(tc.w, tc.h))
			thumb, w, h, err := Thumbnail(src, tc.maxSde)
			if err != nil {
				t.Fatal(err)
			}
			if w != tc.wantW || h != tc.wantH {
				t.Errorf("thumbnail reported %dx%d, want %dx%d", w, h, tc.wantW, tc.wantH)
			}
			cfg, err := jpeg.DecodeConfig(bytes.NewReader(thumb))
			if err != nil {
				t.Fatalf("thumbnail is not JPEG: %v", err)
			}
			if cfg.Width != w || cfg.Height != h {
				t.Errorf("thumbnail bytes are %dx%d, reported %dx%d", cfg.Width, cfg.Height, w, h)
			}
		})
	}
}

func TestThumbnailFromPNGFlattensTransparency(t *testing.T) {
	img := image.NewNRGBA(image.Rect(0, 0, 300, 150))
	for i := range img.Pix {
		img.Pix[i] = 0 // fully transparent black
	}
	thumb, w, h, err := Thumbnail(encodeStdPNG(t, img), 100)
	if err != nil {
		t.Fatal(err)
	}
	if w != 100 || h != 50 {
		t.Errorf("thumbnail %dx%d, want 100x50", w, h)
	}
	decoded, err := jpeg.Decode(bytes.NewReader(thumb))
	if err != nil {
		t.Fatal(err)
	}
	r, g, b, _ := decoded.At(50, 25).RGBA()
	if r>>8 < 240 || g>>8 < 240 || b>>8 < 240 {
		t.Errorf("transparent pixel flattened to %d,%d,%d, want white", r>>8, g>>8, b>>8)
	}
}

func TestThumbnailRejectsBadInput(t *testing.T) {
	if _, _, _, err := Thumbnail(encodeStdPNG(t, gradient(4, 4)), 0); err == nil {
		t.Error("maxSide 0 accepted")
	}
	if _, _, _, err := Thumbnail([]byte("not an image"), 100); err == nil {
		t.Error("garbage accepted")
	}
}

func TestBlurhashSolidWhiteMatchesReference(t *testing.T) {
	img := image.NewNRGBA(image.Rect(0, 0, 1, 1))
	img.Pix = []byte{255, 255, 255, 255}
	// Size flag 0 (one component), quantised max 0, DC 0xFFFFFF encoded in base83.
	if got := blurhash(img, 1, 1); got != "00TSUA" {
		t.Errorf("blurhash = %q, want %q", got, "00TSUA")
	}
}

func TestBlurhashLengthAndStability(t *testing.T) {
	hash, err := Blurhash(encodeStdJPEG(t, gradient(120, 80)))
	if err != nil {
		t.Fatal(err)
	}
	// 4x3 components: 1 size + 1 max + 4 DC + 2 per AC component.
	if want := 6 + 2*(4*3-1); len(hash) != want {
		t.Errorf("hash length %d, want %d", len(hash), want)
	}
	again, _ := Blurhash(encodeStdJPEG(t, gradient(120, 80)))
	if hash != again {
		t.Error("blurhash is not deterministic")
	}
	if strings.ContainsAny(hash, " \"\\") {
		t.Errorf("hash %q contains characters outside base83", hash)
	}
}

func TestToWebPSticker(t *testing.T) {
	out, err := ToWebPSticker(encodeStdPNG(t, gradient(800, 600)))
	if errors.Is(err, ErrUnsupported) {
		t.Skip("WebP encoding not built: " + err.Error())
	}
	if err != nil {
		t.Fatal(err)
	}
	img, format, err := image.Decode(bytes.NewReader(out))
	if err != nil {
		t.Fatalf("sticker does not decode: %v", err)
	}
	if format != "webp" || img.Bounds().Dx() != 512 || img.Bounds().Dy() != 384 {
		t.Errorf("sticker is %s %v, want webp 512x384", format, img.Bounds())
	}
}

// lottieSquare is a minimal Lottie document: a red square that grows over one second.
const lottieSquare = `{"v":"5.7.4","fr":30,"ip":0,"op":30,"w":256,"h":256,"layers":[{"ddd":0,"ind":1,"ty":4,"nm":"s","sr":1,
"ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},"p":{"a":0,"k":[128,128,0]},"a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]}},
"ao":0,"shapes":[{"ty":"gr","it":[{"ty":"rc","d":1,"p":{"a":0,"k":[0,0]},"s":{"a":1,"k":[{"t":0,"s":[40,40]},{"t":30,"s":[200,200]}]},"r":{"a":0,"k":0},"nm":"r"},
{"ty":"fl","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100},"r":1,"nm":"f"},{"ty":"tr","p":{"a":0,"k":[0,0]},"a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100},"sk":{"a":0,"k":0},"sa":{"a":0,"k":0},"nm":"t"}],"nm":"g"}],
"ip":0,"op":30,"st":0,"bm":0}]}`

func gzipBytes(t *testing.T, b []byte) []byte {
	t.Helper()
	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	if _, err := zw.Write(b); err != nil {
		t.Fatal(err)
	}
	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func TestTGSToAnimatedWebP(t *testing.T) {
	out, err := TGSToAnimatedWebP(gzipBytes(t, []byte(lottieSquare)))
	if errors.Is(err, ErrUnsupported) {
		t.Skip("Lottie rendering not built: " + err.Error())
	}
	if err != nil {
		t.Fatal(err)
	}
	if string(out[0:4]) != "RIFF" || string(out[8:12]) != "WEBP" || !bytes.Contains(out, []byte("ANIM")) {
		t.Fatal("output is not an animated WebP")
	}
	if frames := bytes.Count(out, []byte("ANMF")); frames < 2 {
		t.Errorf("animation has %d frames, want several", frames)
	}
}

func TestTGSRejectsNonGzip(t *testing.T) {
	if _, err := TGSToAnimatedWebP([]byte(lottieSquare)); err == nil {
		t.Error("plain JSON accepted as tgs")
	}
}
