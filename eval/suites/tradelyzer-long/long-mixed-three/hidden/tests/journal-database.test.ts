import "fake-indexeddb/auto";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JournalDatabase } from "@/lib/db/journal-database";
import { parseJson } from "@/features/trades/schemas/trade-file.schema";

const fixture = parseJson(
  readFileSync(new URL("./fixtures/trades.json", import.meta.url), "utf8"),
);
const databases: JournalDatabase[] = [];
function open(name = crypto.randomUUID()) {
  const db = new JournalDatabase(name);
  databases.push(db);
  return db;
}
afterEach(async () => {
  vi.restoreAllMocks();
  for (const db of databases.splice(0)) await db.delete();
});

describe("persistent trade imports", () => {
  it("starts empty and restores exact trades after reopening", async () => {
    const db = open();
    expect(await db.loadTrades()).toBeNull();
    await db.saveTrades(fixture);
    db.close();
    const reopened = open(db.name);
    expect(await reopened.loadTrades()).toEqual(fixture);
  });

  it("replaces the active import without leaving old trades", async () => {
    const db = open();
    await db.saveTrades(fixture);
    const replacement = { ...fixture, trades: [fixture.trades[0]] };
    await db.saveTrades(replacement);
    expect(await db.loadTrades()).toEqual(replacement);
    expect(await db.imports.count()).toBe(1);
  });

  it("preserves the previous import when validation or storage fails", async () => {
    const db = open();
    await db.saveTrades(fixture);
    await expect(db.saveTrades({ ...fixture, trades: [] })).rejects.toThrow();
    vi.spyOn(db.imports, "put").mockRejectedValueOnce(
      new DOMException("Storage full", "QuotaExceededError"),
    );
    await expect(db.saveTrades(fixture)).rejects.toThrow("Storage full");
    expect(await db.loadTrades()).toEqual(fixture);
  });
});

it("upgrades version 1 without losing trades and persists separate day entries", async () => {
  const { default: Dexie } = await import("dexie");
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const name = crypto.randomUUID();
  const legacy = new Dexie(name);
  legacy.version(1).stores({ imports: "id" });
  await legacy.table("imports").put({ id: "active", data: fixture });
  legacy.close();
  const db = open(name);
  const entry = {
    ...emptyJournal("2026-09-08"),
    plan: "Wait for support",
    maxLoss: "100.01",
    tags: ["FOMO", "My tag"],
    mood: 4,
  };
  await db.saveJournal(entry);
  await db.saveJournal(emptyJournal("2026-09-09"));
  db.close();
  const reopened = open(name);
  expect(await reopened.loadTrades()).toEqual(fixture);
  expect(await reopened.journals.get(entry.date)).toEqual(entry);
  expect(await reopened.journals.count()).toBe(2);
  await expect(
    reopened.saveJournal({ ...entry, maxLoss: "-1" }),
  ).rejects.toThrow();
  vi.spyOn(reopened.journals, "put").mockRejectedValueOnce(
    new DOMException("Storage full", "QuotaExceededError"),
  );
  await expect(
    reopened.saveJournal({ ...entry, plan: "Changed" }),
  ).rejects.toThrow("Storage full");
  expect(await reopened.journals.get(entry.date)).toEqual(entry);
});

it("shares custom tags across days and reloads without selecting them on other days", async () => {
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const db = open();
  expect(await db.addTag("  Late entry  ")).toBe("Late entry");
  await db.addTag("Late entry");
  await db.saveJournal({ ...emptyJournal("2026-09-08"), tags: ["Late entry"] });
  await db.saveJournal(emptyJournal("2026-09-09"));
  db.close();
  const reopened = open(db.name);
  expect(await reopened.tags.toArray()).toEqual([{ name: "Late entry" }]);
  expect((await reopened.journals.get("2026-09-09"))?.tags).toEqual([]);
  await expect(reopened.addTag(" ")).rejects.toThrow();
  vi.spyOn(reopened.tags, "put").mockRejectedValueOnce(
    new Error("Storage full"),
  );
  await expect(reopened.addTag("Failed tag")).rejects.toThrow("Storage full");
  expect(await reopened.tags.count()).toBe(1);
});

it("migrates existing journal tags into the shared library without changing entries", async () => {
  const { default: Dexie } = await import("dexie");
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const name = crypto.randomUUID();
  const legacy = new Dexie(name);
  legacy.version(2).stores({ imports: "id", journals: "date" });
  const entry = { ...emptyJournal("2026-09-08"), tags: ["Late entry", "FOMO"] };
  await legacy.table("journals").put(entry);
  await legacy
    .table("journals")
    .put({ ...emptyJournal("2026-09-09"), tags: ["Late entry"] });
  legacy.close();
  const db = open(name);
  expect(await db.tags.toArray()).toEqual([
    { name: "FOMO" },
    { name: "Late entry" },
  ]);
  expect(await db.journals.get(entry.date)).toEqual(entry);
});

it("stores original screenshot blobs per day, survives reopening and persists deletion", async () => {
  const db = open();
  const file = new File([new Uint8Array([1, 2, 3])], "chart.png", {
    type: "image/png",
  });
  await db.addScreenshots("2026-09-08", [file, file]);
  await db.addScreenshots("2026-09-09", [file]);
  db.close();
  const reopened = open(db.name);
  const images = await reopened.screenshots
    .where("date")
    .equals("2026-09-08")
    .toArray();
  expect(images).toHaveLength(2);
  expect(images[0].image.type).toBe("image/png");
  expect(new Uint8Array(await images[0].image.arrayBuffer())).toEqual(
    new Uint8Array([1, 2, 3]),
  );
  await reopened.screenshots.delete(images[0].id);
  reopened.close();
  const restored = open(db.name);
  expect(
    await restored.screenshots.where("date").equals("2026-09-08").count(),
  ).toBe(1);
  expect(
    await restored.screenshots.where("date").equals("2026-09-09").count(),
  ).toBe(1);
});

it("rejects invalid screenshot batches and preserves saved images after storage failure", async () => {
  const db = open();
  const image = new File(["image"], "chart.png", { type: "image/png" });
  await db.addScreenshots("2026-09-08", [image]);
  await expect(
    db.addScreenshots("2026-09-08", [
      image,
      new File(["text"], "text.txt", { type: "text/plain" }),
    ]),
  ).rejects.toThrow("Choose PNG");
  await expect(db.addScreenshots("2026-02-30", [image])).rejects.toThrow(
    "valid journal date",
  );
  await expect(
    db.addScreenshots("2026-09-08", [
      new File([], "empty.png", { type: "image/png" }),
    ]),
  ).rejects.toThrow("10 MB");
  await expect(
    db.addScreenshots("2026-09-08", [
      new File([new Uint8Array(10 * 1024 * 1024 + 1)], "large.png", {
        type: "image/png",
      }),
    ]),
  ).rejects.toThrow("10 MB");
  vi.spyOn(db.screenshots, "bulkAdd").mockRejectedValueOnce(
    new Error("Storage full"),
  );
  await expect(db.addScreenshots("2026-09-08", [image])).rejects.toThrow(
    "Storage full",
  );
  expect(await db.screenshots.count()).toBe(1);
});

it("upgrades existing journals and tag libraries without losing them", async () => {
  const { default: Dexie } = await import("dexie");
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const name = crypto.randomUUID();
  const legacy = new Dexie(name);
  legacy.version(3).stores({ imports: "id", journals: "date", tags: "name" });
  const entry = emptyJournal("2026-09-08");
  await legacy.table("journals").put(entry);
  await legacy.table("tags").put({ name: "Late entry" });
  legacy.close();
  const db = open(name);
  expect(await db.journals.get(entry.date)).toEqual(entry);
  expect(await db.tags.toArray()).toEqual([{ name: "Late entry" }]);
  expect(await db.screenshots.count()).toBe(0);
});

it("commits screenshot drafts only with the journal and rolls back failed saves", async () => {
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const db = open();
  const entry = emptyJournal("2026-09-12");
  const image = new Blob(["original"], { type: "image/png" });
  const screenshot = {
    id: crypto.randomUUID(),
    date: entry.date,
    name: "chart.png",
    image,
  };
  expect(await db.screenshots.count()).toBe(0);
  await db.saveJournal(entry, { added: [screenshot], removed: [] });
  expect(await db.screenshots.count()).toBe(1);
  const replacement = { ...screenshot, id: crypto.randomUUID() };
  vi.spyOn(db.journals, "put").mockRejectedValueOnce(new Error("Storage full"));
  await expect(
    db.saveJournal(
      { ...entry, plan: "Changed" },
      { added: [replacement], removed: [screenshot.id] },
    ),
  ).rejects.toThrow("Storage full");
  expect(await db.screenshots.get(screenshot.id)).toBeDefined();
  expect(await db.screenshots.get(replacement.id)).toBeUndefined();
  expect((await db.journals.get(entry.date))?.plan).toBe("");
  await db.saveJournal(entry, { added: [], removed: [screenshot.id] });
  expect(await db.screenshots.count()).toBe(0);
});

it("prevents attachment changes from modifying another day's screenshots", async () => {
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const db = open();
  const entry = emptyJournal("2026-09-12");
  const screenshot = {
    id: crypto.randomUUID(),
    date: "2026-09-13",
    name: "chart.png",
    image: new Blob(["image"], { type: "image/png" }),
  };
  await db.screenshots.add(screenshot);
  await expect(
    db.saveJournal(entry, { added: [], removed: [screenshot.id] }),
  ).rejects.toThrow("another day");
  await expect(
    db.saveJournal(entry, { added: [screenshot], removed: [] }),
  ).rejects.toThrow("another day");
  expect(await db.screenshots.get(screenshot.id)).toBeDefined();
  expect(await db.journals.count()).toBe(0);
});

it("persists and replaces the default plan without saving or changing day journals", async () => {
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const db = open();
  const entry = { ...emptyJournal("2026-09-08"), plan: "Original day plan" };
  await db.saveJournal(entry);
  const template = {
    bias: "bullish" as const,
    levels: "Previous high",
    maxLoss: "200.01",
    plan: "Wait for confirmation",
  };
  await db.savePlanTemplate(template);
  db.close();
  const reopened = open(db.name);
  const restored = await reopened.templates.get("default");
  expect(restored).toEqual({ ...template, id: "default" });
  expect(emptyJournal("2026-09-09", restored).plan).toBe(template.plan);
  expect(await reopened.journals.get("2026-09-09")).toBeUndefined();
  await reopened.savePlanTemplate({ ...template, plan: "New routine" });
  expect(await reopened.templates.count()).toBe(1);
  expect((await reopened.templates.get("default"))?.plan).toBe("New routine");
  expect(await reopened.journals.get(entry.date)).toEqual(entry);
  await expect(
    reopened.savePlanTemplate({ ...template, maxLoss: "-1" }),
  ).rejects.toThrow();
  vi.spyOn(reopened.templates, "put").mockRejectedValueOnce(
    new Error("Storage full"),
  );
  await expect(reopened.savePlanTemplate(template)).rejects.toThrow(
    "Storage full",
  );
  expect((await reopened.templates.get("default"))?.plan).toBe("New routine");
});

it("upgrades version 4 without losing journals, tags, imports or screenshot blobs", async () => {
  const { default: Dexie } = await import("dexie");
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const name = crypto.randomUUID();
  const legacy = new Dexie(name);
  legacy.version(4).stores({
    imports: "id",
    journals: "date",
    tags: "name",
    screenshots: "id, date",
  });
  const entry = emptyJournal("2026-09-08");
  const screenshot = {
    id: "chart",
    date: entry.date,
    name: "chart.png",
    image: new Blob(["chart"], { type: "image/png" }),
  };
  await legacy.table("imports").put({ id: "active", data: fixture });
  await legacy.table("journals").put(entry);
  await legacy.table("tags").put({ name: "Late entry" });
  await legacy.table("screenshots").put(screenshot);
  legacy.close();
  const db = open(name);
  expect(await db.templates.count()).toBe(0);
  expect(await db.loadTrades()).toEqual(fixture);
  expect(await db.journals.get(entry.date)).toEqual(entry);
  expect(await db.tags.toArray()).toEqual([{ name: "Late entry" }]);
  expect(await (await db.screenshots.get("chart"))?.image.text()).toBe("chart");
});

it("limits each journal to ten tags and preserves the saved entry on overflow", async () => {
  const { emptyJournal } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const db = open();
  const entry = {
    ...emptyJournal("2026-10-11"),
    tags: Array.from({ length: 10 }, (_, i) => `Tag ${i + 1}`),
  };
  await db.saveJournal(entry);
  await expect(
    db.saveJournal({ ...entry, tags: [...entry.tags, "Tag 11"] }),
  ).rejects.toThrow("Choose up to 10 mistake tags.");
  expect(await db.journals.get(entry.date)).toEqual(entry);
});

it("preserves legacy journals with more than ten tags while preventing new overflow", async () => {
  const { emptyJournal, journalSchemaFor } = await import(
    "@/features/journal/schemas/journal.schema"
  );
  const db = open();
  const entry = {
    ...emptyJournal("2026-09-08"),
    tags: Array.from({ length: 20 }, (_, i) => `Legacy ${i}`),
  };
  await db.journals.put(entry);
  const edited = { ...entry, plan: "Updated without losing tags" };
  expect(journalSchemaFor(entry).safeParse(edited).success).toBe(true);
  await db.saveJournal(edited);
  expect(await db.journals.get(entry.date)).toEqual(edited);
  await expect(
    db.saveJournal({ ...edited, tags: [...edited.tags, "New tag"] }),
  ).rejects.toThrow("Existing tags");
  await db.saveJournal({ ...edited, tags: edited.tags.slice(0, 10) });
  await expect(db.saveJournal(edited)).rejects.toThrow("Choose up to 10");
  await expect(
    db.saveJournal({ ...edited, date: "2026-09-09" }),
  ).rejects.toThrow("Choose up to 10");
});
