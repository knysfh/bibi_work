ALTER TABLE llm_model_profiles
    ADD COLUMN display_name TEXT;

COMMENT ON COLUMN llm_model_profiles.display_name IS
    'User-facing model name. model_name remains the immutable provider API identifier.';
