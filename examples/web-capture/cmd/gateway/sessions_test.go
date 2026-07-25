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
	"sync/atomic"
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
	m := newSessionManager(&skitClient{client: srv.Client(), baseURL: srv.URL}, maxSessions, 100, idle, maxLife)
	clock := &fakeClock{t: time.Unix(1_000_000, 0)}
	m.now = clock.now
	return m, clock
}

func TestSessionManagerDedupeAndIdleReap(t *testing.T) {
	ms, srv := newMockSkit()
	defer srv.Close()
	m, clock := newTestManager(srv, 4, 30*time.Second, time.Hour)
	ctx := context.Background()

	s1, err := m.acquire(ctx, "https://a.example", testYAML)
	if err != nil {
		t.Fatal(err)
	}
	s2, err := m.acquire(ctx, "https://a.example", testYAML)
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

	if _, err := m.acquire(ctx, "https://a.example", testYAML); err != nil {
		t.Fatal(err)
	}
	if _, err := m.acquire(ctx, "https://b.example", testYAML); !errors.Is(err, errOverCapacity) {
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
	if _, err := m.acquire(ctx, "https://a.example", testYAML); err != nil {
		t.Fatal(err)
	}
	clock.advance(11 * time.Second)
	m.reap(ctx)
	if _, d := ms.counts(); d != 1 {
		t.Fatalf("expected max-lifetime reap, got %d destroys", d)
	}
}

func TestSessionManagerMaxViewers(t *testing.T) {
	_, srv := newMockSkit()
	defer srv.Close()
	m := newSessionManager(&skitClient{client: srv.Client(), baseURL: srv.URL}, 4, 2, time.Hour, time.Hour)
	ctx := context.Background()

	if _, err := m.acquire(ctx, "https://a.example", testYAML); err != nil { // creator (viewers=1)
		t.Fatal(err)
	}
	if _, err := m.acquire(ctx, "https://a.example", testYAML); err != nil { // 2nd viewer (=2)
		t.Fatalf("2nd viewer: %v", err)
	}
	if _, err := m.acquire(ctx, "https://a.example", testYAML); !errors.Is(err, errOverCapacity) {
		t.Fatalf("3rd viewer: expected errOverCapacity, got %v", err)
	}
}

func testYAML() string { return "pipeline: yaml" }

func TestSessionManagerRejectsAfterShutdown(t *testing.T) {
	_, srv := newMockSkit()
	defer srv.Close()
	m := newSessionManager(&skitClient{client: srv.Client(), baseURL: srv.URL}, 4, 10, time.Hour, time.Hour)
	m.closed = true
	if _, err := m.acquire(context.Background(), "https://a.example", testYAML); !errors.Is(err, errShuttingDown) {
		t.Fatalf("expected errShuttingDown, got %v", err)
	}
}

// A session whose creation is in flight when shutdownAll runs must still be torn
// down once creation resolves — otherwise its backend pipeline leaks.
func TestSessionManagerShutdownDuringCreate(t *testing.T) {
	release := make(chan struct{})
	var created, destroyed atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodPost && r.URL.Path == "/api/v1/sessions":
			<-release // block so the create is in flight when shutdownAll runs
			created.Add(1)
			w.Header().Set("Content-Type", "application/json")
			_, _ = fmt.Fprint(w, `{"session_id":"sess-inflight"}`)
		case r.Method == http.MethodDelete:
			destroyed.Add(1)
			w.WriteHeader(http.StatusOK)
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer srv.Close()
	m := newSessionManager(&skitClient{client: srv.Client(), baseURL: srv.URL}, 4, 10, time.Hour, time.Hour)

	acquireDone := make(chan error, 1)
	go func() {
		_, err := m.acquire(context.Background(), "https://a.example", testYAML)
		acquireDone <- err
	}()

	// Wait until acquire has reserved the session and is blocked in createSession.
	deadline := time.Now().Add(2 * time.Second)
	for {
		m.mu.Lock()
		n := len(m.sessions)
		m.mu.Unlock()
		if n == 1 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("acquire never reserved the session")
		}
		time.Sleep(time.Millisecond)
	}

	shutdownDone := make(chan struct{})
	go func() {
		m.shutdownAll(context.Background())
		close(shutdownDone)
	}()

	close(release) // let creation finish
	acquireErr := <-acquireDone
	<-shutdownDone

	if created.Load() != 1 {
		t.Fatalf("created=%d want 1", created.Load())
	}
	if destroyed.Load() != 1 {
		t.Fatalf("in-flight session not torn down on shutdown: destroyed=%d", destroyed.Load())
	}
	// The creator must not be handed the doomed session, and its id must not be
	// re-registered in the byID map that shutdownAll just reset.
	if !errors.Is(acquireErr, errShuttingDown) {
		t.Fatalf("acquire during shutdown: got %v, want errShuttingDown", acquireErr)
	}
	if m.isLive("sess-inflight") {
		t.Fatal("destroyed in-flight session still registered in byID")
	}
}

// A session reaped while a deduped late viewer is still waiting in proxyMSE's
// readiness loop must fail fast (the post-teardown 404 is otherwise
// indistinguishable from a pre-ready 404, so the loop would spin the full
// mseReadyTimeout and return a misleading 502).
func TestProxyMSEFailsFastWhenSessionReaped(t *testing.T) {
	// Upstream always 404s (as it would for both a not-yet-ready and a
	// torn-down session).
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer srv.Close()

	m := newSessionManager(&skitClient{client: srv.Client(), baseURL: srv.URL}, 4, 10, time.Hour, time.Hour)
	gw := &gateway{
		streamClient:    srv.Client(),
		skit:            &skitClient{client: srv.Client(), baseURL: srv.URL},
		sessions:        m,
		mseReadyTimeout: 30 * time.Second, // long: the test must return well before this
	}
	// A session the manager no longer tracks (reaped) — isLive(id) is false.
	s := &liveSession{id: "sess-reaped", ready: make(chan struct{})}
	close(s.ready)

	r := httptest.NewRequest(http.MethodGet, "/cast/example.com", nil)
	w := httptest.NewRecorder()

	start := time.Now()
	gw.proxyMSE(w, r, s)
	elapsed := time.Since(start)

	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503", w.Code)
	}
	if elapsed > 5*time.Second {
		t.Fatalf("proxyMSE spun for %v; expected fast fail well under mseReadyTimeout", elapsed)
	}
}

func TestSessionManagerShutdownAll(t *testing.T) {
	ms, srv := newMockSkit()
	defer srv.Close()
	m, _ := newTestManager(srv, 4, time.Hour, time.Hour)
	ctx := context.Background()

	if _, err := m.acquire(ctx, "https://a.example", testYAML); err != nil {
		t.Fatal(err)
	}
	if _, err := m.acquire(ctx, "https://b.example", testYAML); err != nil {
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
