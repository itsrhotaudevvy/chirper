// SPDX-License-Identifier: AGPL-3.0-or-later

// Package connector implements the Fluxer network connector for mautrix bridgev2.
package connector

import (
	"context"
	"fmt"
	"net/http"
	"time"

	"go.mau.fi/util/configupgrade"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// FluxerConnector is the bridgev2 NetworkConnector for Fluxer.
type FluxerConnector struct {
	Bridge *bridgev2.Bridge
	Config Config
	// HTTP is used for discovery, login, media downloads and avatars.
	HTTP *http.Client
}

var _ bridgev2.NetworkConnector = (*FluxerConnector)(nil)

// NewConnector returns a connector with default settings. Init must be called before use.
func NewConnector() *FluxerConnector {
	return &FluxerConnector{
		Config: Config{DefaultInstance: fluxerapi.DefaultInstance},
		HTTP:   fluxerapi.NewHTTPClient(2 * time.Minute),
	}
}

func (f *FluxerConnector) GetName() bridgev2.BridgeName {
	return bridgev2.BridgeName{
		DisplayName:          "Fluxer",
		NetworkURL:           "https://fluxer.app",
		NetworkID:            "fluxer",
		BeeperBridgeType:     "fluxer",
		DefaultPort:          29350,
		DefaultCommandPrefix: "!fluxer",
	}
}

func (f *FluxerConnector) GetDBMetaTypes() database.MetaTypes {
	return database.MetaTypes{
		UserLogin: func() any { return &UserLoginMetadata{} },
	}
}

func (f *FluxerConnector) Init(bridge *bridgev2.Bridge) {
	f.Bridge = bridge
}

func (f *FluxerConnector) Start(ctx context.Context) error {
	return nil
}

// LoadUserLogin builds the network client for a stored login. It does no I/O; Connect does.
func (f *FluxerConnector) LoadUserLogin(ctx context.Context, login *bridgev2.UserLogin) error {
	meta, ok := login.Metadata.(*UserLoginMetadata)
	if !ok || meta == nil {
		return fmt.Errorf("fluxer: login %s has no metadata", login.ID)
	}
	login.Client = newClient(login, f, meta)
	return nil
}

func (f *FluxerConnector) GetCapabilities() *bridgev2.NetworkGeneralCapabilities {
	return generalCaps
}

// GetConfig returns the connector's config section. The only setting is the default instance
// offered on the login screen.
func (f *FluxerConnector) GetConfig() (example string, data any, upgrader configupgrade.Upgrader) {
	return ExampleConfig, &f.Config, &configupgrade.StructUpgrader{
		SimpleUpgrader: configupgrade.SimpleUpgrader(func(helper configupgrade.Helper) {
			helper.Copy(configupgrade.Str, "default_instance")
		}),
		Blocks: [][]string{},
		Base:   ExampleConfig,
	}
}

func (f *FluxerConnector) GetBridgeInfoVersion() (info, capabilities int) {
	return 1, 1
}
