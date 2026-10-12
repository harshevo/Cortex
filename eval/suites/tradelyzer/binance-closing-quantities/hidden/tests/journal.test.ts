import { describe, expect, it } from "vitest";
import {
  emptyJournal,
  exceedsLossLimit,
  isJournalDate,
  journalSchema,
  planTemplateSchema,
} from "@/features/journal/schemas/journal.schema";
import {
  summarizeTrades,
  type Trade,
} from "@/features/trades/schemas/trade-file.schema";
import { toCents } from "@/lib/money";

describe("day journal logic", () => {
  it("prefills only premarket fields, leaving reviews and dates specific to each day", () => {
    const source = {
      ...emptyJournal("2026-09-08"),
      bias: "bullish" as const,
      levels: "Previous day high",
      maxLoss: "200.01",
      plan: "Wait for confirmation",
      wentWell: "Good entry",
      tags: ["FOMO"],
      mood: 5,
    };
    const template = planTemplateSchema.parse(source);
    expect(template).toEqual({
      bias: "bullish",
      levels: source.levels,
      maxLoss: "200.01",
      plan: source.plan,
    });
    expect(emptyJournal("2026-09-09", template)).toEqual({
      ...emptyJournal("2026-09-09"),
      ...template,
    });
    expect(emptyJournal("2026-09-10", template).tags).not.toBe(source.tags);
  });
  it("validates dates and optional exact-cent loss limits", () => {
    expect(isJournalDate("2026-02-30")).toBe(false);
    expect(isJournalDate("2028-02-29")).toBe(true);
    for (const maxLoss of ["", "0", "250.50"])
      expect(
        journalSchema.safeParse({ ...emptyJournal("2026-09-08"), maxLoss })
          .success,
      ).toBe(true);
    for (const maxLoss of ["-1", "0.001", "abc"])
      expect(
        journalSchema.safeParse({ ...emptyJournal("2026-09-08"), maxLoss })
          .success,
      ).toBe(false);
  });
  it("warns only when net loss strictly exceeds the configured limit", () => {
    expect(exceedsLossLimit(toCents("-100.01"), "100")).toBe(true);
    expect(exceedsLossLimit(toCents("-100"), "100")).toBe(false);
    expect(exceedsLossLimit(toCents("100"), "0")).toBe(false);
    expect(exceedsLossLimit(toCents("-0.01"), "0")).toBe(true);
    expect(exceedsLossLimit(toCents("-100"), "")).toBe(false);
  });
  it("summarizes winning, losing, breakeven and empty days", () => {
    const trades = ["12.40", "-7.10", "0.00", "30.00"].map(
      (netPnl) => ({ netPnl }) as Trade,
    );
    expect(summarizeTrades(trades)).toEqual({
      count: 4,
      wins: 2,
      net: toCents("35.30"),
      largestWin: toCents("30"),
      largestLoss: toCents("-7.10"),
    });
    expect(summarizeTrades([])).toEqual({
      count: 0,
      wins: 0,
      net: BigInt(0),
      largestWin: BigInt(0),
      largestLoss: BigInt(0),
    });
  });
});
