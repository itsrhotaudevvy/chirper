package testnet

import (
	"context"
	"errors"
	"strings"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
)

// loginProcess is the single-step login of the fake network: the user picks an account name.
type loginProcess struct {
	conn *Connector
	user *bridgev2.User
}

var _ bridgev2.LoginProcessUserInput = (*loginProcess)(nil)

// Start implements bridgev2.LoginProcess.
func (p *loginProcess) Start(context.Context) (*bridgev2.LoginStep, error) {
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeUserInput,
		StepID:       "demo.name",
		Instructions: "Enter any account name",
		UserInputParams: &bridgev2.LoginUserInputParams{
			Fields: []bridgev2.LoginInputDataField{{
				Type:        bridgev2.LoginInputFieldTypeUsername,
				ID:          "name",
				Name:        "Account name",
				Description: "Name of the fake account",
			}},
		},
	}, nil
}

// SubmitUserInput implements bridgev2.LoginProcessUserInput. It creates the login and starts
// the fake connection.
func (p *loginProcess) SubmitUserInput(ctx context.Context, input map[string]string) (*bridgev2.LoginStep, error) {
	name := strings.TrimSpace(input["name"])
	if name == "" {
		return nil, errors.New("account name is required")
	}
	login, err := p.user.NewLogin(ctx, &database.UserLogin{
		ID:         networkid.UserLoginID("demo-" + name),
		RemoteName: name,
		Metadata:   &LoginMetadata{Name: name},
	}, &bridgev2.NewLoginParams{DeleteOnConflict: true})
	if err != nil {
		return nil, err
	}
	login.Client.Connect(ctx)
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeComplete,
		StepID:       "demo.complete",
		Instructions: "Logged in as " + name,
		CompleteParams: &bridgev2.LoginCompleteParams{
			UserLoginID: login.ID,
			UserLogin:   login,
		},
	}, nil
}

// Cancel implements bridgev2.LoginProcess.
func (p *loginProcess) Cancel() {}
