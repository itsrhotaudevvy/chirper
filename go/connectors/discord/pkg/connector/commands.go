// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// Portions adapted from mautrix-discord (commands_botinteraction.go),
// Copyright (C) 2023 Tulir Asokan, licensed under the GNU Affero General Public License v3 or later.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"time"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
)

// errBotInteractionsUnsupported is returned for bot logins: Discord does not let bots press buttons
// or run slash commands, only user accounts can.
var errBotInteractionsUnsupported = errors.New("not supported for bot accounts: Discord only allows user accounts to use components and slash commands")

// AppCommand describes a slash command visible in a channel.
type AppCommand struct {
	ID            string             `json:"id"`
	ApplicationID string             `json:"application_id"`
	Name          string             `json:"name"`
	Description   string             `json:"description"`
	Options       []AppCommandOption `json:"options,omitempty"`
}

// AppCommandOption is one parameter of an AppCommand.
type AppCommandOption struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Type        int    `json:"type"`
	Required    bool   `json:"required"`
}

// InvokeComponent presses a button or submits a select menu on a Discord message.
// values is only used for select menus. The component's type is read from the message itself.
func (c *DiscordClient) InvokeComponent(ctx context.Context, portal *bridgev2.Portal, messageID, customID string, values []string) error {
	if c.isBot() {
		return errBotInteractionsUnsupported
	}
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	channelID := string(portal.ID)
	msg, err := sess.ChannelMessage(channelID, messageID)
	if err != nil {
		return fmt.Errorf("failed to fetch message: %w", err)
	}
	componentType, err := findComponentType(msg.Components, customID)
	if err != nil {
		return err
	}
	if msg.Author == nil {
		return errors.New("message has no author to address the interaction to")
	}
	body := interactionPayload{
		Type:          interactionTypeComponent,
		Nonce:         interactionNonce(),
		GuildID:       guildIDForChannel(sess, channelID),
		ChannelID:     channelID,
		MessageFlags:  uint32(msg.Flags),
		MessageID:     messageID,
		ApplicationID: msg.Author.ID,
		SessionID:     c.gatewaySessionIDValue(),
		Data: componentInteractionData{
			ComponentType: int(componentType),
			CustomID:      customID,
			Values:        values,
		},
	}
	return postInteraction(sess, body)
}

// ListApplicationCommands returns the slash commands available in a channel.
func (c *DiscordClient) ListApplicationCommands(ctx context.Context, portal *bridgev2.Portal) ([]AppCommand, error) {
	if c.isBot() {
		return nil, errBotInteractionsUnsupported
	}
	sess, err := c.currentSession()
	if err != nil {
		return nil, err
	}
	commands, err := sess.ApplicationCommandsSearch(string(portal.ID), "")
	if err != nil {
		return nil, fmt.Errorf("failed to list application commands: %w", err)
	}
	result := make([]AppCommand, 0, len(commands))
	for _, cmd := range commands {
		result = append(result, wrapAppCommand(cmd))
	}
	return result, nil
}

// InvokeSlashCommand runs a slash command in a channel. options maps option names to values.
func (c *DiscordClient) InvokeSlashCommand(ctx context.Context, portal *bridgev2.Portal, appID, commandName string, options map[string]any) error {
	if c.isBot() {
		return errBotInteractionsUnsupported
	}
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	channelID := string(portal.ID)
	cmd, err := findApplicationCommand(sess, channelID, appID, commandName)
	if err != nil {
		return err
	}
	inputs, err := buildCommandOptions(cmd.Options, options)
	if err != nil {
		return err
	}
	return sess.SendInteractions(guildIDForChannel(sess, channelID), channelID, cmd, inputs, interactionNonce())
}

func wrapAppCommand(cmd *discordgo.ApplicationCommand) AppCommand {
	result := AppCommand{
		ID:            cmd.ID,
		ApplicationID: cmd.ApplicationID,
		Name:          cmd.Name,
		Description:   cmd.Description,
	}
	for _, opt := range cmd.Options {
		if opt == nil {
			continue
		}
		result.Options = append(result.Options, AppCommandOption{
			Name:        opt.Name,
			Description: opt.Description,
			Type:        int(opt.Type),
			Required:    opt.Required,
		})
	}
	return result
}

func findApplicationCommand(sess *discordgo.Session, channelID, appID, name string) (*discordgo.ApplicationCommand, error) {
	commands, err := sess.ApplicationCommandsSearch(channelID, name)
	if err != nil {
		return nil, fmt.Errorf("failed to search application commands: %w", err)
	}
	for _, cmd := range commands {
		if cmd.Name == name && cmd.ApplicationID == appID {
			return cmd, nil
		}
	}
	return nil, fmt.Errorf("slash command %q of application %s not found in this channel", name, appID)
}

// buildCommandOptions matches the caller's values to the command's declared options.
// Unknown names are rejected so typos do not silently send an incomplete command.
func buildCommandOptions(declared []*discordgo.ApplicationCommandOption, values map[string]any) ([]*discordgo.ApplicationCommandOptionInput, error) {
	known := make(map[string]bool, len(declared))
	inputs := make([]*discordgo.ApplicationCommandOptionInput, 0, len(values))
	for _, opt := range declared {
		if opt == nil {
			continue
		}
		known[opt.Name] = true
		value, ok := values[opt.Name]
		if !ok {
			if opt.Required {
				return nil, fmt.Errorf("missing required option %q", opt.Name)
			}
			continue
		}
		inputs = append(inputs, &discordgo.ApplicationCommandOptionInput{Type: opt.Type, Name: opt.Name, Value: value})
	}
	for name := range values {
		if !known[name] {
			return nil, fmt.Errorf("unknown option %q", name)
		}
	}
	return inputs, nil
}

// findComponentType returns the Discord component type (button or select menu) for a custom ID.
func findComponentType(components []discordgo.MessageComponent, customID string) (discordgo.ComponentType, error) {
	for _, row := range components {
		for _, child := range actionRowChildren(row) {
			switch c := child.(type) {
			case *discordgo.Button:
				if c.CustomID == customID {
					return discordgo.ButtonComponent, nil
				}
			case *discordgo.SelectMenu:
				if c.CustomID == customID {
					return discordgo.SelectMenuComponent, nil
				}
			}
		}
	}
	return 0, fmt.Errorf("component %q not found on message", customID)
}

const interactionTypeComponent = 3

type interactionPayload struct {
	Type          int                      `json:"type"`
	Nonce         string                   `json:"nonce"`
	GuildID       string                   `json:"guild_id,omitempty"`
	ChannelID     string                   `json:"channel_id"`
	MessageFlags  uint32                   `json:"message_flags,omitempty"`
	MessageID     string                   `json:"message_id"`
	ApplicationID string                   `json:"application_id"`
	SessionID     string                   `json:"session_id"`
	Data          componentInteractionData `json:"data"`
}

type componentInteractionData struct {
	ComponentType int      `json:"component_type"`
	CustomID      string   `json:"custom_id"`
	Values        []string `json:"values,omitempty"`
}

func postInteraction(sess *discordgo.Session, body interactionPayload) error {
	raw, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("failed to encode interaction: %w", err)
	}
	_, err = sess.RequestRaw("POST", discordgo.EndpointInteractions, "application/json", raw, discordgo.EndpointInteractions, 0)
	if err != nil {
		return fmt.Errorf("failed to send interaction to Discord: %w", err)
	}
	return nil
}

func guildIDForChannel(sess *discordgo.Session, channelID string) string {
	ch, err := sess.State.Channel(channelID)
	if err != nil {
		return ""
	}
	return ch.GuildID
}

func (c *DiscordClient) gatewaySessionIDValue() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.gatewaySessionID
}

// interactionNonce returns a unique nonce for an interaction request.
func interactionNonce() string {
	return strconv.FormatInt(time.Now().UnixNano(), 10)
}
