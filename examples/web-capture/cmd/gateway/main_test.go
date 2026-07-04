// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import "testing"

func TestClampMin(t *testing.T) {
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
		if got := clampMin("--max-concurrency", tc.in); got != tc.want {
			t.Errorf("clampMin(%d) = %d, want %d", tc.in, got, tc.want)
		}
	}
}
