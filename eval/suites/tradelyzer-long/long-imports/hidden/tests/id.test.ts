import { afterEach, expect, it, vi } from "vitest";
import { createId } from "@/lib/id";

afterEach(() => vi.unstubAllGlobals());

it("creates unique version 4 IDs when randomUUID is unavailable", () => {
  const nativeCrypto = crypto;
  vi.stubGlobal("crypto", {
    getRandomValues: nativeCrypto.getRandomValues.bind(nativeCrypto),
  });
  const ids = Array.from({ length: 100 }, createId);
  expect(new Set(ids).size).toBe(ids.length);
  for (const id of ids)
    expect(id).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
});
