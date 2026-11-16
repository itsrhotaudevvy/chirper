package engine

import (
	"errors"
	"fmt"
	"image"
	"io/fs"
	"mime"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"

	// Register the decoders used to read image dimensions.
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// maxUploadBytes is the largest file SendMedia uploads.
const maxUploadBytes = 2 << 30

// fallbackMime is sent when the media type is missing, malformed or unsafe to display.
const fallbackMime = "application/octet-stream"

// mimePattern accepts only a top-level type of common kinds and a plain subtype.
var mimePattern = regexp.MustCompile(`^(image|video|audio|text|application)/[a-z0-9][a-z0-9.+-]{0,126}$`)

// activeMimes are types a viewer renders as active content (scripts, markup). They are never sent
// as declared; the bytes are served as an opaque file instead.
var activeMimes = map[string]bool{
	"text/html": true, "application/xhtml+xml": true, "image/svg+xml": true,
	"text/xml": true, "application/xml": true,
}

// safeMime returns the media type to upload. The declared type is used when it is well formed,
// otherwise the one of the file's extension, otherwise application/octet-stream.
func safeMime(declared, filePath string) string {
	if declared == "" {
		declared = mime.TypeByExtension(filepath.Ext(filePath))
	}
	mt, _, err := mime.ParseMediaType(declared)
	if err != nil || !mimePattern.MatchString(mt) || activeMimes[mt] {
		return fallbackMime
	}
	return mt
}

// maxFileNameRunes bounds the file name sent with a message.
const maxFileNameRunes = 255

// safeFileName returns the name to send with a file. Only the last path element of the declared
// name is kept, with control characters removed. The name is never used to build a path. Without
// a usable name, the base name of filePath is used.
func safeFileName(declared, filePath string) string {
	if name := baseName(declared); name != "" {
		return name
	}
	if name := baseName(filepath.Base(filePath)); name != "" {
		return name
	}
	return "file"
}

// baseName returns the last element of name without control characters, or "" when nothing is left.
func baseName(name string) string {
	name = path.Base(strings.ReplaceAll(name, `\`, "/"))
	name = strings.TrimSpace(strings.Map(func(r rune) rune {
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, name))
	if name == "" || name == "." || name == ".." || name == "/" {
		return ""
	}
	if utf8.RuneCountInString(name) > maxFileNameRunes {
		name = string([]rune(name)[:maxFileNameRunes])
	}
	return name
}

// checkUploadFile returns the file info of a file that may be sent. Symbolic links, folders and
// devices are refused, and so are files over maxUploadBytes.
func checkUploadFile(filePath string) (os.FileInfo, error) {
	info, err := os.Lstat(filePath)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, errors.New("file does not exist")
	} else if err != nil {
		return nil, errors.New("file cannot be read")
	}
	if !info.Mode().IsRegular() {
		return nil, errors.New("only regular files can be sent (not folders, links or devices)")
	}
	if info.Size() > maxUploadBytes {
		return nil, errors.New("file is larger than 2 GiB")
	}
	return info, nil
}

// openUploadFile opens filePath for upload and checks it is still the file that was checked, so
// that a file swapped for a link in between is not sent.
func openUploadFile(filePath string, checked os.FileInfo) (*os.File, error) {
	f, err := os.Open(filePath)
	if err != nil {
		return nil, errors.New("file cannot be read")
	}
	st, err := f.Stat()
	if err != nil || !os.SameFile(checked, st) {
		_ = f.Close()
		return nil, errors.New("file changed while it was being prepared")
	}
	return f, nil
}

// mediaContent builds the Matrix event content of an uploaded file. Voice messages are audio with
// the MSC3245 and MSC1767 markers; stickers use the m.sticker event type.
func mediaContent(up MediaUpload, uri id.ContentURIString, size int64, mimeType, name string) (*event.MessageEventContent, event.Type, error) {
	info := &event.FileInfo{MimeType: mimeType, Size: int(size)}
	body := up.Caption
	if body == "" {
		body = name
	}
	content := &event.MessageEventContent{Body: body, URL: uri, Info: info, FileName: name}
	evtType := event.EventMessage
	switch up.Kind {
	case "image":
		content.MsgType = event.MsgImage
		info.Width, info.Height = imageSize(up.FilePath)
	case "video":
		content.MsgType = event.MsgVideo
	case "audio":
		content.MsgType = event.MsgAudio
	case "voice":
		content.MsgType = event.MsgAudio
		content.MSC3245Voice = &event.MSC3245Voice{}
		content.MSC1767Audio = &event.MSC1767Audio{}
	case "file":
		content.MsgType = event.MsgFile
	case "sticker":
		evtType = event.EventSticker
		info.Width, info.Height = imageSize(up.FilePath)
	default:
		return nil, event.Type{}, fmt.Errorf("unsupported media kind %q", up.Kind)
	}
	return content, evtType, nil
}

// imageSize returns the pixel size of an image file, or zeros when it cannot be decoded.
func imageSize(path string) (int, int) {
	f, err := os.Open(path)
	if err != nil {
		return 0, 0
	}
	defer f.Close()
	cfg, _, err := image.DecodeConfig(f)
	if err != nil {
		return 0, 0
	}
	return cfg.Width, cfg.Height
}
