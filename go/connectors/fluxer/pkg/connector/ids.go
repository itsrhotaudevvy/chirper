// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
)

// Fluxer IDs are snowflakes, which are unique within an instance. They are used as bridgev2 IDs
// directly. Direct messages and group DMs are scoped to the login (Receiver); guild channels are
// shared (empty Receiver) so that several accounts on one guild share a portal.

func makeUserID(id string) networkid.UserID {
	return networkid.UserID(id)
}

func makeUserLoginID(id string) networkid.UserLoginID {
	return networkid.UserLoginID(id)
}

func makeMessageID(id string) networkid.MessageID {
	return networkid.MessageID(id)
}

// makePartID names the part of a message that carries an attachment. The text part is "".
func makePartID(attachmentID string) networkid.PartID {
	return networkid.PartID(attachmentID)
}

func makePortalKey(channelID string, direct bool, login networkid.UserLoginID) networkid.PortalKey {
	key := networkid.PortalKey{ID: networkid.PortalID(channelID)}
	if direct {
		key.Receiver = login
	}
	return key
}

func senderFor(login networkid.UserLoginID, myID, authorID string) bridgev2.EventSender {
	isMe := authorID == myID
	sender := bridgev2.EventSender{IsFromMe: isMe, Sender: makeUserID(authorID)}
	if isMe {
		sender.SenderLogin = login
	}
	return sender
}
