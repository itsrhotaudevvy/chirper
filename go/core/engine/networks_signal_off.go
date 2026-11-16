//go:build !withsignal

package engine

// signalSpec is the Signal network. This build does not link libsignal, so Signal is unavailable.
func signalSpec() networkSpec {
	return networkSpec{
		id:          "signal",
		color:       "#3A76F0",
		name:        "Signal",
		unavailable: "not supported yet: this build does not include Signal (it needs libsignal, built with the Android target)",
	}
}
