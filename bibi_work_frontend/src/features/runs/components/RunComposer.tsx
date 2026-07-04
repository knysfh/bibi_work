import { Send, Square } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useI18n } from "../../../shared/i18n";
import { Button, TextArea } from "../../../shared/ui";

export function RunComposer({
  disabled,
  streaming,
  draft,
  autoFocusSignal,
  onSubmit,
  onCancel
}: {
  disabled?: boolean;
  streaming?: boolean;
  draft?: { id: string; content: string };
  autoFocusSignal?: number;
  onSubmit: (content: string) => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [content, setContent] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (draft) {
      setContent(draft.content);
    }
  }, [draft]);
  useEffect(() => {
    if (!disabled && autoFocusSignal) {
      textareaRef.current?.focus();
    }
  }, [autoFocusSignal, disabled]);

  function submitContent() {
    const value = content.trim();
    if (!value) {
      return;
    }
    onSubmit(value);
    setContent("");
  }

  return (
    <form
      className="composer"
      onSubmit={(event) => {
        event.preventDefault();
        submitContent();
      }}
    >
      <TextArea
        ref={textareaRef}
        rows={3}
        value={content}
        onChange={(event) => setContent(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            submitContent();
          }
        }}
        disabled={disabled}
        placeholder={t("run.composerPlaceholder")}
      />
      <div className="composer-actions">
        <Button
          type="button"
          variant="ghost"
          icon={<Square size={15} />}
          onClick={onCancel}
          disabled={!streaming}
        >
          {t("common.stop")}
        </Button>
        <Button type="submit" variant="primary" icon={<Send size={15} />} disabled={disabled}>
          {t("common.send")}
        </Button>
      </div>
    </form>
  );
}
