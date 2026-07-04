import {
  Ban,
  CheckCircle2,
  Circle,
  KeyRound,
  Plus,
  RefreshCw,
  Save,
  Server,
  SlidersHorizontal
} from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { jsonValueSchema, type Me } from "../../../shared/contracts/platform";
import { useI18n, type I18nKey } from "../../../shared/i18n";
import type { JsonValue } from "../../../shared/types/json";
import {
  ActionMenu,
  Badge,
  Button,
  ConfirmDialog,
  ConfigPanel,
  EmptyState,
  StatusPill,
  Tabs,
  TextArea,
  TextInput
} from "../../../shared/ui";
import type {
  CreateLlmCredentialInput,
  LlmCredential,
  LlmModelProfile,
  LlmProfileTestResult,
  LlmProvider
} from "../api/llm.adapter";
import {
  useCreateLlmCredentialMutation,
  useCreateLlmProfileMutation,
  useCreateLlmProviderMutation,
  useDisableLlmProfileMutation,
  useDisableLlmProviderMutation,
  useLlmCredentialsQuery,
  useLlmProfilesQuery,
  useLlmProvidersQuery,
  useRevokeLlmCredentialMutation,
  useTestLlmProfileMutation,
  useUpdateLlmProfileMutation,
  useUpdateLlmProviderMutation
} from "../api/llm.queries";
import { useSecretRefsQuery } from "../../secrets/api/secret-refs.queries";

type LlmTab = "providers" | "credentials" | "profiles";
type StatusFilter = "all" | "active" | "disabled" | "revoked";
type EditorMode = "create" | "update";
type LlmConfirmAction =
  | { type: "provider"; provider: LlmProvider }
  | { type: "credential"; credential: LlmCredential }
  | { type: "profile"; profile: LlmModelProfile };
type ProviderTemplateId = "custom" | "openaiCompatible" | "openai" | "anthropic" | "ollama";
type ResponseFormatMode = "default" | "text" | "jsonObject" | "jsonSchema" | "custom";
type ToolChoiceMode = "default" | "auto" | "none" | "required" | "custom";

const tabs: Array<{ id: LlmTab }> = [
  { id: "providers" },
  { id: "credentials" },
  { id: "profiles" }
];

const statusFilters: StatusFilter[] = ["all", "active", "disabled", "revoked"];
const responseFormatModes: Array<{ id: ResponseFormatMode; labelKey: I18nKey }> = [
  { id: "default", labelKey: "llm.profile.responseFormat.default" },
  { id: "text", labelKey: "llm.profile.responseFormat.text" },
  { id: "jsonObject", labelKey: "llm.profile.responseFormat.jsonObject" },
  { id: "jsonSchema", labelKey: "llm.profile.responseFormat.jsonSchema" },
  { id: "custom", labelKey: "llm.profile.responseFormat.custom" }
];
const toolChoiceModes: Array<{ id: ToolChoiceMode; labelKey: I18nKey }> = [
  { id: "default", labelKey: "llm.profile.toolChoice.default" },
  { id: "auto", labelKey: "llm.profile.toolChoice.auto" },
  { id: "none", labelKey: "llm.profile.toolChoice.none" },
  { id: "required", labelKey: "llm.profile.toolChoice.required" },
  { id: "custom", labelKey: "llm.profile.toolChoice.custom" }
];
const modelPresets = [
  {
    id: "balanced",
    labelKey: "llm.profile.preset.balanced",
    contextWindow: "128000",
    maxOutputTokens: "4096",
    temperature: "0.2",
    reasoningEffort: ""
  },
  {
    id: "longContext",
    labelKey: "llm.profile.preset.longContext",
    contextWindow: "200000",
    maxOutputTokens: "8192",
    temperature: "0.1",
    reasoningEffort: ""
  },
  {
    id: "deterministic",
    labelKey: "llm.profile.preset.deterministic",
    contextWindow: "32000",
    maxOutputTokens: "2048",
    temperature: "0",
    reasoningEffort: "low"
  }
] as const;

const providerTemplates: Array<{
  id: ProviderTemplateId;
  labelKey: I18nKey;
  providerKey: string;
  displayName: string;
  baseUrl: string;
  authScheme: string;
  headers: JsonValue;
}> = [
  {
    id: "openaiCompatible",
    labelKey: "llm.provider.template.openaiCompatible",
    providerKey: "openai-compatible",
    displayName: "OpenAI Compatible",
    baseUrl: "https://api.openai.com/v1",
    authScheme: "bearer",
    headers: {}
  },
  {
    id: "openai",
    labelKey: "llm.provider.template.openai",
    providerKey: "openai",
    displayName: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    authScheme: "bearer",
    headers: {}
  },
  {
    id: "anthropic",
    labelKey: "llm.provider.template.anthropic",
    providerKey: "anthropic",
    displayName: "Anthropic",
    baseUrl: "https://api.anthropic.com",
    authScheme: "api_key_header",
    headers: {}
  },
  {
    id: "ollama",
    labelKey: "llm.provider.template.ollama",
    providerKey: "ollama",
    displayName: "Ollama",
    baseUrl: "http://127.0.0.1:11434/v1",
    authScheme: "none",
    headers: {}
  }
];

interface ProviderFormDraft {
  providerKey?: string;
  displayName: string;
  baseUrl?: string;
  authScheme?: string;
  defaultHeadersTemplate?: JsonValue;
}

interface ProfileFormDraft {
  providerId: string;
  credentialId?: string;
  profileName: string;
  modelName: string;
  contextWindow?: number;
  maxInputTokens?: number;
  maxOutputTokens?: number;
  temperature?: number;
  topP?: number;
  reasoningEffort?: string;
  responseFormat?: JsonValue;
  toolChoicePolicy?: JsonValue;
  rateLimitPolicy?: JsonValue;
  costPolicy?: JsonValue;
}

export function LlmManagementScreen({ me }: { me: Me }) {
  const { t } = useI18n();
  const [activeTab, setActiveTab] = useState<LlmTab>("providers");
  const [status, setStatus] = useState<StatusFilter>("all");
  const [hideInactive, setHideInactive] = useState(true);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editorMode, setEditorMode] = useState<EditorMode>("create");
  const [editorOpen, setEditorOpen] = useState(false);
  const [profileTestResult, setProfileTestResult] = useState<LlmProfileTestResult | null>(null);
  const [profileTestError, setProfileTestError] = useState<string | null>(null);
  const [confirmAction, setConfirmAction] = useState<LlmConfirmAction | null>(null);

  const activeProviders = useLlmProvidersQuery({
    tenantId: me.tenantId,
    status: "active",
    limit: 100
  });
  const providerQuery = useLlmProvidersQuery({
    tenantId: me.tenantId,
    status: status === "all" ? undefined : status,
    limit: 100
  });
  const credentialQuery = useLlmCredentialsQuery({
    tenantId: me.tenantId,
    status: status === "all" ? undefined : status,
    limit: 100
  });
  const profileQuery = useLlmProfilesQuery({
    tenantId: me.tenantId,
    status: status === "all" ? undefined : status,
    limit: 100
  });

  const createProvider = useCreateLlmProviderMutation();
  const updateProvider = useUpdateLlmProviderMutation();
  const disableProvider = useDisableLlmProviderMutation();
  const createCredential = useCreateLlmCredentialMutation();
  const revokeCredential = useRevokeLlmCredentialMutation();
  const createProfile = useCreateLlmProfileMutation();
  const updateProfile = useUpdateLlmProfileMutation();
  const disableProfile = useDisableLlmProfileMutation();
  const testProfile = useTestLlmProfileMutation();

  const providers = providerQuery.data ?? [];
  const credentials = credentialQuery.data ?? [];
  const profiles = profileQuery.data ?? [];
  const providerChoices =
    activeProviders.data ?? providers.filter((provider) => provider.status === "active");
  const activeCredentialCount = credentials.filter(
    (credential) => credential.status === "active"
  ).length;
  const activeProfileCount = profiles.filter((profile) => profile.status === "active").length;
  const hasActiveProvider = providerChoices.some((provider) => provider.status === "active");
  const hasActiveCredential = activeCredentialCount > 0;
  const hasActiveProfile = activeProfileCount > 0;

  const currentItems = useMemo(() => {
    const items =
      activeTab === "providers" ? providers : activeTab === "credentials" ? credentials : profiles;
    if (!hideInactive || status !== "all") {
      return items;
    }
    return items.filter((item) => item.status === "active");
  }, [activeTab, credentials, hideInactive, profiles, providers, status]);

  useEffect(() => {
    if (currentItems.length === 0) {
      setSelectedId(null);
      setEditorMode("create");
      return;
    }
    setSelectedId((current) =>
      current && currentItems.some((item) => item.id === current) ? current : currentItems[0].id
    );
  }, [currentItems]);

  useEffect(() => {
    setEditorMode("create");
    setEditorOpen(false);
    setProfileTestResult(null);
    setProfileTestError(null);
  }, [activeTab, status]);

  useEffect(() => {
    setProfileTestResult(null);
    setProfileTestError(null);
  }, [selectedId]);

  const selectedProvider = providers.find((provider) => provider.id === selectedId);
  const selectedCredential = credentials.find((credential) => credential.id === selectedId);
  const selectedProfile = profiles.find((profile) => profile.id === selectedId);
  const loading =
    activeTab === "providers"
      ? providerQuery.isLoading
      : activeTab === "credentials"
        ? credentialQuery.isLoading
        : profileQuery.isLoading;

  async function refreshCurrent() {
    if (activeTab === "providers") {
      await providerQuery.refetch();
    } else if (activeTab === "credentials") {
      await credentialQuery.refetch();
    } else {
      await profileQuery.refetch();
    }
    await activeProviders.refetch();
  }

  async function disableSelectedProvider(provider: LlmProvider) {
    await disableProvider.mutateAsync({ tenantId: me.tenantId, resourceId: provider.id });
    await refreshCurrent();
  }

  async function revokeSelectedCredential(credential: LlmCredential) {
    await revokeCredential.mutateAsync({ tenantId: me.tenantId, credentialId: credential.id });
    await refreshCurrent();
  }

  async function disableSelectedProfile(profile: LlmModelProfile) {
    await disableProfile.mutateAsync({ tenantId: me.tenantId, resourceId: profile.id });
    await refreshCurrent();
  }

  async function confirmDangerAction() {
    if (!confirmAction) {
      return;
    }
    if (confirmAction.type === "provider") {
      await disableSelectedProvider(confirmAction.provider);
    } else if (confirmAction.type === "credential") {
      await revokeSelectedCredential(confirmAction.credential);
    } else {
      await disableSelectedProfile(confirmAction.profile);
    }
    setConfirmAction(null);
  }

  async function testSelectedProfile(profile: LlmModelProfile) {
    setProfileTestResult(null);
    setProfileTestError(null);
    try {
      const result = await testProfile.mutateAsync({
        tenantId: me.tenantId,
        profileId: profile.id
      });
      setProfileTestResult(result);
    } catch (caught) {
      setProfileTestError(errorMessage(caught, t("llm.profileTest.failed")));
    }
  }

  function openEditor(mode: EditorMode) {
    setEditorMode(mode);
    setEditorOpen(true);
  }

  const showProfileTestPrimary =
    activeTab === "profiles" && selectedProfile && selectedProfile.status !== "disabled";
  const showUpdatePrimary = activeTab !== "credentials" && selectedId && !showProfileTestPrimary;

  return (
    <>
      <div className="llm-screen">
        <div className="subroute-bar">
          <div className="subroute-breadcrumbs">
            <span>{t("llm.title")}</span>
            <strong>{t("llm.subtitle")}</strong>
          </div>
          <div className="subroute-summary">
            <Badge tone={hasActiveProvider ? "success" : "neutral"}>
              {t("llm.summary.providers", { count: providerChoices.length })}
            </Badge>
            <Badge tone={hasActiveCredential ? "success" : "neutral"}>
              {t("llm.summary.credentials", { count: activeCredentialCount })}
            </Badge>
            <Badge tone={hasActiveProfile ? "success" : "neutral"}>
              {t("llm.summary.profiles", { count: activeProfileCount })}
            </Badge>
          </div>
        </div>
        <div className="llm-grid">
          <section className="page-panel llm-list-panel">
            <header className="panel-header">
              <div>
                <strong>{t("llm.title")}</strong>
                <span>{t("llm.subtitle")}</span>
              </div>
              <div className="panel-header-actions">
                <Button
                  size="sm"
                  variant="secondary"
                  aria-label={t(llmCreateLabel(activeTab))}
                  icon={<Plus size={15} />}
                  onClick={() => openEditor("create")}
                >
                  {t(llmCreateLabel(activeTab))}
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  aria-label={t("catalog.refresh")}
                  icon={<RefreshCw size={15} />}
                  onClick={refreshCurrent}
                />
              </div>
            </header>
            <div className="catalog-controls">
              <Tabs
                active={activeTab}
                items={tabs.map((item) => ({ id: item.id, label: t(llmTabLabel(item.id)) }))}
                onChange={setActiveTab}
              />
              <label className="field-stack">
                <span>{t("catalog.statusFilter")}</span>
                <select
                  className="text-input"
                  value={status}
                  onChange={(event) => setStatus(event.target.value as StatusFilter)}
                >
                  {statusFilters.map((item) => (
                    <option key={item} value={item}>
                      {t(statusLabel(item))}
                    </option>
                  ))}
                </select>
              </label>
              <label className="checkbox-row">
                <input
                  type="checkbox"
                  checked={hideInactive}
                  onChange={(event) => setHideInactive(event.target.checked)}
                />
                {t("llm.hideDisabled")}
              </label>
            </div>
            <LlmSetupNotice
              activeTab={activeTab}
              hasProvider={hasActiveProvider}
              hasCredential={hasActiveCredential}
              hasProfile={hasActiveProfile}
              onStepChange={setActiveTab}
            />
            <div className="resource-list">
              {loading ? (
                <EmptyState title={t("common.loading")} detail={t("llm.loading")} />
              ) : null}
              {!loading && currentItems.length === 0 ? (
                <EmptyState title={t("llm.empty")} detail={t("llm.emptyDetail")} />
              ) : null}
              {currentItems.map((item) => (
                <button
                  key={item.id}
                  className={`resource-row ${item.id === selectedId ? "active" : ""}`}
                  onClick={() => {
                    setSelectedId(item.id);
                    setEditorMode("update");
                  }}
                >
                  <LlmRowIcon tab={activeTab} />
                  <span>
                    <strong>{itemTitle(activeTab, item)}</strong>
                    <span>{itemSubtitle(activeTab, item, providerChoices)}</span>
                  </span>
                  <StatusPill status={item.status} />
                </button>
              ))}
            </div>
          </section>

          <section className="page-panel llm-detail-panel">
            <header className="panel-header">
              <div>
                <strong>
                  {selectedTitle(
                    activeTab,
                    selectedProvider,
                    selectedCredential,
                    selectedProfile,
                    t
                  )}
                </strong>
                <span>{t(tabDetailLabel(activeTab))}</span>
              </div>
              <div className="panel-header-actions">
                {showUpdatePrimary ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    icon={<Save size={14} />}
                    onClick={() => openEditor("update")}
                  >
                    {t("llm.openUpdate")}
                  </Button>
                ) : null}
                {showProfileTestPrimary ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    icon={<RefreshCw size={14} />}
                    disabled={testProfile.isPending}
                    onClick={() => selectedProfile && testSelectedProfile(selectedProfile)}
                  >
                    {t("llm.profileTest.action")}
                  </Button>
                ) : null}
                {selectedId ? (
                  <ActionMenu
                    label={t("common.moreActions")}
                    items={[
                      ...(showProfileTestPrimary
                        ? [
                            {
                              label: t("llm.openUpdate"),
                              icon: <Save size={14} />,
                              onSelect: () => openEditor("update")
                            }
                          ]
                        : []),
                      ...(activeTab === "providers" &&
                      selectedProvider &&
                      selectedProvider.status !== "disabled"
                        ? [
                            {
                              label: t("common.disable"),
                              icon: <Ban size={14} />,
                              danger: true,
                              disabled: disableProvider.isPending,
                              onSelect: () =>
                                setConfirmAction({ type: "provider", provider: selectedProvider })
                            }
                          ]
                        : []),
                      ...(activeTab === "credentials" &&
                      selectedCredential &&
                      selectedCredential.status !== "revoked"
                        ? [
                            {
                              label: t("common.revoke"),
                              icon: <Ban size={14} />,
                              danger: true,
                              disabled: revokeCredential.isPending,
                              onSelect: () =>
                                setConfirmAction({
                                  type: "credential",
                                  credential: selectedCredential
                                })
                            }
                          ]
                        : []),
                      ...(activeTab === "profiles" &&
                      selectedProfile &&
                      selectedProfile.status !== "disabled"
                        ? [
                            {
                              label: t("common.disable"),
                              icon: <Ban size={14} />,
                              danger: true,
                              disabled: disableProfile.isPending,
                              onSelect: () =>
                                setConfirmAction({ type: "profile", profile: selectedProfile })
                            }
                          ]
                        : [])
                    ]}
                  />
                ) : null}
              </div>
            </header>
            <div className="catalog-detail-scroll">
              {activeTab === "providers" && selectedProvider ? (
                <ProviderDetail provider={selectedProvider} />
              ) : null}
              {activeTab === "credentials" && selectedCredential ? (
                <CredentialDetail credential={selectedCredential} />
              ) : null}
              {activeTab === "profiles" && selectedProfile ? (
                <>
                  {profileTestResult || profileTestError ? (
                    <LlmProfileTestPanel result={profileTestResult} error={profileTestError} />
                  ) : null}
                  <ProfileDetail
                    profile={selectedProfile}
                    providers={providerChoices}
                    credentials={credentials}
                  />
                </>
              ) : null}
              {!selectedId ? (
                <EmptyState title={t("llm.noSelection")} detail={t("llm.noSelectionDetail")} />
              ) : null}
            </div>
          </section>
        </div>
      </div>
      {editorOpen ? (
        <ConfigPanel
          title={t(editorMode === "create" ? llmCreateLabel(activeTab) : llmUpdateLabel(activeTab))}
          subtitle={t(editorMode === "create" ? "llm.editor.create" : "llm.editor.update")}
          closeLabel={t("common.close")}
          onClose={() => setEditorOpen(false)}
        >
          {activeTab === "providers" ? (
            <ProviderForm
              mode={editorMode}
              provider={editorMode === "update" ? selectedProvider : undefined}
              pending={createProvider.isPending || updateProvider.isPending}
              onSubmit={async (draft) => {
                const saved =
                  editorMode === "update" && selectedProvider
                    ? await updateProvider.mutateAsync({
                        tenantId: me.tenantId,
                        providerId: selectedProvider.id,
                        displayName: draft.displayName,
                        baseUrl: draft.baseUrl,
                        authScheme: draft.authScheme,
                        defaultHeadersTemplate: draft.defaultHeadersTemplate
                      })
                    : await createProvider.mutateAsync({
                        tenantId: me.tenantId,
                        providerKey: draft.providerKey ?? "",
                        displayName: draft.displayName,
                        baseUrl: draft.baseUrl,
                        authScheme: draft.authScheme,
                        defaultHeadersTemplate: draft.defaultHeadersTemplate
                      });
                setSelectedId(saved.id);
                setEditorOpen(false);
                await refreshCurrent();
                if (editorMode === "create") {
                  setActiveTab("credentials");
                }
              }}
            />
          ) : null}
          {activeTab === "credentials" ? (
            <CredentialForm
              tenantId={me.tenantId}
              providers={providerChoices}
              pending={createCredential.isPending}
              onSubmit={async (draft) => {
                const saved = await createCredential.mutateAsync({
                  tenantId: me.tenantId,
                  ...draft
                });
                setSelectedId(saved.id);
                setEditorOpen(false);
                await refreshCurrent();
                setActiveTab("profiles");
              }}
            />
          ) : null}
          {activeTab === "profiles" ? (
            <ProfileForm
              mode={editorMode}
              profile={editorMode === "update" ? selectedProfile : undefined}
              providers={providerChoices}
              credentials={credentials.filter((credential) => credential.status === "active")}
              pending={createProfile.isPending || updateProfile.isPending}
              onSubmit={async (draft) => {
                const saved = await saveProfileDraft({
                  draft,
                  tenantId: me.tenantId,
                  selectedProfile,
                  editorMode,
                  createProfile: createProfile.mutateAsync,
                  updateProfile: updateProfile.mutateAsync
                });
                setSelectedId(saved.id);
                setEditorOpen(false);
                await refreshCurrent();
              }}
            />
          ) : null}
        </ConfigPanel>
      ) : null}
      {confirmAction ? (
        <ConfirmDialog
          title={t("common.confirmAction")}
          message={llmConfirmMessage(confirmAction, t)}
          confirmLabel={
            confirmAction.type === "credential" ? t("common.revoke") : t("common.disable")
          }
          cancelLabel={t("common.cancel")}
          pending={
            disableProvider.isPending || revokeCredential.isPending || disableProfile.isPending
          }
          onCancel={() => setConfirmAction(null)}
          onConfirm={confirmDangerAction}
        />
      ) : null}
    </>
  );
}

function llmConfirmMessage(
  action: LlmConfirmAction,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
): string {
  if (action.type === "provider") {
    return t("llm.confirmDisableProvider", { name: action.provider.displayName });
  }
  if (action.type === "credential") {
    return t("llm.confirmRevokeCredential");
  }
  return t("llm.confirmDisableProfile", { name: action.profile.profileName });
}

async function saveProfileDraft({
  draft,
  tenantId,
  selectedProfile,
  editorMode,
  createProfile,
  updateProfile
}: {
  draft: ProfileFormDraft;
  tenantId: string;
  selectedProfile?: LlmModelProfile;
  editorMode: EditorMode;
  createProfile: (input: { tenantId: string } & ProfileFormDraft) => Promise<LlmModelProfile>;
  updateProfile: (
    input: { tenantId: string; profileId: string } & Omit<ProfileFormDraft, "providerId">
  ) => Promise<LlmModelProfile>;
}) {
  if (editorMode === "update" && selectedProfile) {
    return updateProfile({
      tenantId,
      profileId: selectedProfile.id,
      credentialId: draft.credentialId,
      profileName: draft.profileName,
      modelName: draft.modelName,
      contextWindow: draft.contextWindow,
      maxInputTokens: draft.maxInputTokens,
      maxOutputTokens: draft.maxOutputTokens,
      temperature: draft.temperature,
      topP: draft.topP,
      reasoningEffort: draft.reasoningEffort,
      responseFormat: draft.responseFormat,
      toolChoicePolicy: draft.toolChoicePolicy,
      rateLimitPolicy: draft.rateLimitPolicy,
      costPolicy: draft.costPolicy
    });
  }
  return createProfile({ tenantId, ...draft });
}

function LlmSetupNotice({
  activeTab,
  hasProvider,
  hasCredential,
  hasProfile,
  onStepChange
}: {
  activeTab: LlmTab;
  hasProvider: boolean;
  hasCredential: boolean;
  hasProfile: boolean;
  onStepChange: (tab: LlmTab) => void;
}) {
  const { t } = useI18n();
  const steps = [
    {
      id: "providers" as const,
      titleKey: "llm.step.providers" as const,
      detailKey: "llm.setup.providers" as const,
      done: hasProvider
    },
    {
      id: "credentials" as const,
      titleKey: "llm.step.credentials" as const,
      detailKey: "llm.setup.credentials" as const,
      done: hasCredential
    },
    {
      id: "profiles" as const,
      titleKey: "llm.step.profiles" as const,
      detailKey: "llm.setup.profiles" as const,
      done: hasProfile
    }
  ];

  return (
    <div className="llm-stepper-panel">
      <div>
        <strong>{t("llm.setup.title")}</strong>
        <span>{t(llmSetupDetail(activeTab))}</span>
      </div>
      <div className="llm-stepper">
        {steps.map((step) => {
          const state = step.done ? "complete" : step.id === activeTab ? "active" : "pending";
          const Icon = step.done ? CheckCircle2 : Circle;
          return (
            <button
              key={step.id}
              type="button"
              className={`llm-step ${state}`}
              onClick={() => onStepChange(step.id)}
            >
              <Icon size={15} />
              <span>
                <strong>{t(step.titleKey)}</strong>
                <small>{t(step.detailKey)}</small>
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function ProviderDetail({ provider }: { provider: LlmProvider }) {
  const { t } = useI18n();
  return (
    <div className="catalog-section">
      <div className="section-title">
        <Server size={16} />
        <strong>{t("llm.provider.detail")}</strong>
        <StatusPill status={provider.status} />
      </div>
      <dl className="key-value catalog-key-value">
        <dt>{t("llm.provider.key")}</dt>
        <dd>{provider.providerKey}</dd>
        <dt>{t("llm.provider.baseUrl")}</dt>
        <dd>{provider.baseUrl ?? t("common.none")}</dd>
        <dt>{t("llm.provider.authScheme")}</dt>
        <dd>{provider.authScheme}</dd>
      </dl>
      <details className="profile-policy-json">
        <summary>{t("common.advanced")}</summary>
        <dl className="key-value catalog-key-value">
          <dt>{t("catalog.meta.id")}</dt>
          <dd>{provider.id}</dd>
        </dl>
        <pre className="catalog-json">{formatJson(provider.defaultHeadersTemplate)}</pre>
      </details>
    </div>
  );
}

function CredentialDetail({ credential }: { credential: LlmCredential }) {
  const { t } = useI18n();
  return (
    <div className="catalog-section">
      <div className="section-title">
        <KeyRound size={16} />
        <strong>{t("llm.credential.detail")}</strong>
        <Badge tone={credential.hasSecretRef ? "success" : "warning"}>
          {credential.hasSecretRef ? t("llm.credential.hasSecret") : t("llm.credential.noSecret")}
        </Badge>
      </div>
      <dl className="key-value catalog-key-value">
        <dt>{t("llm.provider")}</dt>
        <dd>{credential.providerName ?? credential.providerId}</dd>
        <dt>{t("llm.credential.ownerScope")}</dt>
        <dd>{credential.ownerScope}</dd>
        <dt>{t("llm.credential.ownerResourceId")}</dt>
        <dd>{credential.ownerResourceId ?? t("common.none")}</dd>
        <dt>{t("llm.credential.expiresAt")}</dt>
        <dd>{credential.expiresAt ?? t("common.none")}</dd>
        <dt>{t("llm.credential.revokedAt")}</dt>
        <dd>{credential.revokedAt ?? t("common.none")}</dd>
      </dl>
      <details className="profile-policy-json">
        <summary>{t("common.advanced")}</summary>
        <dl className="key-value catalog-key-value">
          <dt>{t("catalog.meta.id")}</dt>
          <dd>{credential.id}</dd>
        </dl>
      </details>
    </div>
  );
}

function ProfileDetail({
  profile,
  providers,
  credentials
}: {
  profile: LlmModelProfile;
  providers: LlmProvider[];
  credentials: LlmCredential[];
}) {
  const { t } = useI18n();
  const provider = providers.find((item) => item.id === profile.providerId);
  const credential = credentials.find((item) => item.id === profile.credentialId);
  return (
    <div className="catalog-section">
      <div className="section-title">
        <SlidersHorizontal size={16} />
        <strong>{t("llm.profile.detail")}</strong>
        <StatusPill status={profile.status} />
      </div>
      <dl className="key-value catalog-key-value">
        <dt>{t("llm.profile.modelName")}</dt>
        <dd>{profile.modelName}</dd>
        <dt>{t("llm.provider")}</dt>
        <dd>{provider?.displayName ?? profile.providerId}</dd>
        <dt>{t("llm.credential")}</dt>
        <dd>{credential?.label ?? profile.credentialId ?? t("common.unbound")}</dd>
        <dt>{t("llm.profile.contextWindow")}</dt>
        <dd>{profile.contextWindow ?? t("common.none")}</dd>
        <dt>{t("llm.profile.outputTokens")}</dt>
        <dd>{profile.maxOutputTokens ?? t("common.none")}</dd>
        <dt>{t("llm.profile.temperature")}</dt>
        <dd>{profile.temperature ?? t("common.none")}</dd>
      </dl>
      <ProfilePolicySummary profile={profile} />
    </div>
  );
}

function ProfilePolicySummary({ profile }: { profile: LlmModelProfile }) {
  const { t } = useI18n();
  const rateLimitPolicy = jsonRecord(profile.rateLimitPolicy);
  const costPolicy = jsonRecord(profile.costPolicy);
  const rows: Array<{ labelKey: I18nKey; value: string }> = [
    {
      labelKey: "llm.profile.responseFormatMode",
      value: t(responseFormatModeLabel(inferResponseFormatMode(profile.responseFormat)))
    },
    {
      labelKey: "llm.profile.toolChoiceMode",
      value: t(toolChoiceModeLabel(inferToolChoiceMode(profile.toolChoicePolicy)))
    },
    {
      labelKey: "llm.profile.requestsPerMinute",
      value: jsonDisplayNumber(rateLimitPolicy.requests_per_minute, t("common.none"))
    },
    {
      labelKey: "llm.profile.tokensPerMinute",
      value: jsonDisplayNumber(rateLimitPolicy.tokens_per_minute, t("common.none"))
    },
    {
      labelKey: "llm.profile.inputTokenCost",
      value: jsonDisplayNumber(costPolicy.input_usd_per_1m, t("common.none"))
    },
    {
      labelKey: "llm.profile.outputTokenCost",
      value: jsonDisplayNumber(costPolicy.output_usd_per_1m, t("common.none"))
    }
  ];
  return (
    <div className="profile-policy-summary">
      <div className="section-title">
        <SlidersHorizontal size={16} />
        <strong>{t("llm.profile.policySummary")}</strong>
      </div>
      <dl className="compact-dl catalog-summary-dl">
        {rows.map((row) => (
          <div key={row.labelKey}>
            <dt>{t(row.labelKey)}</dt>
            <dd>{row.value}</dd>
          </div>
        ))}
      </dl>
      <details className="profile-policy-json">
        <summary>{t("llm.profile.policyJsonGroup")}</summary>
        <pre className="catalog-json">
          {formatJson({
            response_format: profile.responseFormat,
            tool_choice_policy: profile.toolChoicePolicy,
            rate_limit_policy: profile.rateLimitPolicy,
            cost_policy: profile.costPolicy
          })}
        </pre>
      </details>
    </div>
  );
}

function LlmProfileTestPanel({
  result,
  error
}: {
  result: LlmProfileTestResult | null;
  error: string | null;
}) {
  const { t } = useI18n();
  const success = Boolean(result?.success) && !error;
  return (
    <div className="catalog-section">
      <div className="section-title">
        <RefreshCw size={16} />
        <strong>{t("llm.profileTest.result")}</strong>
        <Badge tone={success ? "success" : "danger"}>
          {success ? t("llm.profileTest.success") : t("llm.profileTest.failure")}
        </Badge>
      </div>
      {error ? <p className="catalog-inline-error">{error}</p> : null}
      {result ? (
        <dl className="key-value catalog-key-value">
          <dt>{t("llm.provider")}</dt>
          <dd>{result.providerKey}</dd>
          <dt>{t("llm.profile.modelName")}</dt>
          <dd>{result.modelName}</dd>
          <dt>{t("llm.profileTest.httpStatus")}</dt>
          <dd>{result.httpStatus ?? t("common.none")}</dd>
          <dt>{t("llm.profileTest.latency")}</dt>
          <dd>{t("llm.profileTest.latencyValue", { value: result.latencyMs })}</dd>
          <dt>{t("llm.profileTest.message")}</dt>
          <dd>{result.message}</dd>
        </dl>
      ) : null}
    </div>
  );
}

function ProviderForm({
  mode,
  provider,
  pending,
  onSubmit
}: {
  mode: EditorMode;
  provider?: LlmProvider;
  pending: boolean;
  onSubmit: (draft: ProviderFormDraft) => Promise<void>;
}) {
  const { t } = useI18n();
  const [templateId, setTemplateId] = useState<ProviderTemplateId>("custom");
  const [providerKey, setProviderKey] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [authScheme, setAuthScheme] = useState("bearer");
  const [headers, setHeaders] = useState("{}");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setProviderKey(provider?.providerKey ?? "");
    setDisplayName(provider?.displayName ?? "");
    setBaseUrl(provider?.baseUrl ?? "");
    setAuthScheme(provider?.authScheme ?? "bearer");
    setHeaders(formatJson(provider?.defaultHeadersTemplate ?? {}));
    setTemplateId("custom");
  }, [provider, mode]);

  function applyProviderTemplate(nextTemplateId: ProviderTemplateId) {
    setTemplateId(nextTemplateId);
    if (nextTemplateId === "custom") {
      return;
    }
    const template = providerTemplates.find((item) => item.id === nextTemplateId);
    if (!template) {
      return;
    }
    setProviderKey(template.providerKey);
    setDisplayName(template.displayName);
    setBaseUrl(template.baseUrl);
    setAuthScheme(template.authScheme);
    setHeaders(formatJson(template.headers));
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!displayName.trim() || (mode === "create" && !providerKey.trim())) {
        setError(t("llm.form.providerRequired"));
        return;
      }
      if (baseUrl.trim() && !isHttpUrl(baseUrl)) {
        setError(t("llm.form.invalidBaseUrl"));
        return;
      }
      await onSubmit({
        providerKey: mode === "create" ? providerKey.trim() : undefined,
        displayName: displayName.trim(),
        baseUrl: optionalText(baseUrl),
        authScheme,
        defaultHeadersTemplate: parseOptionalJson(headers, t("llm.provider.headers"), t)
      });
      if (mode === "create") {
        setTemplateId("custom");
        setProviderKey("");
        setDisplayName("");
        setBaseUrl("");
        setAuthScheme("bearer");
        setHeaders("{}");
      }
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  if (mode === "update" && !provider) {
    return <EmptyState title={t("llm.noSelection")} detail={t("llm.noSelectionDetail")} />;
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      {mode === "create" ? (
        <>
          <label className="field-stack">
            <span>{t("llm.provider.template")}</span>
            <select
              className="text-input"
              value={templateId}
              onChange={(event) => applyProviderTemplate(event.target.value as ProviderTemplateId)}
            >
              <option value="custom">{t("llm.provider.template.custom")}</option>
              {providerTemplates.map((template) => (
                <option key={template.id} value={template.id}>
                  {t(template.labelKey)}
                </option>
              ))}
            </select>
          </label>
          <p className="config-help">{t("llm.provider.templateHint")}</p>
        </>
      ) : null}
      <label className="field-stack">
        <span>{t("llm.provider.key")}</span>
        <TextInput
          required={mode === "create"}
          value={providerKey}
          disabled={mode === "update"}
          onChange={(event) => setProviderKey(event.target.value)}
        />
      </label>
      <label className="field-stack">
        <span>{t("llm.provider.displayName")}</span>
        <TextInput
          required
          value={displayName}
          onChange={(event) => setDisplayName(event.target.value)}
        />
      </label>
      <label className="field-stack">
        <span>{t("llm.provider.baseUrl")}</span>
        <TextInput
          inputMode="url"
          value={baseUrl}
          onChange={(event) => setBaseUrl(event.target.value)}
        />
      </label>
      <label className="field-stack">
        <span>{t("llm.provider.authScheme")}</span>
        <select
          className="text-input"
          value={authScheme}
          onChange={(event) => setAuthScheme(event.target.value)}
        >
          <option value="bearer">{t("llm.auth.bearer")}</option>
          <option value="api_key_header">{t("llm.auth.apiKeyHeader")}</option>
          <option value="none">{t("llm.auth.none")}</option>
        </select>
      </label>
      <details className="config-advanced">
        <summary>{t("common.advanced")}</summary>
        <div className="config-section-heading">
          <strong>{t("common.expertJson")}</strong>
          <span>{t("common.expertJsonHint")}</span>
        </div>
        <label className="field-stack">
          <span>{t("llm.provider.headers")}</span>
          <TextArea value={headers} onChange={(event) => setHeaders(event.target.value)} />
        </label>
      </details>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button type="submit" variant="primary" icon={<Save size={15} />} disabled={pending}>
          {t(mode === "create" ? "common.create" : "llm.update")}
        </Button>
      </div>
    </form>
  );
}

function CredentialForm({
  tenantId,
  providers,
  pending,
  onSubmit
}: {
  tenantId: string;
  providers: LlmProvider[];
  pending: boolean;
  onSubmit: (draft: Omit<CreateLlmCredentialInput, "tenantId">) => Promise<void>;
}) {
  const { t } = useI18n();
  const [providerId, setProviderId] = useState("");
  const [ownerScope, setOwnerScope] = useState("tenant");
  const [ownerResourceId, setOwnerResourceId] = useState("");
  const [secretRef, setSecretRef] = useState("");
  const [secretTemplate, setSecretTemplate] = useState("secret");
  const [secretHash, setSecretHash] = useState("");
  const [expiresAt, setExpiresAt] = useState("");
  const [error, setError] = useState<string | null>(null);
  const selectedProvider = providers.find((provider) => provider.id === providerId);
  const secretRefs = useSecretRefsQuery({ tenantId, purpose: "llm" });

  useEffect(() => {
    setProviderId((current) => current || providers[0]?.id || "");
  }, [providers]);

  useEffect(() => {
    const template = secretRefFromTemplate(secretTemplate, selectedProvider);
    if (template && (!secretRef || secretRef === "secret://llm/provider/default")) {
      setSecretRef(template);
    }
  }, [secretRef, secretTemplate, selectedProvider]);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!providerId || !secretRef.trim()) {
        setError(t("llm.form.credentialRequired"));
        return;
      }
      await onSubmit({
        providerId,
        ownerScope,
        ownerResourceId: optionalText(ownerResourceId),
        secretRef: secretRef.trim(),
        secretHash: optionalText(secretHash),
        expiresAt: optionalText(expiresAt)
      });
      setSecretRef("");
      setSecretTemplate("secret");
      setSecretHash("");
      setExpiresAt("");
      setOwnerResourceId("");
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      {!providers.length ? (
        <p className="form-error" role="alert">
          {t("llm.credential.needProvider")}
        </p>
      ) : null}
      <p className="config-help">{t("llm.credential.secretRefHint")}</p>
      <p className="config-inline-hint">{t("llm.credential.secretRefWorkflow")}</p>
      <label className="field-stack">
        <span>{t("llm.provider")}</span>
        <select
          className="text-input"
          value={providerId}
          required
          onChange={(event) => {
            const nextProviderId = event.target.value;
            setProviderId(nextProviderId);
            const nextProvider = providers.find((provider) => provider.id === nextProviderId);
            const template = secretRefFromTemplate(secretTemplate, nextProvider);
            if (template) {
              setSecretRef(template);
            }
          }}
        >
          {providers.map((provider) => (
            <option key={provider.id} value={provider.id}>
              {provider.displayName}
            </option>
          ))}
        </select>
      </label>
      <label className="field-stack">
        <span>{t("secretRefs.picker")}</span>
        <select
          className="text-input"
          value=""
          onChange={(event) => {
            if (event.target.value) {
              setSecretRef(event.target.value);
              setSecretTemplate("custom");
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
        <span>{t("llm.credential.secretTemplate")}</span>
        <select
          className="text-input"
          value={secretTemplate}
          onChange={(event) => {
            const next = event.target.value;
            setSecretTemplate(next);
            const template = secretRefFromTemplate(next, selectedProvider);
            if (template) {
              setSecretRef(template);
            }
          }}
        >
          <option value="custom">{t("llm.credential.secretTemplate.custom")}</option>
          <option value="env">{t("llm.credential.secretTemplate.env")}</option>
          <option value="vault">{t("llm.credential.secretTemplate.vault")}</option>
          <option value="secret">{t("llm.credential.secretTemplate.secret")}</option>
        </select>
      </label>
      <label className="field-stack">
        <span>{t("llm.credential.secretRef")}</span>
        <TextInput
          required
          value={secretRef}
          placeholder={t("llm.credential.secretRefPlaceholder")}
          onChange={(event) => setSecretRef(event.target.value)}
        />
      </label>
      <details className="config-advanced">
        <summary>{t("common.advanced")}</summary>
        <label className="field-stack">
          <span>{t("llm.credential.ownerScope")}</span>
          <select
            className="text-input"
            value={ownerScope}
            onChange={(event) => setOwnerScope(event.target.value)}
          >
            <option value="tenant">{t("llm.owner.tenant")}</option>
            <option value="department">{t("llm.owner.department")}</option>
            <option value="user">{t("llm.owner.user")}</option>
            <option value="agent">{t("llm.owner.agent")}</option>
          </select>
        </label>
        <label className="field-stack">
          <span>{t("llm.credential.ownerResourceId")}</span>
          <TextInput
            value={ownerResourceId}
            onChange={(event) => setOwnerResourceId(event.target.value)}
          />
        </label>
        <label className="field-stack">
          <span>{t("llm.credential.secretHash")}</span>
          <TextInput value={secretHash} onChange={(event) => setSecretHash(event.target.value)} />
        </label>
        <label className="field-stack">
          <span>{t("llm.credential.expiresAt")}</span>
          <TextInput value={expiresAt} onChange={(event) => setExpiresAt(event.target.value)} />
        </label>
      </details>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button
          type="submit"
          variant="primary"
          icon={<Plus size={15} />}
          disabled={pending || !providers.length}
        >
          {t("common.create")}
        </Button>
      </div>
    </form>
  );
}

function ProfileForm({
  mode,
  profile,
  providers,
  credentials,
  pending,
  onSubmit
}: {
  mode: EditorMode;
  profile?: LlmModelProfile;
  providers: LlmProvider[];
  credentials: LlmCredential[];
  pending: boolean;
  onSubmit: (draft: ProfileFormDraft) => Promise<void>;
}) {
  const { t } = useI18n();
  const [providerId, setProviderId] = useState("");
  const [credentialId, setCredentialId] = useState("");
  const [modelPreset, setModelPreset] = useState("custom");
  const [profileName, setProfileName] = useState("");
  const [modelName, setModelName] = useState("");
  const [contextWindow, setContextWindow] = useState("");
  const [maxInputTokens, setMaxInputTokens] = useState("");
  const [maxOutputTokens, setMaxOutputTokens] = useState("");
  const [temperature, setTemperature] = useState("");
  const [topP, setTopP] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState("");
  const [responseFormatMode, setResponseFormatMode] = useState<ResponseFormatMode>("default");
  const [toolChoiceMode, setToolChoiceMode] = useState<ToolChoiceMode>("default");
  const [requestsPerMinute, setRequestsPerMinute] = useState("");
  const [tokensPerMinute, setTokensPerMinute] = useState("");
  const [inputTokenCost, setInputTokenCost] = useState("");
  const [outputTokenCost, setOutputTokenCost] = useState("");
  const [responseFormat, setResponseFormat] = useState("{}");
  const [toolChoicePolicy, setToolChoicePolicy] = useState("{}");
  const [rateLimitPolicy, setRateLimitPolicy] = useState("{}");
  const [costPolicy, setCostPolicy] = useState("{}");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setProviderId(profile?.providerId ?? providers[0]?.id ?? "");
    setCredentialId(profile?.credentialId ?? "");
    setProfileName(profile?.profileName ?? "");
    setModelName(profile?.modelName ?? "");
    setContextWindow(toInput(profile?.contextWindow));
    setMaxInputTokens(toInput(profile?.maxInputTokens));
    setMaxOutputTokens(toInput(profile?.maxOutputTokens));
    setTemperature(toInput(profile?.temperature));
    setTopP(toInput(profile?.topP));
    setReasoningEffort(profile?.reasoningEffort ?? "");
    const initialResponseFormat = profile?.responseFormat ?? {};
    const initialToolChoicePolicy = profile?.toolChoicePolicy ?? {};
    const initialRateLimitPolicy = jsonRecord(profile?.rateLimitPolicy);
    const initialCostPolicy = jsonRecord(profile?.costPolicy);
    setResponseFormatMode(inferResponseFormatMode(initialResponseFormat));
    setToolChoiceMode(inferToolChoiceMode(initialToolChoicePolicy));
    setRequestsPerMinute(jsonNumberInput(initialRateLimitPolicy.requests_per_minute));
    setTokensPerMinute(jsonNumberInput(initialRateLimitPolicy.tokens_per_minute));
    setInputTokenCost(jsonNumberInput(initialCostPolicy.input_usd_per_1m));
    setOutputTokenCost(jsonNumberInput(initialCostPolicy.output_usd_per_1m));
    setResponseFormat(formatJson(initialResponseFormat));
    setToolChoicePolicy(formatJson(initialToolChoicePolicy));
    setRateLimitPolicy(formatJson(profile?.rateLimitPolicy ?? {}));
    setCostPolicy(formatJson(profile?.costPolicy ?? {}));
    setModelPreset("custom");
  }, [profile, providers, mode]);

  const credentialChoices = credentials.filter(
    (credential) => credential.providerId === providerId
  );

  function applyModelPreset(presetId: string) {
    setModelPreset(presetId);
    const preset = modelPresets.find((item) => item.id === presetId);
    if (!preset) {
      return;
    }
    setContextWindow(preset.contextWindow);
    setMaxOutputTokens(preset.maxOutputTokens);
    setTemperature(preset.temperature);
    setReasoningEffort(preset.reasoningEffort);
  }

  function applyResponseFormatMode(mode: ResponseFormatMode) {
    setResponseFormatMode(mode);
    if (mode !== "custom") {
      setResponseFormat(formatJson(responseFormatFromMode(mode)));
    }
  }

  function applyToolChoiceMode(mode: ToolChoiceMode) {
    setToolChoiceMode(mode);
    if (mode !== "custom") {
      setToolChoicePolicy(formatJson(toolChoicePolicyFromMode(mode)));
    }
  }

  function updateRateLimitField(field: "requests_per_minute" | "tokens_per_minute", value: string) {
    if (field === "requests_per_minute") {
      setRequestsPerMinute(value);
    } else {
      setTokensPerMinute(value);
    }
    setRateLimitPolicy((current) => updateNumberPolicyJson(current, field, value));
  }

  function updateCostField(field: "input_usd_per_1m" | "output_usd_per_1m", value: string) {
    if (field === "input_usd_per_1m") {
      setInputTokenCost(value);
    } else {
      setOutputTokenCost(value);
    }
    setCostPolicy((current) => updateNumberPolicyJson(current, field, value));
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    try {
      if (!providerId || !profileName.trim() || !modelName.trim()) {
        setError(t("llm.form.profileRequired"));
        return;
      }
      const parsedRateLimitPolicy = parseOptionalJson(
        rateLimitPolicy,
        t("llm.profile.rateLimitPolicy"),
        t
      );
      const parsedCostPolicy = parseOptionalJson(costPolicy, t("llm.profile.costPolicy"), t);
      await onSubmit({
        providerId,
        credentialId: optionalText(credentialId),
        profileName: profileName.trim(),
        modelName: modelName.trim(),
        contextWindow: parseOptionalNumber(contextWindow, t("llm.profile.contextWindow"), t),
        maxInputTokens: parseOptionalNumber(maxInputTokens, t("llm.profile.inputTokens"), t),
        maxOutputTokens: parseOptionalNumber(maxOutputTokens, t("llm.profile.outputTokens"), t),
        temperature: parseOptionalNumber(temperature, t("llm.profile.temperature"), t),
        topP: parseOptionalNumber(topP, t("llm.profile.topP"), t),
        reasoningEffort: optionalText(reasoningEffort),
        responseFormat: parseOptionalJson(responseFormat, t("llm.profile.responseFormat"), t),
        toolChoicePolicy: parseOptionalJson(toolChoicePolicy, t("llm.profile.toolChoicePolicy"), t),
        rateLimitPolicy: mergeNumberPolicyFields(parsedRateLimitPolicy, {
          requests_per_minute: parseOptionalNumber(
            requestsPerMinute,
            t("llm.profile.requestsPerMinute"),
            t
          ),
          tokens_per_minute: parseOptionalNumber(
            tokensPerMinute,
            t("llm.profile.tokensPerMinute"),
            t
          )
        }),
        costPolicy: mergeNumberPolicyFields(parsedCostPolicy, {
          input_usd_per_1m: parseOptionalNumber(inputTokenCost, t("llm.profile.inputTokenCost"), t),
          output_usd_per_1m: parseOptionalNumber(
            outputTokenCost,
            t("llm.profile.outputTokenCost"),
            t
          )
        })
      });
      if (mode === "create") {
        setProfileName("");
        setModelName("");
      }
    } catch (caught) {
      setError(errorMessage(caught, t("catalog.form.submitFailed")));
    }
  }

  if (mode === "update" && !profile) {
    return <EmptyState title={t("llm.noSelection")} detail={t("llm.noSelectionDetail")} />;
  }

  return (
    <form className="config-form" onSubmit={handleSubmit}>
      {!providers.length ? (
        <p className="form-error" role="alert">
          {t("llm.profile.needProvider")}
        </p>
      ) : null}
      <div className="two-field-row">
        <label className="field-stack">
          <span>{t("llm.provider")}</span>
          <select
            className="text-input"
            value={providerId}
            required
            disabled={mode === "update"}
            onChange={(event) => setProviderId(event.target.value)}
          >
            {providers.map((provider) => (
              <option key={provider.id} value={provider.id}>
                {provider.displayName}
              </option>
            ))}
          </select>
        </label>
        <label className="field-stack">
          <span>{t("llm.credential")}</span>
          <select
            className="text-input"
            value={credentialId}
            onChange={(event) => setCredentialId(event.target.value)}
          >
            <option value="">{t("common.unbound")}</option>
            {credentialChoices.map((credential) => (
              <option key={credential.id} value={credential.id}>
                {credential.label}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="two-field-row">
        <label className="field-stack">
          <span>{t("llm.profile.name")}</span>
          <TextInput
            required
            value={profileName}
            onChange={(event) => setProfileName(event.target.value)}
          />
        </label>
        <label className="field-stack">
          <span>{t("llm.profile.modelName")}</span>
          <TextInput
            required
            value={modelName}
            onChange={(event) => setModelName(event.target.value)}
          />
        </label>
      </div>
      <div className="two-field-row">
        <label className="field-stack">
          <span>{t("llm.profile.preset")}</span>
          <select
            className="text-input"
            value={modelPreset}
            onChange={(event) => applyModelPreset(event.target.value)}
          >
            <option value="custom">{t("llm.profile.preset.custom")}</option>
            {modelPresets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {t(preset.labelKey)}
              </option>
            ))}
          </select>
        </label>
        <label className="field-stack">
          <span>{t("llm.profile.contextWindow")}</span>
          <TextInput
            value={contextWindow}
            onChange={(event) => setContextWindow(event.target.value)}
          />
        </label>
        <label className="field-stack">
          <span>{t("llm.profile.outputTokens")}</span>
          <TextInput
            value={maxOutputTokens}
            onChange={(event) => setMaxOutputTokens(event.target.value)}
          />
        </label>
      </div>
      <details className="config-advanced">
        <summary>{t("llm.profile.advancedTuning")}</summary>
        <div className="config-section-heading">
          <strong>{t("llm.profile.tuningGroup")}</strong>
        </div>
        <div className="two-field-row">
          <label className="field-stack">
            <span>{t("llm.profile.inputTokens")}</span>
            <TextInput
              value={maxInputTokens}
              onChange={(event) => setMaxInputTokens(event.target.value)}
            />
          </label>
          <label className="field-stack">
            <span>{t("llm.profile.reasoningEffort")}</span>
            <TextInput
              value={reasoningEffort}
              onChange={(event) => setReasoningEffort(event.target.value)}
            />
          </label>
        </div>
        <div className="two-field-row">
          <label className="field-stack">
            <span>{t("llm.profile.temperature")}</span>
            <TextInput
              value={temperature}
              onChange={(event) => setTemperature(event.target.value)}
            />
          </label>
          <label className="field-stack">
            <span>{t("llm.profile.topP")}</span>
            <TextInput value={topP} onChange={(event) => setTopP(event.target.value)} />
          </label>
        </div>
        <div className="config-section-heading">
          <strong>{t("llm.profile.policyStructuredGroup")}</strong>
        </div>
        <div className="two-field-row">
          <label className="field-stack">
            <span>{t("llm.profile.responseFormatMode")}</span>
            <select
              className="text-input"
              value={responseFormatMode}
              onChange={(event) =>
                applyResponseFormatMode(event.target.value as ResponseFormatMode)
              }
            >
              {responseFormatModes.map((modeOption) => (
                <option key={modeOption.id} value={modeOption.id}>
                  {t(modeOption.labelKey)}
                </option>
              ))}
            </select>
          </label>
          <label className="field-stack">
            <span>{t("llm.profile.toolChoiceMode")}</span>
            <select
              className="text-input"
              value={toolChoiceMode}
              onChange={(event) => applyToolChoiceMode(event.target.value as ToolChoiceMode)}
            >
              {toolChoiceModes.map((modeOption) => (
                <option key={modeOption.id} value={modeOption.id}>
                  {t(modeOption.labelKey)}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div className="two-field-row">
          <label className="field-stack">
            <span>{t("llm.profile.requestsPerMinute")}</span>
            <TextInput
              inputMode="numeric"
              value={requestsPerMinute}
              onChange={(event) => updateRateLimitField("requests_per_minute", event.target.value)}
            />
          </label>
          <label className="field-stack">
            <span>{t("llm.profile.tokensPerMinute")}</span>
            <TextInput
              inputMode="numeric"
              value={tokensPerMinute}
              onChange={(event) => updateRateLimitField("tokens_per_minute", event.target.value)}
            />
          </label>
        </div>
        <div className="two-field-row">
          <label className="field-stack">
            <span>{t("llm.profile.inputTokenCost")}</span>
            <TextInput
              inputMode="decimal"
              value={inputTokenCost}
              onChange={(event) => updateCostField("input_usd_per_1m", event.target.value)}
            />
          </label>
          <label className="field-stack">
            <span>{t("llm.profile.outputTokenCost")}</span>
            <TextInput
              inputMode="decimal"
              value={outputTokenCost}
              onChange={(event) => updateCostField("output_usd_per_1m", event.target.value)}
            />
          </label>
        </div>
        <div className="config-section-heading">
          <strong>{t("llm.profile.policyJsonGroup")}</strong>
          <span>{t("common.expertJsonHint")}</span>
        </div>
        <JsonTextArea
          labelKey="llm.profile.responseFormat"
          value={responseFormat}
          onChange={setResponseFormat}
        />
        <JsonTextArea
          labelKey="llm.profile.toolChoicePolicy"
          value={toolChoicePolicy}
          onChange={setToolChoicePolicy}
        />
        <JsonTextArea
          labelKey="llm.profile.rateLimitPolicy"
          value={rateLimitPolicy}
          onChange={setRateLimitPolicy}
        />
        <JsonTextArea
          labelKey="llm.profile.costPolicy"
          value={costPolicy}
          onChange={setCostPolicy}
        />
      </details>
      {error ? <p className="form-error">{error}</p> : null}
      <div className="row-actions">
        <Button
          type="submit"
          variant="primary"
          icon={<Save size={15} />}
          disabled={pending || !providers.length}
        >
          {t(mode === "create" ? "common.create" : "llm.update")}
        </Button>
      </div>
    </form>
  );
}

function JsonTextArea({
  labelKey,
  value,
  onChange
}: {
  labelKey: I18nKey;
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useI18n();
  return (
    <label className="field-stack">
      <span>{t(labelKey)}</span>
      <TextArea value={value} onChange={(event) => onChange(event.target.value)} />
    </label>
  );
}

function LlmRowIcon({ tab }: { tab: LlmTab }) {
  if (tab === "credentials") {
    return <KeyRound size={17} />;
  }
  if (tab === "profiles") {
    return <SlidersHorizontal size={17} />;
  }
  return <Server size={17} />;
}

function itemTitle(tab: LlmTab, item: LlmProvider | LlmCredential | LlmModelProfile): string {
  if (tab === "providers") {
    return (item as LlmProvider).displayName;
  }
  if (tab === "credentials") {
    return (item as LlmCredential).label;
  }
  return (item as LlmModelProfile).profileName;
}

function itemSubtitle(
  tab: LlmTab,
  item: LlmProvider | LlmCredential | LlmModelProfile,
  providers: LlmProvider[]
): string {
  if (tab === "providers") {
    const provider = item as LlmProvider;
    return provider.providerKey || provider.id;
  }
  if (tab === "credentials") {
    const credential = item as LlmCredential;
    return credential.providerName ?? credential.providerId;
  }
  const profile = item as LlmModelProfile;
  const provider = providers.find((provider) => provider.id === profile.providerId);
  return `${profile.modelName} · ${provider?.displayName ?? profile.providerId}`;
}

function selectedTitle(
  tab: LlmTab,
  provider: LlmProvider | undefined,
  credential: LlmCredential | undefined,
  profile: LlmModelProfile | undefined,
  t: (key: I18nKey) => string
): string {
  if (tab === "providers") {
    return provider?.displayName ?? t("llm.noSelection");
  }
  if (tab === "credentials") {
    return credential?.label ?? t("llm.noSelection");
  }
  return profile?.profileName ?? t("llm.noSelection");
}

function tabDetailLabel(tab: LlmTab): I18nKey {
  if (tab === "providers") {
    return "llm.provider.detail";
  }
  if (tab === "credentials") {
    return "llm.credential.detail";
  }
  return "llm.profile.detail";
}

function llmTabLabel(tab: LlmTab): I18nKey {
  if (tab === "providers") {
    return "llm.tab.providersStep";
  }
  if (tab === "credentials") {
    return "llm.tab.credentialsStep";
  }
  return "llm.tab.profilesStep";
}

function llmCreateLabel(tab: LlmTab): I18nKey {
  if (tab === "providers") {
    return "llm.provider.create";
  }
  if (tab === "credentials") {
    return "llm.credential.create";
  }
  return "llm.profile.create";
}

function llmUpdateLabel(tab: LlmTab): I18nKey {
  if (tab === "providers") {
    return "llm.provider.update";
  }
  if (tab === "profiles") {
    return "llm.profile.update";
  }
  return "llm.editor";
}

function llmSetupDetail(tab: LlmTab): I18nKey {
  if (tab === "providers") {
    return "llm.setup.providers";
  }
  if (tab === "credentials") {
    return "llm.setup.credentials";
  }
  return "llm.setup.profiles";
}

function statusLabel(status: StatusFilter): I18nKey {
  return status === "all" ? "catalog.status.all" : (`status.${status}` as I18nKey);
}

function jsonRecord(value: JsonValue | undefined): Record<string, JsonValue> {
  return value && typeof value === "object" && !Array.isArray(value) ? { ...value } : {};
}

function jsonString(value: JsonValue | undefined): string {
  return typeof value === "string" ? value : "";
}

function jsonNumberInput(value: JsonValue | undefined): string {
  return typeof value === "number" ? String(value) : "";
}

function jsonDisplayNumber(value: JsonValue | undefined, fallback: string): string {
  return typeof value === "number" ? String(value) : fallback;
}

function responseFormatFromMode(mode: ResponseFormatMode): JsonValue {
  if (mode === "text") {
    return { type: "text" };
  }
  if (mode === "jsonObject") {
    return { type: "json_object" };
  }
  if (mode === "jsonSchema") {
    return { type: "json_schema", schema: {} };
  }
  return {};
}

function inferResponseFormatMode(value: JsonValue | undefined): ResponseFormatMode {
  const record = jsonRecord(value);
  const type = jsonString(record.type);
  if (Object.keys(record).length === 0) {
    return "default";
  }
  if (type === "text") {
    return "text";
  }
  if (type === "json_object") {
    return "jsonObject";
  }
  if (type === "json_schema") {
    return "jsonSchema";
  }
  return "custom";
}

function responseFormatModeLabel(mode: ResponseFormatMode): I18nKey {
  return (
    responseFormatModes.find((item) => item.id === mode)?.labelKey ??
    "llm.profile.responseFormat.custom"
  );
}

function toolChoicePolicyFromMode(mode: ToolChoiceMode): JsonValue {
  if (mode === "auto" || mode === "none" || mode === "required") {
    return { mode };
  }
  return {};
}

function inferToolChoiceMode(value: JsonValue | undefined): ToolChoiceMode {
  const record = jsonRecord(value);
  const mode = jsonString(record.mode);
  if (Object.keys(record).length === 0) {
    return "default";
  }
  if (mode === "auto" || mode === "none" || mode === "required") {
    return mode;
  }
  return "custom";
}

function toolChoiceModeLabel(mode: ToolChoiceMode): I18nKey {
  return (
    toolChoiceModes.find((item) => item.id === mode)?.labelKey ?? "llm.profile.toolChoice.custom"
  );
}

function updateNumberPolicyJson(text: string, field: string, value: string): string {
  const record = jsonRecordFromText(text);
  const trimmed = value.trim();
  if (!trimmed) {
    delete record[field];
  } else {
    const parsed = Number(trimmed);
    if (Number.isFinite(parsed)) {
      record[field] = parsed;
    }
  }
  return formatJson(record);
}

function jsonRecordFromText(text: string): Record<string, JsonValue> {
  try {
    const parsed = JSON.parse(text) as unknown;
    const result = jsonValueSchema.safeParse(parsed);
    if (
      result.success &&
      result.data &&
      typeof result.data === "object" &&
      !Array.isArray(result.data)
    ) {
      return { ...result.data };
    }
  } catch {
    return {};
  }
  return {};
}

function mergeNumberPolicyFields(
  value: JsonValue | undefined,
  fields: Record<string, number | undefined>
): JsonValue | undefined {
  const record = jsonRecord(value);
  for (const [field, nextValue] of Object.entries(fields)) {
    if (nextValue === undefined) {
      delete record[field];
    } else {
      record[field] = nextValue;
    }
  }
  return Object.keys(record).length ? record : undefined;
}

function formatJson(value: JsonValue): string {
  return JSON.stringify(redactJson(value), null, 2);
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

function parseOptionalNumber(
  value: string,
  fieldLabel: string,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
): number | undefined {
  if (!value.trim()) {
    return undefined;
  }
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw new Error(t("llm.form.invalidNumber", { field: fieldLabel }));
  }
  return parsed;
}

function optionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}

function secretRefFromTemplate(template: string, provider: LlmProvider | undefined): string {
  if (template === "custom") {
    return "";
  }
  const slug = providerSecretSlug(provider);
  if (template === "env") {
    return `env://${slug.toUpperCase()}_API_KEY`;
  }
  if (template === "vault") {
    return `vault://llm/${slug}/default`;
  }
  if (template === "secret") {
    return `secret://llm/${slug}/default`;
  }
  return "";
}

function providerSecretSlug(provider: LlmProvider | undefined): string {
  const raw = provider?.providerKey || provider?.displayName || "provider";
  const slug = raw
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return slug || "provider";
}

function toInput(value: number | undefined): string {
  return value === undefined ? "" : String(value);
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
    normalized.includes("authorization") ||
    normalized.includes("api_key")
  );
}
