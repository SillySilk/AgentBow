import { useCallback, useEffect, useState } from "react";
import { RotateCw, Square } from "lucide-react";
import { useStore } from "../store";
import type { EngineModel } from "../api";
import { engineStatus, listModels, loadModel, setModelsDir, stopEngine } from "../api";
import Button from "./ui/Button";

function fmtGB(bytes: number): string {
  return (bytes / 1024 ** 3).toFixed(1);
}

export default function SettingsPanel() {
  const engine = useStore((s) => s.engine);
  const setEngine = useStore((s) => s.setEngine);
  const [dir, setDir] = useState("");
  const [dirInput, setDirInput] = useState("");
  const [models, setModels] = useState<EngineModel[]>([]);
  const [rescanning, setRescanning] = useState(false);
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refreshModels = useCallback(() => {
    return listModels().then((res) => {
      setDir(res.dir);
      setDirInput(res.dir);
      setModels(res.models);
    });
  }, []);

  // Initial catalog load. State is only ever set inside the promise callback,
  // never synchronously in the effect body (react-hooks/set-state-in-effect).
  useEffect(() => { Promise.resolve().then(refreshModels); }, [refreshModels]);

  // Poll engine status faster than the app-wide 5s poll while the Workshop is
  // open, so load/stop transitions show up promptly. Same promise-callback
  // pattern as CurationGrid's effects.
  useEffect(() => {
    let cancelled = false;
    const poll = () => { engineStatus().then((s) => { if (!cancelled) setEngine(s); }); };
    Promise.resolve().then(poll);
    const id = setInterval(poll, 3000);
    return () => { cancelled = true; clearInterval(id); };
  }, [setEngine]);

  const onRescan = async () => {
    setRescanning(true);
    try {
      const res = await setModelsDir(dirInput.trim());
      setDir(res.dir);
      setModels(res.models);
    } finally {
      setRescanning(false);
    }
  };

  const onLoad = async (path: string) => {
    setLoadError(null);
    setBusyPath(path);
    try {
      const res = await loadModel(path);
      if (res.error) setLoadError(res.error);
      setEngine(await engineStatus());
    } finally {
      setBusyPath(null);
    }
  };

  const onStop = async () => {
    await stopEngine();
    setEngine(await engineStatus());
  };

  const state = engine?.state ?? "stopped";
  const stateColor =
    state === "ready" ? "var(--absinthe)" :
    state === "failed" ? "var(--ember-400)" :
    state === "starting" ? "var(--flame-500)" :
    "var(--text-forge-mute)";
  const stateLabel =
    state === "ready" ? "RUNNING" :
    state === "starting" ? "STARTING" :
    state === "failed" ? "FAILED" :
    "STOPPED";

  return (
    <div
      style={{
        height: "100%",
        backgroundColor: "var(--surface-forge-card)",
        backgroundImage: "linear-gradient(180deg, rgba(20,19,18,.6), rgba(20,19,18,.92)), url('./panel_metal.png')",
        backgroundSize: "cover",
        border: "1px solid var(--border-forge)",
        borderRadius: "var(--radius-forge-lg)",
        padding: 20,
        display: "flex",
        flexDirection: "column",
        gap: 16,
        minHeight: 0,
        overflow: "auto",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <span style={{ fontFamily: "var(--font-type)", color: "var(--gold-500)", fontSize: 12, letterSpacing: ".2em" }}>THE WORKSHOP</span>
        <span style={{ fontFamily: "var(--font-marker)", color: "var(--ember-400)", fontSize: 15, transform: "rotate(-3deg)" }}>keep the machine fed</span>
      </div>

      <div>
        <label className="forge-label">Models folder</label>
        <div style={{ display: "flex", gap: 8, marginTop: 5 }}>
          <input
            className="forge-input"
            style={{ flex: 1, minWidth: 0 }}
            value={dirInput}
            onChange={(e) => setDirInput(e.target.value)}
            placeholder="C:\AI\models"
          />
          <Button variant="ghost" size="md" disabled={rescanning || !dirInput.trim()} onClick={onRescan}>
            <RotateCw size={14} />
            {rescanning ? "Scanning…" : "Rescan"}
          </Button>
        </div>
      </div>

      <div style={{
        display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12,
        padding: "10px 12px", background: "var(--surface-forge-well)",
        border: "1px solid var(--border-forge)", borderRadius: "var(--radius-forge-md)",
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: 9, minWidth: 0 }}>
          <span style={{ width: 8, height: 8, flex: "none", borderRadius: "50%", background: stateColor, boxShadow: `0 0 8px ${stateColor}` }} />
          <div style={{ lineHeight: 1.35, minWidth: 0 }}>
            <div style={{ fontFamily: "var(--font-type)", fontSize: 11, letterSpacing: ".08em", color: "var(--text-forge-cream)" }}>
              {stateLabel}{engine?.model ? ` · ${engine.model.name}` : ""}
            </div>
            {state === "starting" && (
              <div style={{ fontFamily: "var(--font-body)", fontSize: 12, color: "var(--text-forge-mute)" }}>
                Loading model — first load can take a minute…
              </div>
            )}
            {state === "failed" && engine?.error && (
              <div style={{ fontFamily: "var(--font-body)", fontSize: 12, color: "var(--ember-400)" }}>{engine.error}</div>
            )}
          </div>
        </div>
        {state === "ready" && (
          <Button variant="danger" size="sm" onClick={onStop}>
            <Square size={12} />
            Stop
          </Button>
        )}
      </div>

      {loadError && (
        <div style={{ fontFamily: "var(--font-body)", fontSize: 12, color: "var(--ember-400)" }}>{loadError}</div>
      )}

      <div style={{ flex: 1, overflow: "auto", minHeight: 0 }}>
        <div style={{
          display: "grid", gridTemplateColumns: "1fr 72px 68px 68px 92px", gap: 8,
          padding: "0 4px 6px", fontFamily: "var(--font-type)", fontSize: 9.5, letterSpacing: ".1em",
          color: "var(--text-forge-mute)", borderBottom: "1px solid var(--border-forge)",
        }}>
          <span>NAME</span><span>QUANT</span><span>SIZE</span><span>VISION</span><span />
        </div>

        {models.length === 0 ? (
          <div style={{ fontFamily: "var(--font-type)", fontSize: 11, color: "var(--text-forge-mute)", padding: "10px 4px" }}>
            No GGUF models found in {dir || "the models folder"}.
          </div>
        ) : (
          models.map((m) => {
            const isLoaded = state === "ready" && engine?.model?.path === m.path;
            return (
              <div key={m.path} style={{
                display: "grid", gridTemplateColumns: "1fr 72px 68px 68px 92px", gap: 8, alignItems: "center",
                padding: "8px 4px", borderBottom: "1px solid var(--border-forge)",
              }}>
                <span
                  style={{ fontFamily: "var(--font-body)", color: "var(--text-forge-cream)", fontSize: 13, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                  title={m.name}
                >
                  {m.name}
                </span>
                <span className={`tag${m.quant ? " on" : ""}`} style={{ justifySelf: "start", cursor: "default" }}>
                  {m.quant ?? "—"}
                </span>
                <span style={{ fontFamily: "var(--font-type)", fontSize: 11, color: "var(--text-forge-mute)" }}>
                  {fmtGB(m.size_bytes)} GB
                </span>
                <span style={{ fontFamily: "var(--font-type)", fontSize: 10, letterSpacing: ".06em", color: m.vision ? "var(--absinthe)" : "var(--text-forge-faint)" }}>
                  {m.vision ? "VISION" : "—"}
                </span>
                <Button
                  variant={isLoaded ? "ghost" : "forge"}
                  size="sm"
                  disabled={!m.loadable || busyPath === m.path || isLoaded}
                  title={!m.loadable ? "unquantized — not loadable" : undefined}
                  onClick={() => onLoad(m.path)}
                >
                  {isLoaded ? "Loaded" : busyPath === m.path ? "…" : "Load"}
                </Button>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
