package engine

import (
	"context"
	"sync"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// undoOp reverts an optimistic write made for an outgoing event, once the network rejects it. finish,
// when set, makes the write final once the network accepts it. Work that cannot be undone, such as
// deleting media files, belongs in finish.
type undoOp struct {
	roomID id.RoomID
	target id.EventID
	undo   func(context.Context) error
	finish func(context.Context) error
}

// pendingOps tracks outgoing events whose optimistic write must be undone on failure. Message
// sends need no entry: the bridge already records their status.
type pendingOps struct {
	mu  sync.Mutex
	ops map[id.EventID]*undoOp
}

func (p *pendingOps) add(eventID id.EventID, op *undoOp) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.ops[eventID] = op
}

func (p *pendingOps) take(eventID id.EventID) *undoOp {
	p.mu.Lock()
	defer p.mu.Unlock()
	op := p.ops[eventID]
	delete(p.ops, eventID)
	return op
}

// settle is called with the network's verdict on an outgoing event. Pending and retriable
// verdicts keep the write; success keeps it and finishes it; anything else undoes it.
func (e *Engine) settle(eventID id.EventID, status event.MessageStatus) {
	if status == event.MessageStatusPending || status == event.MessageStatusRetriable {
		return
	}
	op := e.pending.take(eventID)
	if op == nil {
		return
	}
	if status == event.MessageStatusSuccess {
		if op.finish != nil {
			e.goSafe(func() { e.finish(op) })
		}
		return
	}
	e.goSafe(func() { e.undo(op) })
}

// finish makes op final after the network accepted it. A failure is logged: the write stays in place.
func (e *Engine) finish(op *undoOp) {
	if err := op.finish(e.ctx); err != nil {
		e.log.Error().Err(err).Msg("Failed to finish optimistic write")
	}
}

// undo reverts op and refreshes the views of the message and its chat.
func (e *Engine) undo(op *undoOp) {
	if err := op.undo(e.ctx); err != nil {
		e.log.Error().Err(err).Msg("Failed to undo optimistic write")
	}
	e.queueMessage(op.target, false)
	e.queueChat(op.roomID)
}
