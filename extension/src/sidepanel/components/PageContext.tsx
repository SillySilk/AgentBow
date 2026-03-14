import { useConnection } from "../store/useConnection";

export function PageContext() {
  const { currentPage } = useConnection();

  if (!currentPage) return null;

  const hostname = (() => {
    try {
      return new URL(currentPage.url).hostname;
    } catch {
      return currentPage.url;
    }
  })();

  return (
    <div className="bg-surface border border-border rounded px-3 py-2 text-xs text-muted">
      <div className="font-medium text-white truncate">{currentPage.title}</div>
      <div className="text-muted/60 truncate">{hostname}</div>
    </div>
  );
}
