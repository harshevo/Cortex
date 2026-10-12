import "fake-indexeddb/auto";
import { expect, it, vi } from "vitest";
import { sampleHistory } from "@/features/imports/server/exchange-sample";
import { JournalDatabase } from "@/lib/db/journal-database";
import { emptyJournal } from "@/features/journal/schemas/journal.schema";
import { summarizeTrades } from "@/features/trades/schemas/trade-file.schema";
import { POST } from "@/app/api/exchanges/route";

it.each([
  ["binance", "spot"],
  ["binance", "perpetual"],
  ["bybit", "perpetual"],
  ["coinbase", "spot"],
] as const)(
  "imports %s %s samples through the real adapter and persists them",
  async (exchange, market) => {
    const network = vi
      .spyOn(globalThis, "fetch")
      .mockRejectedValue(new Error("Samples must not call exchanges"));
    const db = new JournalDatabase(`sample-test-${crypto.randomUUID()}`);
    let reopened: JournalDatabase | undefined;
    try {
      const preview = await sampleHistory(exchange, market);
      expect(network).not.toHaveBeenCalled();
      expect(preview.sample).toBe(true);
      expect(preview.fills).toBe(5);
      expect(
        preview.data.trades.map((trade) => [
          trade.volume,
          trade.commission,
          trade.netPnl,
        ]),
      ).toEqual([
        [1, "-2.00", "18.00"],
        [1, "-2.00", "-12.00"],
      ]);
      expect(summarizeTrades(preview.data.trades)).toMatchObject({
        count: 2,
        wins: 1,
        net: BigInt(600),
      });
      expect(preview.warnings).toContain("1 unmatched closes skipped.");
      expect(
        preview.warnings.filter((warning) =>
          warning.includes("open positions"),
        ),
      ).toEqual(["1 open positions excluded."]);
      expect(
        new Set(preview.data.trades.map((trade) => trade.closedAt.slice(0, 10)))
          .size,
      ).toBe(2);
      expect((await sampleHistory(exchange, market)).data).toEqual(
        preview.data,
      );
      const note = {
        ...emptyJournal(preview.data.trades[0].closedAt.slice(0, 10)),
        plan: "Keep this journal note",
      };
      await db.saveJournal(note);
      await db.saveTrades(preview.data);
      await db.saveTrades(preview.data);
      expect(await db.imports.count()).toBe(1);
      db.close();
      reopened = new JournalDatabase(db.name);
      expect(await reopened.loadTrades()).toEqual(preview.data);
      expect(await reopened.journals.get(note.date)).toEqual(note);
    } finally {
      network.mockRestore();
      reopened?.close();
      await db.delete();
    }
  },
);

it("serves sample previews without credentials and rejects mixed sample/secret requests", async () => {
  const request = (body: object) =>
    new Request("http://localhost:3000/api/exchanges", {
      method: "POST",
      headers: {
        origin: "http://localhost:3000",
        "content-type": "application/json",
      },
      body: JSON.stringify(body),
    });
  const response = await POST(
    request({ action: "sample", exchange: "binance", market: "spot" }),
  );
  expect(response.status).toBe(200);
  expect(await response.json()).toMatchObject({
    sample: true,
    fills: 5,
    data: { account: { name: "Binance · Sample" } },
  });
  expect(
    (
      await POST(
        request({
          action: "sample",
          exchange: "binance",
          apiSecret: "should-not-be-sent",
        }),
      )
    ).status,
  ).toBe(400);
});
