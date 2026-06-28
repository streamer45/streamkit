// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"time"
)

type captureMode int

const (
	modeClip captureMode = iota
	modeCast
)

func (m captureMode) String() string {
	if m == modeCast {
		return "cast"
	}
	return "clip"
}

// Capture resolution is capped at 4K; bigger just wastes work.
const (
	maxResW = 3840
	maxResH = 2160
)

var (
	errNoTarget      = errors.New("no target URL in path")
	errBadDuration   = errors.New("invalid dur option")
	errBadResolution = errors.New("invalid res option (expected WxH)")
	errUnknownOption = errors.New("unknown option")
	errBlockedTarget = errors.New("target host is not allowed")
)

// captureOpts holds the per-request knobs parsed from the options segment,
// seeded from the gateway defaults. res{W,H} is the capture resolution: the
// page renders at that size AND is encoded at it (1:1, no downscale) so text
// stays crisp; a wider resolution also yields a roomier desktop layout.
type captureOpts struct {
	dur  time.Duration // clip only
	resW int
	resH int
}

// optionsSegmentRe matches a leading comma-separated key=value options segment
// (e.g. "dur=30s" or "res=1920x1080,dur=30s"). A real host never contains '=',
// so a first segment that matches this is unambiguously options, not the target.
var optionsSegmentRe = regexp.MustCompile(`^[a-z]+=[^,/]+(,[a-z]+=[^,/]+)*$`)

// detectMode resolves the output mode from the first path segment — `clip` or
// `cast` — so a single host (web.streamkit.dev) serves both: the path is
// /{mode}/[options]/{target-url}. It returns the rest of the raw request target
// with the mode segment stripped, and ok=false when neither matches.
func detectMode(rawTarget string) (mode captureMode, rest string, ok bool) {
	rest = strings.TrimPrefix(rawTarget, "/")
	switch {
	case rest == "clip" || strings.HasPrefix(rest, "clip/"):
		return modeClip, strings.TrimPrefix(strings.TrimPrefix(rest, "clip"), "/"), true
	case rest == "cast" || strings.HasPrefix(rest, "cast/"):
		return modeCast, strings.TrimPrefix(strings.TrimPrefix(rest, "cast"), "/"), true
	}
	return modeClip, rest, false
}

// parseTargetAndOptions splits an optional leading options segment from the
// verbatim target URL. rest MUST come from r.RequestURI (not the cleaned
// r.URL.Path), so the target's own "//" and query string survive intact. opts
// is seeded from def and overridden by any options present.
func parseTargetAndOptions(rest string, def captureOpts, maxDur time.Duration) (target string, opts captureOpts, err error) {
	opts = def
	rest = strings.TrimPrefix(rest, "/")
	if rest == "" {
		return "", def, errNoTarget
	}

	head, tail, _ := strings.Cut(rest, "/")
	headDec, decErr := url.PathUnescape(head)
	if decErr != nil {
		headDec = head
	}

	switch {
	case optionsSegmentRe.MatchString(headDec) && tail == "":
		return "", def, errNoTarget // options given but no target followed
	case optionsSegmentRe.MatchString(headDec):
		opts, err = parseOptions(headDec, def, maxDur)
		if err != nil {
			return "", def, err
		}
		target = tail
	default:
		target = rest
	}

	if target == "" {
		return "", def, errNoTarget
	}
	low := strings.ToLower(target)
	if !strings.HasPrefix(low, "http://") && !strings.HasPrefix(low, "https://") {
		target = "https://" + target
	}
	return target, opts, nil
}

func parseOptions(seg string, def captureOpts, maxDur time.Duration) (captureOpts, error) {
	opts := def
	for kv := range strings.SplitSeq(seg, ",") {
		k, v, found := strings.Cut(kv, "=")
		if !found {
			return def, errUnknownOption
		}
		switch k {
		case "dur":
			d, err := parseDuration(v)
			if err != nil {
				return def, err
			}
			if d > maxDur {
				d = maxDur // clamp rather than reject — friendlier for a demo
			}
			opts.dur = d
		case "res":
			w, h, err := parseResolution(v)
			if err != nil {
				return def, err
			}
			opts.resW, opts.resH = w, h
		default:
			return def, fmt.Errorf("%w: %q", errUnknownOption, k)
		}
	}
	return opts, nil
}

func parseDuration(v string) (time.Duration, error) {
	if d, err := time.ParseDuration(v); err == nil {
		if d <= 0 {
			return 0, errBadDuration
		}
		return d, nil
	}
	if n, err := strconv.Atoi(v); err == nil && n > 0 {
		return time.Duration(n) * time.Second, nil
	}
	return 0, errBadDuration
}

// parseResolution parses a "WxH" string, clamping each dimension to a sane
// maximum. Used for both the gateway default and the res= option.
func parseResolution(s string) (w, h int, err error) {
	a, b, ok := strings.Cut(strings.ToLower(s), "x")
	if !ok {
		return 0, 0, errBadResolution
	}
	w, errW := strconv.Atoi(a)
	h, errH := strconv.Atoi(b)
	if errW != nil || errH != nil || w < 1 || h < 1 {
		return 0, 0, errBadResolution
	}
	if w > maxResW {
		w = maxResW
	}
	if h > maxResH {
		h = maxResH
	}
	return w, h, nil
}

// validateTarget enforces the public/URL-only policy: http(s) only, and the
// host must not resolve to a loopback/private/link-local/metadata address. DNS
// is re-resolved here so a hostname pointing at an internal IP is caught;
// rebinding between this check and Servo's own fetch is a documented limitation.
func validateTarget(ctx context.Context, raw string) (*url.URL, error) {
	u, err := url.Parse(raw)
	if err != nil {
		return nil, fmt.Errorf("parse url: %w", err)
	}
	switch strings.ToLower(u.Scheme) {
	case "http", "https":
	default:
		return nil, errBlockedTarget
	}
	host := u.Hostname()
	if host == "" {
		return nil, errBlockedTarget
	}

	rctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	addrs, err := net.DefaultResolver.LookupIPAddr(rctx, host)
	if err != nil {
		return nil, fmt.Errorf("resolve host: %w", err)
	}
	if len(addrs) == 0 {
		return nil, errBlockedTarget
	}
	for _, a := range addrs {
		if isBlockedIP(a.IP) {
			return nil, errBlockedTarget
		}
	}
	return u, nil
}

func isBlockedIP(ip net.IP) bool {
	if ip.IsLoopback() ||
		ip.IsPrivate() ||
		ip.IsUnspecified() ||
		ip.IsLinkLocalUnicast() ||
		ip.IsLinkLocalMulticast() ||
		ip.IsInterfaceLocalMulticast() ||
		ip.IsMulticast() {
		return true
	}
	if v4 := ip.To4(); v4 != nil {
		return blockedV4(v4)
	}
	// IPv6 forms that embed an internal IPv4 (NAT64, 6to4) dodge the v4 checks
	// above — extract the embedded address and re-check it.
	if v4 := embeddedV4(ip); v4 != nil {
		return isBlockedIP(v4)
	}
	return false
}

// blockedV4 covers reserved IPv4 ranges that the net.IP predicates miss.
func blockedV4(v4 net.IP) bool {
	switch {
	case v4[0] == 0: // 0.0.0.0/8 "this host" — 0.0.0.1 reaches localhost on Linux
		return true
	case v4[0] == 100 && v4[1]&0xc0 == 64: // 100.64.0.0/10 CGNAT
		return true
	case v4[0] == 198 && v4[1]&0xfe == 18: // 198.18.0.0/15 benchmarking
		return true
	default:
		return false
	}
}

// embeddedV4 returns the IPv4 embedded in a NAT64 (64:ff9b::/96) or 6to4
// (2002::/16) address, or nil for anything else.
func embeddedV4(ip net.IP) net.IP {
	v6 := ip.To16()
	if v6 == nil || ip.To4() != nil {
		return nil
	}
	nat64 := v6[0] == 0x00 && v6[1] == 0x64 && v6[2] == 0xff && v6[3] == 0x9b &&
		v6[4] == 0 && v6[5] == 0 && v6[6] == 0 && v6[7] == 0 &&
		v6[8] == 0 && v6[9] == 0 && v6[10] == 0 && v6[11] == 0
	if nat64 {
		return net.IPv4(v6[12], v6[13], v6[14], v6[15])
	}
	if v6[0] == 0x20 && v6[1] == 0x02 { // 6to4
		return net.IPv4(v6[2], v6[3], v6[4], v6[5])
	}
	return nil
}
