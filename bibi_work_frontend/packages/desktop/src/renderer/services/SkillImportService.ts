import { httpFormRequest } from '@/common/adapter/httpBridge';

export interface SkillImportFailure {
  source_name: string;
  code: string;
  error_path?: string;
  actual_bytes?: number;
  limit_bytes?: number;
  line?: number;
  column?: number;
}

export interface SkillImportResult {
  skill_name: string;
  skill_names?: string[];
  failed?: SkillImportFailure[];
}

export async function uploadSkillZip(file: File, signal?: AbortSignal): Promise<SkillImportResult> {
  const form = new FormData();
  form.append('file', file, file.name);
  return httpFormRequest<SkillImportResult>('/api/skills/import-upload', form, { signal });
}
