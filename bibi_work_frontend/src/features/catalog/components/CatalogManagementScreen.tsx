import {
  Ban,
  Bot,
  Brain,
  CheckCircle2,
  Eye,
  FileCode2,
  Layers3,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Send,
  Server,
  ShieldCheck,
  Wrench
} from "lucide-react";
import { useEffect, useMemo, useState, type ComponentType, type FormEvent } from "react";
import {
  jsonValueSchema,
  type AgentVersionCapabilities,
  type CapabilityResource,
  type Me,
  type PolicyBinding,
  type Resource,
  type ValidationResponse,
  type Version
} from "../../../shared/contracts/platform";
import type { JsonValue } from "../../../shared/types/json";
import { useI18n, type I18nKey } from "../../../shared/i18n";
import {
  ActionMenu,
  Badge,
  Button,
  ConfirmDialog,
  ConfigPanel,
  EmptyState,
  Panel,
  ResourceList,
  StatusPill,
  Tabs,
  TextArea,
  TextInput
} from "../../../shared/ui";
import type {
  CatalogResourceKind,
  EditableCatalogKind,
  CreateMcpServerInput,
  UpdateMcpToolInput,
  VersionedCatalogKind
} from "../api/catalog.adapter";
import {
  useAgentVersionCapabilitiesMutation,
  useCreateCatalogResourceMutation,
  useCreateMcpServerMutation,
  useCreatePolicyBindingMutation,
  useCatalogResourcesQuery,
  useCatalogVersionsQuery,
  useDiscoverMcpToolsMutation,
  useDisableMcpToolMutation,
  useDisablePolicyBindingMutation,
  useDisableCatalogResourceMutation,
  useDisableCatalogVersionMutation,
  useMcpToolsQuery,
  usePolicyBindingsQuery,
  usePublishCatalogVersionMutation,
  useUpdateMcpToolMutation,
  useUpdateMcpServerMutation,
  useValidateAgentVersionMutation
} from "../api/catalog.queries";
import { useSecretRefsQuery } from "../../secrets/api/secret-refs.queries";

export type CatalogTab = "agents" | "skills" | "tools" | "mcp" | "llm";
type LlmMode = "providers" | "profiles";
type StatusFilter = "all" | "draft" | "active" | "published" | "disabled";
type CatalogDetailTab = "overview" | "versions" | "capabilities" | "policies";
type CatalogConfirmAction =
  | { type: "resource" }
  | { type: "version"; version: Version }
  | { type: "policy"; policy: PolicyBinding }
  | { type: "mcpTool"; tool: Resource };
type VersionInsight =
  | { versionId: string; mode: "capabilities"; data: AgentVersionCapabilities }
  | { versionId: string; mode: "validation"; data: ValidationResponse }
  | { versionId: string; mode: "error"; message: string };
type McpDiscoverSummary = {
  total: number;
  created: number;
  changed: number;
  unchanged: number;
  missing: number;
};

const tabs: Array<{
  id: CatalogTab;
  labelKey: I18nKey;
  icon: ComponentType<{ size?: number; strokeWidth?: number }>;
}> = [
  { id: "agents", labelKey: "catalog.tab.agents", icon: Bot },
  { id: "skills", labelKey: "catalog.tab.skills", icon: Brain },
  { id: "tools", labelKey: "catalog.tab.tools", icon: Wrench },
  { id: "mcp", labelKey: "catalog.tab.mcp", icon: FileCode2 }
];

const statusFilters: StatusFilter[] = ["all", "draft", "active", "published", "disabled"];
const toolTypes = ["custom", "http", "sql", "mcp", "local_exec"] as const;

export function CatalogManagementScreen({
  me,
  initialTab = "agents",
  onTabRouteChange
}: {
  me: Me;
  initialTab?: CatalogTab;
  onTabRouteChange?: (tab: CatalogTab) => void;
}) {
  const { t } = useI18n();
  const [activeTab, setActiveTab] = useState<CatalogTab>(initialTab);
  const [llmMode, setLlmMode] = useState<LlmMode>("providers");
  const [status, setStatus] = useState<StatusFilter>("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [mcpConfigOpen, setMcpConfigOpen] = useState(false);
  const [mcpConfigTarget, setMcpConfigTarget] = useState<Resource | null>(null);
  const [mcpToolConfigOpen, setMcpToolConfigOpen] = useState(false);
  const [mcpToolConfigTarget, setMcpToolConfigTarget] = useState<Resource | null>(null);
  const [mcpActionError, setMcpActionError] = useState<string | null>(null);
  const [mcpDiscoverSummary, setMcpDiscoverSummary] = useState<McpDiscoverSummary | null>(null);
  const [policyCreateOpen, setPolicyCreateOpen] = useState(false);
  const [publishOpen, setPublishOpen] = useState(false);
  const [detailTab, setDetailTab] = useState<CatalogDetailTab>("overview");
  const [confirmAction, setConfirmAction] = useState<CatalogConfirmAction | null>(null);
  const [versionInsight, setVersionInsight] = useState<VersionInsight | null>(null);
  const createResource = useCreateCatalogResourceMutation();
  const createMcpServer = useCreateMcpServerMutation();
  const updateMcpServer = useUpdateMcpServerMutation();
  const discoverMcpTools = useDiscoverMcpToolsMutation();
  const updateMcpTool = useUpdateMcpToolMutation();
  const disableMcpTool = useDisableMcpToolMutation();
  const disableResource = useDisableCatalogResourceMutation();
  const publishVersion = usePublishCatalogVersionMutation();
  const disableVersion = useDisableCatalogVersionMutation();
  const createPolicyBinding = useCreatePolicyBindingMutation();
  const disablePolicyBinding = useDisablePolicyBindingMutation();
  const loadAgentVersionCapabilities = useAgentVersionCapabilitiesMutation();
  const validateAgentVersion = useValidateAgentVersionMutation();

  useEffect(() => {
    setActiveTab(initialTab);
    setSelectedId(null);
    setCreateOpen(false);
    setMcpConfigOpen(false);
    setMcpConfigTarget(null);
    setMcpToolConfigOpen(false);
    setMcpToolConfigTarget(null);
    setMcpActionError(null);
    setMcpDiscoverSummary(null);
    setPolicyCreateOpen(false);
    setPublishOpen(false);
    setDetailTab("overview");
    setConfirmAction(null);
    setVersionInsight(null);
  }, [initialTab]);

  const resourceKind = resourceKindFor(activeTab, llmMode);
  const editableKind = isEditableTab(activeTab) ? activeTab : null;
  const editable = Boolean(editableKind);
  const resourceQuery = useCatalogResourcesQuery({
    tenantId: me.tenantId,
    kind: resourceKind,
    status: status === "all" ? undefined : status,
    limit: 100
  });
  const resources = useMemo(() => resourceQuery.data ?? [], [resourceQuery.data]);

  useEffect(() => {
    if (resources.length === 0) {
      setSelectedId(null);
      return;
    }
    setSelectedId((current) =>
      current && resources.some((resource) => resource.id === current) ? current : resources[0].id
    );
  }, [resources]);

  useEffect(() => {
    setPolicyCreateOpen(false);
    setCreateOpen(false);
    setMcpConfigOpen(false);
    setMcpConfigTarget(null);
    setMcpToolConfigOpen(false);
    setMcpToolConfigTarget(null);
    setMcpActionError(null);
    setMcpDiscoverSummary(null);
    setVersionInsight(null);
    setPolicyCreateOpen(false);
  }, [activeTab, llmMode, selectedId]);

  const selected = useMemo(
    () => resources.find((resource) => resource.id === selectedId) ?? resources[0],
    [resources, selectedId]
  );
  const versionedKind = isVersionedTab(activeTab) ? activeTab : null;
  const versions = useCatalogVersionsQuery(
    {
      tenantId: me.tenantId,
      kind: versionedKind ?? "agents",
      resourceId: selected?.id ?? "",
      limit: 50
    },
    Boolean(versionedKind && selected)
  );
  const mcpTools = useMcpToolsQuery(
    {
      tenantId: me.tenantId,
      mcpServerId: selected?.id ?? "",
      status: status === "all" ? undefined : status,
      limit: 100
    },
    activeTab === "mcp" && Boolean(selected)
  );
  const policies = usePolicyBindingsQuery(
    {
      tenantId: me.tenantId,
      resourceType: selected ? policyResourceType(activeTab, llmMode) : undefined,
      resourceId: selected?.id,
      includeDisabled: true,
      limit: 100
    },
    Boolean(selected)
  );

  async function disableSelectedResource() {
    if (!selected || !editableKind || selected.status === "disabled") {
      return;
    }
    await disableResource.mutateAsync({
      tenantId: me.tenantId,
      kind: editableKind,
      resourceId: selected.id
    });
    await resourceQuery.refetch();
  }

  async function disableSelectedVersion(version: Version) {
    if (!versionedKind) {
      return;
    }
    await disableVersion.mutateAsync({
      tenantId: me.tenantId,
      kind: versionedKind,
      versionId: version.id,
      resourceId: version.parentId
    });
    await versions.refetch();
  }

  async function showVersionCapabilities(version: Version) {
    try {
      const data = await loadAgentVersionCapabilities.mutateAsync({
        tenantId: me.tenantId,
        agentVersionId: version.id
      });
      setVersionInsight({ versionId: version.id, mode: "capabilities", data });
    } catch (caught) {
      setVersionInsight({
        versionId: version.id,
        mode: "error",
        message: errorMessage(caught, t("catalog.form.submitFailed"))
      });
    }
  }

  async function runVersionValidation(version: Version) {
    try {
      const data = await validateAgentVersion.mutateAsync({
        tenantId: me.tenantId,
        agentVersionId: version.id
      });
      setVersionInsight({ versionId: version.id, mode: "validation", data });
    } catch (caught) {
      setVersionInsight({
        versionId: version.id,
        mode: "error",
        message: errorMessage(caught, t("catalog.form.submitFailed"))
      });
    }
  }

  async function disableSelectedPolicy(policy: PolicyBinding) {
    if (policy.disabledAt) {
      return;
    }
    await disablePolicyBinding.mutateAsync({
      tenantId: me.tenantId,
      bindingId: policy.id,
      resourceType: policy.resourceType,
      resourceId: policy.resourceId
    });
    await policies.refetch();
  }

  async function discoverSelectedMcpTools() {
    if (!selected || activeTab !== "mcp") {
      return;
    }
    setMcpActionError(null);
    setMcpDiscoverSummary(null);
    try {
      const before = new Map(
        (mcpTools.data ?? []).map((tool) => [
          tool.name,
          jsonString(jsonRecord(tool.metadata).schema_hash)
        ])
      );
      const discovered = await discoverMcpTools.mutateAsync({
        tenantId: me.tenantId,
        mcpServerId: selected.id
      });
      setMcpDiscoverSummary(compareMcpDiscoverResult(before, discovered));
      await mcpTools.refetch();
      await resourceQuery.refetch();
    } catch (caught) {
      setMcpActionError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  async function disableSelectedMcpTool(tool: Resource) {
    if (!selected || tool.status === "disabled") {
      return;
    }
    setMcpActionError(null);
    try {
      await disableMcpTool.mutateAsync({
        tenantId: me.tenantId,
        mcpServerId: selected.id,
        mcpToolId: tool.id
      });
      await mcpTools.refetch();
    } catch (caught) {
      setMcpActionError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  async function confirmDangerAction() {
    if (!confirmAction) {
      return;
    }
    if (confirmAction.type === "resource") {
      await disableSelectedResource();
    } else if (confirmAction.type === "version") {
      await disableSelectedVersion(confirmAction.version);
    } else if (confirmAction.type === "policy") {
      await disableSelectedPolicy(confirmAction.policy);
    } else {
      await disableSelectedMcpTool(confirmAction.tool);
    }
    setConfirmAction(null);
  }

  function changeActiveTab(tab: CatalogTab) {
    setActiveTab(tab);
    setSelectedId(null);
    setCreateOpen(false);
    setMcpConfigOpen(false);
    setMcpConfigTarget(null);
    setMcpToolConfigOpen(false);
    setMcpToolConfigTarget(null);
    setMcpActionError(null);
    setMcpDiscoverSummary(null);
    setPolicyCreateOpen(false);
    setDetailTab("overview");
    setConfirmAction(null);
    setVersionInsight(null);
    onTabRouteChange?.(tab);
  }

  const detailTabs = catalogDetailTabs(activeTab, Boolean(versionedKind));
  const safeDetailTab = detailTabs.some((item) => item.id === detailTab) ? detailTab : "overview";
  const confirmMessage = catalogConfirmMessage(confirmAction, selected, t);
  const confirmPending =
    disableResource.isPending ||
    disableVersion.isPending ||
    disablePolicyBinding.isPending ||
    disableMcpTool.isPending;

  return (
    <>
      <div className="catalog-grid">
        <Panel
          className="catalog-list-panel"
          title={t("catalog.title")}
          subtitle={t(catalogSubtitle(activeTab, editable))}
          actions={
            <>
              {editableKind ? (
                <Button
                  size="sm"
                  variant="secondary"
                  aria-label={t(catalogCreateTitle(editableKind))}
                  icon={<Plus size={15} />}
                  onClick={() => setCreateOpen((open) => !open)}
                >
                  {t(catalogCreateTitle(editableKind))}
                </Button>
              ) : null}
              {activeTab === "mcp" ? (
                <Button
                  size="sm"
                  variant="secondary"
                  aria-label={t("catalog.mcp.addServer")}
                  icon={<Plus size={15} />}
                  onClick={() => {
                    setMcpConfigTarget(null);
                    setMcpConfigOpen(true);
                  }}
                >
                  {t("catalog.mcp.addServer")}
                </Button>
              ) : null}
              <Button
                size="icon"
                variant="ghost"
                aria-label={t("catalog.refresh")}
                icon={<RefreshCw size={15} />}
                onClick={() => resourceQuery.refetch()}
              />
            </>
          }
        >
          <div className="catalog-controls">
            <Tabs
              active={activeTab}
              items={tabs.map((item) => ({ id: item.id, label: t(item.labelKey) }))}
              onChange={changeActiveTab}
            />
            <label className="field-stack">
              <span>{t("catalog.statusFilter")}</span>
              <select
                className="text-input"
                value={status}
                onChange={(event) => {
                  setStatus(event.target.value as StatusFilter);
                  setSelectedId(null);
                }}
              >
                {statusFilters.map((item) => (
                  <option key={item} value={item}>
                    {t(statusFilterLabel(item))}
                  </option>
                ))}
              </select>
            </label>
            {activeTab === "llm" ? (
              <label className="field-stack">
                <span>{t("catalog.llmMode")}</span>
                <select
                  className="text-input"
                  value={llmMode}
                  onChange={(event) => {
                    setLlmMode(event.target.value as LlmMode);
                    setSelectedId(null);
                  }}
                >
                  <option value="providers">{t("catalog.llm.providers")}</option>
                  <option value="profiles">{t("catalog.llm.profiles")}</option>
                </select>
              </label>
            ) : null}
          </div>
          <div className="catalog-list-body">
            {activeTab === "mcp" ? <McpSetupNotice /> : null}
            <ResourceList>
              {resourceQuery.isLoading ? (
                <EmptyState title={t("common.loading")} detail={t("catalog.loading")} />
              ) : null}
              {!resourceQuery.isLoading && resources.length === 0 ? (
                <>
                  <EmptyState
                    title={t(catalogEmptyTitle(activeTab))}
                    detail={t(catalogEmptyDetail(activeTab))}
                  />
                  <CatalogEmptyGuide tab={activeTab} />
                  {editableKind ? (
                    <div className="empty-state-action">
                      <Button
                        size="sm"
                        variant="primary"
                        icon={<Plus size={15} />}
                        onClick={() => setCreateOpen(true)}
                      >
                        {t(catalogCreateTitle(editableKind))}
                      </Button>
                    </div>
                  ) : null}
                </>
              ) : null}
              {resources.map((resource) => {
                const Icon = tabs.find((item) => item.id === activeTab)?.icon ?? Layers3;
                return (
                  <button
                    key={resource.id}
                    className={`resource-row ${resource.id === selected?.id ? "active" : ""}`}
                    onClick={() => setSelectedId(resource.id)}
                  >
                    <Icon size={17} />
                    <span>
                      <strong>{resource.name}</strong>
                      <span>{resource.description ?? resource.id}</span>
                    </span>
                    <StatusPill status={resource.status} />
                  </button>
                );
              })}
            </ResourceList>
          </div>
        </Panel>

        <Panel
          className="catalog-detail-panel"
          title={selected?.name ?? t("catalog.detail")}
          subtitle={selected ? resourceTypeLabel(activeTab, llmMode, t) : t("catalog.noSelection")}
          actions={
            <>
              {selected ? <StatusPill status={selected.status} /> : null}
              {activeTab === "mcp" && selected ? (
                <Button
                  size="sm"
                  variant="secondary"
                  icon={<Search size={14} />}
                  disabled={discoverMcpTools.isPending || selected.status !== "active"}
                  onClick={discoverSelectedMcpTools}
                >
                  {t("catalog.mcp.discoverTools")}
                </Button>
              ) : null}
              {versionedKind && selected && selected.status !== "disabled" ? (
                <Button
                  size="sm"
                  variant="secondary"
                  icon={<Send size={14} />}
                  onClick={() => setPublishOpen(true)}
                >
                  {t("catalog.publishVersion")}
                </Button>
              ) : null}
              {selected ? (
                <ActionMenu
                  label={t("common.moreActions")}
                  items={[
                    ...(activeTab === "mcp"
                      ? [
                          {
                            label: t("catalog.mcp.editServer"),
                            icon: <Pencil size={14} />,
                            onSelect: () => {
                              setMcpConfigTarget(selected);
                              setMcpConfigOpen(true);
                            }
                          }
                        ]
                      : []),
                    ...(editableKind && selected.status !== "disabled"
                      ? [
                          {
                            label: t("common.disable"),
                            icon: <Ban size={14} />,
                            danger: true,
                            disabled: disableResource.isPending,
                            onSelect: () => setConfirmAction({ type: "resource" })
                          }
                        ]
                      : [])
                  ]}
                />
              ) : null}
            </>
          }
        >
          {selected ? (
            <>
              <div className="catalog-detail-tabs">
                <Tabs
                  active={safeDetailTab}
                  items={detailTabs.map((item) => ({ id: item.id, label: t(item.labelKey) }))}
                  onChange={setDetailTab}
                />
              </div>
              <div className="catalog-detail-scroll">
                {mcpActionError ? <p className="form-error">{mcpActionError}</p> : null}
                {safeDetailTab === "overview" ? <ResourceDetail resource={selected} /> : null}
                {safeDetailTab === "versions" && versionedKind ? (
                  <VersionList
                    versions={versions.data ?? []}
                    loading={versions.isLoading}
                    editableKind={versionedKind}
                    disablingVersionId={disableVersion.variables?.versionId}
                    onDisableVersion={(version) => setConfirmAction({ type: "version", version })}
                    insight={versionInsight}
                    capabilitiesLoadingId={loadAgentVersionCapabilities.variables?.agentVersionId}
                    validationLoadingId={validateAgentVersion.variables?.agentVersionId}
                    onShowCapabilities={
                      activeTab === "agents" ? showVersionCapabilities : undefined
                    }
                    onValidateVersion={activeTab === "agents" ? runVersionValidation : undefined}
                  />
                ) : null}
                {safeDetailTab === "capabilities" && activeTab === "mcp" ? (
                  <>
                    {mcpDiscoverSummary ? (
                      <McpDiscoverSummaryPanel summary={mcpDiscoverSummary} />
                    ) : null}
                    <ResourceMiniList
                      title={t("catalog.mcpTools")}
                      resources={mcpTools.data ?? []}
                      loading={mcpTools.isLoading}
                      emptyDetail={t("catalog.noMcpTools")}
                      disablingResourceId={disableMcpTool.variables?.mcpToolId}
                      onEditResource={(resource) => {
                        setMcpToolConfigTarget(resource);
                        setMcpToolConfigOpen(true);
                      }}
                      onDisableResource={(tool) => setConfirmAction({ type: "mcpTool", tool })}
                    />
                  </>
                ) : null}
                {safeDetailTab === "capabilities" && activeTab !== "mcp" ? (
                  <CatalogCapabilitiesPanel insight={versionInsight} />
                ) : null}
                {safeDetailTab === "policies" ? (
                  <div className="catalog-section">
                    <div className="section-title">
                      <ShieldCheck size={16} />
                      <strong>{t("catalog.policyBindings")}</strong>
                      <span>{t("catalog.policyScope")}</span>
                      <Button
                        size="sm"
                        variant="secondary"
                        icon={<Plus size={13} />}
                        onClick={() => setPolicyCreateOpen(true)}
                      >
                        {t("catalog.policy.createBinding")}
                      </Button>
                    </div>
                    <PolicyList
                      policies={policies.data ?? []}
                      loading={policies.isLoading}
                      disablingPolicyId={disablePolicyBinding.variables?.bindingId}
                      onDisablePolicy={(policy) => setConfirmAction({ type: "policy", policy })}
                    />
                  </div>
                ) : null}
              </div>
            </>
          ) : (
            <EmptyState
              title={t("catalog.noSelection")}
              detail={t(catalogNoSelectionDetail(activeTab))}
            />
          )}
        </Panel>
      </div>
      {editableKind && createOpen ? (
        <ConfigPanel
          title={t(catalogCreateTitle(editableKind))}
          subtitle={t(catalogCreateHint(editableKind))}
          closeLabel={t("common.close")}
          onClose={() => setCreateOpen(false)}
        >
          <CreateCatalogResourceForm
            key={editableKind}
            kind={editableKind}
            pending={createResource.isPending}
            onCancel={() => setCreateOpen(false)}
            onSubmit={async (draft) => {
              const created = await createResource.mutateAsync({
                tenantId: me.tenantId,
                kind: editableKind,
                ...draft
              });
              setSelectedId(created.id);
              setCreateOpen(false);
            }}
          />
        </ConfigPanel>
      ) : null}
      {mcpConfigOpen ? (
        <ConfigPanel
          title={t(mcpConfigTarget ? "catalog.mcp.editServer" : "catalog.mcp.addServer")}
          subtitle={t("catalog.mcp.configSubtitle")}
          closeLabel={t("common.close")}
          onClose={() => {
            setMcpConfigOpen(false);
            setMcpConfigTarget(null);
          }}
        >
          <McpConfigForm
            tenantId={me.tenantId}
            initialResource={mcpConfigTarget}
            pending={createMcpServer.isPending || updateMcpServer.isPending}
            onCancel={() => {
              setMcpConfigOpen(false);
              setMcpConfigTarget(null);
            }}
            onSubmit={async (draft) => {
              setMcpActionError(null);
              const saved = mcpConfigTarget
                ? await updateMcpServer.mutateAsync({
                    tenantId: me.tenantId,
                    mcpServerId: mcpConfigTarget.id,
                    ...draft
                  })
                : await createMcpServer.mutateAsync({
                    tenantId: me.tenantId,
                    ...draft
                  });
              setSelectedId(saved.id);
              setMcpConfigOpen(false);
              setMcpConfigTarget(null);
              await resourceQuery.refetch();
              if (draft.discoverAfterSave) {
                await discoverMcpTools.mutateAsync({
                  tenantId: me.tenantId,
                  mcpServerId: saved.id
                });
                await mcpTools.refetch();
              }
            }}
          />
        </ConfigPanel>
      ) : null}
      {selected && mcpToolConfigOpen && mcpToolConfigTarget ? (
        <ConfigPanel
          title={t("catalog.mcp.editTool")}
          subtitle={t("catalog.mcp.toolConfigSubtitle")}
          closeLabel={t("common.close")}
          onClose={() => {
            setMcpToolConfigOpen(false);
            setMcpToolConfigTarget(null);
          }}
        >
          <McpToolConfigForm
            initialResource={mcpToolConfigTarget}
            pending={updateMcpTool.isPending}
            onCancel={() => {
              setMcpToolConfigOpen(false);
              setMcpToolConfigTarget(null);
            }}
            onSubmit={async (draft) => {
              setMcpActionError(null);
              await updateMcpTool.mutateAsync({
                tenantId: me.tenantId,
                mcpServerId: selected.id,
                mcpToolId: mcpToolConfigTarget.id,
                ...draft
              });
              setMcpToolConfigOpen(false);
              setMcpToolConfigTarget(null);
              await mcpTools.refetch();
            }}
          />
        </ConfigPanel>
      ) : null}
      {selected && policyCreateOpen ? (
        <ConfigPanel
          title={t("catalog.policy.createBinding")}
          subtitle={t("catalog.policy.createSubtitle")}
          closeLabel={t("common.close")}
          onClose={() => setPolicyCreateOpen(false)}
        >
          <CreatePolicyBindingForm
            resourceType={policyResourceType(activeTab, llmMode)}
            resourceId={selected.id}
            pending={createPolicyBinding.isPending}
            onCancel={() => setPolicyCreateOpen(false)}
            onSubmit={async (draft) => {
              await createPolicyBinding.mutateAsync({
                tenantId: me.tenantId,
                resourceType: policyResourceType(activeTab, llmMode),
                resourceId: selected.id,
                ...draft
              });
              setPolicyCreateOpen(false);
              await policies.refetch();
            }}
          />
        </ConfigPanel>
      ) : null}
      {selected && versionedKind && publishOpen ? (
        <ConfigPanel
          title={t("catalog.publishVersion")}
          subtitle={selected.name}
          closeLabel={t("common.close")}
          onClose={() => setPublishOpen(false)}
        >
          <PublishCatalogVersionForm
            kind={versionedKind}
            pending={publishVersion.isPending}
            onSubmit={async (draft) => {
              await publishVersion.mutateAsync({
                tenantId: me.tenantId,
                kind: versionedKind,
                resourceId: selected.id,
                ...draft
              });
              setPublishOpen(false);
              await versions.refetch();
            }}
          />
        </ConfigPanel>
      ) : null}
      {confirmAction && confirmMessage ? (
        <ConfirmDialog
          title={t("common.confirmAction")}
          message={confirmMessage}
          confirmLabel={t("common.disable")}
          cancelLabel={t("common.cancel")}
          pending={confirmPending}
          onCancel={() => setConfirmAction(null)}
          onConfirm={confirmDangerAction}
        />
      ) : null}
    </>
  );
}

function McpSetupNotice() {
  const { t } = useI18n();
  return (
    <div className="catalog-notice">
      <strong>{t("catalog.mcp.setupTitle")}</strong>
      <span>{t("catalog.mcp.setupDetail")}</span>
    </div>
  );
}

function CatalogEmptyGuide({ tab }: { tab: CatalogTab }) {
  const { t } = useI18n();
  const steps = catalogEmptyGuideSteps(tab);
  return (
    <ol className="catalog-empty-guide" aria-label={t("catalog.emptyGuide.title")}>
      {steps.map((stepKey, index) => (
        <li key={stepKey}>
          <span>{index + 1}</span>
          <strong>{t(stepKey)}</strong>
        </li>
      ))}
    </ol>
  );
}

function catalogEmptyGuideSteps(tab: CatalogTab): I18nKey[] {
  if (tab === "mcp") {
    return [
      "catalog.emptyGuide.mcp.addServer",
      "catalog.emptyGuide.mcp.discover",
      "catalog.emptyGuide.mcp.policy"
    ];
  }
  if (tab === "llm") {
    return [
      "catalog.emptyGuide.llm.provider",
      "catalog.emptyGuide.llm.credential",
      "catalog.emptyGuide.llm.profile"
    ];
  }
  if (tab === "tools") {
    return [
      "catalog.emptyGuide.resource.create",
      "catalog.emptyGuide.tool.schema",
      "catalog.emptyGuide.resource.publish"
    ];
  }
  if (tab === "skills") {
    return [
      "catalog.emptyGuide.resource.create",
      "catalog.emptyGuide.skill.source",
      "catalog.emptyGuide.resource.publish"
    ];
  }
  return [
    "catalog.emptyGuide.resource.create",
    "catalog.emptyGuide.resource.publish",
    "catalog.emptyGuide.agent.bind"
  ];
}

function catalogDetailTabs(
  tab: CatalogTab,
  hasVersions: boolean
): Array<{ id: CatalogDetailTab; labelKey: I18nKey }> {
  const items: Array<{ id: CatalogDetailTab; labelKey: I18nKey }> = [
    { id: "overview", labelKey: "catalog.detailTab.overview" }
  ];
  if (hasVersions) {
    items.push({ id: "versions", labelKey: "catalog.detailTab.versions" });
  }
  items.push({
    id: "capabilities",
    labelKey: tab === "mcp" ? "catalog.mcpTools" : "catalog.detailTab.capabilities"
  });
  items.push({ id: "policies", labelKey: "catalog.detailTab.policies" });
  return items;
}

function catalogConfirmMessage(
  action: CatalogConfirmAction | null,
  selected: Resource | undefined,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
): string | null {
  if (!action) {
    return null;
  }
  if (action.type === "resource") {
    return t("catalog.confirmDisableResource", { name: selected?.name ?? "" });
  }
  if (action.type === "version") {
    return t("catalog.confirmDisableVersion", { label: action.version.versionLabel });
  }
  if (action.type === "policy") {
    return t("catalog.confirmDisablePolicy");
  }
  return t("catalog.mcp.confirmDisableTool", { name: action.tool.name });
}

type McpServerDraft = Omit<CreateMcpServerInput, "tenantId"> & { discoverAfterSave: boolean };

function McpConfigForm({
  tenantId,
  initialResource,
  pending,
  onSubmit,
  onCancel
}: {
  tenantId: string;
  initialResource: Resource | null;
  pending: boolean;
  onSubmit: (draft: McpServerDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const metadata = jsonRecord(initialResource?.metadata);
  const [serverName, setServerName] = useState(initialResource?.name ?? "");
  const [description, setDescription] = useState(initialResource?.description ?? "");
  const [transport, setTransport] = useState(jsonString(metadata.transport, "http"));
  const [endpoint, setEndpoint] = useState("");
  const [secretRef, setSecretRef] = useState("");
  const [config, setConfig] = useState("");
  const [discoverAfterSave, setDiscoverAfterSave] = useState(!initialResource);
  const [error, setError] = useState<string | null>(null);
  const secretRefs = useSecretRefsQuery({ tenantId, purpose: "mcp" });

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!serverName.trim()) {
        setError(t("catalog.form.nameRequired"));
        return;
      }
      const parsedConfig = parseOptionalJson(config, t("catalog.mcp.configJson"), t);
      const mergedConfig = mergeMcpConfig(parsedConfig, endpoint, t("catalog.mcp.configJson"), t);
      await onSubmit({
        name: serverName.trim(),
        description: optionalText(description),
        transport,
        config: mergedConfig,
        secretRef: optionalText(secretRef),
        discoverAfterSave
      });
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <p className="config-help">
        {initialResource ? t("catalog.mcp.updateHint") : t("catalog.mcp.createHint")}
      </p>
      <label className="field-stack">
        <span>{t("catalog.mcp.serverName")}</span>
        <TextInput value={serverName} onChange={(event) => setServerName(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("catalog.form.description")}</span>
        <TextInput value={description} onChange={(event) => setDescription(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("catalog.mcp.transport")}</span>
        <select
          className="text-input"
          value={transport}
          onChange={(event) => setTransport(event.target.value)}
        >
          <option value="http">http</option>
          <option value="json-rpc">json-rpc</option>
          <option value="sse">sse</option>
          <option value="streamable-http">streamable-http</option>
        </select>
      </label>
      <label className="field-stack">
        <span>{t("catalog.mcp.endpoint")}</span>
        <TextInput
          value={endpoint}
          placeholder="http://127.0.0.1:9100/mcp"
          onChange={(event) => setEndpoint(event.target.value)}
        />
      </label>
      <label className="field-stack">
        <span>{t("secretRefs.picker")}</span>
        <select
          className="text-input"
          value=""
          onChange={(event) => {
            if (event.target.value) {
              setSecretRef(event.target.value);
            }
          }}
        >
          <option value="">{t("secretRefs.select")}</option>
          {(secretRefs.data ?? []).map((item) => (
            <option key={item.id} value={item.id}>
              {item.label} · {item.scheme}
              {item.available ? "" : ` · ${t("secretRefs.unavailable")}`}
            </option>
          ))}
        </select>
      </label>
      <label className="field-stack">
        <span>{t("catalog.mcp.auth")}</span>
        <TextInput
          value={secretRef}
          placeholder={t("catalog.mcp.authPlaceholder")}
          onChange={(event) => setSecretRef(event.target.value)}
        />
      </label>
      <details className="config-advanced">
        <summary>{t("catalog.form.advanced")}</summary>
        <label className="field-stack">
          <span>{t("catalog.mcp.configJson")}</span>
          <TextArea
            value={config}
            placeholder={mcpConfigPlaceholder()}
            onChange={(event) => setConfig(event.target.value)}
          />
        </label>
      </details>
      <label className="checkbox-row">
        <input
          type="checkbox"
          checked={discoverAfterSave}
          onChange={(event) => setDiscoverAfterSave(event.target.checked)}
        />
        <span>{t("catalog.mcp.discoverAfterSave")}</span>
      </label>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<Server size={15} />} disabled={pending}>
          {initialResource ? t("llm.update") : t("common.create")}
        </Button>
      </div>
    </form>
  );
}

type McpToolDraft = Omit<UpdateMcpToolInput, "tenantId" | "mcpToolId" | "mcpServerId">;

function McpToolConfigForm({
  initialResource,
  pending,
  onSubmit,
  onCancel
}: {
  initialResource: Resource;
  pending: boolean;
  onSubmit: (draft: McpToolDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const metadata = jsonRecord(initialResource.metadata);
  const [name, setName] = useState(initialResource.name);
  const [description, setDescription] = useState(initialResource.description ?? "");
  const [schema, setSchema] = useState(formatJson(metadata.schema ?? {}));
  const [schemaHash, setSchemaHash] = useState(jsonString(metadata.schema_hash));
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!name.trim()) {
        setError(t("catalog.form.nameRequired"));
        return;
      }
      await onSubmit({
        name: name.trim(),
        description: optionalText(description),
        schema: parseOptionalJson(schema, t("catalog.form.schema"), t),
        schemaHash: optionalText(schemaHash)
      });
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <label className="field-stack">
        <span>{t("catalog.form.name")}</span>
        <TextInput value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("catalog.form.description")}</span>
        <TextInput value={description} onChange={(event) => setDescription(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("catalog.form.schemaHash")}</span>
        <TextInput value={schemaHash} onChange={(event) => setSchemaHash(event.target.value)} />
      </label>
      <label className="field-stack">
        <span>{t("catalog.form.schema")}</span>
        <TextArea value={schema} onChange={(event) => setSchema(event.target.value)} />
      </label>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<Pencil size={15} />} disabled={pending}>
          {t("llm.update")}
        </Button>
      </div>
    </form>
  );
}

function catalogSubtitle(tab: CatalogTab, editable: boolean): I18nKey {
  if (tab === "mcp") {
    return "catalog.mcp.subtitle";
  }
  if (tab === "skills") {
    return "catalog.skills.subtitle";
  }
  if (tab === "tools") {
    return "catalog.tools.subtitle";
  }
  if (tab === "agents") {
    return "catalog.agents.subtitle";
  }
  return editable ? "catalog.management" : "catalog.readonly";
}

function catalogEmptyTitle(tab: CatalogTab): I18nKey {
  if (tab === "mcp") {
    return "catalog.empty.mcp";
  }
  if (tab === "skills") {
    return "catalog.empty.skills";
  }
  if (tab === "tools") {
    return "catalog.empty.tools";
  }
  if (tab === "agents") {
    return "catalog.empty.agents";
  }
  return "catalog.empty";
}

function catalogEmptyDetail(tab: CatalogTab): I18nKey {
  if (tab === "mcp") {
    return "catalog.emptyDetail.mcp";
  }
  if (tab === "skills") {
    return "catalog.emptyDetail.skills";
  }
  if (tab === "tools") {
    return "catalog.emptyDetail.tools";
  }
  if (tab === "agents") {
    return "catalog.emptyDetail.agents";
  }
  return "catalog.emptyDetail";
}

function catalogNoSelectionDetail(tab: CatalogTab): I18nKey {
  if (tab === "mcp") {
    return "catalog.noSelectionDetail.mcp";
  }
  if (tab === "tools") {
    return "catalog.noSelectionDetail.tools";
  }
  if (tab === "skills") {
    return "catalog.noSelectionDetail.skills";
  }
  return "catalog.noSelectionDetail";
}

function ResourceDetail({ resource }: { resource: Resource }) {
  const { t } = useI18n();
  return (
    <div className="catalog-section">
      <div className="section-title">
        <Layers3 size={16} />
        <strong>{t("catalog.detail")}</strong>
      </div>
      <dl className="key-value catalog-key-value">
        <dt>{t("catalog.meta.createdAt")}</dt>
        <dd>{resource.createdAt}</dd>
        <dt>{t("catalog.meta.updatedAt")}</dt>
        <dd>{resource.updatedAt ?? t("common.notRecorded")}</dd>
      </dl>
      <ResourceSummary metadata={resource.metadata} />
      <details className="profile-policy-json">
        <summary>{t("common.advanced")}</summary>
        <dl className="key-value catalog-key-value">
          <dt>{t("catalog.meta.id")}</dt>
          <dd>{resource.id}</dd>
        </dl>
        <pre className="catalog-json">{formatJson(resource.metadata)}</pre>
      </details>
    </div>
  );
}

function ResourceSummary({ metadata }: { metadata: JsonValue }) {
  const { t } = useI18n();
  const fields = [
    ["catalog.meta.transport", jsonString(jsonRecord(metadata).transport)],
    ["catalog.meta.hasConfig", jsonBooleanLabel(jsonRecord(metadata).has_config, t)],
    ["catalog.meta.hasSecret", jsonBooleanLabel(jsonRecord(metadata).has_secret_ref, t)],
    ["catalog.form.schemaHash", jsonString(jsonRecord(metadata).schema_hash)],
    ["catalog.policy.resource", jsonString(jsonRecord(metadata).resource_type)]
  ].filter(([, value]) => value);

  if (!fields.length) {
    return null;
  }

  return (
    <dl className="compact-dl catalog-summary-dl">
      {fields.map(([labelKey, value]) => (
        <div key={labelKey}>
          <dt>{t(labelKey as I18nKey)}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function catalogCreateTitle(kind: EditableCatalogKind): I18nKey {
  if (kind === "skills") {
    return "catalog.createSkill";
  }
  if (kind === "tools") {
    return "catalog.createTool";
  }
  return "catalog.createAgent";
}

function catalogCreateHint(kind: EditableCatalogKind): I18nKey {
  if (kind === "skills") {
    return "catalog.form.skillHint";
  }
  if (kind === "tools") {
    return "catalog.form.toolHint";
  }
  return "catalog.form.agentHint";
}

function catalogNamePlaceholder(kind: EditableCatalogKind): I18nKey {
  if (kind === "skills") {
    return "catalog.form.skillNamePlaceholder";
  }
  if (kind === "tools") {
    return "catalog.form.toolNamePlaceholder";
  }
  return "catalog.form.agentNamePlaceholder";
}

function defaultDraftConfig(_kind: EditableCatalogKind): string {
  return "{\n}";
}

function defaultToolSchema(): string {
  return '{\n  "input": {},\n  "output": {}\n}';
}

function metadataPlaceholder(kind: EditableCatalogKind): string {
  if (kind === "skills") {
    return '{\n  "tags": ["research"],\n  "owner": "team-name"\n}';
  }
  if (kind === "tools") {
    return '{\n  "source": "internal",\n  "side_effects": "none"\n}';
  }
  return '{\n  "purpose": "support team workflow"\n}';
}

interface CreateResourceDraft {
  name: string;
  description?: string;
  metadata?: JsonValue;
  draftConfig?: JsonValue;
  toolType?: string;
  schema?: JsonValue;
}

interface CreateResourceTemplate {
  label: string;
  name: string;
  description: string;
  agentModelProfileId?: string;
  agentSystemPrompt?: string;
  toolType?: string;
  schema?: string;
}

function mergeAgentDraftConfig(
  rawConfig: JsonValue | undefined,
  modelProfileId: string,
  systemPrompt: string
): JsonValue | undefined {
  const normalized =
    rawConfig && typeof rawConfig === "object" && !Array.isArray(rawConfig) ? { ...rawConfig } : {};
  const normalizedModelProfileId = optionalText(modelProfileId);
  const normalizedSystemPrompt = optionalText(systemPrompt);

  if (normalizedModelProfileId) {
    normalized.model_profile_id = normalizedModelProfileId;
  }
  if (normalizedSystemPrompt) {
    normalized.system_prompt = normalizedSystemPrompt;
  }
  if (Object.keys(normalized).length > 0) {
    return normalized;
  }
  return rawConfig;
}

function catalogResourceTemplates(
  kind: EditableCatalogKind,
  t: ReturnType<typeof useI18n>["t"]
): CreateResourceTemplate[] {
  if (kind === "agents") {
    return [
      {
        label: t("catalog.template.agent.qa"),
        name: t("catalog.template.agent.qa.name"),
        description: t("catalog.template.agent.qa.description"),
        agentModelProfileId: "default",
        agentSystemPrompt: t("catalog.template.agent.qa.prompt")
      }
    ];
  }
  if (kind === "skills") {
    return [
      {
        label: t("catalog.template.skill.summary"),
        name: t("catalog.template.skill.summary.name"),
        description: t("catalog.template.skill.summary.description")
      }
    ];
  }
  return [
    {
      label: t("catalog.template.tool.http"),
      name: t("catalog.template.tool.http.name"),
      description: t("catalog.template.tool.http.description"),
      toolType: "http",
      schema: '{\n  "type": "object",\n  "properties": {}\n}'
    }
  ];
}

function CreateCatalogResourceForm({
  kind,
  pending,
  onSubmit,
  onCancel
}: {
  kind: EditableCatalogKind;
  pending: boolean;
  onSubmit: (draft: CreateResourceDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [metadata, setMetadata] = useState("");
  const [agentModelProfileId, setAgentModelProfileId] = useState("");
  const [agentSystemPrompt, setAgentSystemPrompt] = useState("");
  const [draftConfig, setDraftConfig] = useState(defaultDraftConfig(kind));
  const [toolType, setToolType] = useState("custom");
  const [schema, setSchema] = useState(defaultToolSchema());
  const [error, setError] = useState<string | null>(null);
  const templates = catalogResourceTemplates(kind, t);

  function applyTemplate(template: CreateResourceTemplate) {
    setName(template.name);
    setDescription(template.description);
    if (template.agentModelProfileId !== undefined) {
      setAgentModelProfileId(template.agentModelProfileId);
    }
    if (template.agentSystemPrompt !== undefined) {
      setAgentSystemPrompt(template.agentSystemPrompt);
    }
    if (template.toolType !== undefined) {
      setToolType(template.toolType);
    }
    if (template.schema !== undefined) {
      setSchema(template.schema);
    }
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!name.trim()) {
        setError(t("catalog.form.nameRequired"));
        return;
      }
      const parsedDraftConfig =
        kind === "agents"
          ? parseOptionalJson(draftConfig, t("catalog.form.draftConfig"), t)
          : undefined;
      await onSubmit({
        name: name.trim(),
        description: optionalText(description),
        metadata: parseOptionalJson(metadata, t("catalog.form.metadata"), t),
        draftConfig:
          kind === "agents"
            ? mergeAgentDraftConfig(parsedDraftConfig, agentModelProfileId, agentSystemPrompt)
            : undefined,
        toolType: kind === "tools" ? optionalText(toolType) : undefined,
        schema:
          kind === "tools" ? parseOptionalJson(schema, t("catalog.form.schema"), t) : undefined
      });
      setName("");
      setDescription("");
      setMetadata("");
      setAgentModelProfileId("");
      setAgentSystemPrompt("");
      setDraftConfig(defaultDraftConfig(kind));
      setToolType("custom");
      setSchema(defaultToolSchema());
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <div className="config-inline-hint">
        <Badge tone="info">{resourceTypeLabel(kind, "providers", t)}</Badge>
        <span>{t(catalogCreateHint(kind))}</span>
      </div>
      <div className="template-picker">
        <span>{t("catalog.form.template")}</span>
        <div>
          {templates.map((template) => (
            <button type="button" key={template.label} onClick={() => applyTemplate(template)}>
              {template.label}
            </button>
          ))}
        </div>
      </div>
      <label className="field-stack">
        <span>{t("catalog.form.name")}</span>
        <TextInput
          value={name}
          placeholder={t(catalogNamePlaceholder(kind))}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label className="field-stack">
        <span>{t("catalog.form.description")}</span>
        <TextInput value={description} onChange={(event) => setDescription(event.target.value)} />
      </label>
      {kind === "tools" ? (
        <label className="field-stack">
          <span>{t("catalog.form.toolType")}</span>
          <select
            className="text-input"
            value={toolType}
            onChange={(event) => setToolType(event.target.value)}
          >
            {toolTypes.map((item) => (
              <option key={item} value={item}>
                {t(toolTypeLabel(item))}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      {kind === "agents" ? (
        <>
          <label className="field-stack">
            <span>{t("catalog.form.modelProfileId")}</span>
            <TextInput
              value={agentModelProfileId}
              placeholder={t("catalog.form.modelProfileIdPlaceholder")}
              onChange={(event) => setAgentModelProfileId(event.target.value)}
            />
          </label>
          <label className="field-stack">
            <span>{t("catalog.form.systemPrompt")}</span>
            <TextArea
              value={agentSystemPrompt}
              placeholder={t("catalog.form.systemPromptPlaceholder")}
              onChange={(event) => setAgentSystemPrompt(event.target.value)}
            />
          </label>
        </>
      ) : null}
      <details className="config-advanced">
        <summary>{t("catalog.form.advanced")}</summary>
        <div className="config-section-heading">
          <strong>{t("common.expertJson")}</strong>
          <span>{t("common.expertJsonHint")}</span>
        </div>
        {kind === "agents" ? (
          <label className="field-stack">
            <span>{t("catalog.form.draftConfig")}</span>
            <TextArea
              value={draftConfig}
              onChange={(event) => setDraftConfig(event.target.value)}
            />
          </label>
        ) : null}
        {kind === "tools" ? (
          <label className="field-stack">
            <span>{t("catalog.form.schema")}</span>
            <TextArea value={schema} onChange={(event) => setSchema(event.target.value)} />
          </label>
        ) : null}
        <label className="field-stack">
          <span>{t("catalog.form.metadata")}</span>
          <TextArea
            value={metadata}
            placeholder={metadataPlaceholder(kind)}
            onChange={(event) => setMetadata(event.target.value)}
          />
        </label>
      </details>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<Plus size={15} />} disabled={pending}>
          {t("common.create")}
        </Button>
      </div>
    </form>
  );
}

interface VersionDraft {
  versionLabel: string;
  snapshot?: JsonValue;
  schemaHash?: string;
  contentHash?: string;
  sourceUri?: string;
  policyVersion?: string;
}

function PublishCatalogVersionForm({
  kind,
  pending,
  onSubmit
}: {
  kind: VersionedCatalogKind;
  pending: boolean;
  onSubmit: (draft: VersionDraft) => Promise<void>;
}) {
  const { t } = useI18n();
  const [versionLabel, setVersionLabel] = useState("");
  const [snapshot, setSnapshot] = useState("{\n}");
  const [schemaHash, setSchemaHash] = useState("");
  const [contentHash, setContentHash] = useState("");
  const [sourceUri, setSourceUri] = useState("");
  const [policyVersion, setPolicyVersion] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!versionLabel.trim()) {
        setError(t("catalog.form.versionLabelRequired"));
        return;
      }
      await onSubmit({
        versionLabel: versionLabel.trim(),
        snapshot: parseOptionalJson(snapshot, t("catalog.form.snapshot"), t),
        schemaHash: optionalText(schemaHash),
        contentHash: kind === "skills" ? optionalText(contentHash) : undefined,
        sourceUri: kind === "skills" ? optionalText(sourceUri) : undefined,
        policyVersion: kind === "agents" ? optionalText(policyVersion) : undefined
      });
      setVersionLabel("");
      setSnapshot("{\n}");
      setSchemaHash("");
      setContentHash("");
      setSourceUri("");
      setPolicyVersion("");
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <div className="two-field-row">
        <label className="field-stack">
          <span>{t("catalog.form.versionLabel")}</span>
          <TextInput
            value={versionLabel}
            placeholder="v1"
            onChange={(event) => setVersionLabel(event.target.value)}
          />
        </label>
        {kind === "agents" ? (
          <label className="field-stack">
            <span>{t("catalog.form.policyVersion")}</span>
            <TextInput
              value={policyVersion}
              onChange={(event) => setPolicyVersion(event.target.value)}
            />
          </label>
        ) : null}
      </div>
      <label className="field-stack">
        <span>{t("catalog.form.snapshot")}</span>
        <TextArea value={snapshot} onChange={(event) => setSnapshot(event.target.value)} />
      </label>
      {kind === "agents" || kind === "tools" ? (
        <label className="field-stack">
          <span>{t("catalog.form.schemaHash")}</span>
          <TextInput value={schemaHash} onChange={(event) => setSchemaHash(event.target.value)} />
        </label>
      ) : null}
      {kind === "skills" ? (
        <div className="two-field-row">
          <label className="field-stack">
            <span>{t("catalog.form.contentHash")}</span>
            <TextInput
              value={contentHash}
              onChange={(event) => setContentHash(event.target.value)}
            />
          </label>
          <label className="field-stack">
            <span>{t("catalog.form.sourceUri")}</span>
            <TextInput value={sourceUri} onChange={(event) => setSourceUri(event.target.value)} />
          </label>
        </div>
      ) : null}
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="submit" variant="primary" icon={<Send size={15} />} disabled={pending}>
          {t("common.publish")}
        </Button>
      </div>
    </form>
  );
}

function VersionList({
  versions,
  loading,
  editableKind,
  disablingVersionId,
  capabilitiesLoadingId,
  validationLoadingId,
  insight,
  onDisableVersion,
  onShowCapabilities,
  onValidateVersion
}: {
  versions: Version[];
  loading: boolean;
  editableKind?: EditableCatalogKind;
  disablingVersionId?: string;
  capabilitiesLoadingId?: string;
  validationLoadingId?: string;
  insight?: VersionInsight | null;
  onDisableVersion?: (version: Version) => void | Promise<void>;
  onShowCapabilities?: (version: Version) => void | Promise<void>;
  onValidateVersion?: (version: Version) => void | Promise<void>;
}) {
  const { t } = useI18n();
  return (
    <div className="catalog-section">
      <div className="section-title">
        <Layers3 size={16} />
        <strong>{t("catalog.versions")}</strong>
        <span>{t("common.itemCount", { count: versions.length })}</span>
      </div>
      {loading ? (
        <EmptyState title={t("common.loading")} detail={t("catalog.loadingVersions")} />
      ) : null}
      {!loading && versions.length === 0 ? (
        <EmptyState title={t("catalog.noVersions")} detail={t("catalog.noVersionsDetail")} />
      ) : null}
      {versions.map((version) => (
        <details key={version.id} className="catalog-version-row">
          <summary>
            <span>
              <strong>{version.versionLabel}</strong>
              <small>{version.id}</small>
            </span>
            <span className="catalog-row-meta">
              <StatusPill status={version.status} />
              <Badge tone="neutral">{version.policyVersion ?? t("common.unbound")}</Badge>
              {onShowCapabilities ? (
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Eye size={13} />}
                  disabled={capabilitiesLoadingId === version.id}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onShowCapabilities(version);
                  }}
                >
                  {t("catalog.effectiveCapabilities")}
                </Button>
              ) : null}
              {onValidateVersion ? (
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<CheckCircle2 size={13} />}
                  disabled={validationLoadingId === version.id}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onValidateVersion(version);
                  }}
                >
                  {t("catalog.validateVersion")}
                </Button>
              ) : null}
              {editableKind && version.status !== "disabled" ? (
                <Button
                  size="sm"
                  variant="danger"
                  icon={<Ban size={13} />}
                  disabled={disablingVersionId === version.id}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onDisableVersion?.(version);
                  }}
                >
                  {t("common.disable")}
                </Button>
              ) : null}
            </span>
          </summary>
          <pre className="catalog-json">{formatJson(version.snapshot)}</pre>
          {insight?.versionId === version.id ? <VersionInsightPanel insight={insight} /> : null}
        </details>
      ))}
    </div>
  );
}

function VersionInsightPanel({ insight }: { insight: VersionInsight }) {
  const { t } = useI18n();
  if (insight.mode === "error") {
    return <p className="catalog-inline-error">{insight.message}</p>;
  }
  if (insight.mode === "validation") {
    return (
      <div className="catalog-insight">
        <div className="section-title">
          <CheckCircle2 size={16} />
          <strong>{t("catalog.validationResult")}</strong>
          <Badge tone={insight.data.valid ? "success" : "danger"}>
            {insight.data.valid ? t("catalog.valid") : t("catalog.invalid")}
          </Badge>
        </div>
        <InsightList title={t("catalog.validationErrors")} items={insight.data.errors} />
        <InsightList title={t("catalog.validationWarnings")} items={insight.data.warnings} />
      </div>
    );
  }
  return (
    <div className="catalog-insight">
      <div className="section-title">
        <Eye size={16} />
        <strong>{t("catalog.effectiveCapabilities")}</strong>
        <Badge tone="neutral">{insight.data.policyVersion}</Badge>
      </div>
      <CapabilityGroup title={t("catalog.capability.skills")} items={insight.data.skills} />
      <CapabilityGroup title={t("catalog.capability.tools")} items={insight.data.tools} />
      <CapabilityGroup title={t("catalog.capability.mcpTools")} items={insight.data.mcpTools} />
    </div>
  );
}

function CatalogCapabilitiesPanel({ insight }: { insight: VersionInsight | null }) {
  const { t } = useI18n();
  if (insight?.mode === "capabilities") {
    return <VersionInsightPanel insight={insight} />;
  }
  if (insight?.mode === "error") {
    return <p className="catalog-inline-error">{insight.message}</p>;
  }
  return (
    <div className="catalog-section">
      <div className="section-title">
        <Eye size={16} />
        <strong>{t("catalog.detailTab.capabilities")}</strong>
      </div>
      <EmptyState title={t("catalog.noCapabilities")} detail={t("catalog.capabilitiesHint")} />
    </div>
  );
}

function InsightList({ title, items }: { title: string; items: string[] }) {
  const { t } = useI18n();
  return (
    <div className="capability-group">
      <strong>{title}</strong>
      {items.length ? (
        <ul>
          {items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      ) : (
        <span>{t("common.none")}</span>
      )}
    </div>
  );
}

function CapabilityGroup({ title, items }: { title: string; items: CapabilityResource[] }) {
  const { t } = useI18n();
  return (
    <div className="capability-group">
      <strong>{title}</strong>
      {items.length ? (
        items.map((item) => (
          <div key={`${item.resourceType}:${item.resourceId}:${item.versionId ?? ""}`}>
            <span>
              {item.name} · {item.resourceType}
            </span>
            <span>{item.versionId ?? item.resourceId}</span>
            <StatusPill status={item.status} />
          </div>
        ))
      ) : (
        <span>{t("catalog.noCapabilities")}</span>
      )}
    </div>
  );
}

function ResourceMiniList({
  title,
  resources,
  loading,
  emptyDetail,
  disablingResourceId,
  onEditResource,
  onDisableResource
}: {
  title: string;
  resources: Resource[];
  loading: boolean;
  emptyDetail: string;
  disablingResourceId?: string;
  onEditResource?: (resource: Resource) => void;
  onDisableResource?: (resource: Resource) => void | Promise<void>;
}) {
  const { t } = useI18n();
  return (
    <div className="catalog-section">
      <div className="section-title">
        <Wrench size={16} />
        <strong>{title}</strong>
        <span>{t("common.itemCount", { count: resources.length })}</span>
      </div>
      {loading ? <EmptyState title={t("common.loading")} detail={t("catalog.loading")} /> : null}
      {!loading && resources.length === 0 ? (
        <EmptyState title={t("catalog.empty")} detail={emptyDetail} />
      ) : null}
      {resources.map((resource) => (
        <details key={resource.id} className="catalog-version-row">
          <summary>
            <span>
              <strong>{resource.name}</strong>
              <small>{resource.description ?? resource.id}</small>
            </span>
            <span className="catalog-row-meta">
              <StatusPill status={resource.status} />
              {onEditResource ? (
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Pencil size={13} />}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onEditResource(resource);
                  }}
                >
                  {t("catalog.mcp.editTool")}
                </Button>
              ) : null}
              {onDisableResource && resource.status !== "disabled" ? (
                <Button
                  size="sm"
                  variant="danger"
                  icon={<Ban size={13} />}
                  disabled={disablingResourceId === resource.id}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onDisableResource(resource);
                  }}
                >
                  {t("common.disable")}
                </Button>
              ) : null}
            </span>
          </summary>
          <pre className="catalog-json">{formatJson(resource.metadata)}</pre>
        </details>
      ))}
    </div>
  );
}

function McpDiscoverSummaryPanel({ summary }: { summary: McpDiscoverSummary }) {
  const { t } = useI18n();
  return (
    <div className="catalog-section catalog-discover-summary">
      <div className="section-title">
        <Search size={16} />
        <strong>{t("catalog.mcp.discoverSummary")}</strong>
        <span>{t("common.itemCount", { count: summary.total })}</span>
      </div>
      <div className="catalog-summary-grid">
        <Badge tone="success">{t("catalog.mcp.discoverNew", { count: summary.created })}</Badge>
        <Badge tone="warning">{t("catalog.mcp.discoverChanged", { count: summary.changed })}</Badge>
        <Badge tone="neutral">
          {t("catalog.mcp.discoverUnchanged", { count: summary.unchanged })}
        </Badge>
        <Badge tone={summary.missing ? "danger" : "neutral"}>
          {t("catalog.mcp.discoverMissing", { count: summary.missing })}
        </Badge>
      </div>
    </div>
  );
}

interface PolicyBindingDraft {
  action: string;
  subjectType: string;
  subjectId: string;
  effect: string;
  riskLevel?: string;
  obligations?: JsonValue;
  policyVersion?: string;
}

function CreatePolicyBindingForm({
  resourceType,
  resourceId,
  pending,
  onSubmit,
  onCancel
}: {
  resourceType: string;
  resourceId: string;
  pending: boolean;
  onSubmit: (draft: PolicyBindingDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [action, setAction] = useState("execute");
  const [subjectType, setSubjectType] = useState("role");
  const [subjectId, setSubjectId] = useState("");
  const [effect, setEffect] = useState("allow");
  const [riskLevel, setRiskLevel] = useState("low");
  const [policyVersion, setPolicyVersion] = useState("");
  const [obligations, setObligations] = useState("{\n}");
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!action.trim() || !subjectId.trim()) {
        setError(t("catalog.form.policyRequired"));
        return;
      }
      await onSubmit({
        action: action.trim(),
        subjectType,
        subjectId: subjectId.trim(),
        effect,
        riskLevel,
        policyVersion: optionalText(policyVersion),
        obligations: parseOptionalJson(obligations, t("catalog.policy.obligations"), t)
      });
      setAction("execute");
      setSubjectType("role");
      setSubjectId("");
      setEffect("allow");
      setRiskLevel("low");
      setPolicyVersion("");
      setObligations("{\n}");
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      <div className="config-readonly">
        <span>{t("catalog.policy.resource")}</span>
        <strong>
          {resourceType}:{resourceId}
        </strong>
      </div>
      <div className="two-field-row">
        <label className="field-stack">
          <span>{t("catalog.policy.action")}</span>
          <TextInput value={action} onChange={(event) => setAction(event.target.value)} />
        </label>
        <label className="field-stack">
          <span>{t("catalog.policy.subjectType")}</span>
          <select
            className="text-input"
            value={subjectType}
            onChange={(event) => setSubjectType(event.target.value)}
          >
            <option value="role">{t("catalog.policy.subjectType.role")}</option>
            <option value="user">{t("catalog.policy.subjectType.user")}</option>
            <option value="relation">{t("catalog.policy.subjectType.relation")}</option>
          </select>
        </label>
      </div>
      <label className="field-stack">
        <span>{t("catalog.policy.subjectId")}</span>
        <TextInput value={subjectId} onChange={(event) => setSubjectId(event.target.value)} />
      </label>
      <div className="two-field-row">
        <label className="field-stack">
          <span>{t("catalog.policy.effect")}</span>
          <select
            className="text-input"
            value={effect}
            onChange={(event) => setEffect(event.target.value)}
          >
            <option value="allow">{t("catalog.policy.effect.allow")}</option>
            <option value="review">{t("catalog.policy.effect.review")}</option>
            <option value="deny">{t("catalog.policy.effect.deny")}</option>
          </select>
        </label>
        <label className="field-stack">
          <span>{t("catalog.policy.riskLevel")}</span>
          <select
            className="text-input"
            value={riskLevel}
            onChange={(event) => setRiskLevel(event.target.value)}
          >
            <option value="low">{t("catalog.policy.risk.low")}</option>
            <option value="medium">{t("catalog.policy.risk.medium")}</option>
            <option value="high">{t("catalog.policy.risk.high")}</option>
            <option value="critical">{t("catalog.policy.risk.critical")}</option>
          </select>
        </label>
      </div>
      <details className="config-advanced">
        <summary>{t("catalog.form.advanced")}</summary>
        <label className="field-stack">
          <span>{t("catalog.form.policyVersion")}</span>
          <TextInput
            value={policyVersion}
            onChange={(event) => setPolicyVersion(event.target.value)}
          />
        </label>
        <label className="field-stack">
          <span>{t("catalog.policy.obligations")}</span>
          <TextArea value={obligations} onChange={(event) => setObligations(event.target.value)} />
        </label>
      </details>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" icon={<Plus size={15} />} disabled={pending}>
          {t("common.create")}
        </Button>
      </div>
    </form>
  );
}

function PolicyList({
  policies,
  loading,
  disablingPolicyId,
  onDisablePolicy
}: {
  policies: PolicyBinding[];
  loading: boolean;
  disablingPolicyId?: string;
  onDisablePolicy?: (policy: PolicyBinding) => void | Promise<void>;
}) {
  const { t } = useI18n();
  return (
    <>
      {loading ? (
        <EmptyState title={t("common.loading")} detail={t("catalog.loadingPolicies")} />
      ) : null}
      {!loading && policies.length === 0 ? (
        <EmptyState title={t("catalog.noPolicies")} detail={t("catalog.noPoliciesDetail")} />
      ) : null}
      {policies.map((policy) => (
        <details key={policy.id} className="policy-row">
          <summary>
            <span>
              <strong>
                {policy.action} · {policy.subjectType}:{policy.subjectId}
              </strong>
              <small>{policy.policyVersion}</small>
            </span>
            <span className="catalog-row-meta">
              <Badge tone={effectTone(policy.effect)}>{policy.effect}</Badge>
              <Badge tone={riskTone(policy.riskLevel)}>{policy.riskLevel}</Badge>
              {policy.disabledAt ? <StatusPill status="disabled" /> : null}
              {!policy.disabledAt ? (
                <Button
                  size="sm"
                  variant="danger"
                  icon={<Ban size={13} />}
                  disabled={disablingPolicyId === policy.id}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onDisablePolicy?.(policy);
                  }}
                >
                  {t("common.disable")}
                </Button>
              ) : null}
            </span>
          </summary>
          <dl className="compact-dl">
            <dt>{t("catalog.policy.resource")}</dt>
            <dd>
              {policy.resourceType}:{policy.resourceId}
            </dd>
            <dt>{t("catalog.policy.createdAt")}</dt>
            <dd>{policy.createdAt}</dd>
            <dt>{t("catalog.policy.disabledAt")}</dt>
            <dd>{policy.disabledAt ?? t("common.none")}</dd>
          </dl>
          <pre className="catalog-json">{formatJson(policy.obligations)}</pre>
        </details>
      ))}
    </>
  );
}

function isVersionedTab(tab: CatalogTab): tab is VersionedCatalogKind {
  return tab === "agents" || tab === "skills" || tab === "tools";
}

function isEditableTab(tab: CatalogTab): tab is EditableCatalogKind {
  return isVersionedTab(tab);
}

function resourceKindFor(tab: CatalogTab, llmMode: LlmMode): CatalogResourceKind {
  if (tab === "mcp") {
    return "mcpServers";
  }
  if (tab === "llm") {
    return llmMode === "providers" ? "llmProviders" : "llmModelProfiles";
  }
  return tab;
}

function policyResourceType(tab: CatalogTab, llmMode: LlmMode): string {
  if (tab === "mcp") {
    return "mcp_server";
  }
  if (tab === "llm") {
    return llmMode === "providers" ? "llm_provider" : "llm_model_profile";
  }
  return tab.slice(0, -1);
}

function resourceTypeLabel(tab: CatalogTab, llmMode: LlmMode, t: (key: I18nKey) => string): string {
  if (tab === "llm") {
    return llmMode === "providers" ? t("catalog.llm.providers") : t("catalog.llm.profiles");
  }
  const tabItem = tabs.find((item) => item.id === tab);
  return tabItem ? t(tabItem.labelKey) : t("catalog.title");
}

function statusFilterLabel(status: StatusFilter): I18nKey {
  return status === "all" ? "catalog.status.all" : (`status.${status}` as I18nKey);
}

function toolTypeLabel(type: (typeof toolTypes)[number]): I18nKey {
  return `catalog.toolType.${type}` as I18nKey;
}

function effectTone(effect: string): "neutral" | "info" | "success" | "warning" | "danger" {
  if (effect === "allow") {
    return "success";
  }
  if (effect === "review") {
    return "warning";
  }
  if (effect === "deny") {
    return "danger";
  }
  return "neutral";
}

function riskTone(risk: string): "neutral" | "info" | "success" | "warning" | "danger" {
  if (risk === "critical" || risk === "high") {
    return "danger";
  }
  if (risk === "medium") {
    return "warning";
  }
  if (risk === "low") {
    return "success";
  }
  return "neutral";
}

function formatJson(value: JsonValue): string {
  return JSON.stringify(redactJson(value), null, 2);
}

function jsonRecord(value: JsonValue | undefined): Record<string, JsonValue> {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function jsonString(value: JsonValue | undefined, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function jsonBooleanLabel(
  value: JsonValue | undefined,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
): string {
  if (value === true) {
    return t("common.yes");
  }
  if (value === false) {
    return t("common.no");
  }
  return "";
}

function mergeMcpConfig(
  value: JsonValue | undefined,
  endpoint: string,
  fieldLabel: string,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
): JsonValue | undefined {
  if (value && (typeof value !== "object" || Array.isArray(value))) {
    throw new Error(t("catalog.form.invalidJson", { field: fieldLabel }));
  }
  const config = { ...jsonRecord(value) };
  const normalizedEndpoint = endpoint.trim();
  if (normalizedEndpoint) {
    config.endpoint = normalizedEndpoint;
  }
  return Object.keys(config).length ? config : undefined;
}

function mcpConfigPlaceholder(): string {
  return '{\n  "timeout_ms": 30000\n}';
}

function compareMcpDiscoverResult(
  before: Map<string, string>,
  discovered: Resource[]
): McpDiscoverSummary {
  let created = 0;
  let changed = 0;
  let unchanged = 0;
  const seen = new Set<string>();
  for (const tool of discovered) {
    seen.add(tool.name);
    const previousHash = before.get(tool.name);
    const nextHash = jsonString(jsonRecord(tool.metadata).schema_hash);
    if (!previousHash) {
      created += 1;
    } else if (previousHash !== nextHash) {
      changed += 1;
    } else {
      unchanged += 1;
    }
  }
  const missing = [...before.keys()].filter((name) => !seen.has(name)).length;
  return {
    total: discovered.length,
    created,
    changed,
    unchanged,
    missing
  };
}

function optionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}

function parseOptionalJson(
  value: string,
  fieldLabel: string,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
): JsonValue | undefined {
  if (!value.trim()) {
    return undefined;
  }
  try {
    const parsed = JSON.parse(value) as unknown;
    const result = jsonValueSchema.safeParse(parsed);
    if (!result.success) {
      throw new Error("invalid json value");
    }
    return result.data;
  } catch {
    throw new Error(t("catalog.form.invalidJson", { field: fieldLabel }));
  }
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

function redactJson(value: JsonValue): JsonValue {
  if (Array.isArray(value)) {
    return value.map(redactJson);
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, child]) => [
        key,
        shouldRedact(key) ? "[redacted]" : redactJson(child)
      ])
    ) as JsonValue;
  }
  return value;
}

function shouldRedact(key: string): boolean {
  const normalized = key.toLowerCase();
  return (
    normalized.includes("secret") ||
    normalized.includes("token") ||
    normalized.includes("password") ||
    normalized.includes("authorization")
  );
}
