import { useConnection } from "../store/useConnection";

const dots: Record<string, string> = {
  connected: "bg-green-400",
  connecting: "bg-yellow-400 animate-pulse",
  disconnected: "bg-gray-500",
  error: "bg-red-400",
};

const labels: Record<string, string> = {
  connected: "Connected",
  connecting: "Connecting…",
  disconnected: "Disconnected",
  error: "Error",
};

export function ConnectionStatus() {
  const { status, statusMessage } = useConnection();

  return (
    <div className="flex items-center gap-2 text-xs">
      <span className={`w-2 h-2 rounded-full ${dots[status]}`} />
      <span className="text-muted">{labels[status]}</span>
      {statusMessage && status === "error" && (
        <span className="text-red-400 truncate max-w-[120px]" title={statusMessage}>
          — {statusMessage}
        </span>
      )}
    </div>
  );
}
