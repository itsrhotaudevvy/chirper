package engine

import (
	"fmt"
	"sync"
	"time"
)

// coalesceWindow is how long the dispatcher waits after the first pending change before it
// delivers, so that a burst of updates for one chat becomes a single event.
const coalesceWindow = 50 * time.Millisecond

// Listener receives the events of the engine. kind and payload are described in ARCHITECTURE.md.
// It may be slow: events are delivered from one goroutine, never while holding store locks.
type Listener interface {
	OnEvent(kind string, payload string)
}

// builder produces the payload of a pending event when it is delivered, so the event carries the
// latest state. ok is false when there is nothing to deliver.
type builder func() (kind, payload string, ok bool)

type pendingEvent struct {
	key   string
	build builder
}

// dispatcher queues events and delivers them asynchronously. Pending events are keyed: a second
// event with the same key replaces the first, so the queue is bounded by the number of rooms and
// messages that changed.
type dispatcher struct {
	listener Listener
	// onFail receives a panic raised while building or delivering one event, so that a single bad
	// event does not stop delivery. It may be nil, in which case such panics are dropped.
	onFail func(error)

	mu      sync.Mutex
	pending []*pendingEvent
	index   map[string]*pendingEvent

	wake chan struct{}
	done chan struct{}
	once sync.Once
}

func newDispatcher(listener Listener) *dispatcher {
	return &dispatcher{
		listener: listener,
		index:    make(map[string]*pendingEvent),
		wake:     make(chan struct{}, 1),
		done:     make(chan struct{}),
	}
}

// queue schedules build under key. An empty key is never coalesced.
func (d *dispatcher) queue(key string, build builder) {
	d.mu.Lock()
	if existing, ok := d.index[key]; key != "" && ok {
		existing.build = build
	} else {
		ev := &pendingEvent{key: key, build: build}
		d.pending = append(d.pending, ev)
		if key != "" {
			d.index[key] = ev
		}
	}
	d.mu.Unlock()
	select {
	case d.wake <- struct{}{}:
	default:
	}
}

// emit schedules a fixed event.
func (d *dispatcher) emit(kind, payload string) {
	d.queue("", func() (string, string, bool) { return kind, payload, true })
}

// run delivers pending events until stop is called.
func (d *dispatcher) run() {
	for {
		select {
		case <-d.done:
			return
		case <-d.wake:
		}
		select {
		case <-d.done:
			return
		case <-time.After(coalesceWindow):
		}
		d.mu.Lock()
		batch := d.pending
		d.pending = nil
		d.index = make(map[string]*pendingEvent)
		d.mu.Unlock()
		for _, ev := range batch {
			d.deliver(ev)
		}
	}
}

// deliver builds and hands over one event. A panic is recovered and passed to onFail.
func (d *dispatcher) deliver(ev *pendingEvent) {
	defer func() {
		if v := recover(); v != nil && d.onFail != nil {
			d.onFail(fmt.Errorf("internal error: %v", v))
		}
	}()
	if kind, payload, ok := ev.build(); ok {
		d.listener.OnEvent(kind, payload)
	}
}

// stop ends the delivery loop. Events still pending are dropped.
func (d *dispatcher) stop() {
	d.once.Do(func() { close(d.done) })
}
