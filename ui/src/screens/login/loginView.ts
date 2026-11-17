import type { InputField, LoginStep } from '../../lib/core';

export type InputKind = 'text' | 'tel' | 'email' | 'password';
export type InputMode = 'text' | 'tel' | 'email' | 'numeric';
export type Autocomplete = 'tel' | 'email' | 'current-password' | 'one-time-code' | 'username' | 'off';

/** How one connector input field is shown. `options` non-empty means a choice, not free text. */
export interface FieldView {
  id: string;
  label: string;
  input: InputKind;
  inputmode: InputMode;
  autocomplete: Autocomplete;
  options: readonly string[];
  pattern: string;
}

/** What the add-network screen shows for one LoginState step. */
export type LoginView =
  | { kind: 'form'; stepId: string; instructions: string; fields: readonly FieldView[] }
  | { kind: 'qr'; instructions: string; data: string }
  | { kind: 'code'; instructions: string; text: string; emoji: boolean }
  | { kind: 'waiting'; instructions: string }
  | {
      kind: 'cookies';
      instructions: string;
      url: string;
      extractJs: string;
      waitForUrlPattern?: string | undefined;
      userAgent: string;
    }
  | { kind: 'complete'; loginId: string }
  | { kind: 'unsupported' };

/** Maps one LoginState step to the view it needs. Pure, so the rules are unit-tested. */
export function viewForStep(step: LoginStep): LoginView {
  switch (step.type) {
    case 'user_input':
      return {
        kind: 'form',
        stepId: step.stepId,
        instructions: step.instructions,
        fields: (step.userInput?.fields ?? []).map(fieldView),
      };
    case 'display_and_wait':
      return displayView(step);
    case 'cookies':
      return {
        kind: 'cookies',
        instructions: step.instructions,
        url: step.cookies?.url ?? '',
        extractJs: step.cookies?.extractJs ?? '',
        waitForUrlPattern: step.cookies?.waitForUrlPattern,
        userAgent: step.cookies?.userAgent ?? '',
      };
    case 'complete':
      return { kind: 'complete', loginId: step.complete?.loginId ?? '' };
    default:
      return { kind: 'unsupported' };
  }
}

function displayView(step: LoginStep): LoginView {
  const display = step.displayAndWait;
  switch (display?.type) {
    case 'qr':
      return { kind: 'qr', instructions: step.instructions, data: display.data };
    case 'emoji':
      return { kind: 'code', instructions: step.instructions, text: display.data, emoji: true };
    case 'code':
      return { kind: 'code', instructions: step.instructions, text: display.data, emoji: false };
    default:
      return { kind: 'waiting', instructions: step.instructions };
  }
}

/** Keyboard, autofill and input rules for a connector field type. */
export function fieldView(field: InputField): FieldView {
  return {
    id: field.id,
    label: field.name || field.id,
    ...inputFor(field.type),
    options: field.options,
    pattern: field.pattern,
  };
}

function inputFor(type: string): Pick<FieldView, 'input' | 'inputmode' | 'autocomplete'> {
  switch (type) {
    case 'phone':
    case 'phone_number':
      return { input: 'tel', inputmode: 'tel', autocomplete: 'tel' };
    case 'email':
      return { input: 'email', inputmode: 'email', autocomplete: 'email' };
    case 'password':
      return { input: 'password', inputmode: 'text', autocomplete: 'current-password' };
    case '2fa_code':
    case 'code':
    case 'otp':
      return { input: 'text', inputmode: 'numeric', autocomplete: 'one-time-code' };
    case 'username':
      return { input: 'text', inputmode: 'text', autocomplete: 'username' };
    default:
      return { input: 'text', inputmode: 'text', autocomplete: 'off' };
  }
}

/**
 * Validation before a form is submitted. Required fields must not be empty; a connector pattern is
 * matched against the whole value. Returns one message per invalid field id.
 */
export function fieldErrors(fields: readonly FieldView[], values: Readonly<Record<string, string>>): Record<string, string> {
  const errors: Record<string, string> = {};
  for (const field of fields) {
    const value = (values[field.id] ?? '').trim();
    if (value === '') errors[field.id] = 'Required';
    else if (field.pattern !== '' && !matchesPattern(field.pattern, value)) errors[field.id] = 'Check the format';
  }
  return errors;
}

function matchesPattern(pattern: string, value: string): boolean {
  try {
    return new RegExp(`^(?:${pattern})$`).test(value);
  } catch {
    // A pattern the UI cannot compile is left to the connector to judge.
    return true;
  }
}

/** The one word shown on the banner for a view ("Scan", "Confirm", ...). */
export function statusWord(view: LoginView): string {
  switch (view.kind) {
    case 'form':
      return 'Login';
    case 'qr':
      return 'Scan';
    case 'code':
      return 'Confirm';
    case 'waiting':
      return 'Waiting';
    case 'cookies':
      return 'Browser';
    case 'complete':
      return 'Connected';
    case 'unsupported':
      return 'Unsupported';
  }
}
