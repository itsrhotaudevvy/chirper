// Command generate drives the Go libsignal implementation that WhatsApp uses, to produce wire
// vectors for the Rust crate, and verifies Rust-produced replies against it.
//
// Run from this directory. The build is offline: the module cache must hold go.mau.fi/libsignal
// and the dependencies pinned in go.mod.
//
//	export GOTOOLCHAIN=local GOPROXY=off GOFLAGS=-mod=mod GONOSUMDB='*'
//	go run . generate -out ../go_vectors.json
//	go run . verify -vectors ../go_vectors.json -replies ../rust_replies.json
//
// The identity keys and the prekeys are fixed: each private key is the SHA-256 of a label in
// this file, and the seeds are written to the output. The Go library generates the rest itself
// (the ratchet key of each session step, the group sender key and the prekey signature nonce).
// Its API does not let us inject those, so they are not fixed and the output differs between
// runs. Every private key it generated is recorded in the output: the session snapshots and the
// final sessions hold the ratchet private keys, and group.sender_signing_private holds the group
// signing key.
//
// Exit status is non-zero on any failure.
package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"os"

	"go.mau.fi/libsignal/ecc"
	"go.mau.fi/libsignal/groups"
	"go.mau.fi/libsignal/keys/identity"
	"go.mau.fi/libsignal/keys/prekey"
	"go.mau.fi/libsignal/protocol"
	"go.mau.fi/libsignal/serialize"
	"go.mau.fi/libsignal/session"
	"go.mau.fi/libsignal/state/record"
	"go.mau.fi/libsignal/tests"
	"google.golang.org/protobuf/proto"
)

const (
	libsignalVersion  = "go.mau.fi/libsignal v0.2.3-0.20261103023201-5ce3f7a54370"
	ciphertextWhisper = 2
	ciphertextPreKey  = 3

	aliceRegistrationID = 4711
	bobRegistrationID   = 1337
	bobPreKeyID         = 1001
	bobSignedPreKeyID   = 2001
	signedPreKeyTime    = 1_700_000_000_000
	groupName           = "chirper-interop-group"
)

// Plaintexts. The Rust test checks each one after decryption.
const (
	plainPreKey   = "Hello Bob, this is the Go side of the interop test."
	plainReply1   = "Hi Alice, the prekey message arrived intact."
	plainFollowUp = "Alice has seen your reply, so her ratchet has turned."
	plainReply2   = "Bob answers with a fresh ratchet key of his own."
	plainGroup1   = "group message one from alice"
	plainGroup2   = "group message two from alice"
)

// message is one Go-produced one-to-one message, with the receiver's session state.
type message struct {
	Index int    `json:"index"`
	From  string `json:"from"`
	To    string `json:"to"`
	Kind  string `json:"kind"`
	Plain string `json:"plaintext"`
	Wire  string `json:"ciphertext"`
	// ReceiverSession is the receiver's session record just before it decrypted this message, as
	// hex LocalStorageProtocol.proto RecordStructure bytes (the encoding the Rust crate reads).
	// Empty for the prekey message, which Rust decrypts from keys.
	ReceiverSession string `json:"receiver_session,omitempty"`
	// ReceiverSessionGoJSON is the same record in libsignal's own JSON encoding, which the Go
	// verify mode reads back. libsignal's default serializer stores records as JSON.
	ReceiverSessionGoJSON string `json:"receiver_session_go_json,omitempty"`
}

// snapshot is one session record in both encodings.
type snapshot struct {
	Proto  string `json:"proto"`
	Native string `json:"go_json"`
}

// finalSessions are both parties' session records after the conversation, so that every ratchet
// private key the Go side generated appears in the output.
type finalSessions struct {
	Alice snapshot `json:"alice_with_bob"`
	Bob   snapshot `json:"bob_with_alice"`
}

// vectors is the whole file written by generate.
type vectors struct {
	Library     string        `json:"library"`
	Description string        `json:"description"`
	Alice       publicParty   `json:"alice"`
	Bob         bobVectors    `json:"bob"`
	Messages    []message     `json:"messages"`
	Group       groupVector   `json:"group"`
	Final       finalSessions `json:"final_sessions"`
}

type publicParty struct {
	Name            string `json:"name"`
	DeviceID        uint32 `json:"device_id"`
	RegistrationID  uint32 `json:"registration_id"`
	IdentityPublic  string `json:"identity_public"`
	IdentityPrivate string `json:"identity_private"`
}

type bobVectors struct {
	publicParty
	PreKeyID              uint32 `json:"pre_key_id"`
	PreKeyPublic          string `json:"pre_key_public"`
	PreKeyPrivate         string `json:"pre_key_private"`
	SignedPreKeyID        uint32 `json:"signed_pre_key_id"`
	SignedPreKeyTime      int64  `json:"signed_pre_key_timestamp"`
	SignedPreKeyPublic    string `json:"signed_pre_key_public"`
	SignedPreKeyPrivate   string `json:"signed_pre_key_private"`
	SignedPreKeySignature string `json:"signed_pre_key_signature"`
}

type groupVector struct {
	GroupID              string         `json:"group_id"`
	Sender               string         `json:"sender"`
	DistributionMessage  string         `json:"distribution_message"`
	SenderSigningPrivate string         `json:"sender_signing_private"`
	Messages             []groupMessage `json:"messages"`
}

type groupMessage struct {
	Plain string `json:"plaintext"`
	Wire  string `json:"ciphertext"`
}

// replies is the file written by the Rust example interop_reply.
type replies struct {
	Replies []reply `json:"replies"`
}

type reply struct {
	// DecryptWith is the index of a message whose receiver_session Alice starts from.
	DecryptWith int    `json:"decrypt_with"`
	Plain       string `json:"plaintext"`
	Wire        string `json:"ciphertext"`
}

// party is one simulated device, with the stock in-memory libsignal stores.
type party struct {
	name       string
	deviceID   uint32
	address    *protocol.SignalAddress
	identity   *identity.KeyPair
	regID      uint32
	sessions   *tests.InMemorySession
	preKeys    *tests.InMemoryPreKey
	signed     *tests.InMemorySignedPreKey
	idStore    *tests.InMemoryIdentityKey
	senderKeys *tests.InMemorySenderKey
	ser        *serialize.Serializer
}

// fixedKeyPair derives a Curve25519 key pair from a label. The returned seed is the private key
// before clamping, which both implementations clamp the same way.
func fixedKeyPair(label string) (*ecc.ECKeyPair, []byte) {
	seed := sha256.Sum256([]byte("chirper-signal-interop/" + label))
	return ecc.CreateKeyPair(seed[:]), seed[:]
}

func newParty(ser *serialize.Serializer, name string, regID uint32) *party {
	kp, _ := fixedKeyPair(name + "/identity")
	idKey := identity.NewKeyPair(identity.NewKey(kp.PublicKey()), kp.PrivateKey())
	return &party{
		name:       name,
		deviceID:   1,
		address:    protocol.NewSignalAddress(name, 1),
		identity:   idKey,
		regID:      regID,
		sessions:   tests.NewInMemorySession(ser),
		preKeys:    tests.NewInMemoryPreKey(),
		signed:     tests.NewInMemorySignedPreKey(),
		idStore:    tests.NewInMemoryIdentityKey(idKey, regID),
		senderKeys: tests.NewInMemorySenderKey(),
		ser:        ser,
	}
}

func (p *party) builder(remote *protocol.SignalAddress) *session.Builder {
	return session.NewBuilder(p.sessions, p.preKeys, p.signed, p.idStore, remote, p.ser)
}

func (p *party) groupBuilder() *groups.SessionBuilder {
	return groups.NewGroupSessionBuilder(p.senderKeys, p.ser)
}

// snapshot returns the session record that p holds for remote, in both encodings.
func (p *party) snapshot(ctx context.Context, remote *protocol.SignalAddress) (snapshot, error) {
	rec, err := p.sessions.LoadSession(ctx, remote)
	if err != nil {
		return snapshot{}, err
	}
	encoded, err := recordProto(rec.Structure())
	if err != nil {
		return snapshot{}, err
	}
	return snapshot{Proto: hex.EncodeToString(encoded), Native: hex.EncodeToString(rec.Serialize())}, nil
}

func u32(v uint32) *uint32 { return &v }

func i32(v int32) *int32 { return &v }

// recordProto converts libsignal's session structure to LocalStorageProtocol.proto's
// RecordStructure. The Go library encodes records as JSON, so the mapping is field by field.
func recordProto(s *record.SessionStructure) ([]byte, error) {
	out := &serialize.RecordStructure{}
	if s.SessionState != nil {
		out.CurrentSession = stateProto(s.SessionState)
	}
	for _, st := range s.PreviousStates {
		out.PreviousSessions = append(out.PreviousSessions, stateProto(st))
	}
	return proto.Marshal(out)
}

func stateProto(s *record.StateStructure) *serialize.SessionStructure {
	needsRefresh := s.NeedsRefresh
	out := &serialize.SessionStructure{
		SessionVersion:       u32(uint32(s.SessionVersion)),
		LocalIdentityPublic:  s.LocalIdentityPublic,
		RemoteIdentityPublic: s.RemoteIdentityPublic,
		RootKey:              s.RootKey,
		PreviousCounter:      u32(s.PreviousCounter),
		SenderChain:          chainProto(s.SenderChain),
		RemoteRegistrationId: u32(s.RemoteRegistrationID),
		LocalRegistrationId:  u32(s.LocalRegistrationID),
		NeedsRefresh:         &needsRefresh,
		AliceBaseKey:         s.SenderBaseKey,
	}
	for _, c := range s.ReceiverChains {
		out.ReceiverChains = append(out.ReceiverChains, chainProto(c))
	}
	if pk := s.PendingKeyExchange; pk != nil {
		out.PendingKeyExchange = &serialize.SessionStructure_PendingKeyExchange{
			Sequence:                u32(pk.Sequence),
			LocalBaseKey:            pk.LocalBaseKeyPublic,
			LocalBaseKeyPrivate:     pk.LocalBaseKeyPrivate,
			LocalRatchetKey:         pk.LocalRatchetKeyPublic,
			LocalRatchetKeyPrivate:  pk.LocalRatchetKeyPrivate,
			LocalIdentityKey:        pk.LocalIdentityKeyPublic,
			LocalIdentityKeyPrivate: pk.LocalIdentityKeyPrivate,
		}
	}
	if pp := s.PendingPreKey; pp != nil {
		out.PendingPreKey = &serialize.SessionStructure_PendingPreKey{
			SignedPreKeyId: i32(int32(pp.SignedPreKeyID)),
			BaseKey:        pp.BaseKey,
		}
		if pp.PreKeyID != nil && !pp.PreKeyID.IsEmpty {
			out.PendingPreKey.PreKeyId = u32(pp.PreKeyID.Value)
		}
	}
	return out
}

func chainProto(c *record.ChainStructure) *serialize.SessionStructure_Chain {
	if c == nil {
		return nil
	}
	out := &serialize.SessionStructure_Chain{
		SenderRatchetKey:        c.SenderRatchetKeyPublic,
		SenderRatchetKeyPrivate: c.SenderRatchetKeyPrivate,
	}
	if c.ChainKey != nil {
		out.ChainKey = &serialize.SessionStructure_Chain_ChainKey{Index: u32(c.ChainKey.Index), Key: c.ChainKey.Key}
	}
	for _, k := range c.MessageKeys {
		out.MessageKeys = append(out.MessageKeys, &serialize.SessionStructure_Chain_MessageKey{
			Index:     u32(k.Index),
			CipherKey: k.CipherKey,
			MacKey:    k.MacKey,
			Iv:        k.IV,
		})
	}
	return out
}

// encrypt runs one encryption from sender to receiver and checks the expected wire type.
func encrypt(ctx context.Context, sender *party, receiver *party, plain string, wantType uint32) (string, error) {
	ct, err := session.NewCipher(sender.builder(receiver.address), receiver.address).Encrypt(ctx, []byte(plain))
	if err != nil {
		return "", fmt.Errorf("encrypt %q: %w", plain, err)
	}
	if ct.Type() != wantType {
		return "", fmt.Errorf("encrypt %q: wire type %d, want %d", plain, ct.Type(), wantType)
	}
	return hex.EncodeToString(ct.Serialize()), nil
}

// decrypt runs one decryption on receiver and checks the plaintext. The prekey flag selects the
// prekey message parser, which is the only session-establishing path.
func decrypt(ctx context.Context, receiver *party, sender *party, wireHex string, prekey bool, plain string) error {
	wire, err := hex.DecodeString(wireHex)
	if err != nil {
		return err
	}
	cipher := session.NewCipher(receiver.builder(sender.address), sender.address)
	var got []byte
	if prekey {
		msg, err := protocol.NewPreKeySignalMessageFromBytes(wire, receiver.ser.PreKeySignalMessage, receiver.ser.SignalMessage)
		if err != nil {
			return err
		}
		got, err = cipher.DecryptMessage(ctx, msg)
		if err != nil {
			return err
		}
	} else {
		msg, err := protocol.NewSignalMessageFromBytes(wire, receiver.ser.SignalMessage)
		if err != nil {
			return err
		}
		got, err = cipher.Decrypt(ctx, msg)
		if err != nil {
			return err
		}
	}
	if string(got) != plain {
		return fmt.Errorf("decrypted %q, want %q", got, plain)
	}
	return nil
}

func generate(outPath string) error {
	ctx := context.Background()
	ser := serialize.NewProtoBufSerializer()
	alice := newParty(ser, "alice", aliceRegistrationID)
	bob := newParty(ser, "bob", bobRegistrationID)

	// Bob publishes a one-time prekey and a signed prekey, both from fixed keys.
	preKeyPair, preKeySeed := fixedKeyPair("bob/prekey")
	signedPair, signedSeed := fixedKeyPair("bob/signed-prekey")
	signature := ecc.CalculateSignature(bob.identity.PrivateKey(), signedPair.PublicKey().Serialize())
	preKey := record.NewPreKey(bobPreKeyID, preKeyPair, ser.PreKeyRecord)
	signed := record.NewSignedPreKey(bobSignedPreKeyID, signedPreKeyTime, signedPair, signature, ser.SignedPreKeyRecord)
	if err := bob.preKeys.StorePreKey(ctx, bobPreKeyID, preKey); err != nil {
		return err
	}
	if err := bob.signed.StoreSignedPreKey(ctx, bobSignedPreKeyID, signed); err != nil {
		return err
	}
	bundle := prekey.NewBundle(bob.regID, bob.deviceID, preKey.ID(), bobSignedPreKeyID,
		preKeyPair.PublicKey(), signedPair.PublicKey(), signature, bob.identity.PublicKey())
	if err := alice.builder(bob.address).ProcessBundle(ctx, bundle); err != nil {
		return fmt.Errorf("alice processes bundle: %w", err)
	}

	var msgs []message
	add := func(index int, from, to *party, kind, plain, wire string, snap snapshot) {
		msgs = append(msgs, message{
			Index: index, From: from.name, To: to.name, Kind: kind, Plain: plain, Wire: wire,
			ReceiverSession: snap.Proto, ReceiverSessionGoJSON: snap.Native,
		})
	}

	// 0: Alice's prekey message to Bob. Bob decrypts it from keys in Rust.
	wire, err := encrypt(ctx, alice, bob, plainPreKey, ciphertextPreKey)
	if err != nil {
		return err
	}
	if err := decrypt(ctx, bob, alice, wire, true, plainPreKey); err != nil {
		return fmt.Errorf("bob decrypts prekey message: %w", err)
	}
	add(0, alice, bob, "prekey", plainPreKey, wire, snapshot{})

	// 1: Bob replies. Alice's session is snapshotted before she decrypts.
	wire, err = encrypt(ctx, bob, alice, plainReply1, ciphertextWhisper)
	if err != nil {
		return err
	}
	snap, err := alice.snapshot(ctx, bob.address)
	if err != nil {
		return err
	}
	add(1, bob, alice, "signal", plainReply1, wire, snap)
	if err := decrypt(ctx, alice, bob, wire, false, plainReply1); err != nil {
		return fmt.Errorf("alice decrypts reply: %w", err)
	}

	// 2: Alice follows up after her ratchet turned on Bob's reply. Bob's session is snapshotted.
	wire, err = encrypt(ctx, alice, bob, plainFollowUp, ciphertextWhisper)
	if err != nil {
		return err
	}
	snap, err = bob.snapshot(ctx, alice.address)
	if err != nil {
		return err
	}
	add(2, alice, bob, "signal", plainFollowUp, wire, snap)
	if err := decrypt(ctx, bob, alice, wire, false, plainFollowUp); err != nil {
		return fmt.Errorf("bob decrypts follow-up: %w", err)
	}

	// 3: Bob answers with his own new ratchet key.
	wire, err = encrypt(ctx, bob, alice, plainReply2, ciphertextWhisper)
	if err != nil {
		return err
	}
	snap, err = alice.snapshot(ctx, bob.address)
	if err != nil {
		return err
	}
	add(3, bob, alice, "signal", plainReply2, wire, snap)
	if err := decrypt(ctx, alice, bob, wire, false, plainReply2); err != nil {
		return fmt.Errorf("alice decrypts second reply: %w", err)
	}

	// Group: Alice distributes a sender key to Bob, then sends two group messages.
	senderName := protocol.NewSenderKeyName(groupName, alice.address)
	skdm, err := alice.groupBuilder().Create(ctx, senderName)
	if err != nil {
		return fmt.Errorf("alice creates sender key: %w", err)
	}
	skdmWire := skdm.Serialize()
	parsed, err := protocol.NewSenderKeyDistributionMessageFromBytes(skdmWire, ser.SenderKeyDistributionMessage)
	if err != nil {
		return err
	}
	if err := bob.groupBuilder().Process(ctx, senderName, parsed); err != nil {
		return fmt.Errorf("bob processes sender key: %w", err)
	}
	group := groupVector{GroupID: groupName, Sender: alice.name, DistributionMessage: hex.EncodeToString(skdmWire)}
	senderRecord, err := alice.senderKeys.LoadSenderKey(ctx, senderName)
	if err != nil {
		return err
	}
	senderState, err := senderRecord.SenderKeyState()
	if err != nil {
		return err
	}
	signingPriv := senderState.SigningKey().PrivateKey().Serialize()
	group.SenderSigningPrivate = hex.EncodeToString(signingPriv[:])
	for _, plain := range []string{plainGroup1, plainGroup2} {
		gm, err := groups.NewGroupCipher(alice.groupBuilder(), senderName, alice.senderKeys).Encrypt(ctx, []byte(plain))
		if err != nil {
			return fmt.Errorf("alice group encrypt: %w", err)
		}
		group.Messages = append(group.Messages, groupMessage{Plain: plain, Wire: hex.EncodeToString(gm.SignedSerialize())})
	}
	for _, gmsg := range group.Messages {
		raw, err := hex.DecodeString(gmsg.Wire)
		if err != nil {
			return err
		}
		sm, err := protocol.NewSenderKeyMessageFromBytes(raw, ser.SenderKeyMessage)
		if err != nil {
			return err
		}
		got, err := groups.NewGroupCipher(bob.groupBuilder(), senderName, bob.senderKeys).Decrypt(ctx, sm)
		if err != nil {
			return fmt.Errorf("bob group decrypt: %w", err)
		}
		if string(got) != gmsg.Plain {
			return fmt.Errorf("bob group decrypted %q, want %q", got, gmsg.Plain)
		}
	}

	finalAlice, err := alice.snapshot(ctx, bob.address)
	if err != nil {
		return err
	}
	finalBob, err := bob.snapshot(ctx, alice.address)
	if err != nil {
		return err
	}

	seedHex := func(b []byte) string { return hex.EncodeToString(b) }
	out := vectors{
		Library:     libsignalVersion,
		Description: "Generated by tests/interop/generate; do not edit by hand. Identity and prekey private keys are fixed seeds, the rest of the private keys are recorded in the session snapshots and group fields.",
		Alice: publicParty{
			Name:            alice.name,
			DeviceID:        alice.deviceID,
			RegistrationID:  alice.regID,
			IdentityPublic:  seedHex(alice.identity.PublicKey().Serialize()[1:]),
			IdentityPrivate: seedHex(fixedSeed(alice.name + "/identity")),
		},
		Bob: bobVectors{
			publicParty: publicParty{
				Name:            bob.name,
				DeviceID:        bob.deviceID,
				RegistrationID:  bob.regID,
				IdentityPublic:  seedHex(bob.identity.PublicKey().Serialize()[1:]),
				IdentityPrivate: seedHex(fixedSeed(bob.name + "/identity")),
			},
			PreKeyID:              bobPreKeyID,
			PreKeyPublic:          seedHex(preKeyPair.PublicKey().Serialize()[1:]),
			PreKeyPrivate:         seedHex(preKeySeed),
			SignedPreKeyID:        bobSignedPreKeyID,
			SignedPreKeyTime:      signedPreKeyTime,
			SignedPreKeyPublic:    seedHex(signedPair.PublicKey().Serialize()[1:]),
			SignedPreKeyPrivate:   seedHex(signedSeed),
			SignedPreKeySignature: seedHex(signature[:]),
		},
		Messages: msgs,
		Group:    group,
		Final:    finalSessions{Alice: finalAlice, Bob: finalBob},
	}
	raw, err := json.MarshalIndent(out, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(outPath, append(raw, '\n'), 0o644)
}

// fixedSeed returns the seed fixedKeyPair derived for label.
func fixedSeed(label string) []byte {
	_, seed := fixedKeyPair(label)
	return seed
}

// verify decrypts every Rust reply with Alice's session as it stood at the recorded point.
func verify(vectorsPath, repliesPath string) error {
	ctx := context.Background()
	ser := serialize.NewProtoBufSerializer()
	raw, err := os.ReadFile(vectorsPath)
	if err != nil {
		return err
	}
	var vec vectors
	if err := json.Unmarshal(raw, &vec); err != nil {
		return err
	}
	raw, err = os.ReadFile(repliesPath)
	if err != nil {
		return err
	}
	var rep replies
	if err := json.Unmarshal(raw, &rep); err != nil {
		return err
	}
	bob := newParty(ser, "bob", bobRegistrationID)
	failures := 0
	for i, r := range rep.Replies {
		if r.DecryptWith < 0 || r.DecryptWith >= len(vec.Messages) || vec.Messages[r.DecryptWith].ReceiverSessionGoJSON == "" {
			return fmt.Errorf("reply %d: decrypt_with %d has no receiver session", i, r.DecryptWith)
		}
		// Each reply starts from a fresh copy of Alice's recorded session.
		alice := newParty(ser, "alice", aliceRegistrationID)
		snap, err := hex.DecodeString(vec.Messages[r.DecryptWith].ReceiverSessionGoJSON)
		if err != nil {
			return err
		}
		rec, err := record.NewSessionFromBytes(snap, ser.Session, ser.State)
		if err != nil {
			return fmt.Errorf("reply %d: parse session: %w", i, err)
		}
		if err := alice.sessions.StoreSession(ctx, bob.address, rec); err != nil {
			return err
		}
		if err := decrypt(ctx, alice, bob, r.Wire, false, r.Plain); err != nil {
			failures++
			fmt.Fprintf(os.Stderr, "reply %d: FAILED: %v\n", i, err)
			continue
		}
		fmt.Printf("reply %d: ok (%d bytes)\n", i, len(r.Plain))
	}
	if failures > 0 {
		return fmt.Errorf("%d of %d replies failed", failures, len(rep.Replies))
	}
	fmt.Printf("all %d replies decrypted by %s\n", len(rep.Replies), libsignalVersion)
	return nil
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: generate {generate|verify} [flags]")
		os.Exit(2)
	}
	var err error
	switch os.Args[1] {
	case "generate":
		fs := flag.NewFlagSet("generate", flag.ExitOnError)
		out := fs.String("out", "../go_vectors.json", "output JSON path")
		_ = fs.Parse(os.Args[2:])
		err = generate(*out)
	case "verify":
		fs := flag.NewFlagSet("verify", flag.ExitOnError)
		vectors := fs.String("vectors", "../go_vectors.json", "vectors written by generate")
		replies := fs.String("replies", "../rust_replies.json", "Rust-produced replies")
		_ = fs.Parse(os.Args[2:])
		err = verify(*vectors, *replies)
	default:
		err = fmt.Errorf("unknown mode %q", os.Args[1])
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}
