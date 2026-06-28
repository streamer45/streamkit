// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import "testing"

func TestClampConcurrency(t *testing.T) {
	cases := []struct {
		in   int
		want int
	}{
		{-1, 1},
		{0, 1},
		{1, 1},
		{4, 4},
	}
	for _, tc := range cases {
		if got := clampConcurrency(tc.in); got != tc.want {
			t.Errorf("clampConcurrency(%d) = %d, want %d", tc.in, got, tc.want)
		}
	}
}
