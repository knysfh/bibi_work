import { describe, expect, it } from 'vitest';
import { buildToolDisplayProjection, humanizeToolLabel } from '@/common/chat/toolDisplayProjection';

describe('toolDisplayProjection', () => {
  it('unwraps execution wrappers and hides sensitive fields', () => {
    const projection = buildToolDisplayProjection({
      args: [],
      kwargs: {
        query: 'coffee nearby',
        api_key: 'must-not-be-visible',
        options: { limit: 10 },
      },
    });

    expect(projection.summary).toBe('coffee nearby');
    expect(projection.inputFields).toEqual([
      { key: 'query', label: 'Query', value: 'coffee nearby', sensitive: false },
      { key: 'api_key', label: 'Api key', value: 'Hidden', sensitive: true },
      { key: 'options', label: 'Options', value: '1 fields', sensitive: false },
    ]);
  });

  it('turns structured output into a concise result while preserving raw data elsewhere', () => {
    const projection = buildToolDisplayProjection({ path: '/tmp/report.csv' }, '{"output_summary":"238 rows"}');
    expect(projection.resultSummary).toBe('238 rows');
  });

  it('humanizes provider and snake case names', () => {
    expect(humanizeToolLabel('mcp:google_maps.search_places')).toBe('Google maps search places');
  });
});
