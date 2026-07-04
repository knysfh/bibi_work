import {
  dtoList,
  fileListDtoSchema,
  fileRevisionDtoSchema,
  mapFileList,
  mapFileRevision,
  mapResource,
  mapToolResultArtifactRead,
  resourceDtoSchema,
  toolResultArtifactReadDtoSchema,
  type FileList,
  type FileRevision,
  type Resource,
  type ToolResultArtifactRead
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";

export interface ProjectApi {
  listProjects(tenantId: string, limit?: number): Promise<Resource[]>;
  createProject(input: { tenantId: string; name: string; description?: string }): Promise<Resource>;
  listFiles(input: {
    tenantId: string;
    projectId: string;
    prefix?: string;
    pattern?: string;
  }): Promise<FileList>;
  readFile(input: {
    tenantId: string;
    projectId: string;
    path: string;
    revision?: number;
    versionId?: string;
    includeContent?: boolean;
    allowBinary?: boolean;
  }): Promise<FileRevision>;
  searchFiles(input: {
    tenantId: string;
    projectId: string;
    query: string;
    prefix?: string;
    limit?: number;
  }): Promise<FileList>;
  listFileHistory(input: {
    tenantId: string;
    projectId: string;
    path: string;
    limit?: number;
  }): Promise<FileList>;
  listArtifacts(input: { tenantId: string; projectId: string; runId?: string }): Promise<FileList>;
  readToolResultArtifact(input: {
    tenantId: string;
    objectReferenceId: string;
    offset?: number;
    limit?: number;
  }): Promise<ToolResultArtifactRead>;
}

export function createProjectApi(http: HttpClient): ProjectApi {
  return {
    async listProjects(tenantId, limit = 100) {
      return (
        await http.get("/projects", dtoList(resourceDtoSchema), {
          query: { tenant_id: tenantId, limit }
        })
      ).map(mapResource);
    },
    async createProject(input) {
      return mapResource(
        await http.post(
          "/projects",
          {
            tenant_id: input.tenantId,
            name: input.name,
            description: input.description
          },
          resourceDtoSchema
        )
      );
    },
    async listFiles(input) {
      return mapFileList(
        await http.get(`/projects/${input.projectId}/files`, fileListDtoSchema, {
          query: {
            tenant_id: input.tenantId,
            prefix: input.prefix,
            pattern: input.pattern
          }
        })
      );
    },
    async readFile(input) {
      return mapFileRevision(
        await http.get(`/projects/${input.projectId}/files/read`, fileRevisionDtoSchema, {
          query: {
            tenant_id: input.tenantId,
            path: input.path,
            revision: input.revision,
            version_id: input.versionId,
            include_content: input.includeContent ?? true,
            allow_binary: input.allowBinary
          }
        })
      );
    },
    async searchFiles(input) {
      return mapFileList(
        await http.post(
          `/projects/${input.projectId}/files:search`,
          {
            tenant_id: input.tenantId,
            query: input.query,
            prefix: input.prefix,
            limit: input.limit
          },
          fileListDtoSchema
        )
      );
    },
    async listFileHistory(input) {
      return mapFileList(
        await http.get(`/projects/${input.projectId}/files/history`, fileListDtoSchema, {
          query: {
            tenant_id: input.tenantId,
            path: input.path,
            limit: input.limit ?? 50
          }
        })
      );
    },
    async listArtifacts(input) {
      return mapFileList(
        await http.get(`/projects/${input.projectId}/artifacts`, fileListDtoSchema, {
          query: { tenant_id: input.tenantId, run_id: input.runId }
        })
      );
    },
    async readToolResultArtifact(input) {
      return mapToolResultArtifactRead(
        await http.get("/tool-result-artifacts/read", toolResultArtifactReadDtoSchema, {
          query: {
            tenant_id: input.tenantId,
            object_reference_id: input.objectReferenceId,
            offset: input.offset,
            limit: input.limit
          }
        })
      );
    }
  };
}
