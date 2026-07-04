import { Component, type ErrorInfo, type ReactNode } from "react";
import { useI18n } from "../../shared/i18n";

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  error?: Error;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = {};

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Bibi Work UI boundary caught an error", error, info);
  }

  render() {
    if (this.state.error) {
      return this.props.fallback ?? <DefaultErrorFallback error={this.state.error} />;
    }
    return this.props.children;
  }
}

function DefaultErrorFallback({ error }: { error: Error }) {
  const { t } = useI18n();
  return (
    <div className="empty-state">
      <strong>{t("app.errorBoundary")}</strong>
      <span>{error.message}</span>
    </div>
  );
}
