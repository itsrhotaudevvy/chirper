// Command ochat is a desktop REPL for the Chirper core. It drives the same facade as the app,
// so it is the quickest way to test a network with a real account. Commands are read from stdin,
// one per line; run "help" for the list.
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/skip2/go-qrcode"

	"chirper.dev/core/facade"
)

const helpText = `commands:
  networks                          list networks
  accounts                          list logged-in accounts
  login <network> <flow>            log in (prompts for fields, shows QR codes)
  logout <network> <loginId>        log an account out
  chats [query]                     list chats (index, name, roomId)
  msgs <roomId> [count]             show the latest messages
  send <roomId> <text...>           send a text message
  edit <roomId> <eventId> <text...> edit a message
  react <roomId> <eventId> <emoji>  add a reaction
  unreact <roomId> <eventId> <emoji> remove a reaction
  redact <roomId> <eventId>         remove a message
  older <roomId>                    load older history
  read <roomId> <eventId>           mark read up to a message
  settings                          show settings
  quit`

// printer is the EventListener. It reports only event kinds and account states, never bodies.
type printer struct{ out io.Writer }

func (p printer) OnEvent(kind, payload string) {
	switch kind {
	case "account":
		var a struct {
			Network string `json:"network"`
			LoginID string `json:"loginId"`
			State   string `json:"state"`
		}
		if json.Unmarshal([]byte(payload), &a) == nil {
			fmt.Fprintf(p.out, "[account] %s/%s %s\n", a.Network, a.LoginID, a.State)
		}
	case "core_error":
		fmt.Fprintf(p.out, "[error] %s\n", payload)
	}
}

func main() {
	dataDir := flag.String("data", "ochat-data", "data directory")
	flag.Parse()
	core, err := facade.NewCore(*dataDir, printer{out: os.Stderr})
	if err != nil {
		fatal(err)
	}
	if err := core.Start(); err != nil {
		fatal(err)
	}
	defer core.Stop()
	in := bufio.NewScanner(os.Stdin)
	for in.Scan() {
		fields := strings.Fields(in.Text())
		if len(fields) == 0 {
			continue
		}
		if fields[0] == "quit" {
			return
		}
		if err := run(core, in, fields); err != nil {
			fmt.Println("error:", err)
		}
	}
}

func fatal(err error) {
	fmt.Fprintln(os.Stderr, "error:", err)
	os.Exit(1)
}

// run executes one command. in is passed for commands that prompt.
func run(core *facade.Core, in *bufio.Scanner, f []string) error {
	switch f[0] {
	case "help":
		fmt.Println(helpText)
	case "networks":
		fmt.Println(core.Networks())
	case "accounts":
		fmt.Println(core.Accounts())
	case "login":
		if len(f) != 3 {
			return fmt.Errorf("usage: login <network> <flow>")
		}
		return login(core, in, f[1], f[2])
	case "logout":
		if len(f) != 3 {
			return fmt.Errorf("usage: logout <network> <loginId>")
		}
		return core.Logout(f[1], f[2])
	case "chats":
		return listChats(core, strings.Join(f[1:], " "))
	case "msgs":
		if len(f) < 2 {
			return fmt.Errorf("usage: msgs <roomId> [count]")
		}
		fmt.Println(core.ListMessages(f[1], 0, count(f, 2)))
	case "send":
		if len(f) < 3 {
			return fmt.Errorf("usage: send <roomId> <text>")
		}
		eventID, err := core.SendText(f[1], strings.Join(f[2:], " "), "", fmt.Sprint(time.Now().UnixNano()))
		fmt.Println("sent", eventID)
		return err
	case "edit":
		if len(f) < 4 {
			return fmt.Errorf("usage: edit <roomId> <eventId> <text>")
		}
		return core.Edit(f[1], f[2], strings.Join(f[3:], " "))
	case "react":
		if len(f) != 4 {
			return fmt.Errorf("usage: react <roomId> <eventId> <emoji>")
		}
		return core.React(f[1], f[2], f[3])
	case "unreact":
		if len(f) != 4 {
			return fmt.Errorf("usage: unreact <roomId> <eventId> <emoji>")
		}
		return core.Unreact(f[1], f[2], f[3])
	case "redact":
		if len(f) != 3 {
			return fmt.Errorf("usage: redact <roomId> <eventId>")
		}
		return core.Redact(f[1], f[2])
	case "older":
		if len(f) != 2 {
			return fmt.Errorf("usage: older <roomId>")
		}
		_, err := core.LoadOlder(f[1])
		return err
	case "read":
		if len(f) != 3 {
			return fmt.Errorf("usage: read <roomId> <eventId>")
		}
		return core.MarkRead(f[1], f[2])
	case "settings":
		fmt.Println(core.Settings())
	default:
		return fmt.Errorf("unknown command %q (try help)", f[0])
	}
	return nil
}

// count parses f[i] as a positive count, defaulting to 20.
func count(f []string, i int) int {
	if i >= len(f) {
		return 20
	}
	var n int
	if _, err := fmt.Sscan(f[i], &n); err != nil || n <= 0 {
		return 20
	}
	return n
}

// listChats prints the chats matching query, one per line.
func listChats(core *facade.Core, query string) error {
	var chats []struct {
		RoomID  string `json:"roomId"`
		Network string `json:"network"`
		Name    string `json:"name"`
		IsDM    bool   `json:"isDm"`
	}
	filter, err := json.Marshal(map[string]string{"query": query})
	if err != nil {
		return err
	}
	if err := json.Unmarshal([]byte(core.ListChats(string(filter))), &chats); err != nil {
		return err
	}
	for i, c := range chats {
		fmt.Printf("%d\t%s\t%s\t%s\n", i, c.Network, c.Name, c.RoomID)
	}
	return nil
}

// login runs a login flow to completion: it prompts for input, prints QR codes and waits for scans.
func login(core *facade.Core, in *bufio.Scanner, network, flow string) error {
	raw, err := core.StartLogin(network, flow)
	if err != nil {
		return err
	}
	for {
		var state struct {
			ProcessID string `json:"processId"`
			Step      struct {
				Type         string `json:"type"`
				Instructions string `json:"instructions"`
				UserInput    *struct {
					Fields []struct {
						ID   string `json:"id"`
						Name string `json:"name"`
					} `json:"fields"`
				} `json:"userInput"`
				DisplayAndWait *struct {
					Type string `json:"type"`
					Data string `json:"data"`
				} `json:"displayAndWait"`
				Complete *struct {
					LoginID string `json:"loginId"`
				} `json:"complete"`
			} `json:"step"`
		}
		if err := json.Unmarshal([]byte(raw), &state); err != nil {
			return err
		}
		step := state.Step
		if step.Instructions != "" {
			fmt.Println(step.Instructions)
		}
		switch step.Type {
		case "complete":
			fmt.Println("logged in:", step.Complete.LoginID)
			return nil
		case "user_input":
			values := map[string]string{}
			for _, field := range step.UserInput.Fields {
				fmt.Printf("%s: ", field.Name)
				if !in.Scan() {
					core.CancelLogin(state.ProcessID)
					return fmt.Errorf("input ended")
				}
				values[field.ID] = strings.TrimSpace(in.Text())
			}
			encoded, _ := json.Marshal(values)
			raw, err = core.SubmitLoginInput(state.ProcessID, string(encoded))
		case "display_and_wait":
			showCode(step.DisplayAndWait.Type, step.DisplayAndWait.Data)
			raw, err = core.WaitLoginStep(state.ProcessID)
		case "cookies":
			return fmt.Errorf("cookie logins need a browser; use the app")
		default:
			return fmt.Errorf("unsupported login step %q", step.Type)
		}
		if err != nil {
			return err
		}
	}
}

// showCode prints a QR code (or the text of a code) to the terminal.
func showCode(kind, data string) {
	if kind != "qr" {
		fmt.Printf("code: %s\n", data)
		return
	}
	qr, err := qrcode.New(data, qrcode.Medium)
	if err != nil {
		fmt.Println("QR data:", data)
		return
	}
	fmt.Println(qr.ToSmallString(false))
}
