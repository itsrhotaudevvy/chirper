// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"testing"

	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

func TestTextPartForUserMessages(t *testing.T) {
	part := textPart(&fluxerapi.Message{Type: messageTypeDefault, Content: "hello"})
	if part == nil || part.Content.MsgType != event.MsgText || part.Content.Body != "hello" || part.ID != "" {
		t.Fatalf("text part = %+v", part)
	}
	if textPart(&fluxerapi.Message{Type: messageTypeReply, Content: ""}) != nil {
		t.Fatal("empty content must produce no text part")
	}
}

func TestTextPartForSystemMessageIsNotice(t *testing.T) {
	part := textPart(&fluxerapi.Message{Type: 7, Content: ""})
	if part == nil || part.Content.MsgType != event.MsgNotice {
		t.Fatalf("system part = %+v", part)
	}
}

func TestConvertMessageAddsReplyTarget(t *testing.T) {
	c := &fluxerClient{}
	msg := &fluxerapi.Message{
		ID:               "2",
		Type:             messageTypeReply,
		Content:          "answer",
		MessageReference: &fluxerapi.MessageReference{MessageID: "1", Type: 0},
	}
	converted, err := c.convertMessage(t.Context(), nil, nil, msg)
	if err != nil {
		t.Fatal(err)
	}
	if converted.ReplyTo == nil || converted.ReplyTo.MessageID != networkid.MessageID("1") {
		t.Fatalf("reply target = %+v", converted.ReplyTo)
	}
	if len(converted.Parts) != 1 {
		t.Fatalf("parts = %d", len(converted.Parts))
	}
}

func TestConvertMessageForwardHasNoReplyTarget(t *testing.T) {
	c := &fluxerClient{}
	msg := &fluxerapi.Message{
		ID:               "3",
		Content:          "forwarded",
		MessageReference: &fluxerapi.MessageReference{MessageID: "1", Type: 1},
	}
	converted, err := c.convertMessage(t.Context(), nil, nil, msg)
	if err != nil {
		t.Fatal(err)
	}
	if converted.ReplyTo != nil {
		t.Fatalf("forward must not be a reply: %+v", converted.ReplyTo)
	}
}

func TestConvertMessageWithoutContentOrAttachmentsIsUnsupported(t *testing.T) {
	c := &fluxerClient{}
	converted, err := c.convertMessage(t.Context(), nil, nil, &fluxerapi.Message{ID: "4", Type: messageTypeDefault})
	if err != nil {
		t.Fatal(err)
	}
	if len(converted.Parts) != 1 || converted.Parts[0].Content.Body != unsupportedNotice {
		t.Fatalf("parts = %+v", converted.Parts)
	}
}

func TestConvertEditOnlyTouchesTextPart(t *testing.T) {
	c := &fluxerClient{}
	existing := []*database.Message{
		{ID: "9", PartID: ""},
		{ID: "9", PartID: "att-1"},
	}
	edit, err := c.convertEdit(t.Context(), nil, nil, existing, &fluxerapi.Message{ID: "9", Type: messageTypeDefault, Content: "fixed"})
	if err != nil {
		t.Fatal(err)
	}
	if len(edit.ModifiedParts) != 1 || edit.ModifiedParts[0].Part.PartID != "" {
		t.Fatalf("modified = %+v", edit.ModifiedParts)
	}
	if edit.ModifiedParts[0].Content.Body != "fixed" {
		t.Fatalf("body = %q", edit.ModifiedParts[0].Content.Body)
	}
}

func TestEmojiKeys(t *testing.T) {
	id := "123"
	custom := fluxerapi.Emoji{Name: "party", ID: &id}
	if emojiText(custom) != ":party:" || string(emojiID(custom)) != "123" {
		t.Fatalf("custom emoji text=%q id=%q", emojiText(custom), emojiID(custom))
	}
	unicode := fluxerapi.Emoji{Name: "👍"}
	if emojiText(unicode) != "👍" || string(emojiID(unicode)) != "👍" {
		t.Fatalf("unicode emoji text=%q id=%q", emojiText(unicode), emojiID(unicode))
	}
}
