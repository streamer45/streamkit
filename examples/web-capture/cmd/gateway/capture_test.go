// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"context"
	"testing"
	"time"
)

func TestDetectMode(t *testing.T) {
	cases := []struct {
		rawTarget string
		wantMode  captureMode
		wantRest  string
		wantOK    bool
	}{
		{"/clip/example.com", modeClip, "example.com", true},
		{"/cast/dur=5s/example.com", modeCast, "dur=5s/example.com", true},
		{"/clip/dur=30s/example.com", modeClip, "dur=30s/example.com", true},
		{"/cast/example.com", modeCast, "example.com", true},
		{"/example.com", modeClip, "", false}, // no mode segment
		{"/", modeClip, "", false},
	}
	for _, c := range cases {
		mode, rest, ok := detectMode(c.rawTarget)
		if ok != c.wantOK {
			t.Errorf("detectMode(%q) ok=%v want %v", c.rawTarget, ok, c.wantOK)
			continue
		}
		if !ok {
			continue
		}
		if mode != c.wantMode || rest != c.wantRest {
			t.Errorf("detectMode(%q) = (%v,%q) want (%v,%q)", c.rawTarget, mode, rest, c.wantMode, c.wantRest)
		}
	}
}

func TestParseTargetAndOptions(t *testing.T) {
	def := captureOpts{dur: 10 * time.Second, resW: 1920, resH: 1080}
	const maxDur = 60 * time.Second
	cases := []struct {
		name, rest         string
		wantTarget         string
		wantDur            time.Duration
		wantResW, wantResH int
		wantErr            bool
	}{
		{"bare host", "example.com/page", "https://example.com/page", 10 * time.Second, 1920, 1080, false},
		{"scheme + query preserved", "https://e.com/p?a=1&b=2", "https://e.com/p?a=1&b=2", 10 * time.Second, 1920, 1080, false},
		{"explicit http kept", "http://e.com", "http://e.com", 10 * time.Second, 1920, 1080, false},
		{"dur option", "dur=30s/example.com", "https://example.com", 30 * time.Second, 1920, 1080, false},
		{"dur seconds int", "dur=15/example.com", "https://example.com", 15 * time.Second, 1920, 1080, false},
		{"dur clamped to max", "dur=90s/example.com", "https://example.com", maxDur, 1920, 1080, false},
		{"resolution option", "res=2560x1440/example.com", "https://example.com", 10 * time.Second, 2560, 1440, false},
		{"dur and resolution", "dur=20s,res=1280x720/example.com", "https://example.com", 20 * time.Second, 1280, 720, false},
		{"host that looks like a token", "30s.com/x", "https://30s.com/x", 10 * time.Second, 1920, 1080, false},
		{"empty", "", "", 0, 0, 0, true},
		{"options with no target", "dur=30s", "", 0, 0, 0, true},
		{"bad dur", "dur=abc/example.com", "", 0, 0, 0, true},
		{"bad resolution", "res=wide/example.com", "", 0, 0, 0, true},
		{"unknown option", "zoom=2/example.com", "", 0, 0, 0, true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			target, opts, err := parseTargetAndOptions(c.rest, def, maxDur)
			if (err != nil) != c.wantErr {
				t.Fatalf("err=%v wantErr=%v", err, c.wantErr)
			}
			if c.wantErr {
				return
			}
			if target != c.wantTarget {
				t.Errorf("target=%q want %q", target, c.wantTarget)
			}
			if opts.dur != c.wantDur {
				t.Errorf("dur=%v want %v", opts.dur, c.wantDur)
			}
			if opts.resW != c.wantResW || opts.resH != c.wantResH {
				t.Errorf("res=%dx%d want %dx%d", opts.resW, opts.resH, c.wantResW, c.wantResH)
			}
		})
	}
}

// The exact shape a local path-prefix request produces, end to end through
// detectMode + parseTargetAndOptions.
func TestDetectModeThenParseWithOptions(t *testing.T) {
	mode, rest, ok := detectMode("/clip/dur=5s,res=1920x1080/streamkit.dev")
	if !ok || mode != modeClip {
		t.Fatalf("detectMode = (%v, %q, %v)", mode, rest, ok)
	}
	def := captureOpts{dur: 10 * time.Second, resW: 1280, resH: 720}
	target, opts, err := parseTargetAndOptions(rest, def, 60*time.Second)
	if err != nil {
		t.Fatalf("parse error: %v", err)
	}
	if target != "https://streamkit.dev" {
		t.Errorf("target=%q want https://streamkit.dev", target)
	}
	if opts.dur != 5*time.Second || opts.resW != 1920 || opts.resH != 1080 {
		t.Errorf("opts=%+v want dur=5s res=1920x1080", opts)
	}
}

func TestClipFrames(t *testing.T) {
	cases := map[time.Duration]int{
		10 * time.Second:        300,
		1500 * time.Millisecond: 45, // would truncate to 30 (1s) without rounding
		500 * time.Millisecond:  15,
		5 * time.Millisecond:    1, // rounds to 0 -> clamped to 1
	}
	for dur, want := range cases {
		if got := clipFrames(dur); got != want {
			t.Errorf("clipFrames(%v)=%d want %d", dur, got, want)
		}
	}
}

func TestValidateTarget(t *testing.T) {
	// IP literals resolve without DNS, so these run offline.
	cases := []struct {
		raw         string
		wantBlocked bool
	}{
		{"http://127.0.0.1", true},
		{"http://169.254.169.254", true}, // cloud metadata
		{"http://10.1.2.3", true},
		{"http://192.168.0.1", true},
		{"http://172.16.0.1", true},
		{"http://100.64.0.1", true}, // CGNAT
		{"http://0.0.0.0", true},
		{"http://0.0.0.1", true},              // 0.0.0.0/8 "this host"
		{"http://198.18.0.1", true},           // 198.18.0.0/15 benchmarking
		{"http://[64:ff9b::7f00:1]", true},    // NAT64 of 127.0.0.1
		{"http://[2002:7f00:1::]", true},      // 6to4 embedding 127.0.0.1
		{"https://[64:ff9b::808:808]", false}, // NAT64 of public 8.8.8.8 — allowed
		{"http://[::1]", true},
		{"ftp://8.8.8.8", true}, // scheme
		{"https://8.8.8.8", false},
		{"https://1.1.1.1", false},
	}
	for _, c := range cases {
		_, err := validateTarget(context.Background(), c.raw)
		if (err != nil) != c.wantBlocked {
			t.Errorf("validateTarget(%q) err=%v wantBlocked=%v", c.raw, err, c.wantBlocked)
		}
	}
}
