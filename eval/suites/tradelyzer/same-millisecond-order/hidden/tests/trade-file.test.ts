import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  parseJson,
  summarizeTrades,
} from "@/features/trades/schemas/trade-file.schema";
import { formatMoney, toCents } from "@/lib/money";

const fixture = readFileSync(
  new URL("./fixtures/trades.json", import.meta.url),
  "utf8",
);
const valid = JSON.parse(fixture);

describe("completed-trade imports", () => {
  it("loads the supplied file and retains authoritative net P&L", () => {
    const file = parseJson(fixture);
    expect(file.trades.length).toBe(valid.trades.length);
    expect(file.account.name).toBe("Demo Prop 100K");
    expect(file.trades[0].netPnl).toBe("-32.86");
    expect(summarizeTrades(file.trades).count).toBe(valid.trades.length);
  });
  it("accepts a JSON byte-order mark and an explicit missing stop loss", () => {
    const file = parseJson(
      "\uFEFF" +
        JSON.stringify({
          ...valid,
          trades: [{ ...valid.trades[0], stopLoss: null }],
        }),
    );
    expect(file.trades[0].stopLoss).toBeNull();
  });
  it("rejects duplicate IDs atomically", () => {
    expect(() =>
      parseJson(
        JSON.stringify({
          ...valid,
          trades: [valid.trades[0], valid.trades[0]],
        }),
      ),
    ).toThrow("Duplicate trade ID");
  });
  it.each([
    ["netPnl", "1.001"],
    ["closedAt", "2026-09-07T12:00:00"],
    ["closedAt", "2026-02-30T12:00:00Z"],
    ["closedAt", "2020-01-01T00:00:00Z"],
    ["volume", 0],
    ["side", "buy"],
    ["stopLoss", undefined],
  ])("rejects invalid %s: %s", (field, value) => {
    expect(() =>
      parseJson(
        JSON.stringify({
          ...valid,
          trades: [{ ...valid.trades[0], [field]: value }],
        }),
      ),
    ).toThrow();
  });
  it("rejects broken JSON and empty trade arrays", () => {
    expect(() => parseJson("{")).toThrow("not valid JSON");
    expect(() => parseJson(JSON.stringify({ ...valid, trades: [] }))).toThrow(
      "no trades",
    );
  });
  it("preserves exact cents including large values and negative fractions", () => {
    expect(toCents("-0.01")).toBe(BigInt(-1));
    expect(toCents("0.10") + toCents("0.20")).toBe(BigInt(30));
    expect(formatMoney(toCents("9007199254740993.12"))).toBe(
      "$9,007,199,254,740,993.12",
    );
    expect(formatMoney(toCents("-0.01"))).toBe("−$0.01");
  });
});
