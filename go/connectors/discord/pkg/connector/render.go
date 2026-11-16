// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"encoding/json"
	"fmt"
	"html"
	"strings"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/event"
)

// componentsExtraKey carries the raw Discord component JSON so clients can render interactive buttons.
const componentsExtraKey = "org.chirper.discord.components"

// renderComponents returns an HTML and plain-text list of the message's buttons and select menus,
// plus the raw component JSON. It returns empty strings when there are no components.
func renderComponents(components []discordgo.MessageComponent) (htmlList, plain string, raw json.RawMessage, err error) {
	if len(components) == 0 {
		return "", "", nil, nil
	}
	raw, err = json.Marshal(components)
	if err != nil {
		return "", "", nil, fmt.Errorf("failed to encode components: %w", err)
	}
	var items []string
	var plainItems []string
	for _, row := range components {
		for _, component := range actionRowChildren(row) {
			label, ok := describeComponent(component)
			if !ok {
				continue
			}
			items = append(items, "<li>"+label.html+"</li>")
			plainItems = append(plainItems, "- "+label.plain)
		}
	}
	if len(items) == 0 {
		return "", "", raw, nil
	}
	return "<ul>" + strings.Join(items, "") + "</ul>", strings.Join(plainItems, "\n"), raw, nil
}

// actionRowChildren returns the components of an action row, or nil for other component types.
// discordgo decodes components as pointers, so both forms are accepted.
func actionRowChildren(component discordgo.MessageComponent) []discordgo.MessageComponent {
	switch row := component.(type) {
	case *discordgo.ActionsRow:
		return row.Components
	case discordgo.ActionsRow:
		return row.Components
	}
	return nil
}

type componentLabel struct {
	html  string
	plain string
}

func describeComponent(component discordgo.MessageComponent) (componentLabel, bool) {
	switch c := component.(type) {
	case *discordgo.Button:
		return describeButton(*c), true
	case discordgo.Button:
		return describeButton(c), true
	case *discordgo.SelectMenu:
		return describeSelectMenu(*c), true
	case discordgo.SelectMenu:
		return describeSelectMenu(c), true
	}
	return componentLabel{}, false
}

func describeButton(button discordgo.Button) componentLabel {
	text := button.Label
	if button.URL != "" {
		return componentLabel{
			html:  fmt.Sprintf(`<a href="%s">%s</a>`, html.EscapeString(button.URL), html.EscapeString(text)),
			plain: text + " (" + button.URL + ")",
		}
	}
	return componentLabel{
		html:  fmt.Sprintf("<strong>[%s]</strong>", html.EscapeString(text)),
		plain: "[" + text + "]",
	}
}

func describeSelectMenu(menu discordgo.SelectMenu) componentLabel {
	options := make([]string, 0, len(menu.Options))
	for _, opt := range menu.Options {
		options = append(options, opt.Label)
	}
	placeholder := menu.Placeholder
	if placeholder == "" {
		placeholder = "Select an option"
	}
	return componentLabel{
		html:  fmt.Sprintf("<strong>[%s]</strong> %s", html.EscapeString(placeholder), html.EscapeString(strings.Join(options, ", "))),
		plain: "[" + placeholder + "] " + strings.Join(options, ", "),
	}
}

// renderEmbed converts a Discord embed into a notice. Images are linked, not fetched, so that
// embedding a remote image does not make the viewer's client contact a third party.
func renderEmbed(embed *discordgo.MessageEmbed, names mentionNames, ghosts ghostMapper) *event.MessageEventContent {
	var plain, htmlBody strings.Builder
	addLine := func(plainLine, htmlLine string) {
		if plain.Len() > 0 {
			plain.WriteString("\n")
			htmlBody.WriteString("<br>")
		}
		plain.WriteString(plainLine)
		htmlBody.WriteString(htmlLine)
	}
	if embed.Title != "" {
		title := html.EscapeString(embed.Title)
		if embed.URL != "" {
			title = fmt.Sprintf(`<a href="%s">%s</a>`, html.EscapeString(embed.URL), title)
		}
		addLine(embed.Title, "<strong>"+title+"</strong>")
	}
	if embed.Description != "" {
		descHTML, _ := discordToHTML(embed.Description, names, ghosts)
		addLine(embed.Description, descHTML)
	}
	for _, field := range embed.Fields {
		if field == nil {
			continue
		}
		addLine(field.Name+": "+field.Value, fmt.Sprintf("<strong>%s</strong>: %s", html.EscapeString(field.Name), html.EscapeString(field.Value)))
	}
	if embed.Image != nil && embed.Image.URL != "" {
		addLine(embed.Image.URL, fmt.Sprintf(`image: <a href="%s">%s</a>`, html.EscapeString(embed.Image.URL), html.EscapeString(embed.Image.URL)))
	}
	if embed.Footer != nil && embed.Footer.Text != "" {
		addLine(embed.Footer.Text, "<sub>"+html.EscapeString(embed.Footer.Text)+"</sub>")
	}
	if plain.Len() == 0 {
		return nil
	}
	return &event.MessageEventContent{
		MsgType:       event.MsgNotice,
		Body:          plain.String(),
		Format:        event.FormatHTML,
		FormattedBody: htmlBody.String(),
	}
}
