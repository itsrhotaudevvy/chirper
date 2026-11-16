import { describe, expect, it } from 'vitest';
import type { LoginStep } from '../../lib/core';
import { fieldErrors, fieldView, statusWord, viewForStep } from './loginView';

const base = { stepId: 's', instructions: 'Do it' };

describe('viewForStep', () => {
  it('shows form fields for user_input steps', () => {
    const step: LoginStep = {
      ...base,
      type: 'user_input',
      userInput: { fields: [{ id: 'phone', name: 'Phone', type: 'phone_number', description: '', options: [], pattern: '' }] },
    };
    const view = viewForStep(step);
    expect(view.kind).toBe('form');
    if (view.kind !== 'form') return;
    expect(view.stepId).toBe('s');
    expect(view.fields).toEqual([
      expect.objectContaining({ id: 'phone', label: 'Phone', input: 'tel', inputmode: 'tel', autocomplete: 'tel' }),
    ]);
  });

  it('turns a QR display into a qr view with its data', () => {
    const view = viewForStep({ ...base, type: 'display_and_wait', displayAndWait: { type: 'qr', data: '2@abc', imageUrl: '' } });
    expect(view).toEqual({ kind: 'qr', instructions: 'Do it', data: '2@abc' });
  });

  it('shows emoji and codes as huge text, flagged by kind', () => {
    const emoji = viewForStep({ ...base, type: 'display_and_wait', displayAndWait: { type: 'emoji', data: '🐱🌵', imageUrl: '' } });
    const code = viewForStep({ ...base, type: 'display_and_wait', displayAndWait: { type: 'code', data: '4821', imageUrl: '' } });
    expect(emoji).toEqual({ kind: 'code', instructions: 'Do it', text: '🐱🌵', emoji: true });
    expect(code).toEqual({ kind: 'code', instructions: 'Do it', text: '4821', emoji: false });
  });

  it('treats a display with nothing to show as waiting', () => {
    expect(viewForStep({ ...base, type: 'display_and_wait', displayAndWait: { type: 'nothing', data: '', imageUrl: '' } }).kind).toBe('waiting');
    expect(viewForStep({ ...base, type: 'display_and_wait' }).kind).toBe('waiting');
  });

  it('maps cookies and complete steps', () => {
    expect(viewForStep({ ...base, type: 'cookies', cookies: { url: 'https://x.test', userAgent: '', fields: [], extractJs: '' } })).toEqual({
      kind: 'cookies',
      instructions: 'Do it',
      url: 'https://x.test',
      extractJs: '',
      waitForUrlPattern: undefined,
      userAgent: '',
    });
    expect(viewForStep({ ...base, type: 'complete', complete: { loginId: 'acc-1' } })).toEqual({ kind: 'complete', loginId: 'acc-1' });
  });

  it('reports an unknown step type as unsupported', () => {
    expect(viewForStep({ ...base, type: 'mystery' as LoginStep['type'] }).kind).toBe('unsupported');
  });
});

describe('fieldView', () => {
  it.each([
    ['phone_number', 'tel', 'tel', 'tel'],
    ['email', 'email', 'email', 'email'],
    ['password', 'password', 'text', 'current-password'],
    ['2fa_code', 'text', 'numeric', 'one-time-code'],
    ['username', 'text', 'text', 'username'],
    ['token', 'text', 'text', 'off'],
  ])('maps connector type %s to input %s, inputmode %s, autocomplete %s', (type, input, inputmode, autocomplete) => {
    const view = fieldView({ id: 'f', name: '', type, description: '', options: [], pattern: '' });
    expect(view).toEqual(expect.objectContaining({ input, inputmode, autocomplete, label: 'f' }));
  });

  it('keeps options so the field can be a choice', () => {
    expect(fieldView({ id: 'f', name: 'Server', type: 'select', description: '', options: ['a', 'b'], pattern: '' }).options).toEqual(['a', 'b']);
  });
});

describe('fieldErrors', () => {
  const fields = [
    fieldView({ id: 'phone', name: 'Phone', type: 'phone', description: '', options: [], pattern: '\\+[0-9]+' }),
    fieldView({ id: 'code', name: 'Code', type: 'code', description: '', options: [], pattern: '' }),
  ];

  it('requires every field and checks the connector pattern', () => {
    expect(fieldErrors(fields, { phone: '', code: '1' })).toEqual({ phone: 'Required' });
    expect(fieldErrors(fields, { phone: '0123', code: '1' })).toEqual({ phone: 'Check the format' });
  });

  it('is empty when every value is valid', () => {
    expect(fieldErrors(fields, { phone: '+358', code: '123456' })).toEqual({});
  });
});

describe('statusWord', () => {
  it('gives a single word for each view', () => {
    expect(statusWord({ kind: 'qr', instructions: '', data: '' })).toBe('Scan');
    expect(statusWord({ kind: 'complete', loginId: '' })).toBe('Connected');
    for (const view of [
      { kind: 'form', stepId: '', instructions: '', fields: [] },
      { kind: 'code', instructions: '', text: '', emoji: false },
      { kind: 'waiting', instructions: '' },
      { kind: 'cookies', instructions: '', url: '', extractJs: '', userAgent: '' },
      { kind: 'unsupported' },
    ] as const) {
      expect(statusWord(view).split(' ')).toHaveLength(1);
    }
  });
});
