// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	_ "embed"
	"os"

	"gopkg.in/yaml.v3"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// devInsecureEnv enables plain-HTTP and local-host instances. It is a development switch only.
const devInsecureEnv = "CHIRPER_FLUXER_DEV_INSECURE"

// instancePolicy returns the URL policy for instances; insecure hosts need devInsecureEnv=1.
func instancePolicy() fluxerapi.Policy {
	return fluxerapi.Policy{AllowInsecure: os.Getenv(devInsecureEnv) == "1"}
}

// Config is the fluxer section of the bridge config.
type Config struct {
	// DefaultInstance is the origin offered on the login screen, for example https://fluxer.app.
	DefaultInstance string `yaml:"default_instance"`
}

//go:embed example-config.yaml
var ExampleConfig string

type umConfig Config

// UnmarshalYAML decodes the section; unknown keys are ignored.
func (c *Config) UnmarshalYAML(node *yaml.Node) error {
	return node.Decode((*umConfig)(c))
}
