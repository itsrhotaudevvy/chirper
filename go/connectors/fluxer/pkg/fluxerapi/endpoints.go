// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"mime/multipart"
	"net/textproto"
	"net/url"
	"strconv"
)

// guildPageSize is the largest page that GET /users/@me/guilds accepts.
const guildPageSize = 200

// Me returns the authenticated account (GET /users/@me, bot-callable).
func (c *Client) Me(ctx context.Context) (*User, error) {
	var u User
	if err := c.call(ctx, "GET", "/users/@me", callOpts{}, &u); err != nil {
		return nil, err
	}
	return &u, nil
}

// UserByID returns a user (GET /users/{id}).
func (c *Client) UserByID(ctx context.Context, id string) (*User, error) {
	var u User
	if err := c.call(ctx, "GET", "/users/"+url.PathEscape(id), callOpts{}, &u); err != nil {
		return nil, err
	}
	return &u, nil
}

// Guilds lists every guild the account is a member of, following the cursor
// (GET /users/@me/guilds, docs: http-api/guilds.mdx).
func (c *Client) Guilds(ctx context.Context) ([]Guild, error) {
	var all []Guild
	after := ""
	for {
		q := url.Values{"limit": {strconv.Itoa(guildPageSize)}}
		if after != "" {
			q.Set("after", after)
		}
		var page []Guild
		if err := c.call(ctx, "GET", "/users/@me/guilds", callOpts{query: q}, &page); err != nil {
			return nil, err
		}
		all = append(all, page...)
		if len(page) < guildPageSize {
			return all, nil
		}
		after = page[len(page)-1].ID
	}
}

// Guild returns one guild (GET /guilds/{id}, bot-callable).
func (c *Client) Guild(ctx context.Context, id string) (*Guild, error) {
	var g Guild
	if err := c.call(ctx, "GET", "/guilds/"+url.PathEscape(id), callOpts{}, &g); err != nil {
		return nil, err
	}
	return &g, nil
}

// GuildChannels lists the channels of a guild the account can see (GET /guilds/{id}/channels).
func (c *Client) GuildChannels(ctx context.Context, guildID string) ([]Channel, error) {
	var chans []Channel
	err := c.call(ctx, "GET", "/guilds/"+url.PathEscape(guildID)+"/channels", callOpts{}, &chans)
	return chans, err
}

// Channel returns one channel (GET /channels/{id}).
func (c *Client) Channel(ctx context.Context, id string) (*Channel, error) {
	var ch Channel
	if err := c.call(ctx, "GET", "/channels/"+url.PathEscape(id), callOpts{}, &ch); err != nil {
		return nil, err
	}
	return &ch, nil
}

// PrivateChannels lists open DMs and group DMs (GET /users/@me/channels).
func (c *Client) PrivateChannels(ctx context.Context) ([]Channel, error) {
	var chans []Channel
	err := c.call(ctx, "GET", "/users/@me/channels", callOpts{}, &chans)
	return chans, err
}

// OpenDM opens (or returns the existing) direct message with recipientID (POST /users/@me/channels).
func (c *Client) OpenDM(ctx context.Context, recipientID string) (*Channel, error) {
	opt, err := jsonBody(map[string]string{"recipient_id": recipientID})
	if err != nil {
		return nil, err
	}
	var ch Channel
	if err := c.call(ctx, "POST", "/users/@me/channels", opt, &ch); err != nil {
		return nil, err
	}
	return &ch, nil
}

// MessageQuery selects a page of messages. Zero values are omitted.
type MessageQuery struct {
	Limit  int
	Before string
	After  string
}

// Messages lists messages in a channel, newest first (GET /channels/{id}/messages).
func (c *Client) Messages(ctx context.Context, channelID string, q MessageQuery) ([]Message, error) {
	vals := url.Values{}
	if q.Limit > 0 {
		vals.Set("limit", strconv.Itoa(q.Limit))
	}
	if q.Before != "" {
		vals.Set("before", q.Before)
	}
	if q.After != "" {
		vals.Set("after", q.After)
	}
	var msgs []Message
	err := c.call(ctx, "GET", "/channels/"+url.PathEscape(channelID)+"/messages", callOpts{query: vals}, &msgs)
	return msgs, err
}

// Message returns one message (GET /channels/{id}/messages/{message_id}).
func (c *Client) Message(ctx context.Context, channelID, messageID string) (*Message, error) {
	var m Message
	err := c.call(ctx, "GET", messagePath(channelID, messageID), callOpts{}, &m)
	if err != nil {
		return nil, err
	}
	return &m, nil
}

// MessageBody is the JSON body of Create message and Modify message (docs: http-api/messages.mdx).
type MessageBody struct {
	Content          *string           `json:"content,omitempty"`
	Nonce            string            `json:"nonce,omitempty"`
	MessageReference *MessageReference `json:"message_reference,omitempty"`
	Attachments      []AttachmentMeta  `json:"attachments,omitempty"`
}

// AttachmentMeta is a direct multipart attachment metadata object for a file sent with the message.
type AttachmentMeta struct {
	ID          int     `json:"id"`
	Filename    string  `json:"filename"`
	ContentType string  `json:"content_type,omitempty"`
	Description *string `json:"description,omitempty"`
}

// Upload is one file sent as files[ID] in a multipart message.
type Upload struct {
	Meta AttachmentMeta
	Data []byte
}

// CreateMessage sends a text message, optionally with files (POST /channels/{id}/messages).
// Files are sent as multipart/form-data with the JSON body in payload_json.
func (c *Client) CreateMessage(ctx context.Context, channelID string, body MessageBody, files ...Upload) (*Message, error) {
	opt, err := messageCallOpts(body, files)
	if err != nil {
		return nil, err
	}
	var m Message
	if err := c.call(ctx, "POST", "/channels/"+url.PathEscape(channelID)+"/messages", opt, &m); err != nil {
		return nil, err
	}
	return &m, nil
}

// EditMessage replaces the content of a message (PATCH /channels/{id}/messages/{message_id}).
func (c *Client) EditMessage(ctx context.Context, channelID, messageID string, content string) (*Message, error) {
	opt, err := jsonBody(map[string]string{"content": content})
	if err != nil {
		return nil, err
	}
	var m Message
	if err := c.call(ctx, "PATCH", messagePath(channelID, messageID), opt, &m); err != nil {
		return nil, err
	}
	return &m, nil
}

// DeleteMessage deletes a message (DELETE /channels/{id}/messages/{message_id}).
func (c *Client) DeleteMessage(ctx context.Context, channelID, messageID string) error {
	return c.call(ctx, "DELETE", messagePath(channelID, messageID), callOpts{}, nil)
}

// AddReaction adds the account's reaction with emoji (PUT .../reactions/{emoji}/@me).
func (c *Client) AddReaction(ctx context.Context, channelID, messageID, emoji string) error {
	return c.call(ctx, "PUT", reactionPath(channelID, messageID, emoji)+"/@me", callOpts{}, nil)
}

// RemoveOwnReaction removes the account's reaction with emoji (DELETE .../reactions/{emoji}/@me).
func (c *Client) RemoveOwnReaction(ctx context.Context, channelID, messageID, emoji string) error {
	return c.call(ctx, "DELETE", reactionPath(channelID, messageID, emoji)+"/@me", callOpts{}, nil)
}

// Typing shows that the account is typing in a channel (POST /channels/{id}/typing).
func (c *Client) Typing(ctx context.Context, channelID string) error {
	return c.call(ctx, "POST", "/channels/"+url.PathEscape(channelID)+"/typing", callOpts{}, nil)
}

// AckMessage marks a channel as read up to messageID (POST .../messages/{message_id}/ack).
// Only user sessions may call it; bots get 403.
func (c *Client) AckMessage(ctx context.Context, channelID, messageID string) error {
	opt, err := jsonBody(map[string]int{"mention_count": 0})
	if err != nil {
		return err
	}
	return c.call(ctx, "POST", messagePath(channelID, messageID)+"/ack", opt, nil)
}

// GatewayURL returns the Gateway WebSocket URL for bots (GET /gateway/bot).
func (c *Client) GatewayURL(ctx context.Context) (string, error) {
	var info struct {
		URL string `json:"url"`
	}
	if err := c.call(ctx, "GET", "/gateway/bot", callOpts{}, &info); err != nil {
		return "", err
	}
	return info.URL, nil
}

// Logout revokes the session behind a user token (POST /auth/logout). It is bot-callable and no-op for bots.
func (c *Client) Logout(ctx context.Context) error {
	return c.call(ctx, "POST", "/auth/logout", callOpts{}, nil)
}

func messagePath(channelID, messageID string) string {
	return "/channels/" + url.PathEscape(channelID) + "/messages/" + url.PathEscape(messageID)
}

func reactionPath(channelID, messageID, emoji string) string {
	return messagePath(channelID, messageID) + "/reactions/" + url.PathEscape(emoji)
}

// messageCallOpts builds a JSON body, or a multipart body when files are attached.
func messageCallOpts(body MessageBody, files []Upload) (callOpts, error) {
	if len(files) == 0 {
		return jsonBody(body)
	}
	metas := make([]AttachmentMeta, 0, len(files))
	for _, f := range files {
		metas = append(metas, f.Meta)
	}
	body.Attachments = metas
	payload, err := json.Marshal(body)
	if err != nil {
		return callOpts{}, err
	}
	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	if err := writeFormField(mw, "payload_json", "application/json", payload); err != nil {
		return callOpts{}, err
	}
	for _, f := range files {
		h := make(textproto.MIMEHeader)
		h.Set("Content-Disposition", fmt.Sprintf(`form-data; name="files[%d]"; filename=%q`, f.Meta.ID, f.Meta.Filename))
		if f.Meta.ContentType != "" {
			h.Set("Content-Type", f.Meta.ContentType)
		}
		part, err := mw.CreatePart(h)
		if err != nil {
			return callOpts{}, err
		}
		if _, err := part.Write(f.Data); err != nil {
			return callOpts{}, err
		}
	}
	if err := mw.Close(); err != nil {
		return callOpts{}, err
	}
	return callOpts{body: buf.Bytes(), contentType: mw.FormDataContentType()}, nil
}

func writeFormField(mw *multipart.Writer, name, contentType string, data []byte) error {
	h := make(textproto.MIMEHeader)
	h.Set("Content-Disposition", fmt.Sprintf(`form-data; name=%q`, name))
	h.Set("Content-Type", contentType)
	part, err := mw.CreatePart(h)
	if err != nil {
		return err
	}
	_, err = part.Write(data)
	return err
}
