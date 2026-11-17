// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// Portions adapted from mautrix-discord (https://github.com/mautrix/discord),
// Copyright (C) 2023 Tulir Asokan, licensed under the GNU Affero General Public
// License v3 or later.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

package connector

import (
	"context"
	"net/http"
	"sync/atomic"
	"time"

	"maunium.net/go/mautrix/bridgev2"

	"chirper.dev/connectors/discord/pkg/httpsec"
)

// DiscordConnector is the bridgev2 network connector for Discord user and bot accounts.
type DiscordConnector struct {
	br     *bridgev2.Bridge
	Config Config

	maxFileSize atomic.Int64
	httpClient  *http.Client
}

var (
	_ bridgev2.NetworkConnector      = (*DiscordConnector)(nil)
	_ bridgev2.MaxFileSizeingNetwork = (*DiscordConnector)(nil)
	_ bridgev2.StoppableNetwork      = (*DiscordConnector)(nil)
)

// defaultMaxFileSize is the largest attachment the connector downloads; it is also the hard ceiling.
const defaultMaxFileSize = httpsec.MaxMediaBytes

func (dc *DiscordConnector) Init(bridge *bridgev2.Bridge) {
	dc.br = bridge
	dc.Config = Config{
		SyncChannelLimit:     1000,
		CustomEmojiReactions: true,
		Backfill: BackfillConfig{
			Enabled:       true,
			MaxBatchCount: 50,
		},
	}
	dc.maxFileSize.Store(defaultMaxFileSize)
	dc.httpClient = httpsec.NewClient(2*time.Minute, httpsec.MaxMediaBytes)
}

func (dc *DiscordConnector) Start(ctx context.Context) error {
	return nil
}

func (dc *DiscordConnector) Stop() {
	dc.httpClient.CloseIdleConnections()
	apiHTTP.CloseIdleConnections()
}

// SetMaxFileSize sets the attachment size limit, capped at the hard ceiling of httpsec.MaxMediaBytes.
func (dc *DiscordConnector) SetMaxFileSize(maxSize int64) {
	dc.maxFileSize.Store(min(maxSize, httpsec.MaxMediaBytes))
}

func (dc *DiscordConnector) GetName() bridgev2.BridgeName {
	return bridgev2.BridgeName{
		DisplayName:      "Discord",
		NetworkURL:       "https://discord.com",
		NetworkIcon:      "",
		NetworkID:        "discord",
		BeeperBridgeType: "discord",
		DefaultPort:      0,
	}
}

func (dc *DiscordConnector) GetCapabilities() *bridgev2.NetworkGeneralCapabilities {
	return &bridgev2.NetworkGeneralCapabilities{
		AggressiveUpdateInfo: false,
	}
}

func (dc *DiscordConnector) GetBridgeInfoVersion() (info, capabilities int) {
	return 1, 1
}
