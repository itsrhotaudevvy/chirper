// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"encoding/json"
	"strings"
	"time"
)

// Channel types used by the connector (docs: http-api/channels.mdx "Channel types").
const (
	ChannelGuildText         = 0
	ChannelDM                = 1
	ChannelGroupDM           = 3
	ChannelGuildAnnouncement = 5
)

// User is a partial user object (docs: http-api/users.mdx "Partial user object").
type User struct {
	ID            string  `json:"id"`
	Username      string  `json:"username"`
	Discriminator string  `json:"discriminator"`
	GlobalName    *string `json:"global_name"`
	Avatar        *string `json:"avatar"`
	Bot           bool    `json:"bot"`
	System        bool    `json:"system"`
}

// DisplayName returns the global name when set, otherwise the username.
func (u User) DisplayName() string {
	if u.GlobalName != nil && *u.GlobalName != "" {
		return *u.GlobalName
	}
	return u.Username
}

// Guild is a guild object (docs: http-api/guilds.mdx "Guild object"), limited to the fields used.
type Guild struct {
	ID          string  `json:"id"`
	Name        string  `json:"name"`
	Icon        *string `json:"icon"`
	OwnerID     string  `json:"owner_id"`
	MemberCount *int    `json:"member_count,omitempty"`
}

// Channel is a channel object (docs: http-api/channels.mdx "Channel object"), limited to the fields used.
type Channel struct {
	ID            string  `json:"id"`
	Type          int     `json:"type"`
	GuildID       string  `json:"guild_id,omitempty"`
	Name          *string `json:"name,omitempty"`
	Topic         *string `json:"topic,omitempty"`
	ParentID      *string `json:"parent_id,omitempty"`
	Position      *int    `json:"position,omitempty"`
	OwnerID       *string `json:"owner_id,omitempty"`
	Icon          *string `json:"icon,omitempty"`
	Recipients    []User  `json:"recipients,omitempty"`
	LastMessageID *string `json:"last_message_id,omitempty"`
}

// IsMessageChannel reports whether the channel holds plain messages that Chirper can show.
func (c Channel) IsMessageChannel() bool {
	switch c.Type {
	case ChannelGuildText, ChannelGuildAnnouncement, ChannelDM, ChannelGroupDM:
		return true
	}
	return false
}

// Message is a message object (docs: http-api/messages.mdx "Message object").
type Message struct {
	ID                string            `json:"id"`
	ChannelID         string            `json:"channel_id"`
	GuildID           string            `json:"guild_id,omitempty"`
	Author            User              `json:"author"`
	WebhookID         string            `json:"webhook_id,omitempty"`
	Type              int               `json:"type"`
	Flags             int               `json:"flags"`
	Content           string            `json:"content"`
	Timestamp         time.Time         `json:"timestamp"`
	EditedTimestamp   *time.Time        `json:"edited_timestamp"`
	Nonce             json.RawMessage   `json:"nonce,omitempty"`
	Attachments       []Attachment      `json:"attachments"`
	Reactions         []Reaction        `json:"reactions,omitempty"`
	MessageReference  *MessageReference `json:"message_reference,omitempty"`
	ReferencedMessage *Message          `json:"referenced_message,omitempty"`
}

// NonceString returns the nonce sent with the create request. The API allows a string or an integer.
func (m Message) NonceString() string {
	return strings.Trim(string(m.Nonce), `"`)
}

// Attachment is a message attachment object (docs: http-api/messages.mdx "Message attachment object").
type Attachment struct {
	ID          string     `json:"id"`
	Filename    string     `json:"filename"`
	Title       *string    `json:"title"`
	Description *string    `json:"description"`
	ContentType string     `json:"content_type"`
	Size        int64      `json:"size"`
	URL         *string    `json:"url"`
	ProxyURL    *string    `json:"proxy_url"`
	Width       *int       `json:"width,omitempty"`
	Height      *int       `json:"height,omitempty"`
	Duration    *int       `json:"duration,omitempty"`
	ExpiresAt   *time.Time `json:"expires_at"`
	Expired     bool       `json:"expired,omitempty"`
}

// MessageReference is a message reference object (docs: http-api/messages.mdx).
type MessageReference struct {
	MessageID string `json:"message_id,omitempty"`
	ChannelID string `json:"channel_id,omitempty"`
	GuildID   string `json:"guild_id,omitempty"`
	Type      int    `json:"type,omitempty"`
}

// Reaction is a reaction object (docs: http-api/messages.mdx "Reaction object").
type Reaction struct {
	Emoji Emoji `json:"emoji"`
	Count int   `json:"count"`
	Me    bool  `json:"me,omitempty"`
}

// Emoji is an emoji object. ID is set only for custom emoji.
type Emoji struct {
	ID       *string `json:"id"`
	Name     string  `json:"name"`
	Animated bool    `json:"animated,omitempty"`
}

// Key returns the path value used for reaction routes: the Unicode emoji, or "name:id" for custom emoji.
func (e Emoji) Key() string {
	if e.ID != nil && *e.ID != "" {
		return e.Name + ":" + *e.ID
	}
	return e.Name
}

// Gateway payloads.

// Ready is the READY dispatch (docs: gateway/events.md "READY").
type Ready struct {
	SessionID       string       `json:"session_id"`
	User            User         `json:"user"`
	Guilds          []GuildState `json:"guilds"`
	PrivateChannels []Channel    `json:"private_channels"`
}

// GuildState is the guild ready object used in READY and GUILD_CREATE (docs: gateway/events.md).
type GuildState struct {
	ID          string    `json:"id"`
	Properties  Guild     `json:"properties"`
	Channels    []Channel `json:"channels"`
	Unavailable bool      `json:"unavailable,omitempty"`
}

// ReactionEvent is the payload of MESSAGE_REACTION_ADD and MESSAGE_REACTION_REMOVE.
type ReactionEvent struct {
	UserID    string `json:"user_id"`
	ChannelID string `json:"channel_id"`
	MessageID string `json:"message_id"`
	GuildID   string `json:"guild_id,omitempty"`
	Emoji     Emoji  `json:"emoji"`
}

// TypingEvent is the payload of TYPING_START.
type TypingEvent struct {
	ChannelID string `json:"channel_id"`
	UserID    string `json:"user_id"`
	GuildID   string `json:"guild_id,omitempty"`
}

// MessageDeleteEvent is the payload of MESSAGE_DELETE.
type MessageDeleteEvent struct {
	ID        string `json:"id"`
	ChannelID string `json:"channel_id"`
	GuildID   string `json:"guild_id,omitempty"`
}

// MessageDeleteBulkEvent is the payload of MESSAGE_DELETE_BULK.
type MessageDeleteBulkEvent struct {
	IDs       []string `json:"ids"`
	ChannelID string   `json:"channel_id"`
	GuildID   string   `json:"guild_id,omitempty"`
}

// MessageAckEvent is the payload of MESSAGE_ACK.
type MessageAckEvent struct {
	ChannelID    string `json:"channel_id"`
	MessageID    string `json:"message_id"`
	MentionCount int    `json:"mention_count"`
}

// ChannelRecipientEvent is the payload of CHANNEL_RECIPIENT_ADD and CHANNEL_RECIPIENT_REMOVE.
type ChannelRecipientEvent struct {
	ChannelID string `json:"channel_id"`
	User      User   `json:"user"`
}

// GuildRef is the payload of GUILD_DELETE.
type GuildRef struct {
	ID          string `json:"id"`
	Unavailable bool   `json:"unavailable,omitempty"`
}
