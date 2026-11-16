package main

import (
	"sync"

	"chirper.dev/core/facade"
)

// entry is one open core and the relay that forwards its events.
type entry struct {
	core     *facade.Core
	listener *eventRelay
}

var (
	tableMu    sync.Mutex
	table      = map[int64]*entry{}
	lastHandle int64
)

// insert stores e and returns its handle. Handles start at 1 and are never reused.
func insert(e *entry) int64 {
	tableMu.Lock()
	defer tableMu.Unlock()
	lastHandle++
	table[lastHandle] = e
	return lastHandle
}

// lookup returns the open entry of handle h.
func lookup(h int64) (*entry, bool) {
	tableMu.Lock()
	defer tableMu.Unlock()
	e, ok := table[h]
	return e, ok
}

// remove deletes the entry of handle h so that it can no longer be looked up.
func remove(h int64) (*entry, bool) {
	tableMu.Lock()
	defer tableMu.Unlock()
	e, ok := table[h]
	delete(table, h)
	return e, ok
}
