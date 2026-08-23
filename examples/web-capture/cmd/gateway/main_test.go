// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"testing"
	"time"
)

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

func TestMSEReadyTimeoutFor(t *testing.T) {
	cases := []struct {
		loadTimeoutSecs int
		want            time.Duration
	}{
		{1, minMSEReadyTimeout},
		{defaultLoadTimeoutSecs, minMSEReadyTimeout},
		{10, 13 * time.Second},
		{30, 33 * time.Second},
	}
	for _, tc := range cases {
		got := mseReadyTimeoutFor(tc.loadTimeoutSecs)
		if got != tc.want {
			t.Errorf("mseReadyTimeoutFor(%d) = %v, want %v", tc.loadTimeoutSecs, got, tc.want)
		}
		if got < time.Duration(tc.loadTimeoutSecs)*time.Second {
			t.Errorf("mseReadyTimeoutFor(%d) = %v is below the load timeout", tc.loadTimeoutSecs, got)
		}
	}
}
