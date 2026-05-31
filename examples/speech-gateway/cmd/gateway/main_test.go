// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import "testing"

func TestUnwrapPacketJSON(t *testing.T) {
	cases := []struct {
		name string
		in   string
		want string
	}{
		{
			name: "strips transcription wrapper",
			in:   `{"Transcription":{"text":"hi","segments":[],"language":"en","metadata":null}}`,
			want: `{"text":"hi","segments":[],"language":"en","metadata":null}`,
		},
		{
			name: "preserves segments and trailing newline",
			in:   `{"Transcription":{"text":"a","segments":[{"text":"a","start_time_ms":0,"end_time_ms":100,"confidence":null}],"language":"en","metadata":null}}` + "\n",
			want: `{"text":"a","segments":[{"text":"a","start_time_ms":0,"end_time_ms":100,"confidence":null}],"language":"en","metadata":null}` + "\n",
		},
		{
			name: "unwraps each ndjson line",
			in:   `{"Transcription":{"text":"a","segments":[],"language":"en","metadata":null}}` + "\n" + `{"Transcription":{"text":"b","segments":[],"language":"en","metadata":null}}`,
			want: `{"text":"a","segments":[],"language":"en","metadata":null}` + "\n" + `{"text":"b","segments":[],"language":"en","metadata":null}`,
		},
		{
			name: "leaves already-flat object unchanged",
			in:   `{"text":"hi","segments":[]}`,
			want: `{"text":"hi","segments":[]}`,
		},
		{
			name: "leaves non-transcription variant unchanged",
			in:   `{"Text":"hello"}`,
			want: `{"Text":"hello"}`,
		},
		{
			name: "leaves invalid json unchanged",
			in:   `not json`,
			want: `not json`,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := string(unwrapPacketJSON([]byte(tc.in))); got != tc.want {
				t.Errorf("unwrapPacketJSON(%q) = %q, want %q", tc.in, got, tc.want)
			}
		})
	}
}

func TestIsJSONContentType(t *testing.T) {
	cases := map[string]bool{
		"application/json":                true,
		"application/json; charset=utf-8": true,
		"APPLICATION/JSON":                true,
		"audio/ogg":                       false,
		"text/plain":                      false,
		"":                                false,
	}
	for ct, want := range cases {
		if got := isJSONContentType(ct); got != want {
			t.Errorf("isJSONContentType(%q) = %v, want %v", ct, got, want)
		}
	}
}
