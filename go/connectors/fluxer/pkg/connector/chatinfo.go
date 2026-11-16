// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"fmt"

	"go.mau.fi/util/ptr"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

func (c *fluxerClient) GetChatInfo(ctx context.Context, portal *bridgev2.Portal) (*bridgev2.ChatInfo, error) {
	id := string(portal.ID)
	if c.isSpace(id) {
		g, err := c.api.Guild(ctx, id)
		if err != nil {
			return nil, err
		}
		return c.guildChatInfo(*g), nil
	}
	ch, err := c.api.Channel(ctx, id)
	if err != nil {
		return nil, err
	}
	return c.channelChatInfo(*ch, ch.GuildID), nil
}

// channelChatInfo describes a channel as a portal. Guild channels are children of their guild's
// space; DMs and group DMs list their recipients and the account itself.
func (c *fluxerClient) channelChatInfo(ch fluxerapi.Channel, guildID string) *bridgev2.ChatInfo {
	info := &bridgev2.ChatInfo{
		Topic:       ch.Topic,
		CanBackfill: true,
	}
	if guildID != "" {
		parent := networkid.PortalID(guildID)
		info.ParentID = &parent
		info.Name = ch.Name
		return info
	}
	if ch.Type == fluxerapi.ChannelGroupDM {
		info.Type = ptr.Ptr(database.RoomTypeGroupDM)
		info.Name = ch.Name
	} else {
		info.Type = ptr.Ptr(database.RoomTypeDM)
	}
	info.Members = c.directMembers(ch)
	return info
}

func (c *fluxerClient) directMembers(ch fluxerapi.Channel) *bridgev2.ChatMemberList {
	members := &bridgev2.ChatMemberList{
		IsFull:           true,
		TotalMemberCount: len(ch.Recipients) + 1,
		MemberMap:        make(map[networkid.UserID]bridgev2.ChatMember, len(ch.Recipients)+1),
	}
	me := c.senderOf(c.myID())
	members.MemberMap[me.Sender] = bridgev2.ChatMember{
		EventSender: me,
		Membership:  event.MembershipJoin,
	}
	for _, r := range ch.Recipients {
		sender := c.senderOf(r.ID)
		members.MemberMap[sender.Sender] = bridgev2.ChatMember{
			EventSender: sender,
			Membership:  event.MembershipJoin,
			UserInfo:    c.userInfo(r),
		}
	}
	if len(ch.Recipients) == 1 {
		members.OtherUserID = makeUserID(ch.Recipients[0].ID)
	}
	return members
}

// guildChatInfo describes a guild as a space portal.
func (c *fluxerClient) guildChatInfo(g fluxerapi.Guild) *bridgev2.ChatInfo {
	return &bridgev2.ChatInfo{
		Name:   ptr.Ptr(g.Name),
		Type:   ptr.Ptr(database.RoomTypeSpace),
		Avatar: c.avatar("/icons/"+g.ID, g.Icon),
	}
}

func (c *fluxerClient) GetUserInfo(ctx context.Context, ghost *bridgev2.Ghost) (*bridgev2.UserInfo, error) {
	u, err := c.api.UserByID(ctx, string(ghost.ID))
	if err != nil {
		return nil, err
	}
	return c.userInfo(*u), nil
}

func (c *fluxerClient) userInfo(u fluxerapi.User) *bridgev2.UserInfo {
	return &bridgev2.UserInfo{
		Identifiers: []string{fmt.Sprintf("fluxer:%s", u.Username)},
		Name:        ptr.Ptr(u.DisplayName()),
		Avatar:      c.avatar("/avatars/"+u.ID, u.Avatar),
		IsBot:       ptr.Ptr(u.Bot),
	}
}
