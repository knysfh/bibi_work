import { AlertTriangle } from "lucide-react";
import { Button } from "./Button";
import { ConfigPanel } from "./ConfigPanel";

interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
  pending?: boolean;
  onCancel: () => void;
  onConfirm: () => void | Promise<void>;
}

export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  cancelLabel,
  pending = false,
  onCancel,
  onConfirm
}: ConfirmDialogProps) {
  return (
    <ConfigPanel title={title} closeLabel={cancelLabel} onClose={onCancel}>
      <div className="config-form confirm-dialog-body">
        <div className="confirm-dialog-message">
          <AlertTriangle size={18} />
          <p>{message}</p>
        </div>
        <div className="row-actions">
          <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
            {cancelLabel}
          </Button>
          <Button type="button" variant="danger" onClick={onConfirm} disabled={pending}>
            {confirmLabel}
          </Button>
        </div>
      </div>
    </ConfigPanel>
  );
}
