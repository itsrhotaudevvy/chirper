package testnet

import (
	"fmt"
	"time"

	"go.mau.fi/util/ptr"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"
)

const selfID networkid.UserID = "self"

// fakeUser is a remote person of the fake network.
type fakeUser struct {
	id   networkid.UserID
	name string
}

// users are the remote contacts. The local user is not listed: its name is the login name.
var users = map[networkid.UserID]fakeUser{
	"alice": {id: "alice", name: "Alice"},
	"bob":   {id: "bob", name: "Bob"},
}

// seedMessage is a message of the fixed dataset. An empty sender means the local user.
type seedMessage struct {
	sender networkid.UserID
	text   string
}

// seedChat is one chat of the fixed dataset.
type seedChat struct {
	key      networkid.PortalKey
	name     string
	isDM     bool
	others   []networkid.UserID
	messages []seedMessage
}

var seedChats = []seedChat{{
	key:    networkid.PortalKey{ID: "dm-alice"},
	name:   "Alice",
	isDM:   true,
	others: []networkid.UserID{"alice"},
	messages: []seedMessage{
		{sender: "alice", text: "Hi! This is a demo chat."},
		{sender: "alice", text: "Are you around later?"},
		{sender: selfID, text: "Sure, what's up?"},
		{sender: "alice", text: "Let's talk about the demo."},
		{sender: "alice", text: "Thanks!"},
	},
}, {
	key:    networkid.PortalKey{ID: "team"},
	name:   "Team",
	others: []networkid.UserID{"alice", "bob"},
	messages: []seedMessage{
		{sender: "bob", text: "Welcome to the team chat."},
		{sender: "alice", text: "Standup moved to 10:00."},
		{sender: selfID, text: "Noted."},
		{sender: "bob", text: "Who has the release notes?"},
		{sender: "alice", text: "I'll send them tonight."},
	},
}}

// seedStart is the timestamp of the oldest seeded message. It is a fixed date, so every start
// produces the same timeline. It is long before the live window, so seeded messages are never live.
var seedStart = time.Date(2026, 1, 15, 9, 0, 0, 0, time.UTC)

// historyPages and historyPageSize bound FetchMessages: 3 pages of 20 messages.
const (
	historyPages    = 3
	historyPageSize = 20
)

// chatInfo describes a seed chat to bridgev2.
func chatInfo(chat seedChat) *bridgev2.ChatInfo {
	name := chat.name
	info := &bridgev2.ChatInfo{
		Name:        &name,
		Members:     &bridgev2.ChatMemberList{IsFull: true, TotalMemberCount: len(chat.others) + 1, MemberMap: bridgev2.ChatMemberMap{}},
		UserLocal:   &bridgev2.UserLocalPortalInfo{},
		CanBackfill: true,
	}
	if chat.isDM {
		info.Type = ptr.Ptr(database.RoomTypeDM)
	}
	info.Members.MemberMap.Set(bridgev2.ChatMember{
		EventSender: bridgev2.EventSender{IsFromMe: true},
		Membership:  event.MembershipJoin,
	})
	for _, uid := range chat.others {
		u := users[uid]
		info.Members.MemberMap.Set(bridgev2.ChatMember{
			EventSender: bridgev2.EventSender{Sender: u.id},
			Membership:  event.MembershipJoin,
			UserInfo:    &bridgev2.UserInfo{Name: ptr.Ptr(u.name)},
		})
	}
	return info
}

// senderFor maps a seed sender to the event sender of bridgev2.
func senderFor(sender networkid.UserID) bridgev2.EventSender {
	if sender == selfID {
		return bridgev2.EventSender{IsFromMe: true}
	}
	return bridgev2.EventSender{Sender: sender}
}

// historyText is the text of the n-th synthetic history message.
func historyText(n int) string {
	return fmt.Sprintf("history message %d", n)
}
