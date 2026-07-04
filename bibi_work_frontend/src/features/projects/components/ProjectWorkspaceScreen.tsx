import {
  Archive,
  Binary,
  Clock3,
  FileCode2,
  FileText,
  Folder,
  Plus,
  RotateCcw,
  X
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { FileEntry, FileList, FileRevision, Me } from "../../../shared/contracts/platform";
import { languageLocale, useI18n, type I18nKey } from "../../../shared/i18n";
import {
  Badge,
  Button,
  EmptyState,
  Panel,
  ResourceList,
  StatusPill,
  Tabs,
  TextInput
} from "../../../shared/ui";
import {
  useCreateProjectMutation,
  useProjectArtifactsQuery,
  useProjectFileHistoryQuery,
  useProjectFileQuery,
  useProjectFileSearchQuery,
  useProjectFilesQuery,
  useProjectsQuery
} from "../api/project.queries";

type FileTab = "tree" | "search" | "artifacts";

interface SelectedFile {
  path: string;
  revision?: number;
  versionId?: string;
}

const fileTabs = [
  { id: "tree", labelKey: "project.tab.tree" },
  { id: "search", labelKey: "project.tab.search" },
  { id: "artifacts", labelKey: "project.tab.artifacts" }
] satisfies Array<{ id: FileTab; labelKey: I18nKey }>;

export function ProjectWorkspaceScreen({ me }: { me: Me }) {
  const { t } = useI18n();
  const projects = useProjectsQuery(me.tenantId);
  const createProject = useCreateProjectMutation(me.tenantId);
  const [selectedProjectId, setSelectedProjectId] = useState<string | undefined>();
  const [selectedFile, setSelectedFile] = useState<SelectedFile | undefined>();
  const [newProjectName, setNewProjectName] = useState("");
  const [activeTab, setActiveTab] = useState<FileTab>("tree");
  const [prefix, setPrefix] = useState("/");
  const [pattern, setPattern] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [artifactRunId, setArtifactRunId] = useState("");
  const [allowBinary, setAllowBinary] = useState(false);

  useEffect(() => {
    if (!selectedProjectId && projects.data?.[0]) {
      setSelectedProjectId(projects.data[0].id);
    }
  }, [projects.data, selectedProjectId]);

  const selectedProject = projects.data?.find((project) => project.id === selectedProjectId);
  const files = useProjectFilesQuery(me.tenantId, selectedProjectId, prefix, pattern);
  const searchResults = useProjectFileSearchQuery(
    me.tenantId,
    selectedProjectId,
    searchQuery,
    prefix
  );
  const artifacts = useProjectArtifactsQuery(me.tenantId, selectedProjectId, artifactRunId);
  const file = useProjectFileQuery(
    me.tenantId,
    selectedProjectId,
    selectedFile?.path,
    selectedFile?.revision,
    selectedFile?.versionId,
    allowBinary
  );
  const history = useProjectFileHistoryQuery(me.tenantId, selectedProjectId, selectedFile?.path);

  const activeList = useMemo(() => {
    if (activeTab === "search") {
      return searchResults.data;
    }
    if (activeTab === "artifacts") {
      return artifacts.data;
    }
    return files.data;
  }, [activeTab, artifacts.data, files.data, searchResults.data]);

  async function submitProject() {
    const name = newProjectName.trim();
    if (!name) {
      return;
    }
    const created = await createProject.mutateAsync({ name });
    setNewProjectName("");
    setSelectedProjectId(created.id);
    setSelectedFile(undefined);
    setPrefix("/");
  }

  function selectProject(projectId: string) {
    setSelectedProjectId(projectId);
    setSelectedFile(undefined);
    setPrefix("/");
    setPattern("");
    setSearchQuery("");
    setArtifactRunId("");
    setAllowBinary(false);
  }

  function selectFile(next: SelectedFile) {
    setSelectedFile(next);
    setAllowBinary(false);
  }

  return (
    <div className="project-grid">
      <Panel
        className="workbench-list project-list-panel"
        title={t("project.projects")}
        subtitle={t("common.projectCount", { count: projects.data?.length ?? 0 })}
      >
        <div className="inline-create">
          <TextInput
            value={newProjectName}
            onChange={(event) => setNewProjectName(event.target.value)}
            placeholder={t("project.projectName")}
          />
          <Button
            size="sm"
            variant="secondary"
            aria-label={t("project.createProject")}
            title={t("project.createProject")}
            icon={<Plus size={16} />}
            onClick={submitProject}
            disabled={createProject.isPending || !newProjectName.trim()}
          >
            {t("common.create")}
          </Button>
        </div>
        <ResourceList className="list-stack">
          {projects.data?.map((project) => (
            <button
              key={project.id}
              className={`list-row ${project.id === selectedProjectId ? "active" : ""}`}
              onClick={() => selectProject(project.id)}
            >
              <span>{project.name}</span>
              <StatusPill status={project.status} />
            </button>
          ))}
          {!projects.isLoading && projects.data?.length === 0 ? (
            <EmptyState
              title={t("project.emptyProjects")}
              detail={t("project.emptyProjectsDetail")}
              action={
                <Button
                  size="sm"
                  variant="primary"
                  icon={<Plus size={14} />}
                  onClick={submitProject}
                  disabled={createProject.isPending || !newProjectName.trim()}
                >
                  {t("project.createProject")}
                </Button>
              }
            />
          ) : null}
        </ResourceList>
      </Panel>

      <Panel
        className="file-browser"
        title={selectedProject?.name ?? t("project.fileWorkspace")}
        subtitle={t("project.virtualReadonly")}
        actions={<Badge tone="info">{prefix}</Badge>}
      >
        <Tabs
          items={fileTabs.map((item) => ({ id: item.id, label: t(item.labelKey) }))}
          active={activeTab}
          onChange={setActiveTab}
        />
        <FileControls
          activeTab={activeTab}
          prefix={prefix}
          pattern={pattern}
          searchQuery={searchQuery}
          artifactRunId={artifactRunId}
          onPrefixChange={setPrefix}
          onPatternChange={setPattern}
          onSearchQueryChange={setSearchQuery}
          onArtifactRunIdChange={setArtifactRunId}
        />
        <FileListPanel
          list={activeList}
          isLoading={files.isLoading || searchResults.isLoading || artifacts.isLoading}
          activeTab={activeTab}
          activePrefix={prefix}
          selectedFilePath={selectedFile?.path}
          onEnterDirectory={(path) => {
            setPrefix(toDirectoryPrefix(path));
            setActiveTab("tree");
          }}
          onSelectFile={(revision) =>
            selectFile({
              path: revision.path,
              revision: revision.revision,
              versionId: revision.versionId
            })
          }
          onSelectEntry={(entry) => {
            if (entry.entryType === "directory") {
              setPrefix(toDirectoryPrefix(entry.path));
              setActiveTab("tree");
            } else {
              selectFile({ path: entry.path, revision: entry.latestRevision });
            }
          }}
        />
      </Panel>

      <Panel
        className={`file-viewer detail-drawer ${selectedFile ? "detail-open" : ""}`}
        title={selectedFile?.path ?? t("project.filePreview")}
        subtitle={file.data?.contentType ?? t("project.noFileSelected")}
        actions={
          <div className="row-actions">
            {selectedFile?.revision ? <Badge>{`rev ${selectedFile.revision}`}</Badge> : null}
            {file.data ? <StatusPill status={file.data.isBinary ? "binary" : "active"} /> : null}
            {selectedFile ? (
              <Button
                size="icon"
                variant="ghost"
                aria-label={t("project.closePreview")}
                title={t("project.closePreview")}
                icon={<X size={15} />}
                onClick={() => setSelectedFile(undefined)}
              />
            ) : null}
          </div>
        }
      >
        <div className="file-viewer-body">
          {selectedFile && file.data ? (
            <>
              <FilePreview
                file={file.data}
                allowBinary={allowBinary}
                onAllowBinary={setAllowBinary}
              />
              <FileHistoryPanel
                history={history.data}
                activeRevision={selectedFile.revision ?? file.data.revision}
                onSelect={(revision) =>
                  selectFile({
                    path: revision.path,
                    revision: revision.revision,
                    versionId: revision.versionId
                  })
                }
              />
            </>
          ) : (
            <EmptyState title={t("project.selectFile")} detail={t("project.selectFileDetail")} />
          )}
        </div>
      </Panel>
    </div>
  );
}

function FileControls({
  activeTab,
  prefix,
  pattern,
  searchQuery,
  artifactRunId,
  onPrefixChange,
  onPatternChange,
  onSearchQueryChange,
  onArtifactRunIdChange
}: {
  activeTab: FileTab;
  prefix: string;
  pattern: string;
  searchQuery: string;
  artifactRunId: string;
  onPrefixChange: (value: string) => void;
  onPatternChange: (value: string) => void;
  onSearchQueryChange: (value: string) => void;
  onArtifactRunIdChange: (value: string) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="file-controls">
      <label className="field-stack">
        <span>{t("project.prefix")}</span>
        <TextInput
          value={prefix}
          onChange={(event) => onPrefixChange(event.target.value || "/")}
          placeholder="/workspace/"
        />
      </label>
      {activeTab === "tree" ? (
        <label className="field-stack">
          <span>{t("project.pattern")}</span>
          <TextInput
            value={pattern}
            onChange={(event) => onPatternChange(event.target.value)}
            placeholder="*.md"
          />
        </label>
      ) : null}
      {activeTab === "search" ? (
        <label className="field-stack">
          <span>{t("project.query")}</span>
          <TextInput
            value={searchQuery}
            onChange={(event) => onSearchQueryChange(event.target.value)}
            placeholder={t("project.searchPlaceholder")}
          />
        </label>
      ) : null}
      {activeTab === "artifacts" ? (
        <label className="field-stack">
          <span>{t("project.runId")}</span>
          <TextInput
            value={artifactRunId}
            onChange={(event) => onArtifactRunIdChange(event.target.value)}
            placeholder={t("project.optionalRunId")}
          />
        </label>
      ) : null}
    </div>
  );
}

function FileListPanel({
  list,
  isLoading,
  activeTab,
  activePrefix,
  selectedFilePath,
  onEnterDirectory,
  onSelectEntry,
  onSelectFile
}: {
  list?: FileList;
  isLoading: boolean;
  activeTab: FileTab;
  activePrefix: string;
  selectedFilePath?: string;
  onEnterDirectory: (path: string) => void;
  onSelectEntry: (entry: FileEntry) => void;
  onSelectFile: (revision: FileRevision) => void;
}) {
  const { t } = useI18n();
  if (isLoading) {
    return <EmptyState title={t("common.loading")} detail={t("project.loadingFiles")} />;
  }

  if (!list || (list.entries.length === 0 && list.files.length === 0)) {
    const detail =
      activeTab === "search" ? t("project.emptySearchDetail") : t("project.emptyFilesDetail");
    return <EmptyState title={t("project.emptyFiles")} detail={detail} />;
  }

  return (
    <div className="file-list">
      {list.entries.map((entry) => (
        <button
          key={`entry:${entry.path}`}
          className={`file-row ${
            entry.entryType === "directory" && toDirectoryPrefix(entry.path) === activePrefix
              ? "active"
              : ""
          }`}
          onClick={() => onSelectEntry(entry)}
        >
          {entry.entryType === "directory" ? <Folder size={16} /> : <FileText size={16} />}
          <span>{entry.path}</span>
          <small>
            {entry.entryType === "directory"
              ? t("project.itemChildren", { count: entry.childrenCount })
              : entry.latestRevision
                ? `rev ${entry.latestRevision}`
                : "file"}
          </small>
        </button>
      ))}
      {list.files.map((revision) => (
        <button
          key={revision.id}
          className={`file-row ${revision.path === selectedFilePath ? "active" : ""}`}
          onClick={() => onSelectFile(revision)}
        >
          {revision.isBinary ? <Binary size={16} /> : <FileCode2 size={16} />}
          <span>{revision.path}</span>
          <small>rev {revision.revision}</small>
        </button>
      ))}
      {activeTab === "tree" && list.entries.length === 0 ? (
        <button className="file-row" onClick={() => onEnterDirectory("/")}>
          <RotateCcw size={16} />
          <span>{t("project.backToRoot")}</span>
          <small>/</small>
        </button>
      ) : null}
    </div>
  );
}

function FilePreview({
  file,
  allowBinary,
  onAllowBinary
}: {
  file: FileRevision;
  allowBinary: boolean;
  onAllowBinary: (value: boolean) => void;
}) {
  const { t } = useI18n();
  const content = file.inlineContent ?? decodeBase64Content(file.contentBase64);
  if (file.isBinary) {
    return (
      <div className="detail-stack">
        <div className="section-title">
          <Archive size={16} />
          <strong>{t("project.binaryMetadata")}</strong>
        </div>
        <FileMetadata file={file} />
        {!allowBinary ? (
          <Button
            variant="secondary"
            size="sm"
            icon={<Binary size={15} />}
            onClick={() => onAllowBinary(true)}
          >
            {t("project.loadBinary")}
          </Button>
        ) : null}
        {allowBinary && file.contentBase64 ? (
          <pre className="file-content">{file.contentBase64}</pre>
        ) : null}
      </div>
    );
  }

  return (
    <div className="file-preview-grid">
      <pre className="file-content">{content}</pre>
      <FileMetadata file={file} compact />
    </div>
  );
}

function FileMetadata({ file, compact = false }: { file: FileRevision; compact?: boolean }) {
  const { t } = useI18n();
  return (
    <dl className={compact ? "compact-file-meta" : "detail-stack"}>
      <KeyRow label={t("project.meta.path")} value={file.path} />
      <KeyRow label={t("project.meta.revision")} value={String(file.revision)} />
      <KeyRow label={t("project.meta.hash")} value={file.contentHash} />
      <KeyRow label={t("project.meta.size")} value={`${file.sizeBytes} bytes`} />
      <KeyRow label={t("project.meta.reason")} value={file.reason} />
      {file.bucket ? <KeyRow label={t("project.meta.bucket")} value={file.bucket} /> : null}
      {file.versionId ? <KeyRow label={t("project.meta.version")} value={file.versionId} /> : null}
      {file.objectReferenceId ? (
        <KeyRow label={t("project.meta.objectRef")} value={file.objectReferenceId} />
      ) : null}
    </dl>
  );
}

function FileHistoryPanel({
  history,
  activeRevision,
  onSelect
}: {
  history?: FileList;
  activeRevision?: number;
  onSelect: (revision: FileRevision) => void;
}) {
  const { language, t } = useI18n();
  return (
    <aside className="history-panel">
      <div className="section-title">
        <Clock3 size={16} />
        <strong>{t("project.history")}</strong>
      </div>
      <div className="history-list">
        {history?.files.map((revision) => (
          <button
            key={revision.id}
            className={`history-row ${revision.revision === activeRevision ? "active" : ""}`}
            onClick={() => onSelect(revision)}
          >
            <span>rev {revision.revision}</span>
            <small>{formatShortDate(revision.createdAt, languageLocale(language))}</small>
          </button>
        ))}
        {history && history.files.length === 0 ? (
          <EmptyState title={t("project.emptyHistory")} detail={t("project.emptyHistoryDetail")} />
        ) : null}
      </div>
    </aside>
  );
}

function KeyRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="key-value">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function toDirectoryPrefix(path: string): string {
  if (!path || path === "/") {
    return "/";
  }
  return path.endsWith("/") ? path : `${path}/`;
}

function decodeBase64Content(content?: string): string {
  if (!content) {
    return "";
  }
  try {
    return globalThis.atob(content);
  } catch {
    return content;
  }
}

function formatShortDate(value: string, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(value));
}
