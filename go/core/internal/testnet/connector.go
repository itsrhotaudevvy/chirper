// Package testnet is a fake bridgev2 network connector. It produces a fixed set of chats and
// replies to the local user, so the whole bridge pipeline can run without a real network.
package testnet

import (
	"context"
	"fmt"

	"go.mau.fi/util/configupgrade"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
)

// BridgeID is the bridge ID (and network ID) of the fake network.
const BridgeID networkid.BridgeID = "demo"

// LoginMetadata is stored with each login.
type LoginMetadata struct {
	Name string `json:"name"`
}

// Connector implements bridgev2.NetworkConnector for the fake network.
type Connector struct {
	br *bridgev2.Bridge
}

var _ bridgev2.NetworkConnector = (*Connector)(nil)

// New returns a connector. It must be passed to bridgev2.NewBridge, which calls Init.
func New() *Connector {
	return &Connector{}
}

// Init implements bridgev2.NetworkConnector.
func (c *Connector) Init(br *bridgev2.Bridge) {
	c.br = br
}

// Start implements bridgev2.NetworkConnector.
func (c *Connector) Start(context.Context) error {
	return nil
}

// GetName implements bridgev2.NetworkConnector.
func (c *Connector) GetName() bridgev2.BridgeName {
	return bridgev2.BridgeName{
		DisplayName:          "Demo",
		NetworkID:            string(BridgeID),
		BeeperBridgeType:     string(BridgeID),
		DefaultCommandPrefix: "!demo",
	}
}

// GetDBMetaTypes implements bridgev2.NetworkConnector.
func (c *Connector) GetDBMetaTypes() database.MetaTypes {
	return database.MetaTypes{
		UserLogin: func() any { return &LoginMetadata{} },
	}
}

// GetCapabilities implements bridgev2.NetworkConnector.
func (c *Connector) GetCapabilities() *bridgev2.NetworkGeneralCapabilities {
	return &bridgev2.NetworkGeneralCapabilities{}
}

// GetConfig implements bridgev2.NetworkConnector. The fake network has no config file.
func (c *Connector) GetConfig() (string, any, configupgrade.Upgrader) {
	return "", nil, nil
}

// LoadUserLogin implements bridgev2.NetworkConnector.
func (c *Connector) LoadUserLogin(_ context.Context, login *bridgev2.UserLogin) error {
	meta, ok := login.Metadata.(*LoginMetadata)
	if !ok {
		return fmt.Errorf("unexpected login metadata type %T", login.Metadata)
	}
	login.Client = newClient(login, meta.Name)
	return nil
}

// GetLoginFlows implements bridgev2.NetworkConnector.
func (c *Connector) GetLoginFlows() []bridgev2.LoginFlow {
	return []bridgev2.LoginFlow{{
		ID:          "demo",
		Name:        "Demo account",
		Description: "A fake network with a few chats",
	}}
}

// CreateLogin implements bridgev2.NetworkConnector.
func (c *Connector) CreateLogin(_ context.Context, user *bridgev2.User, flowID string) (bridgev2.LoginProcess, error) {
	if flowID != "demo" {
		return nil, bridgev2.ErrInvalidLoginFlowID
	}
	return &loginProcess{conn: c, user: user}, nil
}

// GetBridgeInfoVersion implements bridgev2.NetworkConnector.
func (c *Connector) GetBridgeInfoVersion() (info, capabilities int) {
	return 1, 1
}
