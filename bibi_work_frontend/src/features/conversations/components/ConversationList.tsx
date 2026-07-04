import { PanelLeftClose, PanelLeftOpen, Plus } from "lucide-react";
import type { ReactNode } from "react";
import type { Conversation } from "../../../shared/contracts/platform";
import { useI18n } from "../../../shared/i18n";
import { Button, EmptyState, StatusPill } from "../../../shared/ui";

export function ConversationList({
  conversations,
  selectedId,
  onSelect,
  onCreate,
  creating,
  collapsed = false,
  onToggleCollapsed,
  toolbar
}: {
  conversations: Conversation[];
  selectedId?: string;
  onSelect: (conversationId: string) => void;
  onCreate: () => void;
  creating?: boolean;
  collapsed?: boolean;
  onToggleCollapsed?: () => void;
  toolbar?: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <section className={`workbench-list ${collapsed ? "collapsed" : ""}`}>
      <header className="panel-header">
        {!collapsed ? (
          <div>
            <strong>{t("conversation.title")}</strong>
            <span>{t("common.itemCount", { count: conversations.length })}</span>
          </div>
        ) : null}
        <div className="panel-header-actions">
          {!collapsed ? (
            <Button
              size="sm"
              variant="secondary"
              aria-label={t("conversation.create")}
              title={t("conversation.create")}
              icon={<Plus size={16} />}
              onClick={onCreate}
              disabled={creating}
            >
              {t("conversation.createShort")}
            </Button>
          ) : null}
          {onToggleCollapsed ? (
            <Button
              size="icon"
              variant="ghost"
              aria-label={t(collapsed ? "conversation.expand" : "conversation.collapse")}
              title={t(collapsed ? "conversation.expand" : "conversation.collapse")}
              aria-expanded={!collapsed}
              icon={collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
              onClick={onToggleCollapsed}
            />
          ) : null}
        </div>
      </header>
      {!collapsed ? (
        <>
          {toolbar}
          <div className="list-stack">
            {conversations.length === 0 ? (
              <EmptyState
                title={t("conversation.empty")}
                detail={t("conversation.emptyDetail")}
                action={
                  <Button
                    size="sm"
                    variant="primary"
                    icon={<Plus size={14} />}
                    onClick={onCreate}
                    disabled={creating}
                  >
                    {t("conversation.create")}
                  </Button>
                }
              />
            ) : (
              conversations.map((conversation) => (
                <button
                  key={conversation.id}
                  className={`list-row ${conversation.id === selectedId ? "active" : ""}`}
                  onClick={() => onSelect(conversation.id)}
                >
                  <span>{conversation.title}</span>
                  <StatusPill status={conversation.status} />
                </button>
              ))
            )}
          </div>
        </>
      ) : null}
    </section>
  );
}
