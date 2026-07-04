import { Construction } from "lucide-react";
import { useI18n, type I18nKey } from "../shared/i18n";
import { EmptyState } from "../shared/ui";

export function PlaceholderScreen({ titleKey }: { titleKey: I18nKey }) {
  const { t } = useI18n();
  const title = t(titleKey);
  return (
    <div className="placeholder-screen">
      <Construction size={24} />
      <EmptyState
        title={t("app.placeholderSuffix", { title })}
        detail={t("app.placeholderDetail")}
      />
    </div>
  );
}
