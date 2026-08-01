// Package aelio is the Go SDK for the Aelio conversational runtime.
//
// Install your backend tools with Expose, then Listen to dial out to the Aelio
// server over a single WebSocket. No inbound ports or webhooks are required
// for the Aelio↔SDK link.
package aelio

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

const (
	DefaultSDKPath    = "/sdk"
	HeartbeatInterval = 30 * time.Second
	HeartbeatTimeout  = 60 * time.Second
	SDKVersion        = "0.1.0"
)

// SafetyLevel mirrors the wire protocol.
type SafetyLevel string

const (
	SafetyRead        SafetyLevel = "read"
	SafetyWrite       SafetyLevel = "write"
	SafetyDestructive SafetyLevel = "destructive"
)

// InvocationContext is passed to every tool handler.
type InvocationContext struct {
	CustomerID string                 `json:"customerId"`
	SessionID  string                 `json:"sessionId,omitempty"`
	Channel    string                 `json:"channel,omitempty"`
	Extra      map[string]interface{} `json:"-"`
}

// FunctionSchema describes a registered tool.
type FunctionSchema struct {
	Description string                 `json:"description"`
	Params      map[string]interface{} `json:"params"`
	Safety      SafetyLevel            `json:"safety"`
	Intent      string                 `json:"intent,omitempty"`
	Output      map[string]OutputField `json:"output,omitempty"`
	OutputRole  string                 `json:"outputRole,omitempty"`
}

// OutputField declares the stable meaning and disclosure class of one tool result field.
// Path defaults to the output field name. Type defaults to "auto"; Sensitivity defaults to "none".
type OutputField struct {
	Path        string `json:"path,omitempty"`
	Type        string `json:"type,omitempty"`
	Sensitivity string `json:"sensitivity,omitempty"`
	Meaning     string `json:"meaning"`
}

// Handler is an exposed backend function.
type Handler func(args map[string]interface{}, ctx InvocationContext) (interface{}, error)

// OutboundDelivery is what Aelio asks you to send via your own provider.
type OutboundDelivery struct {
	Channel  string                 `json:"channel"`
	To       string                 `json:"to"`
	Content  string                 `json:"content"`
	Metadata map[string]interface{} `json:"metadata,omitempty"`
}

// SendHandler delivers outbound messages (BYO channel).
type SendHandler func(delivery OutboundDelivery) error

// InboundIngest is a message you received on your own webhook.
type InboundIngest struct {
	Channel   string                 `json:"channel"`
	From      string                 `json:"from"`
	Text      string                 `json:"text"`
	MessageID string                 `json:"messageId,omitempty"`
	Metadata  map[string]interface{} `json:"metadata,omitempty"`
}

// StateSchema declares a customer lifecycle state.
type StateSchema struct {
	Description  string                   `json:"description"`
	AllowedTools []string                 `json:"allowedTools,omitempty"`
	BlockedTools []string                 `json:"blockedTools,omitempty"`
	Guards       map[string]interface{}   `json:"guards,omitempty"`
	Transitions  []map[string]interface{} `json:"transitions,omitempty"`
}

// PolicySchema declares a conversation policy.
type PolicySchema struct {
	Description string                 `json:"description"`
	Severity    string                 `json:"severity,omitempty"` // hard | soft
	Aelio       map[string]interface{} `json:"aelio,omitempty"`
}

// FlowStepSchema is one step inside a guided flow.
type FlowStepSchema struct {
	Goal string `json:"goal"`
	Tool string `json:"tool,omitempty"`
}

// FlowSchema declares a multi-step flow scoped to a lifecycle state.
type FlowSchema struct {
	State       string                    `json:"state"`
	Description string                    `json:"description"`
	Steps       map[string]FlowStepSchema `json:"steps"`
}

// ListenOptions configures the outbound WebSocket connection.
type ListenOptions struct {
	Secret     string
	URL        string // e.g. ws://127.0.0.1:3010
	SDKVersion string
}

type handlerEntry struct {
	fn     Handler
	schema FunctionSchema
}

// Client is the Aelio SDK instance.
type Client struct {
	mu           sync.Mutex
	handlers     map[string]handlerEntry
	states       map[string]StateSchema
	policies     map[string]PolicySchema
	flows        map[string]FlowSchema
	sendHandler  SendHandler
	personaText  string
	productBrief string
	conn         *websocket.Conn
	opts         *ListenOptions
	lastPong     time.Time
	shouldRun    bool
	reconnectN   int
	cancelHB     context.CancelFunc
}

// New creates an empty Client.
func New() *Client {
	return &Client{
		handlers: map[string]handlerEntry{},
		states:   map[string]StateSchema{},
		policies: map[string]PolicySchema{},
		flows:    map[string]FlowSchema{},
	}
}

// Default is a package-level singleton, matching Node's `aelio` export.
var Default = New()

// Expose registers a callable backend function.
func (c *Client) Expose(name string, schema FunctionSchema, handler Handler) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.handlers[name] = handlerEntry{fn: handler, schema: schema}
}

// Persona sets the assistant voice.
func (c *Client) Persona(text string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.personaText = text
}

// Describe sets the product brief that grounds the planner.
func (c *Client) Describe(text string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.productBrief = text
}

// State declares a lifecycle state.
func (c *Client) State(id string, schema StateSchema) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.states[id] = schema
}

// Policy declares a conversation policy.
func (c *Client) Policy(id string, schema PolicySchema) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if schema.Severity == "" {
		schema.Severity = "soft"
	}
	c.policies[id] = schema
}

// Flow declares a guided multi-step flow.
func (c *Client) Flow(id string, schema FlowSchema) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.flows[id] = schema
}

// OnSend registers the BYO outbound delivery handler.
func (c *Client) OnSend(handler SendHandler) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.sendHandler = handler
}

// SetCustomerState pushes the active lifecycle state for a customer.
func (c *Client) SetCustomerState(customerID, stateID, reason string) {
	payload := map[string]interface{}{
		"type":       "set_state",
		"customerId": customerID,
		"stateId":    stateID,
	}
	if reason != "" {
		payload["reason"] = reason
	}
	c.sendJSON(payload)
}

// Ingest hands Aelio an inbound message from your own channel webhook.
func (c *Client) Ingest(msg InboundIngest) {
	payload := map[string]interface{}{
		"type":    "ingest",
		"channel": msg.Channel,
		"from":    msg.From,
		"text":    msg.Text,
	}
	if msg.MessageID != "" {
		payload["messageId"] = msg.MessageID
	}
	if msg.Metadata != nil {
		payload["metadata"] = msg.Metadata
	}
	c.sendJSON(payload)
}

// Listen connects to the Aelio server and blocks until Disconnect or fatal exit.
// It auto-reconnects with exponential backoff.
func (c *Client) Listen(opts ListenOptions) error {
	if opts.Secret == "" {
		return fmt.Errorf("aelio: secret is required")
	}
	if opts.URL == "" {
		opts.URL = "ws://127.0.0.1:3010"
	}
	if opts.SDKVersion == "" {
		opts.SDKVersion = SDKVersion
	}
	c.mu.Lock()
	c.opts = &opts
	c.shouldRun = true
	c.mu.Unlock()

	for {
		c.mu.Lock()
		run := c.shouldRun
		c.mu.Unlock()
		if !run {
			return nil
		}
		err := c.connectOnce()
		c.mu.Lock()
		run = c.shouldRun
		n := c.reconnectN
		c.mu.Unlock()
		if !run {
			return nil
		}
		delay := time.Second << n
		if delay > 30*time.Second {
			delay = 30 * time.Second
		}
		c.mu.Lock()
		c.reconnectN++
		c.mu.Unlock()
		if err != nil {
			log.Printf("[aelio-sdk] disconnected: %v — reconnecting in %s", err, delay)
		}
		time.Sleep(delay)
	}
}

// Disconnect stops reconnecting and closes the socket.
func (c *Client) Disconnect() {
	c.mu.Lock()
	c.shouldRun = false
	if c.cancelHB != nil {
		c.cancelHB()
		c.cancelHB = nil
	}
	conn := c.conn
	c.conn = nil
	c.mu.Unlock()
	if conn != nil {
		_ = conn.Close()
	}
}

func (c *Client) connectOnce() error {
	c.mu.Lock()
	opts := *c.opts
	c.mu.Unlock()

	url := opts.URL + DefaultSDKPath
	header := http.Header{}
	header.Set("Authorization", "Bearer "+opts.Secret)

	dialer := websocket.Dialer{HandshakeTimeout: 15 * time.Second}
	conn, _, err := dialer.Dial(url, header)
	if err != nil {
		return err
	}

	c.mu.Lock()
	c.conn = conn
	c.lastPong = time.Now()
	c.reconnectN = 0
	hbCtx, cancel := context.WithCancel(context.Background())
	c.cancelHB = cancel
	c.mu.Unlock()

	if err := c.sendRegister(); err != nil {
		_ = conn.Close()
		return err
	}

	go c.heartbeatLoop(hbCtx)

	for {
		_, data, err := conn.ReadMessage()
		if err != nil {
			cancel()
			c.mu.Lock()
			if c.conn == conn {
				c.conn = nil
			}
			c.mu.Unlock()
			return err
		}
		c.handleMessage(data)
	}
}

func (c *Client) heartbeatLoop(ctx context.Context) {
	ticker := time.NewTicker(HeartbeatInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			c.mu.Lock()
			stale := time.Since(c.lastPong) > HeartbeatTimeout
			conn := c.conn
			c.mu.Unlock()
			if stale && conn != nil {
				_ = conn.Close()
				return
			}
		}
	}
}

func (c *Client) sendRegister() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	functions := make([]map[string]interface{}, 0, len(c.handlers))
	for name, entry := range c.handlers {
		fn := map[string]interface{}{
			"name":        name,
			"description": entry.schema.Description,
			"params":      entry.schema.Params,
			"safety":      entry.schema.Safety,
		}
		if entry.schema.Intent != "" {
			fn["intent"] = entry.schema.Intent
		}
		if len(entry.schema.Output) > 0 {
			fn["output"] = entry.schema.Output
		}
		if entry.schema.OutputRole != "" {
			fn["outputRole"] = entry.schema.OutputRole
		}
		functions = append(functions, fn)
	}

	payload := map[string]interface{}{
		"type":       "register",
		"sdkVersion": c.opts.SDKVersion,
		"language":   "go",
		"functions":  functions,
		"canSend":    c.sendHandler != nil,
	}
	if c.personaText != "" {
		payload["persona"] = c.personaText
	}
	if c.productBrief != "" {
		payload["productBrief"] = c.productBrief
	}
	if len(c.states) > 0 {
		states := make([]map[string]interface{}, 0, len(c.states))
		for id, s := range c.states {
			st := map[string]interface{}{"id": id, "description": s.Description}
			if len(s.AllowedTools) > 0 {
				st["allowedTools"] = s.AllowedTools
			}
			if len(s.BlockedTools) > 0 {
				st["blockedTools"] = s.BlockedTools
			}
			if s.Guards != nil {
				st["guards"] = s.Guards
			}
			if len(s.Transitions) > 0 {
				st["transitions"] = s.Transitions
			}
			states = append(states, st)
		}
		payload["states"] = states
	}
	if len(c.policies) > 0 {
		policies := make([]map[string]interface{}, 0, len(c.policies))
		for id, p := range c.policies {
			policy := map[string]interface{}{
				"id":          id,
				"description": p.Description,
				"severity":    p.Severity,
			}
			if p.Aelio != nil {
				policy["aelio"] = p.Aelio
			}
			policies = append(policies, policy)
		}
		payload["policies"] = policies
	}
	if len(c.flows) > 0 {
		flows := make([]map[string]interface{}, 0, len(c.flows))
		for id, f := range c.flows {
			steps := make([]map[string]interface{}, 0, len(f.Steps))
			for sid, step := range f.Steps {
				s := map[string]interface{}{"id": sid, "goal": step.Goal}
				if step.Tool != "" {
					s["tool"] = step.Tool
				}
				steps = append(steps, s)
			}
			flows = append(flows, map[string]interface{}{
				"id":          id,
				"state":       f.State,
				"description": f.Description,
				"steps":       steps,
			})
		}
		payload["flows"] = flows
	}

	return c.conn.WriteJSON(payload)
}

func (c *Client) handleMessage(raw []byte) {
	var envelope struct {
		Type string `json:"type"`
	}
	if err := json.Unmarshal(raw, &envelope); err != nil {
		return
	}
	switch envelope.Type {
	case "ping":
		var msg struct {
			TS int64 `json:"ts"`
		}
		_ = json.Unmarshal(raw, &msg)
		c.mu.Lock()
		c.lastPong = time.Now()
		c.mu.Unlock()
		c.sendJSON(map[string]interface{}{"type": "pong", "ts": msg.TS})
	case "invoke":
		go c.handleInvoke(raw)
	case "send":
		go c.handleSend(raw)
	case "error":
		var msg struct {
			Code    string `json:"code"`
			Message string `json:"message"`
		}
		_ = json.Unmarshal(raw, &msg)
		log.Printf("[aelio-sdk] server error [%s]: %s", msg.Code, msg.Message)
	}
}

func (c *Client) handleInvoke(raw []byte) {
	started := time.Now()
	var msg struct {
		ID       string                 `json:"id"`
		Function string                 `json:"function"`
		Args     map[string]interface{} `json:"args"`
		Context  InvocationContext      `json:"context"`
	}
	if err := json.Unmarshal(raw, &msg); err != nil {
		return
	}

	c.mu.Lock()
	entry, ok := c.handlers[msg.Function]
	c.mu.Unlock()

	if !ok {
		c.sendJSON(map[string]interface{}{
			"type": "result",
			"id":   msg.ID,
			"ok":   false,
			"error": map[string]interface{}{
				"code":    "FUNCTION_NOT_FOUND",
				"message": fmt.Sprintf("Function %q is not registered", msg.Function),
			},
			"durationMs": time.Since(started).Milliseconds(),
		})
		return
	}

	data, err := entry.fn(msg.Args, msg.Context)
	if err != nil {
		c.sendJSON(map[string]interface{}{
			"type": "result",
			"id":   msg.ID,
			"ok":   false,
			"error": map[string]interface{}{
				"code":      "HANDLER_ERROR",
				"message":   err.Error(),
				"retryable": false,
			},
			"durationMs": time.Since(started).Milliseconds(),
		})
		return
	}
	c.sendJSON(map[string]interface{}{
		"type":       "result",
		"id":         msg.ID,
		"ok":         true,
		"data":       data,
		"durationMs": time.Since(started).Milliseconds(),
	})
}

func (c *Client) handleSend(raw []byte) {
	started := time.Now()
	var msg struct {
		ID       string                 `json:"id"`
		Channel  string                 `json:"channel"`
		To       string                 `json:"to"`
		Content  string                 `json:"content"`
		Metadata map[string]interface{} `json:"metadata"`
	}
	if err := json.Unmarshal(raw, &msg); err != nil {
		return
	}

	c.mu.Lock()
	handler := c.sendHandler
	c.mu.Unlock()

	if handler == nil {
		c.sendJSON(map[string]interface{}{
			"type": "result",
			"id":   msg.ID,
			"ok":   false,
			"error": map[string]interface{}{
				"code":    "NO_SEND_HANDLER",
				"message": "No OnSend handler is registered",
			},
			"durationMs": time.Since(started).Milliseconds(),
		})
		return
	}

	err := handler(OutboundDelivery{
		Channel:  msg.Channel,
		To:       msg.To,
		Content:  msg.Content,
		Metadata: msg.Metadata,
	})
	if err != nil {
		c.sendJSON(map[string]interface{}{
			"type": "result",
			"id":   msg.ID,
			"ok":   false,
			"error": map[string]interface{}{
				"code":      "SEND_FAILED",
				"message":   err.Error(),
				"retryable": true,
			},
			"durationMs": time.Since(started).Milliseconds(),
		})
		return
	}
	c.sendJSON(map[string]interface{}{
		"type":       "result",
		"id":         msg.ID,
		"ok":         true,
		"durationMs": time.Since(started).Milliseconds(),
	})
}

func (c *Client) sendJSON(payload map[string]interface{}) {
	c.mu.Lock()
	conn := c.conn
	c.mu.Unlock()
	if conn == nil {
		t, _ := payload["type"].(string)
		if t != "result" && t != "pong" {
			log.Printf("[aelio-sdk] not connected — dropped %q message", t)
		}
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil {
		return
	}
	_ = c.conn.WriteJSON(payload)
}

// Convenience wrappers on the Default client.

func Expose(name string, schema FunctionSchema, handler Handler) {
	Default.Expose(name, schema, handler)
}
func Persona(text string)                   { Default.Persona(text) }
func Describe(text string)                  { Default.Describe(text) }
func State(id string, schema StateSchema)   { Default.State(id, schema) }
func Policy(id string, schema PolicySchema) { Default.Policy(id, schema) }
func Flow(id string, schema FlowSchema)     { Default.Flow(id, schema) }
func OnSend(handler SendHandler)            { Default.OnSend(handler) }
func SetCustomerState(customerID, stateID, reason string) {
	Default.SetCustomerState(customerID, stateID, reason)
}
func Ingest(msg InboundIngest)        { Default.Ingest(msg) }
func Listen(opts ListenOptions) error { return Default.Listen(opts) }
func Disconnect()                     { Default.Disconnect() }
