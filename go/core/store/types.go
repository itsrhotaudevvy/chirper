package store

import (
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// ServerName is the Matrix server name used for all locally generated IDs.
const ServerName = "chirper.local"

// SelfUserID is the local user that owns the Chirper account.
const SelfUserID id.UserID = "@me:" + ServerName

// Chat is the JSON view of a room, as described in docs/ARCHITECTURE.md.
type Chat struct {
	RoomID       id.RoomID    `json:"roomId"`
	Network      string       `json:"network"`
	LoginID      string       `json:"loginId"`
	Name         string       `json:"name"`
	AvatarURL    string       `json:"avatarUrl"`
	Topic        string       `json:"topic"`
	IsDM         bool         `json:"isDm"`
	IsGroup      bool         `json:"isGroup"`
	IsSpace      bool         `json:"isSpace"`
	MemberCount  int          `json:"memberCount"`
	LastActivity int64        `json:"lastActivity"`
	Preview      *ChatPreview `json:"preview"`
	UnreadCount  int          `json:"unreadCount"`
	MentionCount int          `json:"mentionCount"`
	Pinned       bool         `json:"pinned"`
	Archived     bool         `json:"archived"`
	MutedUntil   int64        `json:"mutedUntil"`
	Typing       []string     `json:"typing"`
	ParentID     string       `json:"parentId"`
	ServerName   string       `json:"serverName"`
	IsChannel    bool         `json:"isChannel"`
}

// ChatPreview is the last visible, non-state event of a chat.
type ChatPreview struct {
	Sender string `json:"sender"`
	Body   string `json:"body"`
	Type   string `json:"type"`
}

// Message is the JSON view of a timeline event, as described in docs/ARCHITECTURE.md.
type Message struct {
	EventID       id.EventID        `json:"eventId"`
	RoomID        id.RoomID         `json:"roomId"`
	Order         int64             `json:"order"`
	Sender        id.UserID         `json:"sender"`
	SenderName    string            `json:"senderName"`
	SenderAvatar  string            `json:"senderAvatar"`
	FromMe        bool              `json:"fromMe"`
	TS            int64             `json:"ts"`
	Type          string            `json:"type"`
	Body          string            `json:"body"`
	FormattedBody string            `json:"formattedBody"`
	Media         *MessageMedia     `json:"media"`
	ReplyTo       *MessageReply     `json:"replyTo"`
	Edited        bool              `json:"edited"`
	Redacted      bool              `json:"redacted"`
	Reactions     []MessageReaction `json:"reactions"`
	Status        string            `json:"status"`
	Error         string            `json:"error"`
	// Live is set only by the engine, on a message event that arrived in real time. Stored
	// messages (lists and searches) are never live.
	Live bool `json:"live"`
}

// MessageMedia describes the attachment of a media message.
type MessageMedia struct {
	URL          string `json:"url"`
	Mime         string `json:"mime"`
	Size         int    `json:"size"`
	Width        int    `json:"width"`
	Height       int    `json:"height"`
	DurationMs   int    `json:"durationMs"`
	FileName     string `json:"fileName"`
	ThumbnailURL string `json:"thumbnailUrl"`
	Blurhash     string `json:"blurhash"`
}

// MessageReply is the preview of the message a message replies to.
type MessageReply struct {
	EventID    id.EventID `json:"eventId"`
	SenderName string     `json:"senderName"`
	Body       string     `json:"body"`
}

// MessageReaction is a reaction key aggregated over all senders of a message.
type MessageReaction struct {
	Key     string   `json:"key"`
	Count   int      `json:"count"`
	FromMe  bool     `json:"fromMe"`
	Senders []string `json:"senders"`
}

// ChatFilter selects chats for ListChats. Archived selects only archived chats when true and
// only non-archived chats when false.
type ChatFilter struct {
	Network     string `json:"network"`
	Archived    bool   `json:"archived"`
	UnreadOnly  bool   `json:"unreadOnly"`
	Query       string `json:"query"`
	DirectOnly  bool   `json:"directOnly"`
	ServersOnly bool   `json:"serversOnly"`
}

// Room is the stored row of a chat.
type Room struct {
	RoomID         id.RoomID
	BridgeID       string
	LoginID        string
	PortalID       string
	Name           string
	Topic          string
	AvatarURL      string
	IsDM           bool
	IsSpace        bool
	DMUserID       id.UserID
	CreatedTS      int64
	LastActivityTS int64
	LastEventID    id.EventID
	UnreadCount    int
	MentionCount   int
	ReadUpToOrder  int64
	Pinned         bool
	Archived       bool
	MutedUntil     int64
	TagsJSON       string
	Deleted        bool
	ParentID       string
}

// RoomFields is a partial update for SetRoomFields. Nil fields are left unchanged.
type RoomFields struct {
	LoginID    *string
	PortalID   *string
	Name       *string
	Topic      *string
	AvatarURL  *string
	Pinned     *bool
	Archived   *bool
	MutedUntil *int64
	TagsJSON   *string
	ParentID   *string
}

// Member is a room member row.
type Member struct {
	RoomID      id.RoomID
	UserID      id.UserID
	Displayname string
	AvatarURL   id.ContentURIString
	Membership  event.Membership
}

// Profile is the global profile of a user (ghosts and the local user).
type Profile struct {
	UserID      id.UserID
	Displayname string
	AvatarURL   id.ContentURIString
	ExtraJSON   string
}

// Event is a stored timeline event. ContentJSON is the raw JSON of the event content.
type Event struct {
	EventID     id.EventID
	RoomID      id.RoomID
	Order       int64
	Sender      id.UserID
	Type        string
	StateKey    *string
	ContentJSON string
	TS          int64
	TxnID       string
	ReplyTo     id.EventID
	EditJSON    *string
	EditedTS    int64
	Redacted    bool
	Status      string
	Error       string
	PartIndex   int
}

// Reaction is a stored reaction event.
type Reaction struct {
	EventID       id.EventID
	RoomID        id.RoomID
	TargetEventID id.EventID
	Sender        id.UserID
	Key           string
	TS            int64
}

// Redaction describes what RedactEvent changed. For reactions, EventID is the reaction and
// TargetEventID is the message that the reaction belonged to.
type Redaction struct {
	RoomID        id.RoomID
	EventID       id.EventID
	TargetEventID id.EventID
	IsReaction    bool
}

// Media is a stored media file. Path is the absolute path of the bytes on disk.
type Media struct {
	MediaID          string
	Mime             string
	Size             int64
	FileName         string
	Path             string
	EncryptedKeyJSON *string
	CreatedTS        int64
}
