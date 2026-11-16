// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"testing"

	"maunium.net/go/mautrix/bridgev2/networkid"
)

func TestPortalKeyIsScopedForDirectChatsOnly(t *testing.T) {
	login := makeUserLoginID("100")

	dm := makePortalKey("555", true, login)
	if dm.ID != "555" || dm.Receiver != login {
		t.Fatalf("DM key = %+v, want receiver set to the login", dm)
	}

	guild := makePortalKey("777", false, login)
	if guild.ID != "777" || guild.Receiver != "" {
		t.Fatalf("guild key = %+v, want empty receiver so accounts share it", guild)
	}
}

func TestSenderFlagsOwnMessages(t *testing.T) {
	login := makeUserLoginID("100")

	me := senderFor(login, "100", "100")
	if !me.IsFromMe || me.SenderLogin != login || me.Sender != "100" {
		t.Fatalf("own sender = %+v", me)
	}

	other := senderFor(login, "100", "200")
	if other.IsFromMe || other.SenderLogin != "" || other.Sender != networkid.UserID("200") {
		t.Fatalf("other sender = %+v", other)
	}
}

func TestMsgTypeForMIME(t *testing.T) {
	cases := map[string]string{
		"image/png":       "m.image",
		"video/mp4":       "m.video",
		"audio/ogg":       "m.audio",
		"application/pdf": "m.file",
		"":                "m.file",
	}
	for mime, want := range cases {
		if got := string(msgTypeFor(mime)); got != want {
			t.Errorf("msgTypeFor(%q) = %s, want %s", mime, got, want)
		}
	}
}
