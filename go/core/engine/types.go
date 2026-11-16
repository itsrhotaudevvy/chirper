package engine

import (
	"maunium.net/go/mautrix/id"
)

// Network describes a network that the app lists. It is the JSON Network of ARCHITECTURE.md. An
// unavailable network is listed with Available false and the Reason, and has no login flows.
type Network struct {
	ID           string       `json:"id"`
	Name         string       `json:"name"`
	Color        string       `json:"color"`
	Beta         bool         `json:"beta"`
	LoginFlows   []LoginFlow  `json:"loginFlows"`
	Capabilities Capabilities `json:"capabilities"`
	Available    bool         `json:"available"`
	Reason       string       `json:"reason"`
}

// LoginFlow is one way to log into a network.
type LoginFlow struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Description string `json:"description"`
}

// Capabilities lists the features a network supports through Chirper.
type Capabilities struct {
	Edits        bool `json:"edits"`
	Reactions    bool `json:"reactions"`
	Replies      bool `json:"replies"`
	Redactions   bool `json:"redactions"`
	Typing       bool `json:"typing"`
	ReadReceipts bool `json:"readReceipts"`
	Stickers     bool `json:"stickers"`
	Voice        bool `json:"voice"`
}

// Account is one logged-in account of a network.
type Account struct {
	Network   string `json:"network"`
	LoginID   string `json:"loginId"`
	Name      string `json:"name"`
	AvatarURL string `json:"avatarUrl"`
	State     string `json:"state"`
	Error     string `json:"error"`
	Message   string `json:"message"`
}

// LoginStep is the step a login process is waiting on, as shown to the user.
type LoginStep struct {
	Type           string         `json:"type"`
	StepID         string         `json:"stepId"`
	Instructions   string         `json:"instructions"`
	UserInput      *UserInputStep `json:"userInput,omitempty"`
	Cookies        *CookiesStep   `json:"cookies,omitempty"`
	DisplayAndWait *DisplayStep   `json:"displayAndWait,omitempty"`
	Complete       *CompleteStep  `json:"complete,omitempty"`
}

// UserInputStep asks the user for values.
type UserInputStep struct {
	Fields []InputField `json:"fields"`
}

// InputField is one value requested by a user input step.
type InputField struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Type        string   `json:"type"`
	Description string   `json:"description"`
	Options     []string `json:"options"`
	Pattern     string   `json:"pattern"`
}

// CookiesStep asks the user to log in to a web page so that cookies can be collected.
type CookiesStep struct {
	URL               string        `json:"url"`
	UserAgent         string        `json:"userAgent"`
	Fields            []CookieField `json:"fields"`
	ExtractJS         string        `json:"extractJs"`
	WaitForURLPattern string        `json:"waitForUrlPattern,omitempty"`
}

// CookieField is one cookie or token collected from the web page.
type CookieField struct {
	ID       string         `json:"id"`
	Required bool           `json:"required"`
	Sources  []CookieSource `json:"sources"`
}

// CookieSource says where a CookieField can be found.
type CookieSource struct {
	Type         string `json:"type"`
	Name         string `json:"name"`
	CookieDomain string `json:"cookieDomain"`
}

// DisplayStep shows something the user must scan or confirm.
type DisplayStep struct {
	Type     string `json:"type"`
	Data     string `json:"data"`
	ImageURL string `json:"imageUrl"`
}

// CompleteStep reports the login that was created.
type CompleteStep struct {
	LoginID string `json:"loginId"`
}

// LoginState is the JSON LoginState of ARCHITECTURE.md.
type LoginState struct {
	ProcessID string    `json:"processId"`
	Step      LoginStep `json:"step"`
}

// ChatFlags are the local-only chat flags. Nil fields are left unchanged.
type ChatFlags struct {
	Pinned     *bool  `json:"pinned"`
	Archived   *bool  `json:"archived"`
	MutedUntil *int64 `json:"mutedUntil"`
}

// MediaUpload describes a file to send as a media message.
type MediaUpload struct {
	FilePath string
	MimeType string
	FileName string
	Caption  string
	Kind     string
	ReplyTo  id.EventID
	TxnID    string
}
