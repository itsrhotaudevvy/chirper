// Chirper - core engine.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package engine

import (
	"context"
	"sync"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"

	"chirper.dev/core/store"
)

// SyncHistory queues a backfill of past messages for every chat of network (all networks when the
// network is empty). Chats are filled in the background by the backfill queue, so the inbox shows
// real history for conversations the user has not opened yet.
func (e *Engine) SyncHistory(network string) error {
	specs := e.enabledSpecs()
	var wg sync.WaitGroup
	for _, spec := range specs {
		if network != "" && spec.id != network {
			continue
		}
		inst, err := e.instance(spec)
		if err != nil {
			e.reportError(err)
			continue
		}
		if !inst.isStarted() {
			continue
		}
		wg.Add(1)
		e.goSafe(func() {
			defer wg.Done()
			e.backfillInstance(inst)
		})
	}
	wg.Wait()
	return nil
}

// backfillInstance queues every portal of bridge inst that has no history yet.
func (e *Engine) backfillInstance(inst *bridgeInstance) {
	ctx, cancel := context.WithTimeout(e.ctx, syncHistoryTimeout)
	defer cancel()
	portals, err := inst.br.DB.Portal.GetAll(ctx)
	if err != nil {
		e.reportError(err)
		return
	}
	for _, stored := range portals {
		if stored == nil || stored.MXID == "" || stored.RoomType == database.RoomTypeSpace {
			continue
		}
		var login *bridgev2.UserLogin
		if stored.Receiver != "" {
			login, _ = inst.br.GetExistingUserLoginByID(ctx, stored.Receiver)
		}
		portal, err := inst.br.GetPortalByKey(ctx, stored.PortalKey)
		if err != nil || portal == nil {
			continue
		}
		if login == nil {
			if user, err := inst.br.GetUserByMXID(ctx, store.SelfUserID); err == nil && user != nil {
				login, _, _ = portal.FindPreferredLogin(ctx, user, true)
				if login == nil {
					login = user.GetDefaultLogin()
				}
			}
		}
		if login == nil {
			continue
		}
		inst.br.WakeupBackfillQueue(&bridgev2.ManualBackfill{Source: login, Portal: portal})
	}
}
