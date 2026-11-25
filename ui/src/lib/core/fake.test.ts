import { describe, expect, it } from 'vitest';
import { FakeCore } from './fake';
import type { CoreEvent } from './types';

const ALL = { network: '', archived: false, unreadOnly: false, query: '' };

describe('FakeCore', () => {
  it('offers exactly the five networks of this build, with chats on three of them', async () => {
    const core = new FakeCore();
    const ids = (await core.networks()).map((n) => n.id);
    expect(ids).toEqual(['whatsapp', 'telegram', 'discord', 'fluxer', 'sms']);
    const chats = await core.listChats({ filter: { ...ALL, archived: false } });
    const networks = new Set(chats.map((c) => c.network));
    expect(chats.length).toBeGreaterThanOrEqual(6);
    expect(networks.size).toBeGreaterThanOrEqual(3);
    expect([...networks].every((id) => ids.includes(id))).toBe(true);
  });

  it('refuses to log in to a network this build does not offer', async () => {
    const core = new FakeCore();
    await expect(core.startLogin({ network: 'signal', flowId: 'qr' })).rejects.toMatchObject({ code: 'unknown_network' });
  });

  it('walks a phone login: user_input, user_input, complete, then an account event', async () => {
    const core = new FakeCore();
    const events: CoreEvent[] = [];
    core.onEvents((batch) => events.push(...batch));

    let state = await core.startLogin({ network: 'telegram', flowId: 'phone' });
    expect(state.step.type).toBe('user_input');
    state = await core.submitLoginInput({ processId: state.processId, values: { phone: '+15550100' } });
    expect(state.step.type).toBe('user_input');
    state = await core.submitLoginInput({ processId: state.processId, values: { code: '12345' } });
    expect(state.step.type).toBe('complete');

    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(events.some((e) => e.kind === 'account' && e.payload.network === 'telegram' && e.payload.state === 'CONNECTED')).toBe(true);
  });

  it('rejects a step that is not the current one', async () => {
    const core = new FakeCore();
    const state = await core.startLogin({ network: 'telegram', flowId: 'phone' });
    await expect(core.submitLoginCookies({ processId: state.processId, cookies: {} })).rejects.toMatchObject({ code: 'invalid_step' });
  });

  it('refuses to open non-web links', async () => {
    const core = new FakeCore();
    await expect(core.openExternal({ url: 'file:///etc/passwd' })).rejects.toMatchObject({ code: 'invalid_url' });
  });
});
