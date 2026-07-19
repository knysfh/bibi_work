import { describe, expect, it } from 'vitest';
import { isAlwaysAllowConfirmationValue } from '@/renderer/pages/conversation/Messages/components/MessagePermission';

describe('MessagePermission confirmation values', () => {
  it('detects canonical and legacy always-allow values', () => {
    expect(isAlwaysAllowConfirmationValue('proceed_always')).toBe(true);
    expect(isAlwaysAllowConfirmationValue('proceed_always_server')).toBe(true);
    expect(isAlwaysAllowConfirmationValue('proceed_always_tool')).toBe(true);
    expect(isAlwaysAllowConfirmationValue('allow_always')).toBe(true);
  });

  it('does not mark one-shot or cancel values as always-allow', () => {
    expect(isAlwaysAllowConfirmationValue('proceed_once')).toBe(false);
    expect(isAlwaysAllowConfirmationValue('allow_once')).toBe(false);
    expect(isAlwaysAllowConfirmationValue('cancel')).toBe(false);
    expect(isAlwaysAllowConfirmationValue('deny')).toBe(false);
  });
});
