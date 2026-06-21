import { describe, it, expect } from "vitest";
import { thumbUrl } from "./api";

describe("thumbUrl", () => {
  it("encodes the path and width", () => {
    const u = thumbUrl("C:\\x\\a b.jpg", 256);
    expect(u).toBe(`/api/thumb?path=${encodeURIComponent("C:\\x\\a b.jpg")}&w=256`);
  });
});
