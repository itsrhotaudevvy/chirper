// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"errors"
	"fmt"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/id"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// Login flow IDs.
const (
	flowToken    = "token"
	flowBot      = "bot"
	flowPassword = "password"
)

// errNotSupportedYet is returned for sign-in steps that this connector does not implement.
var errNotSupportedYet = errors.New("not supported yet")

var loginFlows = []bridgev2.LoginFlow{
	{
		Name:        "Session token",
		Description: "Sign in with a session token from the Fluxer app",
		ID:          flowToken,
	},
	{
		Name:        "Bot token",
		Description: "Sign in with the token of a Fluxer bot account",
		ID:          flowBot,
	},
	{
		Name:        "Email or username & password",
		Description: "Sign in with a password; authenticator codes are supported",
		ID:          flowPassword,
	},
}

func (f *FluxerConnector) GetLoginFlows() []bridgev2.LoginFlow {
	return loginFlows
}

func (f *FluxerConnector) CreateLogin(_ context.Context, user *bridgev2.User, flowID string) (bridgev2.LoginProcess, error) {
	base := loginBase{user: user, main: f}
	switch flowID {
	case flowToken:
		return &tokenLogin{loginBase: base}, nil
	case flowBot:
		return &tokenLogin{loginBase: base, bot: true}, nil
	case flowPassword:
		return &passwordLogin{loginBase: base}, nil
	}
	return nil, bridgev2.ErrInvalidLoginFlowID
}

// loginBase holds what every flow needs to finish a sign-in.
type loginBase struct {
	user *bridgev2.User
	main *FluxerConnector
}

func instanceField(defaultInstance string) bridgev2.LoginInputDataField {
	return bridgev2.LoginInputDataField{
		Type:         bridgev2.LoginInputFieldTypeURL,
		ID:           "instance",
		Name:         "Instance",
		Description:  "Fluxer instance URL. Leave the default for fluxer.app",
		DefaultValue: defaultInstance,
	}
}

// finish saves the login with its credential in the metadata. The credential is never logged.
func (b loginBase) finish(ctx context.Context, origin string, ep *fluxerapi.Endpoints, cred fluxerapi.Credential, user *fluxerapi.User) (*bridgev2.LoginStep, error) {
	meta := &UserLoginMetadata{
		Instance:  origin,
		APIBase:   ep.API,
		MediaBase: ep.Media,
		Token:     cred.Token,
		Bot:       cred.Bot,
		UserID:    user.ID,
	}
	var avatarURI id.ContentURIString
	if user.Avatar != nil && *user.Avatar != "" {
		mediaBase := ep.Media
		if mediaBase == "" {
			mediaBase = ep.API
		}
		if mediaBase == "" {
			mediaBase = origin
		}
		avatarURI = id.ContentURIString(imageURL(mediaBase, "/avatars/"+user.ID, *user.Avatar))
	}
	ul, err := b.user.NewLogin(ctx, &database.UserLogin{
		ID:         makeUserLoginID(user.ID),
		RemoteName: user.Username,
		RemoteProfile: status.RemoteProfile{
			Username: user.Username,
			Name:     user.DisplayName(),
			Avatar:   avatarURI,
		},
		Metadata: meta,
	}, &bridgev2.NewLoginParams{DeleteOnConflict: true})
	if err != nil {
		return nil, fmt.Errorf("saving login: %w", err)
	}
	if client, ok := ul.Client.(*fluxerClient); ok && client != nil {
		go client.Connect(ul.Log.WithContext(context.Background()))
	}
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeComplete,
		StepID:       "org.chirper.fluxer.complete",
		Instructions: fmt.Sprintf("Signed in as %s", user.Username),
		CompleteParams: &bridgev2.LoginCompleteParams{
			UserLoginID: ul.ID,
			UserLogin:   ul,
		},
	}, nil
}

// discover normalizes the instance URL and returns its endpoints.
func (b loginBase) discover(ctx context.Context, instance string) (string, *fluxerapi.Endpoints, error) {
	policy := instancePolicy()
	origin, err := fluxerapi.NormalizeOrigin(instance, policy)
	if err != nil {
		return "", nil, err
	}
	ep, err := fluxerapi.Discover(ctx, b.main.HTTP, origin, policy)
	if err != nil {
		return "", nil, err
	}
	return origin, ep, nil
}

// tokenLogin signs in with a session token or a bot token. The token is verified with GET /users/@me.
type tokenLogin struct {
	loginBase
	bot bool
}

var _ bridgev2.LoginProcessUserInput = (*tokenLogin)(nil)

func (t *tokenLogin) Start(_ context.Context) (*bridgev2.LoginStep, error) {
	tokenField := bridgev2.LoginInputDataField{
		Type:        bridgev2.LoginInputFieldTypeToken,
		ID:          "token",
		Name:        "Token",
		Description: "Session token, or bot token in the form <application_id>.<secret>",
	}
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeUserInput,
		StepID:       "org.chirper.fluxer.token",
		Instructions: "Enter the instance and token",
		UserInputParams: &bridgev2.LoginUserInputParams{
			Fields: []bridgev2.LoginInputDataField{instanceField(t.main.Config.DefaultInstance), tokenField},
		},
	}, nil
}

func (t *tokenLogin) Cancel() {}

func (t *tokenLogin) SubmitUserInput(ctx context.Context, input map[string]string) (*bridgev2.LoginStep, error) {
	origin, ep, err := t.discover(ctx, input["instance"])
	if err != nil {
		return nil, err
	}
	cred := fluxerapi.Credential{Token: input["token"], Bot: t.bot}
	user, err := fluxerapi.NewClient(ep.API, cred, t.main.HTTP).Me(ctx)
	if err != nil {
		return nil, fmt.Errorf("the token was rejected: %w", err)
	}
	return t.finish(ctx, origin, ep, cred, user)
}

// passwordLogin signs in with a login and password, then with an authenticator code when needed.
// The password is only used for the first request and is never stored.
type passwordLogin struct {
	loginBase
	origin string
	ep     *fluxerapi.Endpoints
	ticket string
}

var _ bridgev2.LoginProcessUserInput = (*passwordLogin)(nil)

func (p *passwordLogin) Start(_ context.Context) (*bridgev2.LoginStep, error) {
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeUserInput,
		StepID:       "org.chirper.fluxer.password",
		Instructions: "Enter your instance, email or username, and password",
		UserInputParams: &bridgev2.LoginUserInputParams{
			Fields: []bridgev2.LoginInputDataField{
				instanceField(p.main.Config.DefaultInstance),
				{Type: bridgev2.LoginInputFieldTypeUsername, ID: "login", Name: "Email or username", Description: "The email address or username of the account"},
				{Type: bridgev2.LoginInputFieldTypePassword, ID: "password", Name: "Password", Description: "The account password"},
			},
		},
	}, nil
}

func (p *passwordLogin) Cancel() {}

func (p *passwordLogin) SubmitUserInput(ctx context.Context, input map[string]string) (*bridgev2.LoginStep, error) {
	if p.ticket != "" {
		return p.submitCode(ctx, input["code"])
	}
	origin, ep, err := p.discover(ctx, input["instance"])
	if err != nil {
		return nil, err
	}
	p.origin, p.ep = origin, ep
	api := fluxerapi.NewClient(ep.API, fluxerapi.Credential{}, p.main.HTTP)
	res, err := api.Login(ctx, fluxerapi.LoginRequest{Login: input["login"], Password: input["password"]})
	if err != nil {
		return nil, loginError(err)
	}
	if !res.MFA {
		return p.finish(ctx, origin, ep, fluxerapi.Credential{Token: res.Token}, &res.User)
	}
	if !res.TOTP && !res.BackupCodes {
		return nil, fmt.Errorf("%w: this account only has a passkey as second factor", errNotSupportedYet)
	}
	p.ticket = res.Ticket
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeUserInput,
		StepID:       "org.chirper.fluxer.password.mfa",
		Instructions: "Enter the 6-digit code from your authenticator app, or a backup code",
		UserInputParams: &bridgev2.LoginUserInputParams{
			Fields: []bridgev2.LoginInputDataField{
				{Type: bridgev2.LoginInputFieldType2FACode, ID: "code", Name: "Code", Description: "Authenticator or backup code"},
			},
		},
	}, nil
}

func (p *passwordLogin) submitCode(ctx context.Context, code string) (*bridgev2.LoginStep, error) {
	api := fluxerapi.NewClient(p.ep.API, fluxerapi.Credential{}, p.main.HTTP)
	res, err := api.LoginTOTP(ctx, p.ticket, code)
	if err != nil {
		return nil, loginError(err)
	}
	return p.finish(ctx, p.origin, p.ep, fluxerapi.Credential{Token: res.Token}, &res.User)
}

// loginError turns API failures into messages that are safe to show the user.
func loginError(err error) error {
	if fluxerapi.HasCode(err, "IP_AUTHORIZATION_REQUIRED") {
		return fmt.Errorf("%w: signing in from a new IP address needs email authorization", errNotSupportedYet)
	}
	return err
}
