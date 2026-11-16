// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import "testing"

func TestHTMLToMarkdown(t *testing.T) {
	cases := map[string]string{
		"plain text":                                            "plain text",
		"<strong>bold</strong> and <em>it</em>":                 "**bold** and *it*",
		"<b>b</b> <i>i</i> <del>gone</del>":                     "**b** *i* ~~gone~~",
		"line one<br>line two":                                  "line one\nline two",
		"<p>para one</p><p>para two</p>":                        "para one\npara two",
		"use <code>fmt.Println</code> here":                     "use `fmt.Println` here",
		`<a href="https://example.org/x">docs</a>`:              "[docs](https://example.org/x)",
		`<a href="https://example.org">https://example.org</a>`: "https://example.org",
		"<span>&lt;tag&gt; &amp; more</span>":                   "<tag> & more",
		"<ul><li>one</li><li>two</li></ul>":                     "- one\n- two",
	}
	for in, want := range cases {
		if got := htmlToMarkdown(in); got != want {
			t.Errorf("htmlToMarkdown(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestHTMLToMarkdownPreBlock(t *testing.T) {
	got := htmlToMarkdown("<pre><code>x := 1</code></pre>")
	want := "```\n`x := 1`\n```"
	if got != want {
		t.Fatalf("got %q want %q", got, want)
	}
}
