import { Bot, Check, Copy, Edit3, RotateCcw, ThumbsDown, ThumbsUp, User } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useI18n } from "../../../shared/i18n";
import { Button, EmptyState, StatusPill } from "../../../shared/ui";
import type {
  RunProjection,
  TimelineMessage,
  TimelineProjectionItem,
  ToolCallProjection
} from "../domain/run.types";
import { MessageContentRenderer } from "./MessageContentRenderer";
import { ToolCallCard } from "./ToolCallCard";

type AssistantTimelineItem =
  | { kind: "message"; message: TimelineMessage }
  | { kind: "tool_call"; toolCall: ToolCallProjection };

type TimelineRenderItem =
  | { kind: "message"; message: TimelineMessage }
  | { kind: "assistant_group"; items: AssistantTimelineItem[] };

export function RunTimeline({
  projection,
  onEditMessage,
  onRegenerateMessage
}: {
  projection: RunProjection;
  onEditMessage?: (messageId: string) => void;
  onRegenerateMessage?: (messageId: string) => void;
}) {
  const { t } = useI18n();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [copiedMessageId, setCopiedMessageId] = useState<string | null>(null);
  const [feedbackByMessage, setFeedbackByMessage] = useState<Record<string, "up" | "down">>({});
  const timelineItems = projection.timeline.length
    ? projection.timeline
    : fallbackTimeline(projection);
  const messagesById = new Map(projection.messages.map((message) => [message.id, message]));
  const toolCallsById = new Map(projection.toolCalls.map((toolCall) => [toolCall.id, toolCall]));
  const renderItems = buildRenderItems(timelineItems, messagesById, toolCallsById);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) {
      return;
    }
    element.scrollTop = element.scrollHeight;
  }, [
    renderItems.length,
    projection.messages.at(-1)?.content,
    projection.toolCalls.at(-1)?.status
  ]);

  async function copyMessage(messageId: string, content: string) {
    try {
      await window.navigator.clipboard.writeText(content);
    } catch {
      return;
    }
    setCopiedMessageId(messageId);
    window.setTimeout(() => setCopiedMessageId(null), 1200);
  }

  function toggleFeedback(messageId: string, value: "up" | "down") {
    setFeedbackByMessage((current) => {
      const next = { ...current };
      if (next[messageId] === value) {
        delete next[messageId];
      } else {
        next[messageId] = value;
      }
      return next;
    });
  }

  return (
    <section className="timeline-panel">
      <header className="panel-header">
        <div>
          <strong>{t("run.chat")}</strong>
          <span>seq {projection.lastSeq}</span>
        </div>
        <StatusPill status={projection.status} />
      </header>
      <div className="timeline-scroll" ref={scrollRef}>
        {renderItems.length === 0 ? (
          <EmptyState title={t("run.waitingMessages")} detail={t("run.waitingMessagesDetail")} />
        ) : null}
        {renderItems.map((item) => {
          if (item.kind === "message") {
            return (
              <MessageRow
                key={`message:${item.message.id}`}
                message={item.message}
                copiedMessageId={copiedMessageId}
                feedback={feedbackByMessage[item.message.id]}
                onCopy={copyMessage}
                onEdit={onEditMessage}
                onFeedback={toggleFeedback}
                onRegenerate={onRegenerateMessage}
              />
            );
          }

          return (
            <AssistantGroupRow
              key={`assistant_group:${item.items
                .map((groupItem) =>
                  groupItem.kind === "tool_call"
                    ? `tool:${groupItem.toolCall.id}`
                    : `message:${groupItem.message.id}`
                )
                .join("|")}`}
              items={item.items}
              copiedMessageId={copiedMessageId}
              feedbackByMessage={feedbackByMessage}
              onCopy={copyMessage}
              onFeedback={toggleFeedback}
              onRegenerate={onRegenerateMessage}
            />
          );
        })}
      </div>
    </section>
  );
}

function AssistantGroupRow({
  items,
  copiedMessageId,
  feedbackByMessage,
  onCopy,
  onFeedback,
  onRegenerate
}: {
  items: AssistantTimelineItem[];
  copiedMessageId: string | null;
  feedbackByMessage: Record<string, "up" | "down">;
  onCopy: (messageId: string, content: string) => void;
  onFeedback: (messageId: string, value: "up" | "down") => void;
  onRegenerate?: (messageId: string) => void;
}) {
  const assistantMessages = items.flatMap((item) =>
    item.kind === "message" ? [item.message] : []
  );
  const status = assistantMessages.at(-1)?.status ?? toolGroupStatus(items);

  return (
    <article className="message-row message-assistant">
      <span className="message-icon">
        <Bot size={16} />
      </span>
      <div className="message-body">
        <div className="message-meta">
          <strong>assistant</strong>
          <StatusPill status={status} />
        </div>
        <div className="assistant-item-stack">
          {items.map((item) =>
            item.kind === "tool_call" ? (
              <div key={`tool_call:${item.toolCall.id}`} className="tool-call-stack">
                <ToolCallCard toolCall={item.toolCall} />
              </div>
            ) : (
              <AssistantMessageContent
                key={`message:${item.message.id}`}
                message={item.message}
                copiedMessageId={copiedMessageId}
                feedback={feedbackByMessage[item.message.id]}
                onCopy={onCopy}
                onFeedback={onFeedback}
                onRegenerate={onRegenerate}
              />
            )
          )}
        </div>
      </div>
    </article>
  );
}

function MessageRow({
  message,
  copiedMessageId,
  feedback,
  onCopy,
  onEdit,
  onFeedback,
  onRegenerate
}: {
  message: TimelineMessage;
  copiedMessageId: string | null;
  feedback?: "up" | "down";
  onCopy: (messageId: string, content: string) => void;
  onEdit?: (messageId: string) => void;
  onFeedback: (messageId: string, value: "up" | "down") => void;
  onRegenerate?: (messageId: string) => void;
}) {
  const { t } = useI18n();

  return (
    <article className={`message-row message-${message.role}`}>
      <span className="message-icon">
        {message.role === "user" ? <User size={16} /> : <Bot size={16} />}
      </span>
      <div className="message-body">
        <div className="message-meta">
          <strong>{message.role}</strong>
          <StatusPill status={message.status} />
        </div>
        {message.role === "user" ? (
          <p className="message-plain-text">{message.content}</p>
        ) : (
          <AssistantMessageContent
            message={message}
            copiedMessageId={copiedMessageId}
            feedback={feedback}
            onCopy={onCopy}
            onFeedback={onFeedback}
            onRegenerate={onRegenerate}
          />
        )}
        {message.role === "user" ? (
          <div className="message-actions">
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label={t("run.message.copy")}
              title={t("run.message.copy")}
              icon={copiedMessageId === message.id ? <Check size={15} /> : <Copy size={15} />}
              onClick={() => onCopy(message.id, message.content)}
            />
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label={t("run.message.edit")}
              title={t("run.message.edit")}
              icon={<Edit3 size={15} />}
              onClick={() => onEdit?.(message.id)}
            />
          </div>
        ) : null}
      </div>
    </article>
  );
}

function AssistantMessageContent({
  message,
  copiedMessageId,
  feedback,
  onCopy,
  onFeedback,
  onRegenerate
}: {
  message: TimelineMessage;
  copiedMessageId: string | null;
  feedback?: "up" | "down";
  onCopy: (messageId: string, content: string) => void;
  onFeedback: (messageId: string, value: "up" | "down") => void;
  onRegenerate?: (messageId: string) => void;
}) {
  const { t } = useI18n();

  return (
    <div className="assistant-message-block">
      <MessageContentRenderer content={message.content} />
      <div className="message-actions">
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label={t("run.message.copy")}
          title={t("run.message.copy")}
          icon={copiedMessageId === message.id ? <Check size={15} /> : <Copy size={15} />}
          onClick={() => onCopy(message.id, message.content)}
        />
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label={t("run.message.regenerate")}
          title={t("run.message.regenerate")}
          icon={<RotateCcw size={15} />}
          onClick={() => onRegenerate?.(message.id)}
        />
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label={t("run.message.feedbackUp")}
          title={t("run.message.feedbackUp")}
          aria-pressed={feedback === "up"}
          icon={<ThumbsUp size={15} />}
          onClick={() => onFeedback(message.id, "up")}
        />
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label={t("run.message.feedbackDown")}
          title={t("run.message.feedbackDown")}
          aria-pressed={feedback === "down"}
          icon={<ThumbsDown size={15} />}
          onClick={() => onFeedback(message.id, "down")}
        />
      </div>
    </div>
  );
}

function buildRenderItems(
  timelineItems: TimelineProjectionItem[],
  messagesById: Map<string, TimelineMessage>,
  toolCallsById: Map<string, ToolCallProjection>
): TimelineRenderItem[] {
  const renderItems: TimelineRenderItem[] = [];
  for (const item of timelineItems) {
    if (item.kind === "tool_call") {
      const toolCall = toolCallsById.get(item.id);
      if (toolCall) {
        appendAssistantItem(renderItems, { kind: "tool_call", toolCall });
      }
      continue;
    }

    const message = messagesById.get(item.id);
    if (!message) {
      continue;
    }
    if (message.role === "assistant") {
      appendAssistantItem(renderItems, { kind: "message", message });
    } else {
      renderItems.push({ kind: "message", message });
    }
  }
  return renderItems;
}

function appendAssistantItem(renderItems: TimelineRenderItem[], item: AssistantTimelineItem) {
  const latest = renderItems.at(-1);
  if (latest?.kind === "assistant_group") {
    latest.items.push(item);
    return;
  }
  renderItems.push({ kind: "assistant_group", items: [item] });
}

function toolGroupStatus(items: AssistantTimelineItem[]): string {
  const latestTool = [...items].reverse().find((item) => item.kind === "tool_call");
  return latestTool?.kind === "tool_call" ? latestTool.toolCall.status : "completed";
}

function fallbackTimeline(projection: RunProjection): TimelineProjectionItem[] {
  return [
    ...projection.messages.map((message) => ({ kind: "message" as const, id: message.id })),
    ...projection.toolCalls.map((toolCall) => ({ kind: "tool_call" as const, id: toolCall.id }))
  ];
}
