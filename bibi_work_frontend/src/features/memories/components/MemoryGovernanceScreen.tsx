import {
  Archive,
  Check,
  ClipboardList,
  Eye,
  Layers3,
  LockKeyhole,
  Plus,
  Search,
  ShieldAlert,
  X
} from "lucide-react";
import { useMemo, useState } from "react";
import type { Me, MemoryItem } from "../../../shared/contracts/platform";
import { languageLocale, useI18n, type I18nKey } from "../../../shared/i18n";
import {
  Badge,
  Button,
  ConfigPanel,
  EmptyState,
  Panel,
  ResourceList,
  SegmentedControl,
  StatusPill,
  TextArea,
  TextInput
} from "../../../shared/ui";
import type {
  MemoryDecision,
  MemoryLayer,
  MemorySensitivity,
  MemoryStatus,
  MemoryVisibility
} from "../api/memory.adapter";
import {
  useCreateMemoryMutation,
  useMemoriesQuery,
  useMemoryBatchDecisionMutation,
  useMemoryDecisionMutation
} from "../api/memory.queries";

const layers = ["core_profile", "episodic", "semantic", "procedural"] as const;
const statuses = ["candidate", "approved", "rejected", "archived"] as const;
const visibilities = ["all", "private", "tenant", "public"] as const;
const sensitivities = ["all", "normal", "sensitive", "secret"] as const;

const layerLabelKeys: Record<MemoryLayer, I18nKey> = {
  core_profile: "memory.layer.core_profile",
  episodic: "memory.layer.episodic",
  semantic: "memory.layer.semantic",
  procedural: "memory.layer.procedural"
};

const statusLabelKeys: Record<MemoryStatus, I18nKey> = {
  candidate: "memory.status.candidate",
  approved: "memory.status.approved",
  rejected: "memory.status.rejected",
  archived: "memory.status.archived"
};

const visibilityLabelKeys: Record<(typeof visibilities)[number], I18nKey> = {
  all: "memory.visibility.all",
  private: "memory.visibility.private",
  tenant: "memory.visibility.tenant",
  public: "memory.visibility.public"
};

const sensitivityLabelKeys: Record<(typeof sensitivities)[number], I18nKey> = {
  all: "memory.sensitivity.all",
  normal: "memory.sensitivity.normal",
  sensitive: "memory.sensitivity.sensitive",
  secret: "memory.sensitivity.secret"
};

export function MemoryGovernanceScreen({ me }: { me: Me }) {
  const { t } = useI18n();
  const [layer, setLayer] = useState<MemoryLayer | "all">("all");
  const [status, setStatus] = useState<MemoryStatus>("candidate");
  const [visibility, setVisibility] = useState<MemoryVisibility | "all">("all");
  const [sensitivity, setSensitivity] = useState<MemorySensitivity | "all">("all");
  const [query, setQuery] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [draftLayer, setDraftLayer] = useState<MemoryLayer>("episodic");
  const [draftContent, setDraftContent] = useState("");
  const [draftSensitivity, setDraftSensitivity] = useState<MemorySensitivity>("normal");
  const [draftVisibility, setDraftVisibility] = useState<MemoryVisibility>("private");
  const [candidateOpen, setCandidateOpen] = useState(false);

  const memories = useMemoriesQuery({
    tenantId: me.tenantId,
    layer: layer === "all" ? undefined : layer,
    status,
    visibility,
    sensitivity,
    query,
    limit: 100
  });
  const decideMemory = useMemoryDecisionMutation(me.tenantId);
  const batchDecision = useMemoryBatchDecisionMutation(me.tenantId);
  const createMemory = useCreateMemoryMutation(me.tenantId);

  const selectedMemories = useMemo(
    () => memories.data?.filter((memory) => selectedIds.has(memory.id)) ?? [],
    [memories.data, selectedIds]
  );

  async function decide(memoryId: string, decision: MemoryDecision) {
    await decideMemory.mutateAsync({ memoryId, decision });
    setSelectedIds((current) => {
      const next = new Set(current);
      next.delete(memoryId);
      return next;
    });
  }

  async function batchDecide(decision: MemoryDecision) {
    if (selectedIds.size === 0) {
      return;
    }
    await batchDecision.mutateAsync({ decision, memoryIds: Array.from(selectedIds) });
    setSelectedIds(new Set());
  }

  async function createCandidate() {
    const content = draftContent.trim();
    if (!content) {
      return;
    }
    await createMemory.mutateAsync({
      layer: draftLayer,
      content,
      status: "candidate",
      confidence: 0.5,
      sensitivity: draftSensitivity,
      visibility: draftVisibility
    });
    setDraftContent("");
    setStatus("candidate");
    setCandidateOpen(false);
  }

  return (
    <>
      <div className="memory-grid">
        <Panel
          className="memory-filter-panel"
          title={t("memory.filters")}
          subtitle={t("common.itemCount", { count: memories.data?.length ?? 0 })}
          actions={
            <Button
              size="sm"
              variant="secondary"
              icon={<Plus size={15} />}
              onClick={() => setCandidateOpen(true)}
            >
              {t("memory.addCandidate")}
            </Button>
          }
        >
          <div className="filter-stack">
            <label className="field-stack">
              <span>{t("memory.label.layer")}</span>
              <select
                className="text-input"
                value={layer}
                onChange={(event) => setLayer(event.target.value as MemoryLayer | "all")}
              >
                <option value="all">{t("memory.allLayers")}</option>
                {layers.map((item) => (
                  <option key={item} value={item}>
                    {t(layerLabelKeys[item])}
                  </option>
                ))}
              </select>
            </label>
            <div className="field-stack">
              <span>{t("memory.label.status")}</span>
              <SegmentedControl
                label={t("memory.label.status")}
                items={statuses.map((item) => ({ id: item, label: t(statusLabelKeys[item]) }))}
                active={status}
                onChange={setStatus}
              />
            </div>
            <label className="field-stack">
              <span>{t("memory.label.visibility")}</span>
              <select
                className="text-input"
                value={visibility}
                onChange={(event) => setVisibility(event.target.value as MemoryVisibility | "all")}
              >
                {visibilities.map((item) => (
                  <option key={item} value={item}>
                    {t(visibilityLabelKeys[item])}
                  </option>
                ))}
              </select>
            </label>
            <label className="field-stack">
              <span>{t("memory.label.sensitivity")}</span>
              <select
                className="text-input"
                value={sensitivity}
                onChange={(event) =>
                  setSensitivity(event.target.value as MemorySensitivity | "all")
                }
              >
                {sensitivities.map((item) => (
                  <option key={item} value={item}>
                    {t(sensitivityLabelKeys[item])}
                  </option>
                ))}
              </select>
            </label>
            <label className="field-stack">
              <span>{t("memory.label.query")}</span>
              <div className="input-with-icon">
                <Search size={16} />
                <TextInput
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder={t("memory.queryPlaceholder")}
                />
              </div>
            </label>
          </div>
        </Panel>

        <Panel
          className="memory-list-panel"
          title={t(statusLabelKeys[status])}
          subtitle={t("common.selectedCount", { count: selectedIds.size })}
          actions={
            <div className="row-actions">
              <Button
                size="sm"
                icon={<Check size={15} />}
                onClick={() => batchDecide("activate")}
                disabled={selectedIds.size === 0 || batchDecision.isPending}
              >
                {t("common.activate")}
              </Button>
              <Button
                size="sm"
                variant="secondary"
                icon={<X size={15} />}
                onClick={() => batchDecide("reject")}
                disabled={selectedIds.size === 0 || batchDecision.isPending}
              >
                {t("common.reject")}
              </Button>
              <Button
                size="sm"
                variant="secondary"
                icon={<Archive size={15} />}
                onClick={() => batchDecide("archive")}
                disabled={selectedIds.size === 0 || batchDecision.isPending}
              >
                {t("common.archive")}
              </Button>
            </div>
          }
        >
          <ResourceList className="memory-list">
            {memories.isLoading ? (
              <EmptyState title={t("common.loading")} detail={t("memory.loading")} />
            ) : null}
            {memories.data?.map((memory) => (
              <MemoryRow
                key={memory.id}
                memory={memory}
                selected={selectedIds.has(memory.id)}
                onToggle={() => {
                  setSelectedIds((current) => {
                    const next = new Set(current);
                    if (next.has(memory.id)) {
                      next.delete(memory.id);
                    } else {
                      next.add(memory.id);
                    }
                    return next;
                  });
                }}
                onDecide={decide}
                deciding={decideMemory.isPending}
              />
            ))}
            {!memories.isLoading && memories.data?.length === 0 ? (
              <EmptyState title={t("memory.empty")} detail={t("memory.emptyDetail")} />
            ) : null}
          </ResourceList>
        </Panel>

        <Panel
          className={`memory-detail-panel detail-drawer ${
            selectedMemories.length ? "detail-open" : ""
          }`}
          title={t("memory.governanceSummary")}
          subtitle={t("common.selectedCount", { count: selectedMemories.length })}
          actions={
            selectedMemories.length ? (
              <Button
                size="icon"
                variant="ghost"
                aria-label={t("memory.closeSummary")}
                title={t("memory.closeSummary")}
                icon={<X size={15} />}
                onClick={() => setSelectedIds(new Set())}
              />
            ) : null
          }
        >
          <div className="detail-stack">
            {selectedMemories.length ? (
              selectedMemories.map((memory) => <MemoryDetail key={memory.id} memory={memory} />)
            ) : (
              <EmptyState title={t("memory.select")} detail={t("memory.selectDetail")} />
            )}
          </div>
        </Panel>
      </div>
      {candidateOpen ? (
        <ConfigPanel
          title={t("memory.addCandidate")}
          subtitle={t("memory.addCandidateHint")}
          closeLabel={t("common.close")}
          onClose={() => setCandidateOpen(false)}
        >
          <form
            className="config-form"
            onSubmit={(event) => {
              event.preventDefault();
              void createCandidate();
            }}
          >
            <label className="field-stack">
              <span>{t("memory.label.layer")}</span>
              <select
                className="text-input"
                value={draftLayer}
                onChange={(event) => setDraftLayer(event.target.value as MemoryLayer)}
              >
                {layers.map((item) => (
                  <option key={item} value={item}>
                    {t(layerLabelKeys[item])}
                  </option>
                ))}
              </select>
            </label>
            <label className="field-stack">
              <span>{t("memory.content")}</span>
              <TextArea
                value={draftContent}
                onChange={(event) => setDraftContent(event.target.value)}
                placeholder={t("memory.contentPlaceholder")}
              />
            </label>
            <div className="two-field-row">
              <label className="field-stack">
                <span>{t("memory.label.visibility")}</span>
                <select
                  className="text-input"
                  value={draftVisibility}
                  onChange={(event) => setDraftVisibility(event.target.value as MemoryVisibility)}
                >
                  {visibilities
                    .filter((item) => item !== "all")
                    .map((item) => (
                      <option key={item} value={item}>
                        {t(visibilityLabelKeys[item])}
                      </option>
                    ))}
                </select>
              </label>
              <label className="field-stack">
                <span>{t("memory.label.sensitivity")}</span>
                <select
                  className="text-input"
                  value={draftSensitivity}
                  onChange={(event) => setDraftSensitivity(event.target.value as MemorySensitivity)}
                >
                  {sensitivities
                    .filter((item) => item !== "all")
                    .map((item) => (
                      <option key={item} value={item}>
                        {t(sensitivityLabelKeys[item])}
                      </option>
                    ))}
                </select>
              </label>
            </div>
            <div className="row-actions">
              <Button
                type="button"
                variant="ghost"
                onClick={() => setCandidateOpen(false)}
                disabled={createMemory.isPending}
              >
                {t("common.cancel")}
              </Button>
              <Button
                type="submit"
                variant="primary"
                icon={<Plus size={15} />}
                disabled={createMemory.isPending || !draftContent.trim()}
              >
                {t("common.create")}
              </Button>
            </div>
          </form>
        </ConfigPanel>
      ) : null}
    </>
  );
}

function MemoryRow({
  memory,
  selected,
  deciding,
  onToggle,
  onDecide
}: {
  memory: MemoryItem;
  selected: boolean;
  deciding: boolean;
  onToggle: () => void;
  onDecide: (memoryId: string, decision: MemoryDecision) => void;
}) {
  const { language, t } = useI18n();
  return (
    <article className={`memory-row ${selected ? "active" : ""}`}>
      <input
        type="checkbox"
        aria-label={t("memory.selectAria", { id: memory.id })}
        checked={selected}
        onChange={onToggle}
      />
      <div className="memory-row-main">
        <div className="memory-row-title">
          <strong>{memory.content}</strong>
          <StatusPill status={memory.status} />
        </div>
        <div className="memory-badges">
          <Badge tone="info">{memoryLayerLabel(memory.layer, t)}</Badge>
          <Badge
            tone={
              memory.sensitivity === "secret"
                ? "danger"
                : memory.sensitivity === "sensitive"
                  ? "warning"
                  : "neutral"
            }
          >
            {memorySensitivityLabel(memory.sensitivity, t)}
          </Badge>
          <Badge>{memoryVisibilityLabel(memory.visibility, t)}</Badge>
          <Badge tone="warning">{t("memory.untrusted")}</Badge>
        </div>
        <div className="memory-meta">
          <span>
            {t("memory.confidence")} {memory.confidence.toFixed(2)}
          </span>
          <span>{formatShortDate(memory.updatedAt, languageLocale(language))}</span>
          {memory.sourceRunId ? <span>run {shortId(memory.sourceRunId)}</span> : null}
        </div>
      </div>
      <div className="memory-actions">
        <Button
          size="sm"
          title={t("common.activate")}
          aria-label={t("common.activate")}
          icon={<Check size={15} />}
          onClick={() => onDecide(memory.id, "activate")}
          disabled={deciding || memory.status === "approved"}
        >
          {t("common.activate")}
        </Button>
        <Button
          size="sm"
          variant="secondary"
          title={t("common.reject")}
          aria-label={t("common.reject")}
          icon={<X size={15} />}
          onClick={() => onDecide(memory.id, "reject")}
          disabled={deciding || memory.status === "rejected"}
        >
          {t("common.reject")}
        </Button>
        <Button
          size="sm"
          variant="secondary"
          title={t("common.archive")}
          aria-label={t("common.archive")}
          icon={<Archive size={15} />}
          onClick={() => onDecide(memory.id, "archive")}
          disabled={deciding || memory.status === "archived"}
        >
          {t("common.archive")}
        </Button>
      </div>
    </article>
  );
}

function MemoryDetail({ memory }: { memory: MemoryItem }) {
  const { t } = useI18n();
  return (
    <section className="memory-detail">
      <div className="section-title">
        <ClipboardList size={16} />
        <strong>{shortId(memory.id)}</strong>
        <StatusPill status={memory.status} />
      </div>
      <p>{memory.content}</p>
      <dl className="compact-dl">
        <dt>
          <Layers3 size={14} /> {t("memory.label.layer")}
        </dt>
        <dd>{memoryLayerLabel(memory.layer, t)}</dd>
        <dt>
          <Eye size={14} /> {t("memory.label.visibility")}
        </dt>
        <dd>{memoryVisibilityLabel(memory.visibility, t)}</dd>
        <dt>
          <LockKeyhole size={14} /> {t("memory.label.sensitivity")}
        </dt>
        <dd>{memorySensitivityLabel(memory.sensitivity, t)}</dd>
        <dt>
          <ShieldAlert size={14} /> {t("memory.trust")}
        </dt>
        <dd>{t("memory.untrustedContext")}</dd>
        <dt>{t("memory.sourceRun")}</dt>
        <dd>{memory.sourceRunId ?? t("common.notRecorded")}</dd>
        <dt>{t("memory.user")}</dt>
        <dd>{memory.userId ?? t("common.currentUser")}</dd>
        <dt>{t("memory.project")}</dt>
        <dd>{memory.projectId ?? t("common.unbound")}</dd>
      </dl>
    </section>
  );
}

function shortId(value: string): string {
  return value.slice(0, 8);
}

function formatShortDate(value: string, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(value));
}

function memoryLayerLabel(layer: string, t: (key: I18nKey) => string): string {
  return layerLabelKeys[layer as MemoryLayer] ? t(layerLabelKeys[layer as MemoryLayer]) : layer;
}

function memoryVisibilityLabel(visibility: string, t: (key: I18nKey) => string): string {
  return visibilityLabelKeys[visibility as (typeof visibilities)[number]]
    ? t(visibilityLabelKeys[visibility as (typeof visibilities)[number]])
    : visibility;
}

function memorySensitivityLabel(sensitivity: string, t: (key: I18nKey) => string): string {
  return sensitivityLabelKeys[sensitivity as (typeof sensitivities)[number]]
    ? t(sensitivityLabelKeys[sensitivity as (typeof sensitivities)[number]])
    : sensitivity;
}
