import { useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  CircleDot,
  FolderOpen,
  FolderPlus,
  PanelRightClose,
  PanelRightOpen,
  Search,
  Star
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { usePlatformApi } from "../../../app/providers";
import type { Me, RunEvent, Workspace } from "../../../shared/contracts/platform";
import { useI18n } from "../../../shared/i18n";
import { asRecord, stringFromJson } from "../../../shared/types/json";
import {
  Button,
  ConfigPanel,
  EmptyState,
  StatusPill,
  Tabs,
  TextArea,
  TextInput
} from "../../../shared/ui";
import { ApprovalCenterCompact } from "../../approvals/components/ApprovalCenterCompact";
import {
  useConversationsQuery,
  useCreateConversationMutation
} from "../../conversations/api/conversation.queries";
import { ConversationList } from "../../conversations/components/ConversationList";
import { invalidateProjectFileQueries } from "../../projects/api/project.queries";
import {
  patchConversationEvents,
  useCancelRunMutation,
  useConversationEventsQuery,
  useConversationEventStream
} from "../../runs/api/run.queries";
import { RunComposer } from "../../runs/components/RunComposer";
import { RunTimeline } from "../../runs/components/RunTimeline";
import { SubagentList } from "../../runs/components/SubagentList";
import { TaskList } from "../../runs/components/TaskList";
import type { TimelineMessage } from "../../runs/domain/run.types";
import { projectRunEvents } from "../../runs/domain/run.projections";
import {
  useCreateLocalMountMutation,
  useCreateWorkspaceMutation,
  useLocalMountsQuery,
  useWorkspacesQuery
} from "../../workspaces/api/workspace.queries";

type InspectorTab = "tasks" | "subagents" | "approvals" | "files" | "activity";
type WorkbenchConfigPanel = "workspace" | "localMount" | "conversation" | null;
const MAX_RECENT_WORKSPACES = 6;

interface WorkspaceConfigDraft {
  name: string;
  trustState: string;
  includeGlobs: string[];
  excludeGlobs: string[];
}

interface LocalMountConfigDraft {
  displayName: string;
  realPath: string;
  virtualPath: string;
  capabilities: string[];
  trustState: string;
  includeGlobs: string[];
  excludeGlobs: string[];
}

export function WorkbenchScreen({ me }: { me: Me }) {
  const { desktopAuthApi, runApi } = usePlatformApi();
  const { t } = useI18n();
  const queryClient = useQueryClient();
  const workspaces = useWorkspacesQuery(me.tenantId);
  const createWorkspace = useCreateWorkspaceMutation(me.tenantId);
  const conversations = useConversationsQuery(me.tenantId);
  const createConversation = useCreateConversationMutation(me.tenantId);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | undefined>();
  const [selectedConversationId, setSelectedConversationId] = useState<string | undefined>();
  const [workspaceSwitcherOpen, setWorkspaceSwitcherOpen] = useState(false);
  const [workspaceSearch, setWorkspaceSearch] = useState("");
  const [favoriteWorkspaceIds, setFavoriteWorkspaceIds] = useState<Set<string>>(
    () => new Set(readStoredStringList(workspaceStorageKey("favorites", me.tenantId)))
  );
  const [recentWorkspaceIds, setRecentWorkspaceIds] = useState<string[]>(() =>
    readStoredStringList(workspaceStorageKey("recent", me.tenantId))
  );
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("tasks");
  const [conversationListCollapsed, setConversationListCollapsed] = useState(false);
  const [inspectorCollapsed, setInspectorCollapsed] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [pendingUserMessage, setPendingUserMessage] = useState<TimelineMessage | null>(null);
  const [composerDraft, setComposerDraft] = useState<{ id: string; content: string } | undefined>();
  const [configPanel, setConfigPanel] = useState<WorkbenchConfigPanel>(null);
  const [workspaceCreateError, setWorkspaceCreateError] = useState<string | null>(null);
  const [workspaceNotice, setWorkspaceNotice] = useState<string | null>(null);
  const [mountingLocalFolder, setMountingLocalFolder] = useState(false);
  const [localMountError, setLocalMountError] = useState<string | null>(null);
  const [localMountNotice, setLocalMountNotice] = useState<string | null>(null);
  const [composerFocusSignal, setComposerFocusSignal] = useState(0);
  const workspaceSwitcherRef = useRef<HTMLDivElement | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const localMounts = useLocalMountsQuery(me.tenantId, selectedWorkspaceId);
  const createLocalMount = useCreateLocalMountMutation(me.tenantId, selectedWorkspaceId);

  useEffect(() => {
    if (!selectedWorkspaceId && workspaces.data?.[0]) {
      setSelectedWorkspaceId(workspaces.data[0].id);
    }
  }, [selectedWorkspaceId, workspaces.data]);

  useEffect(() => {
    if (!workspaceSwitcherOpen) {
      return;
    }

    function closeOnOutsidePointer(event: PointerEvent) {
      const target = event.target;
      if (target instanceof Node && !workspaceSwitcherRef.current?.contains(target)) {
        setWorkspaceSwitcherOpen(false);
      }
    }

    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setWorkspaceSwitcherOpen(false);
      }
    }

    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [workspaceSwitcherOpen]);

  useEffect(() => {
    writeStoredStringList(workspaceStorageKey("favorites", me.tenantId), [...favoriteWorkspaceIds]);
  }, [favoriteWorkspaceIds, me.tenantId]);

  useEffect(() => {
    writeStoredStringList(workspaceStorageKey("recent", me.tenantId), recentWorkspaceIds);
  }, [me.tenantId, recentWorkspaceIds]);

  const selectedWorkspace = useMemo(
    () => (workspaces.data ?? []).find((workspace) => workspace.id === selectedWorkspaceId),
    [selectedWorkspaceId, workspaces.data]
  );

  const filteredWorkspaces = useMemo(() => {
    const query = workspaceSearch.trim().toLocaleLowerCase();
    const allWorkspaces = workspaces.data ?? [];
    if (!query) {
      return allWorkspaces;
    }
    return allWorkspaces.filter((workspace) =>
      [workspace.name, workspace.status, workspace.trustState].some((value) =>
        value.toLocaleLowerCase().includes(query)
      )
    );
  }, [workspaceSearch, workspaces.data]);

  const workspaceGroups = useMemo(
    () => groupWorkspaces(filteredWorkspaces, favoriteWorkspaceIds, recentWorkspaceIds, t),
    [favoriteWorkspaceIds, filteredWorkspaces, recentWorkspaceIds, t]
  );

  const workspaceConversations = useMemo(
    () =>
      (conversations.data ?? []).filter(
        (conversation) => conversation.workspaceId === selectedWorkspaceId
      ),
    [conversations.data, selectedWorkspaceId]
  );

  useEffect(() => {
    if (!selectedWorkspaceId) {
      setSelectedConversationId(undefined);
      return;
    }
    if (
      selectedConversationId &&
      workspaceConversations.some((conversation) => conversation.id === selectedConversationId)
    ) {
      return;
    }
    setSelectedConversationId(workspaceConversations[0]?.id);
  }, [selectedConversationId, selectedWorkspaceId, workspaceConversations]);

  const eventQuery = useConversationEventsQuery(me.tenantId, selectedConversationId);
  const projection = useMemo(() => projectRunEvents(eventQuery.data ?? []), [eventQuery.data]);
  const chatProjection = useMemo(() => {
    if (!pendingUserMessage) {
      return projection;
    }
    return {
      ...projection,
      messages: [...projection.messages, pendingUserMessage],
      timeline: [...projection.timeline, { kind: "message" as const, id: pendingUserMessage.id }]
    };
  }, [pendingUserMessage, projection]);
  const cancelRun = useCancelRunMutation(me.tenantId);
  const latestRunId = [...projection.runEvents].reverse().find((event) => event.runId)?.runId;
  const handleRunEventSideEffects = useCallback(
    (event: RunEvent) => {
      if (event.type !== "file.changed") {
        return;
      }
      const payload = asRecord(event.payload);
      const projectId = stringFromJson(payload.project_id) || stringFromJson(payload.projectId);
      void invalidateProjectFileQueries(queryClient, event.tenantId, projectId || undefined);
    },
    [queryClient]
  );
  const streamState = useConversationEventStream(
    me.tenantId,
    selectedConversationId,
    handleRunEventSideEffects
  );

  async function createConfiguredConversation(title: string) {
    if (!selectedWorkspaceId) {
      return;
    }
    const time = new Date().toLocaleTimeString();
    setWorkspaceNotice(null);
    try {
      const created = await createConversation.mutateAsync({
        workspaceId: selectedWorkspaceId,
        title: title.trim() || t("conversation.newTitle", { time })
      });
      setSelectedConversationId(created.id);
      setWorkspaceNotice(t("conversation.createdNotice"));
      setConfigPanel(null);
      setComposerFocusSignal((value) => value + 1);
    } catch (error) {
      setWorkspaceNotice(error instanceof Error ? error.message : t("conversation.createFailed"));
    }
  }

  async function createConfiguredWorkspace(draft: WorkspaceConfigDraft) {
    if (createWorkspace.isPending) {
      return;
    }
    setWorkspaceCreateError(null);
    setWorkspaceNotice(null);
    try {
      const created = await createWorkspace.mutateAsync({
        name: draft.name.trim(),
        trustState: draft.trustState,
        includeGlobs: draft.includeGlobs,
        excludeGlobs: draft.excludeGlobs
      });
      setSelectedWorkspaceId(created.id);
      setSelectedConversationId(undefined);
      setWorkspaceNotice(t("workspace.createdNotice"));
      setConfigPanel(null);
    } catch (error) {
      setWorkspaceCreateError(error instanceof Error ? error.message : t("workspace.createFailed"));
    }
  }

  async function pickLocalMountFolder() {
    setLocalMountError(null);
    setLocalMountNotice(null);
    setMountingLocalFolder(true);
    try {
      return await desktopAuthApi.pickLocalMountFolder();
    } finally {
      setMountingLocalFolder(false);
    }
  }

  async function createConfiguredLocalMount(draft: LocalMountConfigDraft) {
    if (!selectedWorkspaceId || mountingLocalFolder) {
      return;
    }
    setLocalMountError(null);
    setLocalMountNotice(null);
    try {
      const created = await createLocalMount.mutateAsync({
        displayName: draft.displayName.trim() || t("workspace.localMountDefaultName"),
        virtualPath: draft.virtualPath.trim() || "/local/main/",
        capabilities: draft.capabilities,
        includeGlobs: draft.includeGlobs,
        excludeGlobs: draft.excludeGlobs,
        trustState: draft.trustState,
        metadata: { source: "tauri_folder_picker" }
      });
      await desktopAuthApi.saveLocalMountRealPath(created.id, draft.realPath);
      setLocalMountNotice(t("workspace.localMountCreated", { name: created.displayName }));
      setConfigPanel(null);
    } catch (error) {
      setLocalMountError(error instanceof Error ? error.message : t("workspace.localMountFailed"));
    }
  }

  function selectWorkspace(workspaceId: string | undefined) {
    setWorkspaceCreateError(null);
    setWorkspaceNotice(null);
    setLocalMountError(null);
    setLocalMountNotice(null);
    setSelectedWorkspaceId(workspaceId);
    setSelectedConversationId(undefined);
    setWorkspaceSearch("");
    setWorkspaceSwitcherOpen(false);
    if (workspaceId) {
      setRecentWorkspaceIds((current) =>
        [workspaceId, ...current.filter((candidate) => candidate !== workspaceId)].slice(
          0,
          MAX_RECENT_WORKSPACES
        )
      );
    }
  }

  function toggleFavoriteWorkspace(workspaceId: string) {
    setFavoriteWorkspaceIds((current) => {
      const next = new Set(current);
      if (next.has(workspaceId)) {
        next.delete(workspaceId);
      } else {
        next.add(workspaceId);
      }
      return next;
    });
  }

  async function submitRun(content: string) {
    const conversationId = selectedConversationId;
    if (!conversationId) {
      return;
    }
    const controller = new AbortController();
    const idempotencyKey = crypto.randomUUID();
    abortRef.current = controller;
    setComposerDraft(undefined);
    setPendingUserMessage({
      id: `pending.${idempotencyKey}`,
      role: "user",
      content,
      status: "streaming"
    });
    setStreaming(true);
    try {
      await runApi.createRunStream(
        {
          tenantId: me.tenantId,
          conversationId,
          input: { messages: [{ role: "user", content }] },
          idempotencyKey,
          afterSeq: projection.lastSeq
        },
        (event) => {
          const eventPayload = asRecord(event.payload);
          if (event.type === "message.completed" && stringFromJson(eventPayload.role) === "user") {
            setPendingUserMessage(null);
          }
          patchConversationEvents(queryClient, event.tenantId, event.conversationId, [event]);
          handleRunEventSideEffects(event);
        },
        controller.signal
      );
    } finally {
      setStreaming(false);
      abortRef.current = null;
      setPendingUserMessage(null);
      await eventQuery.refetch();
    }
  }

  async function cancelCurrentRun() {
    abortRef.current?.abort();
    if (latestRunId) {
      await cancelRun.mutateAsync(latestRunId);
    }
  }

  function editMessage(messageId: string) {
    const message = projection.messages.find((candidate) => candidate.id === messageId);
    if (!message || message.role !== "user") {
      return;
    }
    setComposerDraft({ id: crypto.randomUUID(), content: message.content });
  }

  async function regenerateAssistantMessage(messageId: string) {
    if (streaming) {
      return;
    }
    const messageIndex = projection.messages.findIndex((message) => message.id === messageId);
    if (messageIndex < 0) {
      return;
    }
    const previousUserMessage = projection.messages
      .slice(0, messageIndex)
      .reverse()
      .find((message) => message.role === "user");
    if (previousUserMessage) {
      await submitRun(previousUserMessage.content);
    }
  }

  const workbenchGridClassName = [
    "workbench-grid",
    conversationListCollapsed ? "conversation-collapsed" : "",
    inspectorCollapsed ? "inspector-collapsed" : ""
  ]
    .filter(Boolean)
    .join(" ");
  const workspaceContextBar = (
    <div className="workbench-context-bar">
      <div className="workspace-context-main">
        <div className="workspace-switcher" ref={workspaceSwitcherRef}>
          <span id="workspace-switcher-label" className="workspace-context-label">
            {t("workspace.label")}
          </span>
          <button
            type="button"
            className="workspace-switcher-trigger"
            aria-haspopup="listbox"
            aria-expanded={workspaceSwitcherOpen}
            aria-labelledby="workspace-switcher-label workspace-switcher-selected"
            onClick={() => setWorkspaceSwitcherOpen((open) => !open)}
          >
            <strong id="workspace-switcher-selected">
              {selectedWorkspace?.name ?? t("workspace.select")}
            </strong>
            <ChevronDown size={15} />
          </button>
          {workspaceSwitcherOpen ? (
            <div className="workspace-switcher-popover">
              <div className="input-with-icon">
                <Search size={15} />
                <TextInput
                  autoFocus
                  aria-label={t("workspace.search")}
                  value={workspaceSearch}
                  onChange={(event) => setWorkspaceSearch(event.target.value)}
                  placeholder={t("workspace.search")}
                />
              </div>
              <div
                className="workspace-switcher-list"
                role="listbox"
                aria-label={t("workspace.label")}
              >
                {workspaceGroups.map((group) => (
                  <div className="workspace-switcher-group" key={group.label}>
                    <span>{group.label}</span>
                    {group.items.map((workspace) => {
                      const favorite = favoriteWorkspaceIds.has(workspace.id);
                      return (
                        <div
                          className={`workspace-switcher-item ${
                            workspace.id === selectedWorkspaceId ? "active" : ""
                          }`}
                          key={workspace.id}
                        >
                          <button
                            type="button"
                            role="option"
                            aria-selected={workspace.id === selectedWorkspaceId}
                            className="workspace-switcher-main"
                            onClick={() => selectWorkspace(workspace.id)}
                          >
                            <span>
                              <strong>{workspace.name}</strong>
                              <small>
                                {workspace.status} · {workspace.trustState}
                              </small>
                            </span>
                            {workspace.id === selectedWorkspaceId ? <Check size={15} /> : null}
                          </button>
                          <button
                            type="button"
                            className={`workspace-favorite ${favorite ? "active" : ""}`}
                            aria-label={t("workspace.favoriteToggle", { name: workspace.name })}
                            title={t("workspace.favoriteToggle", { name: workspace.name })}
                            onClick={() => toggleFavoriteWorkspace(workspace.id)}
                          >
                            <Star size={14} fill={favorite ? "currentColor" : "none"} />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                ))}
                {!filteredWorkspaces.length ? (
                  <span className="workspace-switcher-empty">{t("workspace.noMatches")}</span>
                ) : null}
              </div>
            </div>
          ) : null}
        </div>
        <Button
          size="sm"
          variant="secondary"
          aria-label={t("workspace.create")}
          title={t("workspace.create")}
          icon={<FolderPlus size={16} />}
          onClick={() => setConfigPanel("workspace")}
          disabled={createWorkspace.isPending}
        >
          {t("common.create")}
        </Button>
      </div>
      <div className="workspace-context-actions">
        <Button
          size="sm"
          variant="ghost"
          icon={<FolderOpen size={14} />}
          onClick={() => setConfigPanel("localMount")}
          disabled={!selectedWorkspaceId || mountingLocalFolder || createLocalMount.isPending}
        >
          {t("workspace.mountLocalFolder")}
        </Button>
        {localMounts.data?.length ? (
          <span className="workspace-mount-count">
            {t("workspace.localMountCount", { count: localMounts.data.length })}
          </span>
        ) : null}
      </div>
      {localMounts.data?.length ? (
        <details className="workspace-mount-details">
          <summary>{t("workspace.localMounts")}</summary>
          <div className="workspace-mount-list" aria-label={t("workspace.localMounts")}>
            {localMounts.data.map((mount) => (
              <div className="workspace-mount-item" key={mount.id}>
                <strong>{mount.displayName}</strong>
                <span>{mount.virtualPath}</span>
                <StatusPill status={mount.trustState} />
              </div>
            ))}
          </div>
        </details>
      ) : null}
      {workspaceCreateError ? (
        <span className="workspace-mount-error" title={workspaceCreateError}>
          {workspaceCreateError}
        </span>
      ) : null}
      {workspaceNotice ? (
        <span className="workspace-mount-message" title={workspaceNotice}>
          {workspaceNotice}
        </span>
      ) : null}
      {localMountNotice ? (
        <span className="workspace-mount-message" title={localMountNotice}>
          {localMountNotice}
        </span>
      ) : null}
      {localMountError ? (
        <span className="workspace-mount-error" title={localMountError}>
          {localMountError}
        </span>
      ) : null}
    </div>
  );

  return (
    <>
      <div className={workbenchGridClassName}>
        <ConversationList
          conversations={workspaceConversations}
          selectedId={selectedConversationId}
          onSelect={(conversationId) => {
            setSelectedConversationId(conversationId);
          }}
          onCreate={() => setConfigPanel("conversation")}
          creating={createConversation.isPending || !selectedWorkspaceId}
          collapsed={conversationListCollapsed}
          onToggleCollapsed={() => setConversationListCollapsed((value) => !value)}
        />
        <section className={`workbench-center ${selectedConversationId ? "has-composer" : ""}`}>
          {workspaceContextBar}
          {!selectedWorkspaceId ? (
            <EmptyState
              title={t("workspace.selectRequired")}
              detail={t("workspace.selectRequiredDetail")}
              action={
                <Button
                  variant="primary"
                  icon={<FolderPlus size={15} />}
                  onClick={() => setConfigPanel("workspace")}
                  disabled={createWorkspace.isPending}
                >
                  {t("workspace.create")}
                </Button>
              }
            />
          ) : selectedConversationId ? (
            <>
              <RunTimeline
                projection={chatProjection}
                onEditMessage={editMessage}
                onRegenerateMessage={(messageId) => void regenerateAssistantMessage(messageId)}
              />
              <RunComposer
                disabled={streaming || !selectedConversationId}
                streaming={streaming}
                draft={composerDraft}
                autoFocusSignal={composerFocusSignal}
                onSubmit={submitRun}
                onCancel={cancelCurrentRun}
              />
            </>
          ) : (
            <EmptyState
              title={t("workbench.selectConversation")}
              detail={t("workbench.selectConversationDetail")}
              action={
                <Button
                  variant="primary"
                  icon={<CircleDot size={15} />}
                  onClick={() => setConfigPanel("conversation")}
                  disabled={createConversation.isPending}
                >
                  {t("conversation.create")}
                </Button>
              }
            />
          )}
        </section>
        <aside className={`inspector ${inspectorCollapsed ? "collapsed" : ""}`}>
          <header className="panel-header">
            {!inspectorCollapsed ? (
              <div>
                <strong>{t("workbench.inspector.title")}</strong>
                <span>{streamStatusLabel(streamState.status, t)}</span>
              </div>
            ) : null}
            <div className="panel-header-actions">
              {!inspectorCollapsed ? <StatusPill status={projection.status} /> : null}
              <Button
                size="icon"
                variant="ghost"
                aria-label={t(
                  inspectorCollapsed ? "workbench.inspector.expand" : "workbench.inspector.collapse"
                )}
                title={t(
                  inspectorCollapsed ? "workbench.inspector.expand" : "workbench.inspector.collapse"
                )}
                aria-expanded={!inspectorCollapsed}
                icon={
                  inspectorCollapsed ? <PanelRightOpen size={16} /> : <PanelRightClose size={16} />
                }
                onClick={() => setInspectorCollapsed((value) => !value)}
              />
            </div>
          </header>
          {!inspectorCollapsed ? (
            <>
              <Tabs<InspectorTab>
                active={inspectorTab}
                onChange={setInspectorTab}
                items={[
                  { id: "tasks", label: t("workbench.inspector.tasks") },
                  { id: "subagents", label: t("workbench.inspector.subagents") },
                  { id: "approvals", label: t("workbench.inspector.approvals") },
                  { id: "files", label: t("workbench.inspector.files") },
                  { id: "activity", label: t("workbench.inspector.activity") }
                ]}
              />
              <div className="inspector-body">
                {inspectorTab === "tasks" ? <TaskList tasks={projection.tasks} /> : null}
                {inspectorTab === "subagents" ? (
                  <SubagentList subagents={projection.subagents} />
                ) : null}
                {inspectorTab === "approvals" ? (
                  <ApprovalCenterCompact tenantId={me.tenantId} />
                ) : null}
                {inspectorTab === "files" ? (
                  projection.files.length ? (
                    <div className="inspector-list">
                      {projection.files.map((file) => (
                        <div className="inspector-row" key={file.path}>
                          <div>
                            <strong>{file.path}</strong>
                            <span>
                              {t("project.meta.revision")} {file.revision ?? "-"}
                            </span>
                          </div>
                        </div>
                      ))}
                    </div>
                  ) : (
                    <EmptyState title={t("workbench.noFileChanges")} />
                  )
                ) : null}
                {inspectorTab === "activity" ? (
                  projection.runEvents.length ? (
                    <div className="inspector-list">
                      {projection.runEvents.map((event) => {
                        const payload = asRecord(event.payload);
                        return (
                          <div className="inspector-row" key={event.id}>
                            <CircleDot size={14} />
                            <div>
                              <strong>{event.type}</strong>
                              <span>
                                seq {event.seq} · {new Date(event.createdAt).toLocaleTimeString()}
                              </span>
                            </div>
                            <StatusPill status={stringFromJson(payload.status, event.type)} />
                          </div>
                        );
                      })}
                    </div>
                  ) : (
                    <EmptyState title={t("workbench.noActivity")} />
                  )
                ) : null}
              </div>
            </>
          ) : null}
        </aside>
      </div>
      {configPanel === "workspace" ? (
        <ConfigPanel
          title={t("workspace.configureCreate")}
          subtitle={t("workspace.createSubtitle")}
          closeLabel={t("common.close")}
          onClose={() => setConfigPanel(null)}
        >
          <WorkspaceConfigForm
            pending={createWorkspace.isPending}
            onCancel={() => setConfigPanel(null)}
            onSubmit={createConfiguredWorkspace}
          />
        </ConfigPanel>
      ) : null}
      {configPanel === "localMount" ? (
        <ConfigPanel
          title={t("workspace.mountConfigTitle")}
          subtitle={t("workspace.mountConfigSubtitle")}
          closeLabel={t("common.close")}
          onClose={() => setConfigPanel(null)}
        >
          <LocalMountConfigForm
            picking={mountingLocalFolder}
            pending={createLocalMount.isPending}
            onPickFolder={pickLocalMountFolder}
            onCancel={() => setConfigPanel(null)}
            onSubmit={createConfiguredLocalMount}
          />
        </ConfigPanel>
      ) : null}
      {configPanel === "conversation" ? (
        <ConfigPanel
          title={t("conversation.configureCreate")}
          subtitle={t("conversation.createSubtitle")}
          closeLabel={t("common.close")}
          onClose={() => setConfigPanel(null)}
        >
          <ConversationConfigForm
            pending={createConversation.isPending}
            onCancel={() => setConfigPanel(null)}
            onSubmit={createConfiguredConversation}
          />
        </ConfigPanel>
      ) : null}
    </>
  );
}

function WorkspaceConfigForm({
  pending,
  onSubmit,
  onCancel
}: {
  pending: boolean;
  onSubmit: (draft: WorkspaceConfigDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [name, setName] = useState(() =>
    t("workspace.newTitle", { time: new Date().toLocaleTimeString() })
  );
  const [trustState, setTrustState] = useState("untrusted");
  const [includeGlobs, setIncludeGlobs] = useState("");
  const [excludeGlobs, setExcludeGlobs] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    if (!name.trim()) {
      setError(t("catalog.form.nameRequired"));
      return;
    }
    await onSubmit({
      name,
      trustState,
      includeGlobs: parseLineList(includeGlobs),
      excludeGlobs: parseLineList(excludeGlobs)
    });
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <label className="field-stack">
        <span>{t("workspace.name")}</span>
        <TextInput value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("workspace.trustState")}</span>
        <select
          className="text-input"
          value={trustState}
          onChange={(event) => setTrustState(event.target.value)}
        >
          <option value="untrusted">{t("workspace.trust.untrusted")}</option>
          <option value="trusted">{t("workspace.trust.trusted")}</option>
        </select>
      </label>
      <details className="config-advanced">
        <summary>{t("common.advanced")}</summary>
        <label className="field-stack">
          <span>{t("workspace.includeGlobs")}</span>
          <TextArea
            value={includeGlobs}
            placeholder={t("workspace.globsPlaceholder")}
            onChange={(event) => setIncludeGlobs(event.target.value)}
          />
        </label>
        <label className="field-stack">
          <span>{t("workspace.excludeGlobs")}</span>
          <TextArea
            value={excludeGlobs}
            placeholder={t("workspace.globsPlaceholder")}
            onChange={(event) => setExcludeGlobs(event.target.value)}
          />
        </label>
      </details>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<FolderPlus size={15} />} disabled={pending}>
          {t("common.create")}
        </Button>
      </div>
    </form>
  );
}

function LocalMountConfigForm({
  picking,
  pending,
  onPickFolder,
  onSubmit,
  onCancel
}: {
  picking: boolean;
  pending: boolean;
  onPickFolder: () => Promise<{ displayName: string; realPath: string } | null>;
  onSubmit: (draft: LocalMountConfigDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [displayName, setDisplayName] = useState("");
  const [realPath, setRealPath] = useState("");
  const virtualPath = "/local/main/";
  const [writeEnabled, setWriteEnabled] = useState(true);
  const [trustState, setTrustState] = useState("untrusted");
  const [includeGlobs, setIncludeGlobs] = useState("");
  const [excludeGlobs, setExcludeGlobs] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function chooseFolder() {
    setError(null);
    const selection = await onPickFolder();
    if (!selection) {
      setError(t("workspace.localMountNotSelected"));
      return;
    }
    setDisplayName(selection.displayName.trim() || t("workspace.localMountDefaultName"));
    setRealPath(selection.realPath);
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    if (!realPath.trim()) {
      setError(t("workspace.realPathRequired"));
      return;
    }
    await onSubmit({
      displayName,
      realPath,
      virtualPath,
      capabilities: writeEnabled ? ["read", "write"] : ["read"],
      trustState,
      includeGlobs: parseLineList(includeGlobs),
      excludeGlobs: parseLineList(excludeGlobs)
    });
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <Button
        type="button"
        variant="secondary"
        icon={<FolderOpen size={15} />}
        onClick={() => void chooseFolder()}
        disabled={picking || pending}
      >
        {t("workspace.chooseFolder")}
      </Button>
      <p className="config-help">{t("workspace.mountQuickHint")}</p>
      {realPath ? (
        <div className="config-readonly">
          <span>{t("workspace.selectedFolder")}</span>
          <strong>{realPath}</strong>
        </div>
      ) : null}
      <label className="field-stack">
        <span>{t("workspace.localMountName")}</span>
        <TextInput value={displayName} onChange={(event) => setDisplayName(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("workspace.virtualPath")}</span>
        <TextInput value={virtualPath} readOnly />
      </label>
      <label className="field-stack">
        <span>{t("workspace.trustState")}</span>
        <select
          className="text-input"
          value={trustState}
          onChange={(event) => setTrustState(event.target.value)}
        >
          <option value="untrusted">{t("workspace.trust.untrusted")}</option>
          <option value="trusted">{t("workspace.trust.trusted")}</option>
        </select>
      </label>
      <div className="field-stack">
        <span>{t("workspace.capabilities")}</span>
        <div className="checkbox-row">
          <label>
            <input type="checkbox" checked disabled />
            {t("workspace.capability.read")}
          </label>
          <label>
            <input
              type="checkbox"
              checked={writeEnabled}
              onChange={(event) => setWriteEnabled(event.target.checked)}
            />
            {t("workspace.capability.write")}
          </label>
        </div>
      </div>
      <details className="config-advanced">
        <summary>{t("common.advanced")}</summary>
        <label className="field-stack">
          <span>{t("workspace.includeGlobs")}</span>
          <TextArea
            value={includeGlobs}
            placeholder={t("workspace.globsPlaceholder")}
            onChange={(event) => setIncludeGlobs(event.target.value)}
          />
        </label>
        <label className="field-stack">
          <span>{t("workspace.excludeGlobs")}</span>
          <TextArea
            value={excludeGlobs}
            placeholder={t("workspace.globsPlaceholder")}
            onChange={(event) => setExcludeGlobs(event.target.value)}
          />
        </label>
      </details>
      {error ? (
        <p className="form-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<FolderOpen size={15} />} disabled={pending}>
          {t("common.create")}
        </Button>
      </div>
    </form>
  );
}

function ConversationConfigForm({
  pending,
  onSubmit,
  onCancel
}: {
  pending: boolean;
  onSubmit: (title: string) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [title, setTitle] = useState(() =>
    t("conversation.newTitle", { time: new Date().toLocaleTimeString() })
  );

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await onSubmit(title);
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <label className="field-stack">
        <span>{t("conversation.titleField")}</span>
        <TextInput value={title} onChange={(event) => setTitle(event.target.value)} />
      </label>
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<CircleDot size={15} />} disabled={pending}>
          {t("common.create")}
        </Button>
      </div>
    </form>
  );
}

function parseLineList(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function workspaceStorageKey(kind: "favorites" | "recent", tenantId: string) {
  return `bibi-work.workspace.${kind}.${tenantId}`;
}

function readStoredStringList(key: string): string[] {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) {
      return [];
    }
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
  }
}

function writeStoredStringList(key: string, values: string[]) {
  try {
    window.localStorage.setItem(key, JSON.stringify(values));
  } catch {
    return;
  }
}

function groupWorkspaces(
  workspaces: Workspace[],
  favoriteIds: Set<string>,
  recentIds: string[],
  t: ReturnType<typeof useI18n>["t"]
) {
  const used = new Set<string>();
  const groups: Array<{ label: string; items: Workspace[] }> = [];
  const favorites = workspaces.filter((workspace) => favoriteIds.has(workspace.id));
  if (favorites.length) {
    groups.push({ label: t("workspace.group.favorites"), items: favorites });
    favorites.forEach((workspace) => used.add(workspace.id));
  }

  const recent = recentIds
    .map((workspaceId) => workspaces.find((workspace) => workspace.id === workspaceId))
    .filter((workspace): workspace is Workspace => Boolean(workspace))
    .filter((workspace) => !used.has(workspace.id));
  if (recent.length) {
    groups.push({ label: t("workspace.group.recent"), items: recent });
    recent.forEach((workspace) => used.add(workspace.id));
  }

  const byStatus = new Map<string, Workspace[]>();
  workspaces
    .filter((workspace) => !used.has(workspace.id))
    .forEach((workspace) => {
      const status = workspace.status || t("common.none");
      byStatus.set(status, [...(byStatus.get(status) ?? []), workspace]);
    });
  byStatus.forEach((items, status) => {
    groups.push({ label: t("workspace.group.status", { status }), items });
  });
  return groups;
}

function streamStatusLabel(
  status: ReturnType<typeof useConversationEventStream>["status"],
  t: ReturnType<typeof useI18n>["t"]
) {
  if (status === "connected") {
    return t("workbench.stream.connected");
  }
  if (status === "connecting") {
    return t("workbench.stream.connecting");
  }
  if (status === "reconnecting") {
    return t("workbench.stream.reconnecting");
  }
  return t("workbench.stream.idle");
}
