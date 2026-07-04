ALTER TABLE tool_calls
ADD COLUMN IF NOT EXISTS error_summary TEXT;
