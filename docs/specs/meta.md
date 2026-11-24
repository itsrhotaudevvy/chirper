# Task `meta`: Messenger and Instagram in `crates/chirper-net-meta`

**Owned:** `crates/chirper-net-meta/**` (two drivers: `Messenger`, network id `messenger`, and
`Instagram`, network id `instagram`; keep their existing ids, names and colours). Read
`README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/meta/pkg/` — `messagix/` is the protocol
client (`socket/` MQTT-over-websocket "lightspeed", `lightspeed/` + `table/` decoding of the
LS stored-procedure responses, `methods/`, `graphql/`, `cookies/`, `bloks/` logins, `crypto/`,
`e2ee-client.go` + `e2ee-register.go` for Messenger end-to-end encryption), `connector/` and
`igconnector/` the behaviour, `msgconv/` conversion, `metaid/` ids, `instameow/`.

**Login flows (parity, keep every id):** messenger: `facebook`, `messenger`, `messenger-lite`,
`messenger-lite-android`; instagram: `android`, `instagram`, `instagram-password`. Cookie flows
use `LoginStep::Cookies` with the reference's cookie names, URLs, user agents and extract JS;
password/bloks flows run the reference's request sequence and its 2FA/checkpoint steps as
`user_input` steps.

**Phase A (non-E2EE, do first, complete):** lightspeed MQTT socket (connect payload, topics,
publish/ack, reconnect), initial sync (`/ls_req` tasks and the LS table decoder), threads,
messages, reactions, edits, unsends, typing, read receipts, attachments (download; upload via the
reference's endpoint), backfill via the reference's fetch-messages task, Instagram's equivalent.

**Phase B (Messenger E2EE):** Messenger's encrypted chats run the WhatsApp multi-device protocol
(`e2ee-client.go`, whatsmeow "armadillo" messages). Depend on `chirper-net-whatsapp` (path
dependency; use its public modules: binary node codec, noise socket, JIDs) and
`chirper-signal-protocol`. Never edit those crates; if something you need is private there, list
it in your report and the lead will expose it. Start phase B only when the lead tells you the
WhatsApp task is done; until then encrypted threads produce one `Unsupported` notice
("End-to-end encrypted chat — this Chirper build cannot read it yet").

Capabilities as the Go build (all but voice). Tests: LS table decoder fixtures, MQTT framing,
login request sequences against a fake server, conversion fixtures, one fake end-to-end per driver.
