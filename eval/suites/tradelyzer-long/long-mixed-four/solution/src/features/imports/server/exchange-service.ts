import {
  createHash,
  createHmac,
  createPrivateKey,
  randomBytes,
  sign,
} from "node:crypto";
import Decimal from "decimal.js";
import { z } from "zod";
import {
  exchanges,
  type Exchange,
  type ExchangeRequest,
} from "@/features/imports/schemas/exchange.schema";
import {
  validateTradeFile,
  type TradeFile,
} from "@/features/trades/schemas/trade-file.schema";
import {
  matchFills,
  withBinanceClosingQuantities,
  type Fill,
} from "./fill-matching";

const DAY_IN_MILLISECONDS = 86_400_000;
const MINUTE_IN_MILLISECONDS = 60_000;
const IMPORT_TIMEOUT_MILLISECONDS = 50_000;
const REQUEST_TIMEOUT_MILLISECONDS = 12_000;
const MAX_EXCHANGE_CALLS = 150;
const MAX_FILLS = 50_000;
const BINANCE_PAGE_SIZE = 1000;
const BINANCE_SPOT_HOST = "https://api.binance.com";

const hosts = {
  binance: "https://fapi.binance.com",
  bybit: "https://api.bybit.com",
  coinbase: "https://api.coinbase.com",
};

const exchangeRowSchema = z.record(
  z.string(),
  z.union([
    z.string(),
    z.number(),
    z.boolean(),
    z.object({}).passthrough(),
    z.array(z.unknown()),
    z.null(),
  ]),
);

const positionSchema = z.array(
  z.object({
    symbol: z.string(),
    positionSide: z.enum(["LONG", "SHORT", "BOTH"]),
    positionAmt: z.string().refine((amount) => new Decimal(amount).isFinite()),
    updateTime: z.number().int().nonnegative(),
  }),
);

const candleSchema = z.array(z.array(z.union([z.string(), z.number()])));

type ExchangeRow = z.infer<typeof exchangeRowSchema>;
type Position = z.infer<typeof positionSchema>[number];
type ExchangeGet = (
  path: string,
  params?: Record<string, string>,
) => Promise<unknown>;

function parseRows(input: unknown): ExchangeRow[] {
  return z.array(exchangeRowSchema).parse(input);
}

function requiredValue(row: ExchangeRow, key: string): string {
  const value = row[key];

  if (typeof value !== "string" && typeof value !== "number") {
    throw new Error("Exchange response is missing a required field.");
  }

  return String(value);
}

function parseSide(value: string): Fill["side"] {
  if (value !== "BUY" && value !== "SELL") {
    throw new Error("Unknown exchange trade side.");
  }

  return value;
}

function isSupportedSymbol(exchange: Exchange, symbol: string): boolean {
  if (exchange === "coinbase") {
    return /^[A-Z0-9]+-USD$/.test(symbol);
  }

  return /^[A-Z0-9]+USDT$/.test(symbol);
}

export function coinbaseToken(apiKey: string, secret: string, path: string) {
  let key;

  try {
    key = createPrivateKey(secret.replace(/\\n/g, "\n"));
  } catch {
    throw new Error("Use the Coinbase ECDSA private key in PEM format.");
  }

  if (
    key.asymmetricKeyType !== "ec" ||
    key.asymmetricKeyDetails?.namedCurve !== "prime256v1"
  ) {
    throw new Error("Coinbase requires an ECDSA P-256 API key.");
  }

  const now = Math.floor(Date.now() / 1000);
  const encode = (input: object) =>
    Buffer.from(JSON.stringify(input)).toString("base64url");
  const header = encode({
    alg: "ES256",
    typ: "JWT",
    kid: apiKey,
    nonce: randomBytes(16).toString("hex"),
  });
  const claims = encode({
    sub: apiKey,
    iss: "cdp",
    nbf: now,
    exp: now + 120,
    uri: `GET api.coinbase.com${path}`,
  });
  const payload = `${header}.${claims}`;
  const signature = sign("sha256", Buffer.from(payload), {
    key,
    dsaEncoding: "ieee-p1363",
  }).toString("base64url");

  return `${payload}.${signature}`;
}

function createExchangeGet(
  input: ExchangeRequest,
  transport: typeof fetch,
): ExchangeGet {
  const { exchange, apiKey, apiSecret } = input;
  const deadline = Date.now() + IMPORT_TIMEOUT_MILLISECONDS;
  let calls = 0;

  function signHmac(text: string): string {
    return createHmac("sha256", apiSecret).update(text).digest("hex");
  }

  return async function get(path, params = {}) {
    calls += 1;

    if (calls > MAX_EXCHANGE_CALLS || Date.now() >= deadline) {
      throw new Error(
        "History is too large for one import. Choose fewer symbols or a shorter date range.",
      );
    }

    const timestamp = String(Date.now());
    const query = new URLSearchParams(params);
    const headers: Record<string, string> = {};
    let host = hosts[exchange];

    if (exchange === "binance" && !path.endsWith("/klines")) {
      query.set("timestamp", timestamp);
      query.set("recvWindow", "5000");
      query.set("signature", signHmac(query.toString()));
      headers["X-MBX-APIKEY"] = apiKey;
    } else if (exchange === "bybit") {
      headers["X-BAPI-API-KEY"] = apiKey;
      headers["X-BAPI-TIMESTAMP"] = timestamp;
      headers["X-BAPI-RECV-WINDOW"] = "5000";
      headers["X-BAPI-SIGN"] = signHmac(
        timestamp + apiKey + "5000" + query.toString(),
      );
    } else if (exchange === "coinbase") {
      headers.Authorization = `Bearer ${coinbaseToken(apiKey, apiSecret, path)}`;
    }

    if (exchange === "binance" && path.startsWith("/api/")) {
      host = BINANCE_SPOT_HOST;
    }

    let response: Response;

    try {
      const remaining = Math.max(1, deadline - Date.now());

      response = await transport(`${host}${path}?${query}`, {
        headers,
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(
          Math.min(REQUEST_TIMEOUT_MILLISECONDS, remaining),
        ),
      });
    } catch {
      throw new Error(
        "Couldn’t reach the exchange. Check connectivity and regional API availability.",
      );
    }

    if (!response.ok) {
      const error = await response.json().catch(() => null);
      let code = "";

      if (response.status === 429) {
        throw new Error("Exchange rate limit reached. Try again shortly.");
      }

      if (Number.isInteger(error?.code)) {
        code = `, code ${error.code}`;
      }

      throw new Error(
        `${exchanges[exchange].name} rejected the request (HTTP ${response.status}${code}). Check your key, read permissions, IP allowlist, and server clock.`,
      );
    }

    const data = await response.json();

    if (exchange === "bybit" && data.retCode !== 0) {
      throw new Error(
        "Bybit rejected the request. Check your credentials, permissions, IP allowlist, and account region.",
      );
    }

    return data;
  };
}

async function verifyBinanceConnection(
  isSpot: boolean,
  get: ExchangeGet,
): Promise<void> {
  if (isSpot) {
    const account = (await get("/api/v3/account", {
      omitZeroBalances: "true",
    })) as { balances?: unknown };

    parseRows(account.balances);
    return;
  }

  try {
    parseRows(await get("/fapi/v3/balance"));
  } catch (error) {
    if (!(error instanceof Error) || !error.message.includes("code -2015")) {
      throw error;
    }

    try {
      await get("/api/v3/account", { omitZeroBalances: "true" });
    } catch {
      throw error;
    }

    throw new Error(
      "Binance spot access verified, but Futures access was rejected (code -2015). Select Spot to import with this read-only key.",
    );
  }
}

async function verifyConnection(input: ExchangeRequest, get: ExchangeGet) {
  if (input.exchange === "binance") {
    await verifyBinanceConnection(input.market === "spot", get);
  } else if (input.exchange === "bybit") {
    await get("/v5/execution/list", { category: "linear", limit: "1" });
  } else {
    const data = (await get("/api/v3/brokerage/accounts", {
      limit: "1",
    })) as { accounts?: unknown };

    parseRows(data.accounts);
  }

  return { connected: true };
}

async function collectCoinbaseFills(
  get: ExchangeGet,
  fills: Fill[],
  symbol: string,
  start: number,
  end: number,
): Promise<void> {
  const seenCursors = new Set<string>();
  let cursor = "";

  do {
    const params: Record<string, string> = {
      product_ids: symbol,
      product_types: "SPOT",
      start_sequence_timestamp: new Date(start).toISOString(),
      end_sequence_timestamp: new Date(end).toISOString(),
      limit: "100",
    };

    if (cursor) {
      params.cursor = cursor;
    }

    const data = (await get(
      "/api/v3/brokerage/orders/historical/fills",
      params,
    )) as { fills: unknown; cursor?: string; proof_token_required?: boolean };

    if (data.proof_token_required) {
      throw new Error(
        "Coinbase requires additional account verification to read this history. Complete verification on Coinbase first.",
      );
    }

    const page = parseRows(data.fills);

    for (const row of page) {
      if (requiredValue(row, "product_id") !== symbol) {
        throw new Error("Coinbase returned an unexpected market.");
      }

      const price = requiredValue(row, "price");
      const id = requiredValue(row, "entry_id");
      const side = parseSide(requiredValue(row, "side"));
      const time = Date.parse(requiredValue(row, "trade_time"));
      let quantity = requiredValue(row, "size");

      if (row.size_in_quote === true) {
        quantity = new Decimal(quantity).div(price).toString();
      }

      fills.push({
        id,
        symbol,
        side,
        time,
        price,
        quantity,
        fee: requiredValue(row, "commission"),
      });
    }

    cursor = "";

    if (page.length > 0) {
      cursor = data.cursor ?? "";
    }

    if (cursor && seenCursors.has(cursor)) {
      throw new Error(
        "Coinbase pagination did not advance. Retry with a smaller range.",
      );
    }

    seenCursors.add(cursor);
  } while (cursor);
}

async function readBinanceSpotFill(
  get: ExchangeGet,
  feePrices: Map<number, string>,
  symbol: string,
  row: ExchangeRow,
): Promise<Fill> {
  let fee = new Decimal(requiredValue(row, "commission"));
  let quantity = new Decimal(requiredValue(row, "qty"));
  const feeAsset = requiredValue(row, "commissionAsset");
  const price = requiredValue(row, "price");
  const baseAsset = symbol.slice(0, -4);

  if (!fee.isZero() && feeAsset !== "USDT") {
    if (feeAsset === baseAsset && row.isBuyer === true) {
      quantity = quantity.minus(fee);
      fee = fee.mul(price);
    } else if (feeAsset === "BNB") {
      const minute =
        Math.floor(Number(row.time) / MINUTE_IN_MILLISECONDS) *
        MINUTE_IN_MILLISECONDS;

      if (!feePrices.has(minute)) {
        const candles = candleSchema.parse(
          await get("/api/v3/klines", {
            symbol: "BNBUSDT",
            interval: "1m",
            startTime: String(minute),
            endTime: String(minute + MINUTE_IN_MILLISECONDS - 1),
            limit: "1",
          }),
        );

        if (!candles[0]?.[4]) {
          throw new Error(
            "BNB fee conversion price is unavailable. Use a converted file export.",
          );
        }

        feePrices.set(minute, String(candles[0][4]));
      }

      fee = fee.mul(feePrices.get(minute)!);
    } else {
      throw new Error(
        "Unsupported Binance spot fee currency. Use a converted file export.",
      );
    }
  }

  if (typeof row.isBuyer !== "boolean") {
    throw new Error("Binance returned an invalid trade side.");
  }

  let side: Fill["side"] = "SELL";

  if (row.isBuyer) {
    side = "BUY";
  }

  return {
    id: requiredValue(row, "id"),
    symbol,
    side,
    time: Number(row.time),
    price,
    quantity: quantity.toString(),
    fee: fee.toString(),
  };
}

function readBinanceFuturesFill(symbol: string, row: ExchangeRow): Fill {
  if (requiredValue(row, "commissionAsset") !== "USDT") {
    throw new Error(
      "Binance Futures history contains fees outside USDT. Use a converted file export.",
    );
  }

  const lane = z.enum(["LONG", "SHORT", "BOTH"]).parse(row.positionSide);

  return {
    id: requiredValue(row, "id"),
    symbol,
    side: parseSide(requiredValue(row, "side")),
    time: Number(row.time),
    price: requiredValue(row, "price"),
    quantity: requiredValue(row, "qty"),
    fee: requiredValue(row, "commission"),
    pnl: requiredValue(row, "realizedPnl"),
    lane,
  };
}

async function collectBinanceFills(
  get: ExchangeGet,
  fills: Fill[],
  feePrices: Map<number, string>,
  symbol: string,
  isSpot: boolean,
  from: number,
  to: number,
): Promise<void> {
  let path = "/fapi/v1/userTrades";

  if (isSpot) {
    path = "/api/v3/myTrades";
  }

  const batch = parseRows(
    await get(path, {
      symbol,
      startTime: String(from),
      endTime: String(to),
      limit: String(BINANCE_PAGE_SIZE),
    }),
  );

  if (batch.length === BINANCE_PAGE_SIZE) {
    if (from === to) {
      throw new Error(
        "Too many Binance fills in one millisecond. Import this history using a file export.",
      );
    }

    const midpoint = Math.floor((from + to) / 2);

    await collectBinanceFills(
      get,
      fills,
      feePrices,
      symbol,
      isSpot,
      from,
      midpoint,
    );
    await collectBinanceFills(
      get,
      fills,
      feePrices,
      symbol,
      isSpot,
      midpoint + 1,
      to,
    );
    return;
  }

  for (const row of batch) {
    if (requiredValue(row, "symbol") !== symbol) {
      throw new Error("Binance returned an unexpected market.");
    }

    if (isSpot) {
      fills.push(await readBinanceSpotFill(get, feePrices, symbol, row));
    } else {
      fills.push(readBinanceFuturesFill(symbol, row));
    }
  }
}

async function collectBybitFills(
  get: ExchangeGet,
  fills: Fill[],
  symbol: string,
  from: number,
  to: number,
): Promise<void> {
  const seenCursors = new Set<string>();
  let cursor = "";

  do {
    const params: Record<string, string> = {
      category: "linear",
      symbol,
      execType: "Trade",
      startTime: String(from),
      endTime: String(to),
      limit: "100",
    };

    if (cursor) {
      params.cursor = cursor;
    }

    const data = (await get("/v5/execution/list", params)) as {
      result: { list: unknown; nextPageCursor?: string };
    };

    for (const row of parseRows(data.result.list)) {
      const hasUnsupportedFee =
        row.extraFees || (row.feeCurrency && row.feeCurrency !== "USDT");

      if (requiredValue(row, "symbol") !== symbol || hasUnsupportedFee) {
        throw new Error(
          "Bybit returned unsupported fee details. Use a file export for this market.",
        );
      }

      if (row.closedSize === "" || row.closedSize === undefined) {
        throw new Error(
          "Bybit did not supply closing quantities. Use an account with supported linear execution history.",
        );
      }

      let sequence: string | undefined;

      if (row.seq !== undefined) {
        sequence = requiredValue(row, "seq");
      }

      fills.push({
        id: requiredValue(row, "execId"),
        symbol,
        side: parseSide(requiredValue(row, "side").toUpperCase()),
        time: Number(row.execTime),
        price: requiredValue(row, "execPrice"),
        quantity: requiredValue(row, "execQty"),
        fee: requiredValue(row, "execFee"),
        closed: requiredValue(row, "closedSize"),
        sequence,
      });
    }

    cursor = data.result.nextPageCursor ?? "";

    if (cursor && seenCursors.has(cursor)) {
      throw new Error(
        "Bybit pagination did not advance. Retry with a smaller range.",
      );
    }

    seenCursors.add(cursor);
  } while (cursor);
}

async function importHistory(input: ExchangeRequest, get: ExchangeGet) {
  const { exchange, apiKey } = input;
  const isBinanceSpot = exchange === "binance" && input.market === "spot";
  const isBinanceFutures = exchange === "binance" && !isBinanceSpot;
  const isSpot = exchange === "coinbase" || isBinanceSpot;
  const start = Date.parse(input.start ?? "");
  const end = Date.parse(input.end ?? "");

  if (
    !Number.isFinite(start) ||
    !Number.isFinite(end) ||
    start >= end ||
    end - start > 90 * DAY_IN_MILLISECONDS ||
    end > Date.now() + DAY_IN_MILLISECONDS
  ) {
    throw new Error("Choose a valid date range of up to 90 days.");
  }

  if (isBinanceFutures && start < Date.now() - 90 * DAY_IN_MILLISECONDS) {
    throw new Error(
      "Binance Futures REST history is limited to the last three months.",
    );
  }

  if (exchange === "bybit" && start < Date.now() - 730 * DAY_IN_MILLISECONDS) {
    throw new Error(
      "Bybit execution history is limited to the last two years.",
    );
  }

  const symbols = [...new Set(input.symbols ?? [])];
  const hasUnsupportedSymbol = symbols.some(
    (symbol) => !isSupportedSymbol(exchange, symbol),
  );

  if (symbols.length === 0 || hasUnsupportedSymbol) {
    throw new Error(
      "Enter USD spot pairs for Coinbase or USDT pairs for Binance and Bybit.",
    );
  }

  async function readPositions(): Promise<Position[]> {
    return positionSchema
      .parse(await get("/fapi/v2/positionRisk"))
      .filter((position) => symbols.includes(position.symbol))
      .sort((left, right) =>
        `${left.symbol}:${left.positionSide}`.localeCompare(
          `${right.symbol}:${right.positionSide}`,
        ),
      );
  }

  const fills: Fill[] = [];
  const feePrices = new Map<number, string>();
  let positions: Position[] = [];
  let historyEnd = end;
  let historyWindow = 7 * DAY_IN_MILLISECONDS;

  if (isBinanceFutures) {
    positions = await readPositions();
    historyEnd = Math.max(end, Date.now() + 1);
  }

  if (isBinanceSpot) {
    historyWindow = DAY_IN_MILLISECONDS;
  }

  for (const symbol of symbols) {
    if (exchange === "coinbase") {
      await collectCoinbaseFills(get, fills, symbol, start, end);
    } else {
      for (let from = start; from < historyEnd; from += historyWindow) {
        const to = Math.min(from + historyWindow - 1, historyEnd - 1);

        if (exchange === "binance") {
          await collectBinanceFills(
            get,
            fills,
            feePrices,
            symbol,
            isBinanceSpot,
            from,
            to,
          );
        } else {
          await collectBybitFills(get, fills, symbol, from, to);
        }
      }
    }

    if (fills.length > MAX_FILLS) {
      throw new Error(
        "Choose a smaller range: imports support up to 50,000 fills.",
      );
    }
  }

  const accountId = createHash("sha256")
    .update(apiKey)
    .digest("hex")
    .slice(0, 10);
  let marketPrefix = "";
  let marketLabel = "";
  let currency = "USDT";
  let verifiedFills = fills;

  if (isBinanceSpot) {
    marketPrefix = "spot:";
    marketLabel = " Spot";
  }

  if (exchange === "coinbase") {
    currency = "USD";
  }

  if (isBinanceFutures) {
    const currentPositions = await readPositions();

    if (JSON.stringify(positions) !== JSON.stringify(currentPositions)) {
      throw new Error(
        "Binance positions changed during the import. Retry when trading is paused.",
      );
    }

    const oneWayPositions = positions
      .filter((position) => position.positionSide === "BOTH")
      .map((position): [string, string] => [
        position.symbol,
        position.positionAmt,
      ]);

    verifiedFills = withBinanceClosingQuantities(
      fills,
      new Map(oneWayPositions),
    );
  }

  const result = matchFills(
    verifiedFills.filter((fill) => fill.time >= start && fill.time < end),
    `${exchange}:${marketPrefix}${accountId}`,
    isSpot,
  );
  const warnings = [
    "Only trades opened and closed in this range.",
    "Partial exits count separately. Stop loss unavailable.",
  ];
  let data: TradeFile | null = null;

  if (!isSpot) {
    warnings.push("Funding excluded.");
  }

  if (isBinanceSpot) {
    warnings.push("Spot uses FIFO; fees included.");
  }

  if (feePrices.size > 0) {
    warnings.push("BNB fees estimated in USDT.");
  }

  if (result.unmatched > 0) {
    warnings.push(`${result.unmatched} unmatched closes skipped.`);
  }

  if (result.openLots > 0) {
    warnings.push(`${result.openLots} open positions excluded.`);
  }

  if (result.trades.length > 0) {
    data = validateTradeFile({
      account: {
        name: `${exchanges[exchange].name}${marketLabel} · ${accountId}`,
        currency,
        startingBalance: input.startingBalance,
      },
      trades: result.trades,
    });
  }

  return { data, warnings, fills: result.fills };
}

export async function exchangeOperation(
  input: ExchangeRequest,
  transport: typeof fetch = fetch,
) {
  const get = createExchangeGet(input, transport);

  if (input.action === "connect") {
    return verifyConnection(input, get);
  }

  return importHistory(input, get);
}
