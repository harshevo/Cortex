import { generateKeyPairSync, verify } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  matchFills,
  withBinanceClosingQuantities,
  type Fill,
} from "@/features/imports/server/fill-matching";
import {
  coinbaseToken,
  exchangeOperation,
} from "@/features/imports/server/exchange-service";
import { POST } from "@/app/api/exchanges/route";

const fill = (
  id: string,
  side: Fill["side"],
  price: string,
  quantity = "1",
  extra: Partial<Fill> = {},
): Fill => ({
  id,
  side,
  symbol: "BTCUSDT",
  price,
  quantity,
  fee: "1",
  time: 1_700_000_000_000 + Number(id),
  ...extra,
});
const testStart = Date.now() - 2 * 86_400_000;
const input = {
  exchange: "bybit" as const,
  action: "import" as const,
  apiKey: "test-key",
  apiSecret: "test-secret",
  symbols: ["BTCUSDT"],
  start: new Date(testStart).toISOString(),
  end: new Date(testStart + 86_400_000).toISOString(),
  startingBalance: "1000.00",
};
afterEach(() => vi.unstubAllGlobals());

describe("completed exchange trades", () => {
  it("rejects zero-price and zero-quantity fills", () => {
    expect(() => matchFills([fill("1", "BUY", "0")], "test", true)).toThrow(
      "invalid fill",
    );
    expect(() =>
      matchFills([fill("1", "BUY", "100", "0")], "test", true),
    ).toThrow("invalid fill");
  });
  it("matches spot partial exits with proportional entry fees and stable IDs", () => {
    const fills = [
      fill("1", "BUY", "100", "2", { fee: "2" }),
      fill("2", "SELL", "120"),
      fill("3", "SELL", "90"),
    ];
    const result = matchFills([...fills, fills[0]], "coinbase:account", true);
    expect(result.trades.map((trade) => trade.netPnl)).toEqual([
      "18.00",
      "-12.00",
    ]);
    expect(result.trades.map((trade) => trade.commission)).toEqual([
      "-2.00",
      "-2.00",
    ]);
    expect(result.fills).toBe(3);
    expect(result.openLots).toBe(0);
    expect(result.trades[0].openedAt).toBe(
      new Date(fills[0].time).toISOString(),
    );
    expect(result.trades[0].stopLoss).toBeNull();
    expect(matchFills(fills, "coinbase:account", true).trades).toEqual(
      result.trades,
    );
  });
  it("supports simultaneous long and short Bybit positions", () => {
    const result = matchFills(
      [
        fill("1", "BUY", "100", "1", { closed: "0" }),
        fill("2", "SELL", "110", "1", { closed: "0" }),
        fill("3", "SELL", "120", "1", { closed: "1" }),
        fill("4", "BUY", "90", "1", { closed: "1" }),
      ],
      "bybit:account",
      false,
    );
    expect(result.trades.map((trade) => [trade.side, trade.netPnl])).toEqual([
      ["long", "18.00"],
      ["short", "18.00"],
    ]);
  });
  it("preserves Binance reported P&L and hedge lanes", () => {
    const result = matchFills(
      [
        fill("1", "SELL", "100", "1", { lane: "SHORT", pnl: "0" }),
        fill("2", "BUY", "90", "1", { lane: "SHORT", pnl: "9.75" }),
      ],
      "binance:account",
      false,
    );
    expect(result.trades[0].netPnl).toBe("7.75");
    expect(result.trades[0].side).toBe("short");
  });
  it("does not invent entries for unmatched closes or import open lots", () => {
    const result = matchFills(
      [fill("1", "SELL", "100"), fill("2", "BUY", "90")],
      "coinbase:account",
      true,
    );
    expect(result.trades).toHaveLength(0);
    expect(result.unmatched).toBe(1);
    expect(result.openLots).toBe(1);
  });
  it("orders Bybit fills at the same millisecond by sequence rather than random IDs", () => {
    const result = matchFills(
      [
        fill("1", "SELL", "110", "1", { id: "a", closed: "1", sequence: "2" }),
        fill("1", "BUY", "100", "1", { id: "z", closed: "0", sequence: "1" }),
      ],
      "bybit:account",
      false,
    );
    expect(result.trades[0].netPnl).toBe("8.00");
  });
  it("handles one-way reversals and exact fractional fees", () => {
    const result = matchFills(
      [
        fill("1", "BUY", "100", "1", { lane: "BOTH", pnl: "0", closed: "0" }),
        fill("2", "SELL", "110", "2", {
          lane: "BOTH",
          fee: "0.20",
          pnl: "10",
          closed: "1",
        }),
        fill("3", "BUY", "100", "1", {
          lane: "BOTH",
          fee: "0.10",
          pnl: "10",
          closed: "1",
        }),
      ],
      "binance:account",
      false,
    );
    expect(result.trades.map((trade) => trade.netPnl)).toEqual([
      "8.90",
      "9.80",
    ]);
    expect(result.trades.map((trade) => trade.side)).toEqual(["long", "short"]);
  });
});

describe("exchange REST adapters", () => {
  it("signs Coinbase JWTs for the method and path, with a valid ES256 signature", () => {
    const { privateKey, publicKey } = generateKeyPairSync("ec", {
      namedCurve: "prime256v1",
    });
    const token = coinbaseToken(
      "organizations/org/apiKeys/key",
      privateKey.export({ type: "sec1", format: "pem" }).toString(),
      "/api/v3/brokerage/accounts",
    );
    const [header, payload, signature] = token.split(".");
    const claims = JSON.parse(Buffer.from(payload, "base64url").toString());
    expect(claims.uri).toBe("GET api.coinbase.com/api/v3/brokerage/accounts");
    expect(claims.exp - claims.nbf).toBe(120);
    expect(
      verify(
        "sha256",
        Buffer.from(`${header}.${payload}`),
        { key: publicKey, dsaEncoding: "ieee-p1363" },
        Buffer.from(signature, "base64url"),
      ),
    ).toBe(true);
    expect(() => coinbaseToken("key", "bad-secret", "/path")).toThrow("PEM");
  });
  it("follows Bybit cursors and validates the resulting journal schema", async () => {
    const time = Date.parse(input.start!) + 1000;
    const row = (
      id: string,
      side: string,
      price: string,
      closedSize: string,
    ) => ({
      symbol: "BTCUSDT",
      execId: id,
      side,
      execPrice: price,
      execQty: "1",
      execFee: "0.1",
      execTime: String(time + Number(id)),
      closedSize,
    });
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(
        Response.json({
          retCode: 0,
          result: {
            list: [row("2", "Sell", "110", "1")],
            nextPageCursor: "next",
          },
        }),
      )
      .mockResolvedValueOnce(
        Response.json({
          retCode: 0,
          result: { list: [row("1", "Buy", "100", "0")], nextPageCursor: "" },
        }),
      );
    vi.stubGlobal("fetch", fetcher);
    const result = await exchangeOperation(input);
    expect("data" in result && result.data?.account.currency).toBe("USDT");
    expect("data" in result && result.data?.trades[0].netPnl).toBe("9.80");
    expect(String(fetcher.mock.calls[1][0])).toContain("cursor=next");
    expect(fetcher.mock.calls[0][1].headers["X-BAPI-SIGN"]).toMatch(
      /^[a-f0-9]{64}$/,
    );
  });
  it("fails instead of silently truncating repeating cursors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation(() =>
        Promise.resolve(
          Response.json({
            retCode: 0,
            result: { list: [], nextPageCursor: "same" },
          }),
        ),
      ),
    );
    await expect(exchangeOperation(input)).rejects.toThrow(
      "pagination did not advance",
    );
  });
  it("splits saturated Binance time windows without mixing fromId parameters", async () => {
    const time = Date.parse(input.start!) + 1000;
    const row = (id: number, side: string, price: string, pnl: string) => ({
      id,
      symbol: "BTCUSDT",
      side,
      price,
      qty: "1",
      commission: "0.1",
      commissionAsset: "USDT",
      realizedPnl: pnl,
      positionSide: "BOTH",
      time: time + id,
    });
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(
        Response.json([
          {
            symbol: "BTCUSDT",
            positionSide: "BOTH",
            positionAmt: "0",
            updateTime: time + 2,
          },
        ]),
      )
      .mockResolvedValueOnce(
        Response.json(Array(1000).fill(row(1, "BUY", "100", "0"))),
      )
      .mockResolvedValueOnce(
        Response.json([row(1, "BUY", "100", "0"), row(2, "SELL", "110", "10")]),
      )
      .mockResolvedValueOnce(Response.json([]))
      .mockResolvedValueOnce(
        Response.json([
          {
            symbol: "BTCUSDT",
            positionSide: "BOTH",
            positionAmt: "0",
            updateTime: time + 2,
          },
        ]),
      );
    vi.stubGlobal("fetch", fetcher);
    const result = await exchangeOperation({ ...input, exchange: "binance" });
    expect(fetcher).toHaveBeenCalledTimes(5);
    expect("data" in result && result.data?.trades).toHaveLength(1);
    expect(
      fetcher.mock.calls.every(([url]) => !String(url).includes("fromId")),
    ).toBe(true);
  });
  it("imports Coinbase quote-size fills with FIFO and USD currency", async () => {
    const { privateKey } = generateKeyPairSync("ec", {
      namedCurve: "prime256v1",
    });
    const time = Date.parse(input.start!) + 1000;
    const row = (
      id: string,
      side: string,
      price: string,
      size: string,
      quote: boolean,
    ) => ({
      entry_id: id,
      product_id: "BTC-USD",
      side,
      price,
      size,
      size_in_quote: quote,
      commission: "0.1",
      trade_time: new Date(time + Number(id)).toISOString(),
    });
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(
        Response.json({
          fills: [
            row("1", "BUY", "100", "100", true),
            row("2", "SELL", "110", "1", false),
          ],
          cursor: "next",
        }),
      )
      .mockResolvedValueOnce(Response.json({ fills: [] }));
    vi.stubGlobal("fetch", fetcher);
    const result = await exchangeOperation({
      ...input,
      exchange: "coinbase",
      symbols: ["BTC-USD"],
      apiSecret: privateKey.export({ type: "sec1", format: "pem" }).toString(),
    });
    expect("data" in result && result.data?.account.currency).toBe("USD");
    expect("data" in result && result.data?.trades[0].netPnl).toBe("9.80");
  });
  it("rejects wrong currencies, excessive ranges and API errors", async () => {
    await expect(
      exchangeOperation({ ...input, symbols: ["BTCEUR"] }),
    ).rejects.toThrow("USDT pairs");
    await expect(
      exchangeOperation({ ...input, start: "2020-01-01T00:00:00Z" }),
    ).rejects.toThrow("90 days");
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response(null, { status: 401 })),
    );
    await expect(
      exchangeOperation({ ...input, action: "connect" }),
    ).rejects.toThrow("rejected");
  });
  it("distinguishes valid Binance spot credentials from missing Futures access", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(Response.json({ code: -2015 }, { status: 401 }))
      .mockResolvedValueOnce(Response.json({ balances: [] }));
    vi.stubGlobal("fetch", fetcher);
    await expect(
      exchangeOperation({ ...input, exchange: "binance", action: "connect" }),
    ).rejects.toThrow("spot access verified");
    expect(String(fetcher.mock.calls[1][0])).toContain(
      "https://api.binance.com/api/v3/account",
    );
  });
  it("connects Binance spot without requiring any Futures or trading access", async () => {
    const fetcher = vi.fn().mockResolvedValue(Response.json({ balances: [] }));
    vi.stubGlobal("fetch", fetcher);
    await expect(
      exchangeOperation({
        ...input,
        exchange: "binance",
        market: "spot",
        action: "connect",
      }),
    ).resolves.toEqual({ connected: true });
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(String(fetcher.mock.calls[0][0])).toContain(
      "https://api.binance.com/api/v3/account",
    );
  });
  it("matches Binance spot fills with base-asset buy fees and USDT sale fees", async () => {
    const time = Date.parse(input.start!) + 1000;
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        Response.json([
          {
            id: 1,
            symbol: "BTCUSDT",
            isBuyer: true,
            price: "100",
            qty: "1",
            commission: "0.01",
            commissionAsset: "BTC",
            time,
          },
          {
            id: 2,
            symbol: "BTCUSDT",
            isBuyer: false,
            price: "110",
            qty: "0.99",
            commission: "0.1",
            commissionAsset: "USDT",
            time: time + 1,
          },
        ]),
      ),
    );
    const result = await exchangeOperation({
      ...input,
      exchange: "binance",
      market: "spot",
    });
    expect("data" in result && result.data?.trades[0].netPnl).toBe("8.80");
    expect("data" in result && result.data?.trades[0].volume).toBe(0.99);
  });
  it("converts BNB fee costs using unsigned historical candles and reports the estimate", async () => {
    const time = Date.parse(input.start!) + 1000;
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(
        Response.json([
          {
            id: 1,
            symbol: "BTCUSDT",
            isBuyer: true,
            price: "100",
            qty: "1",
            commission: "0.01",
            commissionAsset: "BNB",
            time,
          },
          {
            id: 2,
            symbol: "BTCUSDT",
            isBuyer: false,
            price: "110",
            qty: "1",
            commission: "0.01",
            commissionAsset: "BNB",
            time: time + 1,
          },
        ]),
      )
      .mockResolvedValue(Response.json([[time, "200", "200", "200", "200"]]));
    vi.stubGlobal("fetch", fetcher);
    const result = await exchangeOperation({
      ...input,
      exchange: "binance",
      market: "spot",
    });
    expect("data" in result && result.data?.trades[0].netPnl).toBe("6.00");
    expect(String(fetcher.mock.calls[1][0])).not.toContain("signature");
    expect(
      "warnings" in result &&
        result.warnings?.some((warning) => warning.includes("estimated")),
    ).toBe(true);
  });
  it("requires same-origin JSON requests and disables caching", async () => {
    const foreign = await POST(
      new Request("http://localhost/api/exchanges", {
        method: "POST",
        headers: {
          origin: "https://foreign.example",
          "Content-Type": "application/json",
        },
        body: JSON.stringify(input),
      }),
    );
    expect(foreign.status).toBe(403);
    expect(foreign.headers.get("cache-control")).toBe("no-store");
    const invalid = await POST(
      new Request("http://localhost/api/exchanges", {
        method: "POST",
        headers: {
          origin: "http://localhost",
          "Content-Type": "application/json",
        },
        body: "{}",
      }),
    );
    expect(invalid.status).toBe(400);
  });
});

describe("import accounting regressions", () => {
  it("requires position evidence for ambiguous Binance one-way fills", () => {
    expect(() =>
      matchFills(
        [fill("1", "SELL", "100", "1", { lane: "BOTH", pnl: "0" })],
        "binance",
        false,
      ),
    ).toThrow("verified closing quantities");
  });
  it("skips a breakeven boundary close and retains the actual new opening", () => {
    const fills = [
      fill("1", "SELL", "100", "1", { lane: "BOTH", pnl: "0" }),
      fill("2", "BUY", "110", "1", { lane: "BOTH", pnl: "0" }),
    ];
    const verified = withBinanceClosingQuantities(
      fills,
      new Map([["BTCUSDT", "1"]]),
    );
    expect(verified.map((item) => item.closed)).toEqual(["1", "0"]);
    expect(matchFills(verified, "binance", false)).toMatchObject({
      trades: [],
      unmatched: 1,
      openLots: 1,
    });
  });
  it("reconstructs one-way reversals without counting duplicate fills twice", () => {
    const fills = [
      fill("1", "BUY", "100", "1", { lane: "BOTH", pnl: "0" }),
      fill("2", "SELL", "110", "2", { lane: "BOTH", pnl: "10" }),
      fill("3", "BUY", "90", "1", { lane: "BOTH", pnl: "20" }),
    ];
    const verified = withBinanceClosingQuantities(
      [...fills, fills[0]],
      new Map([["BTCUSDT", "0"]]),
    );
    expect(verified.slice(0, 3).map((item) => item.closed)).toEqual([
      "0",
      "1",
      "1",
    ]);
    expect(
      matchFills(verified, "binance", false).trades.map((trade) => trade.side),
    ).toEqual(["long", "short"]);
  });
  it("preserves the total one-cent fee when an exit matches multiple lots", () => {
    const result = matchFills(
      [
        fill("1", "BUY", "1", "1", { fee: "0" }),
        fill("2", "BUY", "1", "1", { fee: "0" }),
        fill("3", "SELL", "1", "2", { fee: "0.01" }),
      ],
      "spot",
      true,
    );
    expect(result.trades.map((trade) => trade.commission)).toEqual([
      "-0.01",
      "0.00",
    ]);
    expect(result.trades.map((trade) => trade.netPnl)).toEqual([
      "-0.01",
      "0.00",
    ]);
  });
  it("preserves fractional entry fees across multiple partial exits", () => {
    const result = matchFills(
      [
        fill("1", "BUY", "1", "3", { fee: "0.01" }),
        ...[2, 3, 4].map((id) =>
          fill(String(id), "SELL", "1", "1", { fee: "0" }),
        ),
      ],
      "spot",
      true,
    );
    expect(result.trades.map((trade) => trade.netPnl)).toEqual([
      "0.00",
      "-0.01",
      "0.00",
    ]);
  });
  it("rejects non-finite closing quantities", () => {
    expect(() =>
      matchFills(
        [fill("1", "BUY", "100", "1", { closed: "NaN" })],
        "bybit",
        false,
      ),
    ).toThrow("closing quantity");
  });
});

it("accepts the public Host behind a proxy while rejecting foreign or missing origins", async () => {
  const request = (origin: string | null, host = "tradelyzer.harshevo.dev") => {
    const headers: Record<string, string> = {
      host,
      "Content-Type": "application/json",
      "X-Forwarded-Host": "foreign.example",
      "X-Forwarded-Proto": "https",
    };

    if (origin) {
      headers.origin = origin;
    }

    return new Request("http://127.0.0.1:4100/api/exchanges", {
      method: "POST",
      headers,
      body: JSON.stringify({
        action: "sample",
        exchange: "binance",
        market: "spot",
      }),
    });
  };
  expect((await POST(request("https://tradelyzer.harshevo.dev"))).status).toBe(
    200,
  );
  for (const origin of [
    null,
    "null",
    "https://foreign.example",
    "https://tradelyzer.harshevo.dev:444",
    "https://tradelyzer.harshevo.dev/path",
  ])
    expect((await POST(request(origin))).status).toBe(403);
});

it("aborts a Binance import if position state changes during history collection", async () => {
  const time = Date.parse(input.start!) + 1000;
  const positions = (updateTime: number) =>
    Response.json([
      { symbol: "BTCUSDT", positionSide: "BOTH", positionAmt: "0", updateTime },
    ]);
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(positions(time))
      .mockResolvedValueOnce(Response.json([]))
      .mockResolvedValueOnce(positions(time + 1)),
  );
  await expect(
    exchangeOperation({ ...input, exchange: "binance" }),
  ).rejects.toThrow("positions changed");
});

it("uses fills after the selected range only to reconcile Binance position context", async () => {
  const time = Date.parse(input.start!) + 1000;
  const position = [
    {
      symbol: "BTCUSDT",
      positionSide: "BOTH",
      positionAmt: "0",
      updateTime: time + 86_400_000,
    },
  ];
  const row = (
    id: number,
    side: string,
    price: string,
    realizedPnl: string,
    at: number,
  ) => ({
    id,
    symbol: "BTCUSDT",
    side,
    price,
    qty: "1",
    commission: "0",
    commissionAsset: "USDT",
    positionSide: "BOTH",
    time: at,
    realizedPnl,
  });
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(Response.json(position))
      .mockResolvedValueOnce(
        Response.json([
          row(1, "SELL", "100", "0", time),
          row(2, "BUY", "110", "0", time + 1),
          row(3, "SELL", "120", "10", time + 86_400_000),
        ]),
      )
      .mockResolvedValueOnce(Response.json(position)),
  );
  const result = await exchangeOperation({ ...input, exchange: "binance" });
  expect(result).toMatchObject({ data: null, fills: 2 });
  expect("warnings" in result && result.warnings).toContain(
    "1 unmatched closes skipped.",
  );
  expect("warnings" in result && result.warnings).toContain(
    "1 open positions excluded.",
  );
});

it("retains a verified reversal opening when its prior position is outside the range", () => {
  const result = matchFills(
    [
      fill("1", "SELL", "100", "2", {
        lane: "BOTH",
        closed: "1",
        pnl: "0",
        fee: "0.2",
      }),
      fill("2", "BUY", "90", "1", {
        lane: "BOTH",
        closed: "1",
        pnl: "10",
        fee: "0.1",
      }),
    ],
    "binance",
    false,
  );
  expect(result).toMatchObject({ unmatched: 1, openLots: 0 });
  expect(result.trades[0]).toMatchObject({ side: "short", netPnl: "9.80" });
});

it("does not assume a flat position when Binance omits position evidence", () => {
  expect(() =>
    withBinanceClosingQuantities(
      [fill("1", "BUY", "100", "1", { lane: "BOTH", pnl: "0" })],
      new Map(),
    ),
  ).toThrow("position state is missing");
});
