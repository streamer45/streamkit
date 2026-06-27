// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"net/http"
	"testing"
)

func TestMethodLabel(t *testing.T) {
	cases := map[string]string{
		http.MethodGet:  "GET",
		http.MethodPost: "POST",
		http.MethodPut:  "PUT",
		http.MethodHead: "HEAD",
		"PATCH":         "other",
		"FROBNICATE":    "other",
		"":              "other",
	}
	for in, want := range cases {
		if got := methodLabel(in); got != want {
			t.Errorf("methodLabel(%q)=%q want %q", in, got, want)
		}
	}
}

func TestCodeLabel(t *testing.T) {
	cases := map[int]string{
		200: "200",
		404: "404",
		499: "499", // client closed
		502: "502",
		550: "5xx", // non-canonical, folds to class
		99:  "other",
		700: "other",
	}
	for in, want := range cases {
		if got := codeLabel(in); got != want {
			t.Errorf("codeLabel(%d)=%q want %q", in, got, want)
		}
	}
}
