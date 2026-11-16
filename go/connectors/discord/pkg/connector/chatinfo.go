// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"context"
	"fmt"
	"strings"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
)

const discordCDN = "https://cdn.discordapp.com"

// GetChatInfo returns the current state of a guild (space) or channel (room).
func (c *DiscordClient) GetChatInfo(ctx context.Context, portal *bridgev2.Portal) (*bridgev2.ChatInfo, error) {
	sess, err := c.currentSession()
	if err != nil {
		return nil, err
	}
	if portal.RoomType == database.RoomTypeSpace {
		guild, err := sess.State.Guild(string(portal.ID))
		if err != nil {
			return nil, fmt.Errorf("failed to find guild: %w", err)
		}
		return c.wrapGuildInfo(guild), nil
	}
	ch, err := c.getChannel(sess, string(portal.ID))
	if err != nil {
		return nil, err
	}
	return c.wrapChannelInfo(ch), nil
}

// getChannel looks up a channel in the gateway cache first and falls back to the REST API.
func (c *DiscordClient) getChannel(sess *discordgo.Session, channelID string) (*discordgo.Channel, error) {
	if ch, err := sess.State.Channel(channelID); err == nil {
		return ch, nil
	}
	ch, err := sess.Channel(channelID)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch channel: %w", err)
	}
	return ch, nil
}

func (c *DiscordClient) wrapGuildInfo(guild *discordgo.Guild) *bridgev2.ChatInfo {
	roomType := database.RoomTypeSpace
	return &bridgev2.ChatInfo{
		Name:   &guild.Name,
		Type:   &roomType,
		Avatar: c.guildAvatar(guild),
	}
}

// wrapChannelInfo maps a Discord channel to a room. DMs and group DMs become DM rooms, guild threads
// become separate rooms whose parent is the channel they were started in, and other guild channels
// are placed in their guild's space.
func (c *DiscordClient) wrapChannelInfo(ch *discordgo.Channel) *bridgev2.ChatInfo {
	canBackfill := c.Main.Config.Backfill.Enabled
	info := &bridgev2.ChatInfo{Topic: ptrString(ch.Topic), CanBackfill: canBackfill}
	switch ch.Type {
	case discordgo.ChannelTypeDM, discordgo.ChannelTypeGroupDM:
		c.fillDMInfo(info, ch)
	case discordgo.ChannelTypeGuildNewsThread, discordgo.ChannelTypeGuildPublicThread, discordgo.ChannelTypeGuildPrivateThread:
		name := ch.Name
		info.Name = &name
		info.ParentID = ptrPortalID(networkid.PortalID(ch.ParentID))
	default:
		name := ch.Name
		info.Name = &name
		if ch.GuildID != "" {
			info.ParentID = ptrPortalID(networkid.PortalID(ch.GuildID))
		}
	}
	return info
}

func (c *DiscordClient) fillDMInfo(info *bridgev2.ChatInfo, ch *discordgo.Channel) {
	roomType := database.RoomTypeDM
	members := &bridgev2.ChatMemberList{
		IsFull:           true,
		TotalMemberCount: len(ch.Recipients) + 1,
		MemberMap:        bridgev2.ChatMemberMap{},
	}
	names := make([]string, 0, len(ch.Recipients))
	for _, user := range ch.Recipients {
		names = append(names, displayName(user))
		members.MemberMap.Set(bridgev2.ChatMember{
			EventSender: c.makeSender(user.ID),
			Membership:  "join",
			UserInfo:    c.wrapUserInfo(user),
		})
	}
	self := c.selfUserID()
	members.MemberMap.Set(bridgev2.ChatMember{
		EventSender: c.makeSender(self),
		Membership:  "join",
	})
	if len(ch.Recipients) == 1 {
		members.OtherUserID = makeUserID(ch.Recipients[0].ID)
	} else {
		roomType = database.RoomTypeGroupDM
	}
	name := ch.Name
	if name == "" {
		name = strings.Join(names, ", ")
	}
	info.Name = &name
	info.Type = &roomType
	info.Members = members
}

func ptrString(s string) *string {
	return &s
}

func ptrPortalID(id networkid.PortalID) *networkid.PortalID {
	return &id
}

// displayName prefers the global display name and falls back to the username.
func displayName(user *discordgo.User) string {
	if user.GlobalName != "" {
		return user.GlobalName
	}
	return user.Username
}

// GetUserInfo fetches a remote user's profile.
func (c *DiscordClient) GetUserInfo(ctx context.Context, ghost *bridgev2.Ghost) (*bridgev2.UserInfo, error) {
	sess, err := c.currentSession()
	if err != nil {
		return nil, err
	}
	user, err := sess.User(string(ghost.ID))
	if err != nil {
		return nil, fmt.Errorf("failed to fetch user: %w", err)
	}
	return c.wrapUserInfo(user), nil
}

func (c *DiscordClient) wrapUserInfo(user *discordgo.User) *bridgev2.UserInfo {
	name := displayName(user)
	isBot := user.Bot
	return &bridgev2.UserInfo{
		Identifiers: []string{user.Username},
		Name:        &name,
		Avatar:      c.userAvatar(user),
		IsBot:       &isBot,
	}
}

// userAvatar returns the avatar for a user, or a removal marker when the user has none.
func (c *DiscordClient) userAvatar(user *discordgo.User) *bridgev2.Avatar {
	if user.Avatar == "" {
		return &bridgev2.Avatar{Remove: true}
	}
	ext := "png"
	if strings.HasPrefix(user.Avatar, "a_") {
		ext = "gif"
	}
	url := fmt.Sprintf("%s/avatars/%s/%s.%s?size=256", discordCDN, user.ID, user.Avatar, ext)
	return c.cdnAvatar(fmt.Sprintf("avatar/%s/%s", user.ID, user.Avatar), url)
}

func (c *DiscordClient) guildAvatar(guild *discordgo.Guild) *bridgev2.Avatar {
	if guild.Icon == "" {
		return &bridgev2.Avatar{Remove: true}
	}
	url := fmt.Sprintf("%s/icons/%s/%s.png?size=256", discordCDN, guild.ID, guild.Icon)
	return c.cdnAvatar(fmt.Sprintf("icon/%s/%s", guild.ID, guild.Icon), url)
}

func (c *DiscordClient) cdnAvatar(id, url string) *bridgev2.Avatar {
	return &bridgev2.Avatar{
		ID: networkid.AvatarID(id),
		Get: func(ctx context.Context) ([]byte, error) {
			data, _, err := c.Main.download(ctx, url)
			return data, err
		},
	}
}
