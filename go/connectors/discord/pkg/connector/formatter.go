// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// Portions adapted from mautrix-discord (formatter.go, formatter_tag.go, formatter_everyone.go),
// Copyright (C) 2023 Tulir Asokan, licensed under the GNU Affero General Public License v3 or later.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"context"
	"fmt"
	"html"
	"regexp"
	"strings"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"go.mau.fi/util/variationselector"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/format"
	"maunium.net/go/mautrix/format/mdext"
	"maunium.net/go/mautrix/id"
)

// escapeFixer handles Discord's one-backslash escaping (\__a__) which standard markdown requires doubled.
var escapeFixer = regexp.MustCompile(`\\(__[^_]|\*\*[^*])`)

func escapeReplacement(s string) string {
	return s[:2] + `\` + s[2:]
}

// discordFeaturesDisabled lists block features Discord does not render. Links are kept.
var discordFeaturesDisabled = []any{
	parser.NewListParser(), parser.NewListItemParser(), parser.NewHTMLBlockParser(), parser.NewRawHTMLParser(),
	parser.NewSetextHeadingParser(), parser.NewThematicBreakParser(), parser.NewCodeBlockParser(),
}

var discordMarkdownRenderer = goldmark.New(
	goldmark.WithParser(mdext.ParserWithoutFeatures(discordFeaturesDisabled...)),
	mdext.FixIndentedParagraphs,
	format.HTMLOptions,
	goldmark.WithExtensions(extension.Strikethrough, mdext.SimpleSpoiler, mdext.DiscordUnderline),
)

var (
	userMentionPattern    = regexp.MustCompile(`&lt;@!?(\d+)&gt;`)
	roleMentionPattern    = regexp.MustCompile(`&lt;@&amp;(\d+)&gt;`)
	channelMentionPattern = regexp.MustCompile(`&lt;#(\d+)&gt;`)
	customEmojiPattern    = regexp.MustCompile(`&lt;a?:(\w+):\d+&gt;`)
)

// mentionNames maps Discord user IDs to the names used when rendering their mentions.
type mentionNames map[string]string

// discordToHTML renders Discord markdown as HTML and returns the Discord user IDs that were mentioned.
// ghosts names the Matrix users used for matrix.to links to mentioned users.
func discordToHTML(text string, names mentionNames, ghosts ghostMapper) (string, []string) {
	text = escapeFixer.ReplaceAllStringFunc(text, escapeReplacement)
	var buf strings.Builder
	if err := discordMarkdownRenderer.Convert([]byte(text), &buf); err != nil {
		// goldmark only fails on writer errors, which cannot happen for a strings.Builder.
		return html.EscapeString(text), nil
	}
	rendered := format.UnwrapSingleParagraph(buf.String())
	var mentioned []string
	rendered = userMentionPattern.ReplaceAllStringFunc(rendered, func(match string) string {
		userID := userMentionPattern.FindStringSubmatch(match)[1]
		mentioned = appendUnique(mentioned, userID)
		name, ok := names[userID]
		if !ok {
			name = userID
		}
		ghost := ghosts.FormatGhostMXID(networkid.UserID(userID))
		return fmt.Sprintf(`<a href="https://matrix.to/#/%s">@%s</a>`, ghost, html.EscapeString(name))
	})
	rendered = roleMentionPattern.ReplaceAllString(rendered, "@role")
	rendered = channelMentionPattern.ReplaceAllString(rendered, "#channel")
	rendered = customEmojiPattern.ReplaceAllString(rendered, ":$1:")
	return variationselector.FullyQualify(rendered), mentioned
}

func appendUnique(list []string, item string) []string {
	for _, existing := range list {
		if existing == item {
			return list
		}
	}
	return append(list, item)
}

// ghostMapper is the part of bridgev2.MatrixConnector that names ghost users. Ghost MXIDs follow the
// bridge's own template, so the formatter never builds or parses them by hand.
type ghostMapper interface {
	FormatGhostMXID(userID networkid.UserID) id.UserID
	ParseGhostMXID(userID id.UserID) (networkid.UserID, bool)
}

var discordMarkdownEscaper = strings.NewReplacer(
	`\`, `\\`,
	`_`, `\_`,
	`*`, `\*`,
	`~`, `\~`,
	"`", "\\`",
	`|`, `\|`,
	`<`, `\<`,
	`#`, `\#`,
)

var discordLinkPattern = regexp.MustCompile(`https?://[^<\p{Zs}\x{feff}]*[^"'),.:;\]\p{Zs}\x{feff}]`)

// escapeDiscordMarkdown escapes markdown in plain text, but leaves URLs intact so they keep their preview.
func escapeDiscordMarkdown(s string) string {
	matches := discordLinkPattern.FindAllStringIndex(s, -1)
	if matches == nil {
		return discordMarkdownEscaper.Replace(s)
	}
	var b strings.Builder
	offset := 0
	for _, m := range matches {
		b.WriteString(discordMarkdownEscaper.Replace(s[offset:m[0]]))
		b.WriteString(s[m[0]:m[1]])
		offset = m[1]
	}
	b.WriteString(discordMarkdownEscaper.Replace(s[offset:]))
	return b.String()
}

// newMatrixToDiscordParser returns a parser that converts Matrix HTML into Discord markdown.
// Pills for ghost users become Discord mentions; ghosts are recognised through ghosts.
func newMatrixToDiscordParser(ghosts ghostMapper) *format.HTMLParser {
	return &format.HTMLParser{
		TabsToSpaces:   4,
		Newline:        "\n",
		HorizontalLine: "\n---\n",
		BoldConverter: func(s string, ctx format.Context) string {
			return fmt.Sprintf("**%s**", s)
		},
		ItalicConverter: func(s string, ctx format.Context) string {
			return fmt.Sprintf("*%s*", s)
		},
		StrikethroughConverter: func(s string, ctx format.Context) string {
			return fmt.Sprintf("~~%s~~", s)
		},
		UnderlineConverter: func(s string, ctx format.Context) string {
			return fmt.Sprintf("__%s__", s)
		},
		MonospaceConverter: func(s string, ctx format.Context) string {
			return fmt.Sprintf("`%s`", s)
		},
		MonospaceBlockConverter: format.DefaultMonospaceBlockConverter,
		SpoilerConverter: func(text, reason string, ctx format.Context) string {
			if reason != "" {
				return fmt.Sprintf("(%s) ||%s||", reason, text)
			}
			return fmt.Sprintf("||%s||", text)
		},
		LinkConverter: func(text, href string, ctx format.Context) string {
			if text == href {
				return href
			}
			return fmt.Sprintf("[%s](%s)", escapeDiscordMarkdown(text), href)
		},
		TextConverter: func(s string, ctx format.Context) string {
			if ctx.TagStack.Has("pre") || ctx.TagStack.Has("code") {
				return s
			}
			return escapeDiscordMarkdown(s)
		},
		PillConverter: func(displayname, mxid, eventID string, ctx format.Context) string {
			if userID, ok := ghosts.ParseGhostMXID(id.UserID(mxid)); ok {
				mentions := ctx.ReturnData[mentionedUsersKey].(*[]string)
				*mentions = appendUnique(*mentions, string(userID))
				return fmt.Sprintf("<@%s>", userID)
			}
			return displayname
		},
	}
}

const mentionedUsersKey = "org.chirper.discord.mentioned_users"

// matrixHTMLToDiscord converts an event body to Discord markdown. It also returns the Discord user IDs
// of ghosts mentioned in the text, which are the only users Discord should ping.
func matrixHTMLToDiscord(ghosts ghostMapper, content *event.MessageEventContent) (string, []string) {
	if content.Format != event.FormatHTML || content.FormattedBody == "" {
		return variationselector.FullyQualify(escapeDiscordMarkdown(content.Body)), nil
	}
	ctx := format.NewContext(context.Background())
	mentions := []string{}
	ctx.ReturnData[mentionedUsersKey] = &mentions
	text := newMatrixToDiscordParser(ghosts).Parse(content.FormattedBody, ctx)
	return variationselector.FullyQualify(text), mentions
}
