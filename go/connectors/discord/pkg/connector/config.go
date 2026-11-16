// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	_ "embed"

	up "go.mau.fi/util/configupgrade"
)

//go:embed example-config.yaml
var ExampleConfig string

// Config is the connector-specific part of the bridge configuration.
type Config struct {
	SyncChannelLimit     int            `yaml:"sync_channel_limit"`
	CustomEmojiReactions bool           `yaml:"custom_emoji_reactions"`
	Backfill             BackfillConfig `yaml:"backfill"`
}

// BackfillConfig controls history fetching from Discord.
type BackfillConfig struct {
	Enabled       bool `yaml:"enabled"`
	MaxBatchCount int  `yaml:"max_batch_count"`
}

// maxDiscordPageSize is the largest page Discord returns from the channel messages endpoint.
const maxDiscordPageSize = 100

func (dc *DiscordConnector) GetConfig() (example string, data any, upgrader up.Upgrader) {
	return ExampleConfig, &dc.Config, up.SimpleUpgrader(upgradeConfig)
}

func upgradeConfig(helper up.Helper) {
	helper.Copy(up.Int, "sync_channel_limit")
	helper.Copy(up.Bool, "custom_emoji_reactions")
	helper.Copy(up.Bool, "backfill", "enabled")
	helper.Copy(up.Int, "backfill", "max_batch_count")
}

// ValidateConfig clamps values that would otherwise make the connector misbehave.
func (dc *DiscordConnector) ValidateConfig() error {
	if dc.Config.Backfill.MaxBatchCount <= 0 || dc.Config.Backfill.MaxBatchCount > maxDiscordPageSize {
		dc.Config.Backfill.MaxBatchCount = maxDiscordPageSize / 2
	}
	if dc.Config.SyncChannelLimit < 0 {
		dc.Config.SyncChannelLimit = 0
	}
	return nil
}
