// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

// mockSkit stands in for the skit dynamic-session API and counts create/destroy.
type mockSkit struct {
	mu       sync.Mutex
	creates  int
	destroys int
	live     map[string]bool
	nextID   int
}

func newMockSkit() (*mockSkit, *httptest.Server) {
	ms := &mockSkit{live: map[string]bool{}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ms.mu.Lock()
		defer ms.mu.Unlock()
		switch {
		case r.Method == http.MethodPost && r.URL.Path == "/api/v1/sessions":
			ms.creates++
			ms.nextID++
			id := fmt.Sprintf("sess-%d", ms.nextID)
			ms.live[id] = true
			w.Header().Set("Content-Type", "application/json")
			_, _ = fmt.Fprintf(w, `{"session_id":%q}`, id)
		case r.Method == http.MethodDelete && strings.HasPrefix(r.URL.Path, "/api/v1/sessions/"):
			ms.destroys++
			delete(ms.live, strings.TrimPrefix(r.URL.Path, "/api/v1/sessions/"))
			w.WriteHeader(http.StatusOK)
		case r.Method == http.MethodGet && r.URL.Path == "/api/v1/sessions":
			w.Header().Set("Content-Type", "application/json")
			parts := make([]string, 0, len(ms.live))
			for id := range ms.live {
				parts = append(parts, fmt.Sprintf(`{"id":%q}`, id))
			}
			_, _ = fmt.Fprintf(w, "[%s]", strings.Join(parts, ","))
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	return ms, srv
}

func (ms *mockSkit) counts() (int, int) {
	ms.mu.Lock()
	defer ms.mu.Unlock()
	return ms.creates, ms.destroys
}

func (ms *mockSkit) liveCount() int {
	ms.mu.Lock()
	defer ms.mu.Unlock()
	return len(ms.live)
}

type fakeClock struct{ t time.Time }

func (c *fakeClock) now() time.Time          { return c.t }
func (c *fakeClock) advance(d time.Duration) { c.t = c.t.Add(d) }

func newTestManager(srv *httptest.Server, maxSessions int, idle, maxLife time.Duration) (*sessionManager, *fakeClock) {
	m := newSessionManager(&skitClient{client: srv.Client(), baseURL: srv.URL}, maxSessions, idle, maxLife)
	clock := &fakeClock{t: time.Unix(1_000_000, 0)}
	m.now = clock.now
	return m, clock
}

func TestSessionManagerDedupeAndIdleReap(t *testing.T) {
	ms, srv := newMockSkit()
	defer srv.Close()
	m, clock := newTestManager(srv, 4, 30*time.Second, time.Hour)
	ctx := context.Background()

	s1, err := m.acquire(ctx, "https://a.example", "yaml")
	if err != nil {
		t.Fatal(err)
	}
	s2, err := m.acquire(ctx, "https://a.example", "yaml")
	if err != nil {
		t.Fatal(err)
	}
	if s1 != s2 {
		t.Fatal("expected the same session to be reused for the same URL")
	}
	if c, _ := ms.counts(); c != 1 {
		t.Fatalf("expected 1 create, got %d", c)
	}
	if s1.viewers != 2 {
		t.Fatalf("viewers=%d want 2", s1.viewers)
	}

	m.release(s1)
	m.release(s2)
	if s1.viewers != 0 {
		t.Fatalf("viewers=%d want 0", s1.viewers)
	}

	clock.advance(10 * time.Second)
	m.reap(ctx)
	if _, d := ms.counts(); d != 0 {
		t.Fatalf("premature destroy: %d", d)
	}

	clock.advance(30 * time.Second)
	m.reap(ctx)
	if _, d := ms.counts(); d != 1 {
		t.Fatalf("expected 1 destroy after idle TTL, got %d", d)
	}
	if ms.liveCount() != 0 {
		t.Fatalf("server still has %d sessions", ms.liveCount())
	}
}

func TestSessionManagerOverCapacity(t *testing.T) {
	ms, srv := newMockSkit()
	defer srv.Close()
	m, _ := newTestManager(srv, 1, time.Hour, time.Hour)
	ctx := context.Background()

	if _, err := m.acquire(ctx, "https://a.example", "y"); err != nil {
		t.Fatal(err)
	}
	if _, err := m.acquire(ctx, "https://b.example", "y"); !errors.Is(err, errOverCapacity) {
		t.Fatalf("expected errOverCapacity, got %v", err)
	}
	if c, _ := ms.counts(); c != 1 {
		t.Fatalf("expected exactly 1 create, got %d", c)
	}
}

func TestSessionManagerMaxLifetimeReapsActiveViewer(t *testing.T) {
	ms, srv := newMockSkit()
	defer srv.Close()
	m, clock := newTestManager(srv, 4, time.Hour, 10*time.Second)
	ctx := context.Background()

	// Acquire and keep the viewer (never release): the max-lifetime cap must
	// still tear the session down.
	if _, err := m.acquire(ctx, "https://a.example", "y"); err != nil {
		t.Fatal(err)
	}
	clock.advance(11 * time.Second)
	m.reap(ctx)
	if _, d := ms.counts(); d != 1 {
		t.Fatalf("expected max-lifetime reap, got %d destroys", d)
	}
}

func TestSessionManagerShutdownAll(t *testing.T) {
	ms, srv := newMockSkit()
	defer srv.Close()
	m, _ := newTestManager(srv, 4, time.Hour, time.Hour)
	ctx := context.Background()

	if _, err := m.acquire(ctx, "https://a.example", "y"); err != nil {
		t.Fatal(err)
	}
	if _, err := m.acquire(ctx, "https://b.example", "y"); err != nil {
		t.Fatal(err)
	}
	m.shutdownAll(ctx)
	if _, d := ms.counts(); d != 2 {
		t.Fatalf("expected 2 destroys, got %d", d)
	}
	if ms.liveCount() != 0 {
		t.Fatalf("server still has %d sessions", ms.liveCount())
	}
}
