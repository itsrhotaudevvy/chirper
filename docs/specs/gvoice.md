# Task `gvoice`: Google Voice in `crates/chirper-net-gvoice`

**Owned:** `crates/chirper-net-gvoice/**`. Read `README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/gvoice/pkg/` (`libgv/` protocol client incl.
`gvproto/` protobufs and `utf16chunk/`; `connector/` behaviour). Port the `.proto` files you need
as hand-written `prost` derive structs (no build script), with an attribution comment.

- Login flow `cookies`: `LoginStep::Cookies` on `https://voice.google.com/` with the cookie
  fields the reference requires (`SAPISID`, `__Secure-1PSID`, …, check `login.go`) and its
  `extract_js`; store them. Authorization header `SAPISIDHASH <ts>_<sha1(ts SAPISID origin)>` as
  the reference computes it (port and test with a fixed timestamp).
- Realtime: the reference's push/long-poll channel (`libgv` realtime / signaler). If it depends on
  the Electron helper (`electron.go`/`electron.mjs`) for a token, implement the HTTP-only path and
  report what the Electron part did.
- Threads list, messages, SMS/MMS send (text and image), mark read, delete; numbers are E.164
  strings (never log them).
