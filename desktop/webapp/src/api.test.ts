import { describe, it, expect, vi, afterEach } from "vitest";
import { thumbUrl, engineStatus, listModels, loadModel, stopEngine, setModelsDir } from "./api";

describe("thumbUrl", () => {
  it("encodes the path and width", () => {
    const u = thumbUrl("C:\\x\\a b.jpg", 256);
    expect(u).toBe(`/api/thumb?path=${encodeURIComponent("C:\\x\\a b.jpg")}&w=256`);
  });
});

function mockFetch(response: { ok: boolean; status?: number; json?: () => unknown; text?: () => unknown }) {
  const fn = vi.fn().mockResolvedValue({
    ok: response.ok,
    status: response.status ?? (response.ok ? 200 : 400),
    json: response.json ?? (async () => ({})),
    text: response.text ?? (async () => ""),
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("engineStatus", () => {
  it("parses a successful status response", async () => {
    const status = {
      state: "ready", error: null, base_url: "http://127.0.0.1:8080", vision: true,
      model: { name: "llava.gguf", path: "C:\\models\\llava.gguf", size_bytes: 123, quant: "Q4_K_M", mmproj: null },
    };
    mockFetch({ ok: true, json: async () => status });
    expect(await engineStatus()).toEqual(status);
  });

  it("falls back to a stopped status when the request fails", async () => {
    mockFetch({ ok: false, status: 500 });
    expect(await engineStatus()).toEqual({ state: "stopped", error: null, base_url: null, vision: false, model: null });
  });
});

describe("listModels", () => {
  it("returns the parsed catalog on success", async () => {
    const body = { dir: "C:\\models", models: [{ name: "a.gguf", path: "C:\\models\\a.gguf", size_bytes: 1, quant: "Q4_K_M", vision: false, loadable: true }] };
    mockFetch({ ok: true, json: async () => body });
    expect(await listModels()).toEqual(body);
  });

  it("falls back to an empty catalog on failure", async () => {
    mockFetch({ ok: false, status: 404 });
    expect(await listModels()).toEqual({ dir: "", models: [] });
  });
});

describe("loadModel", () => {
  it("returns the parsed ok body on success", async () => {
    mockFetch({ ok: true, json: async () => ({ ok: true }) });
    expect(await loadModel("C:\\models\\a.gguf")).toEqual({ ok: true });
  });

  it("parses the JSON {error} body from a 400 response", async () => {
    mockFetch({ ok: false, status: 400, text: async () => JSON.stringify({ error: "model not found" }) });
    expect(await loadModel("C:\\models\\missing.gguf")).toEqual({ error: "model not found" });
  });

  it("falls back to the raw text when the error body isn't JSON", async () => {
    mockFetch({ ok: false, status: 500, text: async () => "internal server error" });
    expect(await loadModel("C:\\models\\a.gguf")).toEqual({ error: "internal server error" });
  });
});

describe("setModelsDir", () => {
  it("returns the parsed catalog on success", async () => {
    const body = { dir: "C:\\models2", models: [] };
    mockFetch({ ok: true, json: async () => body });
    expect(await setModelsDir("C:\\models2")).toEqual(body);
  });

  it("falls back to an empty catalog on a 400 error", async () => {
    mockFetch({ ok: false, status: 400, text: async () => JSON.stringify({ error: "no such directory" }) });
    expect(await setModelsDir("C:\\nope")).toEqual({ dir: "", models: [] });
  });
});

describe("stopEngine", () => {
  it("resolves without throwing on success", async () => {
    mockFetch({ ok: true });
    await expect(stopEngine()).resolves.toBeUndefined();
  });

  it("resolves without throwing on failure", async () => {
    mockFetch({ ok: false, status: 500 });
    await expect(stopEngine()).resolves.toBeUndefined();
  });
});
