package main

import (
	"encoding/json"
	"fmt"

	"chirper.dev/core/facade"
)

// maxArgsBytes is the largest accepted args_json.
const maxArgsBytes = 1 << 20

// handler decodes its args and calls the facade. It returns the raw JSON value (nil means null).
type handler func(c *facade.Core, args []byte) (json.RawMessage, error)

// dispatch runs the facade method named method with the JSON object args and returns its raw JSON
// value. Unknown methods and oversized args are errors.
func dispatch(c *facade.Core, method string, args []byte) (json.RawMessage, error) {
	if len(args) > maxArgsBytes {
		return nil, fmt.Errorf("args_json exceeds %d bytes", maxArgsBytes)
	}
	h, ok := handlers[method]
	if !ok {
		return nil, fmt.Errorf("unknown method %q", method)
	}
	if len(args) == 0 {
		args = []byte("{}")
	}
	return h(c, args)
}

// withArgs adapts fn to a handler: args decode into A and fn's raw JSON result is the value.
func withArgs[A any](fn func(*facade.Core, A) (json.RawMessage, error)) handler {
	return func(c *facade.Core, args []byte) (json.RawMessage, error) {
		var a A
		if err := json.Unmarshal(args, &a); err != nil {
			return nil, fmt.Errorf("invalid args: %w", err)
		}
		return fn(c, a)
	}
}

// unit adapts a facade method that returns only an error. Its value is null.
func unit[A any](fn func(*facade.Core, A) error) handler {
	return withArgs(func(c *facade.Core, a A) (json.RawMessage, error) {
		return nil, fn(c, a)
	})
}

// jsonString returns the facade's JSON string as a raw value. An empty string means null.
func jsonString(s string) json.RawMessage {
	if s == "" {
		return nil
	}
	return json.RawMessage(s)
}

// rawFrom returns a facade JSON string result with its error.
func rawFrom(s string, err error) (json.RawMessage, error) {
	if err != nil {
		return nil, err
	}
	return jsonString(s), nil
}

// objectOf marshals a value built from the facade's results.
func objectOf(v any, err error) (json.RawMessage, error) {
	if err != nil {
		return nil, err
	}
	b, merr := json.Marshal(v)
	return b, merr
}

type (
	processArgs struct {
		ProcessID string `json:"processId"`
	}
	networkArgs struct {
		Network string `json:"network"`
	}
	startLoginArgs struct {
		Network string `json:"network"`
		FlowID  string `json:"flowId"`
	}
	submitInputArgs struct {
		ProcessID string            `json:"processId"`
		Values    map[string]string `json:"values"`
	}
	submitCookiesArgs struct {
		ProcessID string            `json:"processId"`
		Cookies   map[string]string `json:"cookies"`
	}
	logoutArgs struct {
		Network string `json:"network"`
		LoginID string `json:"loginId"`
	}
	listChatsArgs struct {
		Filter json.RawMessage `json:"filter"`
	}
	roomArgs struct {
		RoomID string `json:"roomId"`
	}
	listMessagesArgs struct {
		RoomID      string `json:"roomId"`
		BeforeOrder int64  `json:"beforeOrder"`
		Limit       int    `json:"limit"`
	}
	markUnreadArgs struct {
		RoomID string `json:"roomId"`
		Unread bool   `json:"unread"`
	}
	sendTextArgs struct {
		RoomID  string `json:"roomId"`
		Text    string `json:"text"`
		ReplyTo string `json:"replyTo"`
		TxnID   string `json:"txnId"`
	}
	sendMediaArgs struct {
		RoomID   string `json:"roomId"`
		Path     string `json:"path"`
		Mime     string `json:"mime"`
		FileName string `json:"fileName"`
		Caption  string `json:"caption"`
		Kind     string `json:"kind"`
		ReplyTo  string `json:"replyTo"`
		TxnID    string `json:"txnId"`
	}
	editArgs struct {
		RoomID  string `json:"roomId"`
		EventID string `json:"eventId"`
		Text    string `json:"text"`
	}
	eventArgs struct {
		RoomID  string `json:"roomId"`
		EventID string `json:"eventId"`
	}
	reactArgs struct {
		RoomID  string `json:"roomId"`
		EventID string `json:"eventId"`
		Key     string `json:"key"`
	}
	typingArgs struct {
		RoomID string `json:"roomId"`
		Typing bool   `json:"typing"`
	}
	chatFlagsArgs struct {
		RoomID string          `json:"roomId"`
		Flags  json.RawMessage `json:"flags"`
	}
	mediaFileArgs struct {
		URL string `json:"url"`
	}
	searchArgs struct {
		Query string `json:"query"`
		Limit int    `json:"limit"`
	}
	settingsArgs struct {
		Settings json.RawMessage `json:"settings"`
	}
	empty struct{}
)

// Results with a fixed shape.
type (
	eventIDResult struct {
		EventID string `json:"eventId"`
	}
	hasMoreResult struct {
		HasMore bool `json:"hasMore"`
	}
	pathResult struct {
		Path string `json:"path"`
	}
)

// eventIDValue returns {"eventId":id}.
func eventIDValue(id string, err error) (json.RawMessage, error) {
	return objectOf(eventIDResult{EventID: id}, err)
}

// stringMap marshals a login field map for the facade's JSON parameters.
func stringMap(m map[string]string) (string, error) {
	b, err := json.Marshal(m)
	return string(b), err
}

var handlers = map[string]handler{
	"start": unit(func(c *facade.Core, _ empty) error { return c.Start() }),
	"networks": withArgs(func(c *facade.Core, _ empty) (json.RawMessage, error) {
		return jsonString(c.Networks()), nil
	}),
	"accounts": withArgs(func(c *facade.Core, _ empty) (json.RawMessage, error) {
		return jsonString(c.Accounts()), nil
	}),
	"startLogin": withArgs(func(c *facade.Core, a startLoginArgs) (json.RawMessage, error) {
		return rawFrom(c.StartLogin(a.Network, a.FlowID))
	}),
	"submitLoginInput": withArgs(func(c *facade.Core, a submitInputArgs) (json.RawMessage, error) {
		values, err := stringMap(a.Values)
		if err != nil {
			return nil, err
		}
		return rawFrom(c.SubmitLoginInput(a.ProcessID, values))
	}),
	"submitLoginCookies": withArgs(func(c *facade.Core, a submitCookiesArgs) (json.RawMessage, error) {
		cookies, err := stringMap(a.Cookies)
		if err != nil {
			return nil, err
		}
		return rawFrom(c.SubmitLoginCookies(a.ProcessID, cookies))
	}),
	"waitLoginStep": withArgs(func(c *facade.Core, a processArgs) (json.RawMessage, error) {
		return rawFrom(c.WaitLoginStep(a.ProcessID))
	}),
	"cancelLogin": unit(func(c *facade.Core, a processArgs) error {
		c.CancelLogin(a.ProcessID)
		return nil
	}),
	"logout": unit(func(c *facade.Core, a logoutArgs) error {
		return c.Logout(a.Network, a.LoginID)
	}),
	"listChats": withArgs(func(c *facade.Core, a listChatsArgs) (json.RawMessage, error) {
		return jsonString(c.ListChats(string(a.Filter))), nil
	}),
	"getChat": withArgs(func(c *facade.Core, a roomArgs) (json.RawMessage, error) {
		return jsonString(c.GetChat(a.RoomID)), nil
	}),
	"listMessages": withArgs(func(c *facade.Core, a listMessagesArgs) (json.RawMessage, error) {
		return jsonString(c.ListMessages(a.RoomID, a.BeforeOrder, a.Limit)), nil
	}),
	"loadOlder": withArgs(func(c *facade.Core, a roomArgs) (json.RawMessage, error) {
		hasMore, err := c.LoadOlder(a.RoomID)
		return objectOf(hasMoreResult{HasMore: hasMore}, err)
	}),
	"syncHistory": unit(func(c *facade.Core, a networkArgs) error {
		return c.SyncHistory(a.Network)
	}),
	"markUnread": unit(func(c *facade.Core, a markUnreadArgs) error {
		return c.MarkUnread(a.RoomID, a.Unread)
	}),
	"sendText": withArgs(func(c *facade.Core, a sendTextArgs) (json.RawMessage, error) {
		return eventIDValue(c.SendText(a.RoomID, a.Text, a.ReplyTo, a.TxnID))
	}),
	"sendMedia": withArgs(func(c *facade.Core, a sendMediaArgs) (json.RawMessage, error) {
		return eventIDValue(c.SendMedia(a.RoomID, a.Path, a.Mime, a.FileName, a.Caption, a.Kind, a.ReplyTo, a.TxnID))
	}),
	"edit": unit(func(c *facade.Core, a editArgs) error {
		return c.Edit(a.RoomID, a.EventID, a.Text)
	}),
	"redact": unit(func(c *facade.Core, a eventArgs) error {
		return c.Redact(a.RoomID, a.EventID)
	}),
	"react": unit(func(c *facade.Core, a reactArgs) error {
		return c.React(a.RoomID, a.EventID, a.Key)
	}),
	"unreact": unit(func(c *facade.Core, a reactArgs) error {
		return c.Unreact(a.RoomID, a.EventID, a.Key)
	}),
	"markRead": unit(func(c *facade.Core, a eventArgs) error {
		return c.MarkRead(a.RoomID, a.EventID)
	}),
	"setTyping": unit(func(c *facade.Core, a typingArgs) error {
		return c.SetTyping(a.RoomID, a.Typing)
	}),
	"setChatFlags": unit(func(c *facade.Core, a chatFlagsArgs) error {
		return c.SetChatLocalFlags(a.RoomID, string(a.Flags))
	}),
	"mediaFile": withArgs(func(c *facade.Core, a mediaFileArgs) (json.RawMessage, error) {
		path, err := c.MediaFile(a.URL)
		return objectOf(pathResult{Path: path}, err)
	}),
	"search": withArgs(func(c *facade.Core, a searchArgs) (json.RawMessage, error) {
		return jsonString(c.Search(a.Query, a.Limit)), nil
	}),
	"settings": withArgs(func(c *facade.Core, _ empty) (json.RawMessage, error) {
		return jsonString(c.Settings()), nil
	}),
	"setSettings": unit(func(c *facade.Core, a settingsArgs) error {
		return c.SetSettings(string(a.Settings))
	}),
}
