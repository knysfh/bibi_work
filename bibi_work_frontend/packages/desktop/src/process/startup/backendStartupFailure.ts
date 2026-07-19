/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import type { BackendStartupFailureInfo } from '@/common/types/platform/electron';

type ErrorWithDetails = Error & {
  details?: {
    stage?: unknown;
    isPackaged?: unknown;
    causeMessage?: unknown;
    stderrTail?: unknown;
    stdoutTail?: unknown;
    backendBoundaryCode?: unknown;
    backendBoundaryStage?: unknown;
    packageArch?: unknown;
    deviceArch?: unknown;
    expectedDownloadArch?: unknown;
    isRosettaTranslated?: unknown;
  };
};

const GLIBC_VERSION_RE = /GLIBC_(\d+\.\d+)/g;
const GLIBC_NOT_FOUND_RE = /GLIBC_\d+\.\d+[\s\S]{0,160}not found|not found[\s\S]{0,160}GLIBC_\d+\.\d+/i;
const DATA_MIGRATION_BOUNDARY_STAGES = new Set(['database.migration', 'database.schema_repair']);
const RECOVERABLE_DATABASE_CORRUPTION_BOUNDARY_STAGE = 'database.recoverable_corruption';
const LOCAL_DATA_REPAIR_BOUNDARY_CODE = 'BOOTSTRAP_SERVICE_INIT_FAILED';
const LOCAL_DATA_REPAIR_BOUNDARY_STAGE = 'services.init';
const LOAD_AGENT_METADATA_RE = /\bload agent_metadata\b/i;
const DATABASE_QUERY_FAILED_RE = /\bDatabase query failed\b/i;
const INVALID_UTF8_RE = /\binvalid utf-?8\b/i;
const AGENT_METADATA_CACHE_FIELD_RE =
  /\b(agent_capabilities|auth_methods|config_options|available_modes|available_models|available_commands)\b/i;
const STARTUP_DIRECTORY_FAILURE_STAGES = new Set(['spawn']);
const STARTUP_DIRECTORY_PERMISSION_RE = /\b(?:EACCES|EPERM)\b|permission denied|operation not permitted/i;
const STARTUP_DIRECTORY_UNAVAILABLE_RE =
  /startup directory preparation failed|(?:\b(?:ENOENT|ENOTDIR|EEXIST)\b[\s\S]{0,160}\bmkdir\b)|(?:\bmkdir\b[\s\S]{0,160}\b(?:ENOENT|ENOTDIR|EEXIST)\b)/i;

function collectBackendStartupText(error: unknown): string {
  const parts: string[] = [];
  if (error instanceof Error) parts.push(error.message);
  if (typeof error === 'string') parts.push(error);

  const details = (error as ErrorWithDetails | undefined)?.details;
  for (const value of [details?.causeMessage, details?.stderrTail, details?.stdoutTail]) {
    if (typeof value === 'string') parts.push(value);
  }

  return parts.join('\n');
}

function extractMissingGlibcVersions(text: string): string[] {
  if (!GLIBC_NOT_FOUND_RE.test(text)) return [];

  const versions = new Set<string>();
  for (const match of text.matchAll(GLIBC_VERSION_RE)) {
    versions.add(match[1]);
  }

  return [...versions].toSorted((a, b) => {
    const [aMajor, aMinor] = a.split('.').map(Number);
    const [bMajor, bMinor] = b.split('.').map(Number);
    return aMajor - bMajor || aMinor - bMinor;
  });
}

function getBackendStartupDetails(error: unknown): ErrorWithDetails['details'] | undefined {
  return (error as ErrorWithDetails | undefined)?.details;
}

function getString(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function classifyPackageArchitectureMismatch(
  details: ErrorWithDetails['details']
): BackendStartupFailureInfo | undefined {
  if (!details) return undefined;
  if (details.stage !== 'startup_architecture_check') return undefined;

  return {
    reason: 'backend_package_architecture_mismatch',
    packageArch: getString(details.packageArch),
    deviceArch: getString(details.deviceArch),
    expectedDownloadArch: getString(details.expectedDownloadArch),
    isRosettaTranslated: typeof details.isRosettaTranslated === 'boolean' ? details.isRosettaTranslated : undefined,
  };
}

function classifyLocalDataRepairFailure(
  backendBoundaryCode: string | undefined,
  backendBoundaryStage: string | undefined,
  text: string
): BackendStartupFailureInfo | undefined {
  if (backendBoundaryCode !== LOCAL_DATA_REPAIR_BOUNDARY_CODE) return undefined;
  if (backendBoundaryStage !== LOCAL_DATA_REPAIR_BOUNDARY_STAGE) return undefined;
  if (!LOAD_AGENT_METADATA_RE.test(text)) return undefined;
  if (!DATABASE_QUERY_FAILED_RE.test(text)) return undefined;
  if (!INVALID_UTF8_RE.test(text)) return undefined;
  if (!AGENT_METADATA_CACHE_FIELD_RE.test(text)) return undefined;

  return {
    reason: 'backend_local_data_repair_failed',
    backendBoundaryCode,
    backendBoundaryStage,
    localDataIssueKind: 'agent_metadata_invalid_utf8',
  };
}

function classifyStartupDirectoryFailure(
  details: ErrorWithDetails['details'],
  text: string
): BackendStartupFailureInfo | undefined {
  if (!details || typeof details.stage !== 'string') return undefined;
  if (!STARTUP_DIRECTORY_FAILURE_STAGES.has(details.stage)) return undefined;

  if (STARTUP_DIRECTORY_PERMISSION_RE.test(text)) {
    return {
      reason: 'backend_startup_directory_unavailable',
      startupDirectoryIssueKind: 'permission_denied',
    };
  }

  if (STARTUP_DIRECTORY_UNAVAILABLE_RE.test(text)) {
    return {
      reason: 'backend_startup_directory_unavailable',
      startupDirectoryIssueKind: 'missing_or_unavailable_directory',
    };
  }

  return undefined;
}

export function classifyBackendStartupFailure(error: unknown): BackendStartupFailureInfo {
  const details = getBackendStartupDetails(error);
  const packageArchitectureMismatch = classifyPackageArchitectureMismatch(details);
  if (packageArchitectureMismatch) return packageArchitectureMismatch;

  const text = collectBackendStartupText(error);
  const startupDirectoryFailure = classifyStartupDirectoryFailure(details, text);
  if (startupDirectoryFailure) return startupDirectoryFailure;

  const requiredVersions = extractMissingGlibcVersions(text);
  if (requiredVersions.length > 0) {
    return {
      reason: 'backend_incompatible_runtime',
      runtime: 'glibc',
      requiredVersions,
    };
  }

  const backendBoundaryCode =
    typeof details?.backendBoundaryCode === 'string' ? details.backendBoundaryCode : undefined;
  const backendBoundaryStage =
    typeof details?.backendBoundaryStage === 'string' ? details.backendBoundaryStage : undefined;

  const localDataRepairFailure = classifyLocalDataRepairFailure(backendBoundaryCode, backendBoundaryStage, text);
  if (localDataRepairFailure) return localDataRepairFailure;

  if (
    backendBoundaryCode === 'BOOTSTRAP_DATA_INIT_FAILED' &&
    backendBoundaryStage === RECOVERABLE_DATABASE_CORRUPTION_BOUNDARY_STAGE
  ) {
    return {
      reason: 'backend_recoverable_database_corruption',
      backendBoundaryCode,
      backendBoundaryStage,
    };
  }

  if (
    backendBoundaryCode === 'BOOTSTRAP_DATA_INIT_FAILED' &&
    backendBoundaryStage &&
    DATA_MIGRATION_BOUNDARY_STAGES.has(backendBoundaryStage)
  ) {
    return {
      reason: 'backend_data_migration_failed',
      backendBoundaryCode,
      backendBoundaryStage,
    };
  }

  return {
    reason: 'backend_startup_failed',
    backendBoundaryCode,
    backendBoundaryStage,
  };
}
