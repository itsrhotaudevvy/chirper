module chirper.dev/core

go 1.26.0

replace (
	chirper.dev/connectors/discord => ../connectors/discord
	chirper.dev/connectors/fluxer => ../connectors/fluxer
	github.com/bwmarrin/discordgo => github.com/beeper/discordgo v0.0.0-20261104162414-49256f3e1ca7
	github.com/imroc/req/v3 => github.com/beeper/req/v3 v3.0.0-20260925135712-b49593750e56
	github.com/slack-go/slack => github.com/beeper/slackgo v0.0.0-20261109170826-c51406334f5e
	go.mau.fi/mautrix-bluesky => ../../third_party/bluesky
	go.mau.fi/mautrix-gmessages => ../../third_party/gmessages
	go.mau.fi/mautrix-gvoice => ../../third_party/gvoice
	go.mau.fi/mautrix-linkedin => ../../third_party/linkedin
	go.mau.fi/mautrix-meta => ../../third_party/meta
	go.mau.fi/mautrix-signal => ../../third_party/signal
	go.mau.fi/mautrix-slack => ../../third_party/slack
	go.mau.fi/mautrix-telegram => ../../third_party/telegram
	go.mau.fi/mautrix-twitter => ../../third_party/twitter
	go.mau.fi/mautrix-whatsapp => ../../third_party/whatsapp
	maunium.net/go/mautrix => ../../third_party/mautrix-go
)

require (
	chirper.dev/connectors/discord v0.0.0-00010101000000-000000000000
	chirper.dev/connectors/fluxer v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-bluesky v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-gmessages v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-gvoice v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-linkedin v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-meta v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-signal v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-slack v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-telegram v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-twitter v0.0.0-00010101000000-000000000000
	go.mau.fi/mautrix-whatsapp v0.0.0-00010101000000-000000000000
)

require (
	github.com/google/uuid v1.6.0
	github.com/mattn/go-sqlite3 v1.14.53
	github.com/rs/zerolog v1.35.1
	github.com/skip2/go-qrcode v0.0.0-20200617195104-da1b6568686e
	go.mau.fi/util v0.11.1-0.20261026174803-9e6b9c87cfbc
	golang.org/x/image v0.48.0
	gopkg.in/natefinch/lumberjack.v2 v2.2.1
	gopkg.in/yaml.v3 v3.0.1
	maunium.net/go/mautrix v0.32.1-0.20261111005907-b2652e840224
)

require (
	filippo.io/edwards25519 v1.2.0 // indirect
	github.com/PuerkitoBio/goquery v1.13.0 // indirect
	github.com/andybalholm/brotli v1.2.7 // indirect
	github.com/andybalholm/cascadia v1.3.4 // indirect
	github.com/apache/thrift v0.24.0 // indirect
	github.com/armfazh/h2c-go-ref v0.0.0-20210215173008-07e12a6f8e0d // indirect
	github.com/armfazh/tozan-ecc v0.1.3 // indirect
	github.com/beeper/argo-go v1.1.2 // indirect
	github.com/beeper/poly1305 v0.0.0-20250815183548-d4eede7bbf3c // indirect
	github.com/beorn7/perks v1.0.1 // indirect
	github.com/bluesky-social/indigo v0.0.0-20260415173638-b5f5bed0bd3c // indirect
	github.com/bwesterb/go-ristretto v1.2.4 // indirect
	github.com/bwmarrin/discordgo v0.0.0-00010101000000-000000000000 // indirect
	github.com/bytemare/cryptotools v0.0.0-20210309005813-f9985b05742f // indirect
	github.com/bytemare/opaque v0.0.0-20210310201842-16a10d180656 // indirect
	github.com/bytemare/voprf v0.0.0-20210221170502-f0af88d24d16 // indirect
	github.com/cenkalti/backoff/v4 v4.3.0 // indirect
	github.com/cespare/xxhash/v2 v2.3.0 // indirect
	github.com/coder/websocket v1.8.15 // indirect
	github.com/coreos/go-systemd/v22 v22.7.0 // indirect
	github.com/decred/dcrd/dcrec/secp256k1/v4 v4.4.0 // indirect
	github.com/earthboundkid/versioninfo/v2 v2.24.1 // indirect
	github.com/elliotchance/orderedmap/v3 v3.1.0 // indirect
	github.com/emersion/go-vcard v0.0.0-20241024213814-c9703dde27ff // indirect
	github.com/felixge/httpsnoop v1.1.0 // indirect
	github.com/fxamacker/cbor/v2 v2.9.4 // indirect
	github.com/gabriel-vasile/mimetype v1.4.16 // indirect
	github.com/go-faster/errors v0.7.1 // indirect
	github.com/go-faster/jx v1.2.0 // indirect
	github.com/go-faster/xor v1.0.0 // indirect
	github.com/go-logr/logr v1.4.4 // indirect
	github.com/go-logr/stdr v1.2.2 // indirect
	github.com/goccy/go-json v0.10.3 // indirect
	github.com/gogo/protobuf v1.3.2 // indirect
	github.com/google/flatbuffers v25.12.19+incompatible // indirect
	github.com/google/go-querystring v1.2.0 // indirect
	github.com/gorilla/websocket v1.5.3 // indirect
	github.com/gotd/ige v0.2.2 // indirect
	github.com/gotd/neo v0.1.5 // indirect
	github.com/gtank/ristretto255 v0.2.0 // indirect
	github.com/hashicorp/go-cleanhttp v0.5.2 // indirect
	github.com/hashicorp/go-retryablehttp v0.7.7 // indirect
	github.com/hashicorp/golang-lru v1.0.2 // indirect
	github.com/hashicorp/golang-lru/v2 v2.0.7 // indirect
	github.com/icholy/digest v1.2.0 // indirect
	github.com/imroc/req/v3 v3.56.0 // indirect
	github.com/ipfs/bbloom v0.0.4 // indirect
	github.com/ipfs/go-block-format v0.2.0 // indirect
	github.com/ipfs/go-cid v0.4.1 // indirect
	github.com/ipfs/go-datastore v0.6.0 // indirect
	github.com/ipfs/go-ipfs-blockstore v1.3.1 // indirect
	github.com/ipfs/go-ipfs-ds-help v1.1.1 // indirect
	github.com/ipfs/go-ipfs-util v0.0.3 // indirect
	github.com/ipfs/go-ipld-cbor v0.1.0 // indirect
	github.com/ipfs/go-ipld-format v0.6.0 // indirect
	github.com/ipfs/go-log v1.0.5 // indirect
	github.com/ipfs/go-log/v2 v2.5.1 // indirect
	github.com/ipfs/go-metrics-interface v0.0.1 // indirect
	github.com/jbenet/goprocess v0.1.4 // indirect
	github.com/klauspost/compress v1.20.2 // indirect
	github.com/klauspost/cpuid/v2 v2.2.7 // indirect
	github.com/lestrrat-go/blackmagic v1.0.3 // indirect
	github.com/lestrrat-go/httpcc v1.0.1 // indirect
	github.com/lestrrat-go/httprc v1.0.6 // indirect
	github.com/lestrrat-go/iter v1.0.2 // indirect
	github.com/lestrrat-go/jwx/v2 v2.1.6 // indirect
	github.com/lestrrat-go/option v1.0.1 // indirect
	github.com/lib/pq v1.12.3 // indirect
	github.com/mattn/go-colorable v0.1.15 // indirect
	github.com/mattn/go-isatty v0.0.20 // indirect
	github.com/mattn/go-pointer v0.0.1 // indirect
	github.com/matttproud/golang_protobuf_extensions/v2 v2.0.0 // indirect
	github.com/minio/sha256-simd v1.0.1 // indirect
	github.com/mr-tron/base58 v1.2.0 // indirect
	github.com/multiformats/go-base32 v0.1.0 // indirect
	github.com/multiformats/go-base36 v0.2.0 // indirect
	github.com/multiformats/go-multibase v0.2.0 // indirect
	github.com/multiformats/go-multihash v0.2.3 // indirect
	github.com/multiformats/go-varint v0.0.7 // indirect
	github.com/opentracing/opentracing-go v1.2.0 // indirect
	github.com/openziti/secretstream v0.1.53 // indirect
	github.com/petermattis/goid v0.0.0-20260918085751-abfca077860b // indirect
	github.com/polydawn/refmt v0.89.1-0.20221221234430-40501e09de1f // indirect
	github.com/prometheus/client_golang v1.17.0 // indirect
	github.com/prometheus/client_model v0.5.0 // indirect
	github.com/prometheus/common v0.45.0 // indirect
	github.com/prometheus/procfs v0.12.0 // indirect
	github.com/quic-go/qpack v0.6.0 // indirect
	github.com/quic-go/quic-go v0.64.1 // indirect
	github.com/refraction-networking/utls v1.8.2 // indirect
	github.com/rs/xid v1.6.0 // indirect
	github.com/segmentio/asm v1.2.1 // indirect
	github.com/slack-go/slack v0.24.0 // indirect
	github.com/spaolacci/murmur3 v1.1.0 // indirect
	github.com/tidwall/gjson v1.20.0 // indirect
	github.com/tidwall/match v1.2.0 // indirect
	github.com/tidwall/pretty v1.2.1 // indirect
	github.com/tidwall/sjson v1.2.5 // indirect
	github.com/vektah/gqlparser/v2 v2.5.27 // indirect
	github.com/vmihailenco/msgpack/v5 v5.4.1 // indirect
	github.com/vmihailenco/tagparser/v2 v2.0.0 // indirect
	github.com/whyrusleeping/cbor-gen v0.2.1-0.20241030202151-b7a6831be65e // indirect
	github.com/whyrusleeping/go-did v0.0.0-20240828165449-bcaa7ae21371 // indirect
	github.com/x448/float16 v0.8.4 // indirect
	github.com/yuin/goldmark v1.8.6 // indirect
	gitlab.com/yawning/secp256k1-voi v0.0.0-20230925100816-f2616030848b // indirect
	gitlab.com/yawning/tuplehash v0.0.0-20230713102510-df83abbf9a02 // indirect
	go.mau.fi/libsignal v0.2.3-0.20261103023201-5ce3f7a54370 // indirect
	go.mau.fi/webp v0.3.0 // indirect
	go.mau.fi/whatsmeow v0.0.0-20261027104441-1eec1136a50b // indirect
	go.mau.fi/zeroconfig v0.2.0 // indirect
	go.mau.fi/zerozap v0.1.2 // indirect
	go.opentelemetry.io/auto/sdk v1.2.1 // indirect
	go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp v0.69.0 // indirect
	go.opentelemetry.io/otel v1.45.0 // indirect
	go.opentelemetry.io/otel/metric v1.45.0 // indirect
	go.opentelemetry.io/otel/trace v1.45.0 // indirect
	go.uber.org/atomic v1.11.0 // indirect
	go.uber.org/multierr v1.11.0 // indirect
	go.uber.org/zap v1.28.0 // indirect
	golang.org/x/crypto v0.59.0 // indirect
	golang.org/x/exp v0.0.0-20261014143641-f0251eb4aa3e // indirect
	golang.org/x/mobile v0.0.0-20261011034528-197770444867 // indirect
	golang.org/x/mod v0.42.0 // indirect
	golang.org/x/net v0.62.0 // indirect
	golang.org/x/sync v0.25.0 // indirect
	golang.org/x/sys v0.50.0 // indirect
	golang.org/x/text v0.44.0 // indirect
	golang.org/x/time v0.16.0 // indirect
	golang.org/x/tools v0.52.0 // indirect
	golang.org/x/xerrors v0.0.0-20231012003039-104605ab7028 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20260706201446-f0a921348800 // indirect
	google.golang.org/grpc v1.84.0 // indirect
	google.golang.org/protobuf v1.36.12 // indirect
	lukechampine.com/blake3 v1.2.1 // indirect
	rsc.io/qr v0.2.0 // indirect
)

tool golang.org/x/mobile/cmd/gobind
