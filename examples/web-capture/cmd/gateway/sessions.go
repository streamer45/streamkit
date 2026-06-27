// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

var errOverCapacity = errors.New("session capacity reached")

// skitClient talks to the skit dynamic-session REST API.
type skitClient struct {
	client  *http.Client
	baseURL string
	token   string
}

func (c *skitClient) auth(req *http.Request) {
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}
}

func (c *skitClient) createSession(ctx context.Context, yaml string) (string, error) {
	payload, err := json.Marshal(struct {
		YAML string `json:"yaml"`
	}{yaml})
	if err != nil {
		return "", err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+"/api/v1/sessions", bytes.NewReader(payload))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	c.auth(req)

	resp, err := c.client.Do(req)
	if err != nil {
		return "", err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return "", fmt.Errorf("create session: status %d: %s", resp.StatusCode, strings.TrimSpace(string(b)))
	}
	var out struct {
		SessionID string `json:"session_id"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", fmt.Errorf("decode session response: %w", err)
	}
	if out.SessionID == "" {
		return "", errors.New("empty session_id in response")
	}
	return out.SessionID, nil
}

func (c *skitClient) destroySession(ctx context.Context, id string) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodDelete, c.baseURL+"/api/v1/sessions/"+url.PathEscape(id), nil)
	if err != nil {
		return err
	}
	c.auth(req)
	resp, err := c.client.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
	// A 404 means the session is already gone — the desired end state.
	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusNotFound {
		return fmt.Errorf("destroy session: status %d", resp.StatusCode)
	}
	return nil
}

func (c *skitClient) listSessionIDs(ctx context.Context) (map[string]bool, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.baseURL+"/api/v1/sessions", nil)
	if err != nil {
		return nil, err
	}
	c.auth(req)
	resp, err := c.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("list sessions: status %d", resp.StatusCode)
	}
	var arr []struct {
		ID string `json:"id"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&arr); err != nil {
		return nil, err
	}
	ids := make(map[string]bool, len(arr))
	for _, s := range arr {
		ids[s.ID] = true
	}
	return ids, nil
}

func (c *skitClient) streamURL(id string) string {
	return c.baseURL + "/mse/" + url.PathEscape(id) + "/video"
}

// liveSession is one running cast pipeline, shared by all viewers of the same
// normalized URL. ready is closed once creation resolves (id set or createErr).
type liveSession struct {
	id         string
	key        string
	viewers    int
	createdAt  time.Time
	idleSince  time.Time // set when viewers drops to 0; zero while viewers > 0
	reapReason string
	ready      chan struct{}
	createErr  error
}

// sessionManager owns cast pipeline lifetime. The engine does NOT auto-stop a
// pipeline when its MSE viewers disconnect, so the gateway must: dedupe by URL,
// refcount viewers, and reap idle / over-aged sessions via DELETE. The zero
// value is not usable — construct via newSessionManager.
type sessionManager struct {
	mu          sync.Mutex
	sessions    map[string]*liveSession // key (normalized URL) -> session
	byID        map[string]*liveSession
	skit        *skitClient
	maxSessions int
	idleTTL     time.Duration
	maxLifetime time.Duration
	createTO    time.Duration
	now         func() time.Time
}

func newSessionManager(skit *skitClient, maxSessions int, idleTTL, maxLifetime time.Duration) *sessionManager {
	return &sessionManager{
		sessions:    make(map[string]*liveSession),
		byID:        make(map[string]*liveSession),
		skit:        skit,
		maxSessions: maxSessions,
		idleTTL:     idleTTL,
		maxLifetime: maxLifetime,
		createTO:    15 * time.Second,
		now:         time.Now,
	}
}

func (m *sessionManager) updateGaugesLocked() {
	activeSessions.Set(float64(len(m.sessions)))
	viewers := 0
	for _, s := range m.sessions {
		viewers += s.viewers
	}
	activeViewers.Set(float64(viewers))
}

// acquire returns a ready session for key, creating one (and the skit pipeline)
// if none exists. The caller must call release exactly once when its viewer
// connection ends.
func (m *sessionManager) acquire(ctx context.Context, key, yaml string) (*liveSession, error) {
	m.mu.Lock()
	if s := m.sessions[key]; s != nil {
		s.viewers++
		s.idleSince = time.Time{}
		m.updateGaugesLocked()
		m.mu.Unlock()
		select {
		case <-s.ready:
			if s.createErr != nil {
				m.release(s)
				return nil, s.createErr
			}
			return s, nil
		case <-ctx.Done():
			m.release(s)
			return nil, ctx.Err()
		}
	}
	if len(m.sessions) >= m.maxSessions {
		m.mu.Unlock()
		return nil, errOverCapacity
	}
	s := &liveSession{key: key, viewers: 1, createdAt: m.now(), ready: make(chan struct{})}
	m.sessions[key] = s
	m.updateGaugesLocked()
	m.mu.Unlock()

	// Bound the create call independently of the (possibly long-lived) viewer ctx.
	cctx, cancel := context.WithTimeout(ctx, m.createTO)
	defer cancel()
	id, err := m.skit.createSession(cctx, yaml)

	m.mu.Lock()
	if err != nil {
		delete(m.sessions, key)
		s.createErr = err
		m.updateGaugesLocked()
		m.mu.Unlock()
		close(s.ready)
		return nil, err
	}
	s.id = id
	m.byID[id] = s
	m.mu.Unlock()
	close(s.ready)
	log.Printf("cast: session %s created (key=%q)", id, key)
	return s, nil
}

func (m *sessionManager) release(s *liveSession) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if s.viewers > 0 {
		s.viewers--
	}
	if s.viewers == 0 {
		s.idleSince = m.now()
	}
	m.updateGaugesLocked()
	log.Printf("cast: viewer left %s (viewers=%d)", s.id, s.viewers)
}

// reap tears down idle and over-aged sessions, then reconciles bookkeeping
// against the server's actual session list.
func (m *sessionManager) reap(ctx context.Context) {
	now := m.now()
	var toDestroy []*liveSession

	m.mu.Lock()
	for key, s := range m.sessions {
		if s.id == "" {
			continue // still being created
		}
		switch {
		case s.viewers == 0 && !s.idleSince.IsZero() && now.Sub(s.idleSince) >= m.idleTTL:
			s.reapReason = "idle"
		case now.Sub(s.createdAt) >= m.maxLifetime:
			s.reapReason = "max_lifetime"
		default:
			continue
		}
		delete(m.sessions, key)
		delete(m.byID, s.id)
		toDestroy = append(toDestroy, s)
	}
	m.updateGaugesLocked()
	m.mu.Unlock()

	for _, s := range toDestroy {
		m.destroy(ctx, s)
	}
	m.reconcile(ctx)
}

func (m *sessionManager) destroy(ctx context.Context, s *liveSession) {
	dctx, cancel := context.WithTimeout(ctx, m.createTO)
	defer cancel()
	if err := m.skit.destroySession(dctx, s.id); err != nil {
		log.Printf("destroy session %s: %v", s.id, err)
	}
	sessionsReaped.WithLabelValues(s.reapReason).Inc()
	sessionLifetime.Observe(m.now().Sub(s.createdAt).Seconds())
	log.Printf("cast: reaped session %s (reason=%s)", s.id, s.reapReason)
}

// reconcile drops sessions the server no longer has (e.g. it failed or was
// removed out of band), so the gateway's view self-heals.
func (m *sessionManager) reconcile(ctx context.Context) {
	lctx, cancel := context.WithTimeout(ctx, m.createTO)
	defer cancel()
	ids, err := m.skit.listSessionIDs(lctx)
	if err != nil {
		return // best effort
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	for key, s := range m.sessions {
		if s.id == "" {
			continue
		}
		if !ids[s.id] {
			log.Printf("reconcile: session %s gone server-side, dropping", s.id)
			delete(m.sessions, key)
			delete(m.byID, s.id)
		}
	}
	m.updateGaugesLocked()
}

func (m *sessionManager) runReaper(ctx context.Context, interval time.Duration) {
	t := time.NewTicker(interval)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			m.reap(ctx)
		}
	}
}

// shutdownAll destroys every owned session so a deploy/restart never leaks
// Servo pipelines.
func (m *sessionManager) shutdownAll(ctx context.Context) {
	m.mu.Lock()
	all := make([]*liveSession, 0, len(m.sessions))
	for _, s := range m.sessions {
		if s.id != "" {
			s.reapReason = "shutdown"
			all = append(all, s)
		}
	}
	m.sessions = make(map[string]*liveSession)
	m.byID = make(map[string]*liveSession)
	m.updateGaugesLocked()
	m.mu.Unlock()

	for _, s := range all {
		m.destroy(ctx, s)
	}
}
