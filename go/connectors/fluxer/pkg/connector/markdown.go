// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"html"
	"regexp"
	"strings"

	"maunium.net/go/mautrix/event"
)

// Fluxer messages are Markdown (docs: packages/markdown_parser). Matrix clients send HTML in
// formatted_body; outgoing text is converted to the Markdown subset that Fluxer renders.
var (
	htmlBreak      = regexp.MustCompile(`(?i)<br\s*/?>`)
	htmlBlockEnd   = regexp.MustCompile(`(?i)</(p|div|blockquote|h[1-6])>`)
	htmlListItem   = regexp.MustCompile(`(?i)<li[^>]*>`)
	htmlPreOpen    = regexp.MustCompile(`(?i)<pre[^>]*>`)
	htmlPreClose   = regexp.MustCompile(`(?i)</pre>`)
	htmlAnchor     = regexp.MustCompile(`(?is)<a\s[^>]*?href="([^"]*)"[^>]*>(.*?)</a>`)
	htmlTag        = regexp.MustCompile(`(?s)<[^>]+>`)
	htmlStrong     = regexp.MustCompile(`(?i)</?(strong|b)>`)
	htmlEmphasis   = regexp.MustCompile(`(?i)</?(em|i)>`)
	htmlStrike     = regexp.MustCompile(`(?i)</?(del|s|strike)>`)
	htmlInlineCode = regexp.MustCompile(`(?is)<code[^>]*>(.*?)</code>`)
)

// bodyText returns the text to send for an outgoing Matrix message.
func bodyText(content *event.MessageEventContent) string {
	if content.Format == event.FormatHTML && content.FormattedBody != "" {
		return htmlToMarkdown(content.FormattedBody)
	}
	return content.Body
}

// htmlToMarkdown converts the HTML subset that Matrix clients send into Fluxer Markdown.
// Unknown tags are removed and their text kept.
func htmlToMarkdown(src string) string {
	s := htmlPreOpen.ReplaceAllString(src, "\n```\n")
	s = htmlPreClose.ReplaceAllString(s, "\n```\n")
	s = htmlInlineCode.ReplaceAllString(s, "`$1`")
	s = htmlAnchor.ReplaceAllStringFunc(s, anchorToMarkdown)
	s = htmlStrong.ReplaceAllString(s, "**")
	s = htmlEmphasis.ReplaceAllString(s, "*")
	s = htmlStrike.ReplaceAllString(s, "~~")
	s = htmlBreak.ReplaceAllString(s, "\n")
	s = htmlListItem.ReplaceAllString(s, "\n- ")
	s = htmlBlockEnd.ReplaceAllString(s, "\n")
	s = htmlTag.ReplaceAllString(s, "")
	return strings.TrimSpace(html.UnescapeString(s))
}

// anchorToMarkdown renders <a href="u">text</a> as [text](u), or as the bare URL when the text is the URL.
func anchorToMarkdown(anchor string) string {
	m := htmlAnchor.FindStringSubmatch(anchor)
	if m == nil {
		return anchor
	}
	href, text := html.UnescapeString(m[1]), htmlTag.ReplaceAllString(m[2], "")
	if strings.TrimSpace(text) == "" || text == href {
		return href
	}
	return "[" + text + "](" + href + ")"
}
