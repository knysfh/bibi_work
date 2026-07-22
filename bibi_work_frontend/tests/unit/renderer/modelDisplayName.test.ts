import { describe, expect, it } from 'vitest';
import type { IProvider } from '@/common/config/storage';
import { findConfiguredModelDisplayName, getConfiguredModelDisplayName } from '@/renderer/utils/model/modelDisplayName';

const provider = {
  id: 'provider-1',
  platform: 'openai',
  name: 'Luna',
  base_url: 'https://example.test/v1',
  api_key: '',
  models: ['gpt-wire-name'],
  model_labels: { 'gpt-wire-name': 'Luna Chat' },
  model_profile_ids: { 'gpt-wire-name': '11111111-1111-1111-1111-111111111111' },
} satisfies IProvider;

describe('model display name resolution', () => {
  it('resolves provider model ids, compatibility tags, and profile UUIDs to the user-facing name', () => {
    expect(findConfiguredModelDisplayName([provider], 'gpt-wire-name')).toBe('Luna Chat');
    expect(findConfiguredModelDisplayName([provider], 'provider-1:gpt-wire-name')).toBe('Luna Chat');
    expect(findConfiguredModelDisplayName([provider], '11111111-1111-1111-1111-111111111111')).toBe('Luna Chat');
  });

  it('keeps unknown runtime model identifiers unchanged', () => {
    expect(getConfiguredModelDisplayName([provider], 'runtime-model')).toBe('runtime-model');
  });
});
