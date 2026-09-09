package main

import "testing"

func TestDesktopOnlyEnabled(t *testing.T) {
	for _, value := range []string{"true", "TRUE", "1", "yes"} {
		t.Setenv("DESKTOP_ONLY", value)
		if !desktopOnlyEnabled() {
			t.Fatalf("expected %q to enable desktop-only mode", value)
		}
	}
	for _, value := range []string{"", "false", "0", "no"} {
		t.Setenv("DESKTOP_ONLY", value)
		if desktopOnlyEnabled() {
			t.Fatalf("expected %q to disable desktop-only mode", value)
		}
	}
}
