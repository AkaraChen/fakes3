package fakes3

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"time"
)

// Control methods call the server's /_fakes3/ HTTP API. They are not synchronized with S3
// requests in flight, so call them between steps of a test, while no requests are pending.

// Error is an error returned by the control API.
type Error struct {
	Status  int    `json:"-"`    // HTTP status
	Code    string `json:"code"` // such as "NotFound" or "Conflict"
	Message string `json:"message"`
}

func (e *Error) Error() string {
	return fmt.Sprintf("fakes3: %s (%d): %s", e.Code, e.Status, e.Message)
}

// ResetOptions adjusts [Server.ResetWith].
type ResetOptions struct {
	KeepClock    bool // leave the clock as it is
	KeepRequests bool // keep the request log
	Snapshots    bool // delete all snapshots too
}

// Reset deletes all buckets, objects and multipart uploads, clears the request log and puts the
// clock back on the host clock. Snapshots are kept.
func (s *Server) Reset() error {
	return s.ResetWith(ResetOptions{})
}

// ResetWith is [Server.Reset] with options.
func (s *Server) ResetWith(opts ResetOptions) error {
	q := url.Values{}
	q.Set("clock", strconv.FormatBool(!opts.KeepClock))
	q.Set("requests", strconv.FormatBool(!opts.KeepRequests))
	q.Set("snapshots", strconv.FormatBool(opts.Snapshots))
	return s.call(http.MethodPost, "/reset?"+q.Encode(), nil, nil)
}

// SnapshotInfo describes a snapshot.
type SnapshotInfo struct {
	Name    string    `json:"name"`
	Created time.Time `json:"created"`
	Buckets int       `json:"buckets"`
	Objects int       `json:"objects"` // versioned keys, delete markers included
	Uploads int       `json:"uploads"` // multipart uploads in progress
}

// Snapshot saves the current buckets, objects and uploads under name, or under a generated name
// when name is empty. Object data is shared, not copied. Restore it as often as you like.
func (s *Server) Snapshot(name string) (SnapshotInfo, error) {
	var body any = struct{}{}
	if name != "" {
		body = map[string]string{"name": name}
	}
	var info SnapshotInfo
	err := s.call(http.MethodPost, "/snapshots", body, &info)
	return info, err
}

// Restore replaces all data with the snapshot's. The clock, the request log and other snapshots
// are left alone, and the snapshot can be restored again.
func (s *Server) Restore(name string) (SnapshotInfo, error) {
	var info SnapshotInfo
	err := s.call(http.MethodPost, "/snapshots/"+url.PathEscape(name)+"/restore", nil, &info)
	return info, err
}

// Snapshots lists the snapshots.
func (s *Server) Snapshots() ([]SnapshotInfo, error) {
	var out struct {
		Snapshots []SnapshotInfo `json:"snapshots"`
	}
	err := s.call(http.MethodGet, "/snapshots", nil, &out)
	return out.Snapshots, err
}

// DeleteSnapshot deletes a snapshot.
func (s *Server) DeleteSnapshot(name string) error {
	return s.call(http.MethodDelete, "/snapshots/"+url.PathEscape(name), nil, nil)
}

// Memory is the object data a server holds.
type Memory struct {
	Used  int64 // bytes of object data, including data only snapshots still refer to
	Limit int64 // the memory limit in bytes; 0 when unlimited
}

// Memory reports how much object data the server holds.
func (s *Server) Memory() (Memory, error) {
	var out struct {
		Memory struct {
			Used  int64  `json:"used"`
			Limit *int64 `json:"limit"`
		} `json:"memory"`
	}
	if err := s.call(http.MethodGet, "/stats", nil, &out); err != nil {
		return Memory{}, err
	}
	m := Memory{Used: out.Memory.Used}
	if out.Memory.Limit != nil {
		m.Limit = *out.Memory.Limit
	}
	return m, nil
}

// Clock returns the server clock. Every timestamp the server produces reads it, and so does
// everything that depends on time: presigned URL expiry, lifecycle rules, Object Lock retention
// and archive restores.
func (s *Server) Clock() *Clock { return &s.clock }

// Requests returns the request log, which records every S3 request the server receives.
func (s *Server) Requests() *RequestLog { return &s.requests }

// Clock controls a server's time.
type Clock struct{ s *Server }

// ClockState is the clock's reading.
type ClockState struct {
	Now     time.Time `json:"now"`
	Frozen  bool      `json:"frozen"`  // time stands still until moved
	Shifted bool      `json:"shifted"` // the clock differs from the host clock
}

// State reads the clock.
func (c *Clock) State() (ClockState, error) {
	var st ClockState
	err := c.s.call(http.MethodGet, "/clock", nil, &st)
	return st, err
}

// Now returns the server time.
func (c *Clock) Now() (time.Time, error) {
	st, err := c.State()
	return st.Now, err
}

// Freeze stops the clock at the current server time.
func (c *Clock) Freeze() error { return c.post("/clock/freeze", struct{}{}) }

// FreezeAt stops the clock at t.
func (c *Clock) FreezeAt(t time.Time) error { return c.post("/clock/freeze", at(t)) }

// Unfreeze lets time run again from the current server time.
func (c *Clock) Unfreeze() error { return c.post("/clock/unfreeze", nil) }

// Set jumps to t, keeping the clock frozen or running.
func (c *Clock) Set(t time.Time) error { return c.post("/clock/set", at(t)) }

// Advance moves the clock forward by d, or back if d is negative. Presigned URLs whose expiry
// it passes are rejected with 403, as on S3.
func (c *Clock) Advance(d time.Duration) error {
	return c.post("/clock/advance", map[string]string{"by": strconv.FormatInt(int64(d), 10) + "ns"})
}

// Reset puts the clock back on the host clock.
func (c *Clock) Reset() error { return c.post("/clock/reset", nil) }

func (c *Clock) post(path string, body any) error {
	return c.s.call(http.MethodPost, path, body, nil)
}

func at(t time.Time) map[string]string {
	return map[string]string{"at": t.UTC().Format(time.RFC3339Nano)}
}

// RequestLog is a server's request log.
type RequestLog struct{ s *Server }

// Request is a recorded S3 request.
type Request struct {
	Seq       uint64    `json:"seq"`       // arrival order, from 1
	Time      time.Time `json:"time"`      // server time of arrival
	Method    string    `json:"method"`    // HTTP method
	Path      string    `json:"path"`      // path and query, as sent
	Bucket    string    `json:"bucket"`    // empty when there is none
	Key       string    `json:"key"`       // empty when there is none
	Operation string    `json:"operation"` // such as "PutObject"; empty if not recognized
	Status    int       `json:"status"`    // HTTP status
	LatencyUs int64     `json:"latency_us"`
	BytesIn   int64     `json:"bytes_in"`
	BytesOut  int64     `json:"bytes_out"`
	AccessKey string    `json:"access_key"` // the signing access key; empty when unsigned or with Auth
	RequestID string    `json:"request_id"` // the x-amz-request-id response header
}

// RequestFilter selects requests. Zero fields match everything.
type RequestFilter struct {
	Bucket    string
	KeyPrefix string
	Operation string // such as "PutObject", case-insensitive
	Method    string // case-insensitive
	Status    string // "404", "4xx", or "error" (400 and above)
	Since     time.Time
	After     uint64 // only requests with a larger Seq
	Limit     int    // keep only the most recent matches
}

func (f RequestFilter) query() url.Values {
	q := url.Values{}
	set := func(k, v string) {
		if v != "" {
			q.Set(k, v)
		}
	}
	set("bucket", f.Bucket)
	set("key_prefix", f.KeyPrefix)
	set("op", f.Operation)
	set("method", f.Method)
	set("status", f.Status)
	if !f.Since.IsZero() {
		q.Set("since", f.Since.UTC().Format(time.RFC3339Nano))
	}
	if f.After > 0 {
		q.Set("after", strconv.FormatUint(f.After, 10))
	}
	if f.Limit > 0 {
		q.Set("limit", strconv.Itoa(f.Limit))
	}
	return q
}

type requestList struct {
	Requests []Request `json:"requests"`
	Total    int       `json:"total"`
}

// List returns the matching requests, oldest first.
func (l *RequestLog) List(f RequestFilter) ([]Request, error) {
	var out requestList
	err := l.s.call(http.MethodGet, "/requests?"+f.query().Encode(), nil, &out)
	return out.Requests, err
}

// Count returns how many requests match, ignoring f.Limit.
func (l *RequestLog) Count(f RequestFilter) (int, error) {
	f.Limit = 1
	var out requestList
	err := l.s.call(http.MethodGet, "/requests?"+f.query().Encode(), nil, &out)
	return out.Total, err
}

// Clear empties the log. Sequence numbers keep counting.
func (l *RequestLog) Clear() error {
	return l.s.call(http.MethodDelete, "/requests", nil, nil)
}

// RequestLogConfig is the request log's settings and fill level.
type RequestLogConfig struct {
	Enabled  bool  `json:"enabled"`
	Capacity int   `json:"capacity"`
	Stored   int   `json:"stored"`
	Dropped  int64 `json:"dropped"` // records dropped because the log was full
}

// Config returns the log's settings.
func (l *RequestLog) Config() (RequestLogConfig, error) {
	var c RequestLogConfig
	err := l.s.call(http.MethodGet, "/requests/config", nil, &c)
	return c, err
}

// SetEnabled starts or stops recording. Records already kept stay.
func (l *RequestLog) SetEnabled(enabled bool) error {
	return l.s.call(http.MethodPut, "/requests/config", map[string]bool{"enabled": enabled}, nil)
}

// SetCapacity sets how many records the log keeps, dropping the oldest if it shrinks.
func (l *RequestLog) SetCapacity(n int) error {
	return l.s.call(http.MethodPut, "/requests/config", map[string]int{"capacity": n}, nil)
}

func (s *Server) call(method, path string, body, out any) error {
	var rd io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return err
		}
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, s.Endpoint+"/_fakes3"+path, rd)
	if err != nil {
		return err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := s.http.Do(req)
	if err != nil {
		return fmt.Errorf("fakes3: %w", err)
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("fakes3: %w", err)
	}
	if resp.StatusCode >= 300 {
		var e struct {
			Error Error `json:"error"`
		}
		if json.Unmarshal(data, &e) != nil || e.Error.Code == "" {
			return &Error{Status: resp.StatusCode, Code: http.StatusText(resp.StatusCode), Message: string(data)}
		}
		e.Error.Status = resp.StatusCode
		return &e.Error
	}
	if out != nil {
		if err := json.Unmarshal(data, out); err != nil {
			return fmt.Errorf("fakes3: decode %s %s: %w", method, path, err)
		}
	}
	return nil
}
