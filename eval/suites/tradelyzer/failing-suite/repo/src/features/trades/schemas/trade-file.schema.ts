import { DateTime } from "luxon";
import { z } from "zod";
import { largerCents, smallerCents, toCents } from "@/lib/money";

export const MAX_FILE_SIZE = 10 * 1024 * 1024;
export const MAX_TRADES = 50_000;
const money = z
  .string()
  .trim()
  .regex(
    /^-?\d+(\.\d{1,2})?$/,
    "Use a decimal amount with at most two decimal places",
  )
  .refine((value) => value.length <= 24, "Amount is too large");
const instant = z
  .string()
  .trim()
  .refine(
    (value) =>
      /(?:Z|[+-]\d{2}:\d{2})$/i.test(value) &&
      DateTime.fromISO(value, { setZone: true }).isValid,
    "Use an ISO timestamp with Z or an explicit offset",
  );
const positive = z.number().finite().positive();
const tradeSchema = z
  .object({
    id: z.string().trim().min(1).max(120),
    symbol: z.string().trim().min(1).max(80),
    side: z.enum(["long", "short"]),
    volume: positive,
    openedAt: instant,
    closedAt: instant,
    entryPrice: positive,
    exitPrice: positive,
    stopLoss: positive.nullable(),
    commission: money,
    swap: money,
    netPnl: money,
  })
  .refine(
    (trade) =>
      DateTime.fromISO(trade.closedAt).toMillis() >=
      DateTime.fromISO(trade.openedAt).toMillis(),
    { path: ["closedAt"], message: "Close time must be on or after open time" },
  );
const fileSchema = z.object({
  account: z.object({
    name: z.string().trim().min(1).max(120),
    currency: z.enum(["USD", "USDT"]),
    startingBalance: money.refine(
      (value) => !value.startsWith("-"),
      "Starting balance cannot be negative",
    ),
  }),
  trades: z
    .array(tradeSchema)
    .min(1, "The file contains no trades")
    .max(MAX_TRADES),
});
export type Trade = z.infer<typeof tradeSchema>;
export type TradeFile = z.infer<typeof fileSchema>;

function issueLocation(path: ReadonlyArray<PropertyKey>): string {
  if (path[0] === "trades" && typeof path[1] === "number") {
    return `Row ${path[1] + 1} · ${path.slice(2).join(".") || "trade"}`;
  }

  return path.join(".");
}

export function validateTradeFile(input: unknown): TradeFile {
  const result = fileSchema.safeParse(input);
  if (!result.success) {
    throw new Error(
      result.error.issues
        .slice(0, 5)
        .map((issue) => `${issueLocation(issue.path)}: ${issue.message}`)
        .join("\n"),
    );
  }
  const ids = new Set<string>();
  for (const trade of result.data.trades) {
    if (ids.has(trade.id))
      throw new Error(
        `Duplicate trade ID: ${trade.id}. Each completed trade needs a unique ID.`,
      );
  }
  return result.data;
}

export function parseJson(text: string): TradeFile {
  let input: unknown;
  try {
    input = JSON.parse(text);
  } catch {
    throw new Error(
      "This file is not valid JSON. Check its commas, quotes, and brackets.",
    );
  }
  return validateTradeFile(input);
}

export function summarizeTrades(trades: Trade[]) {
  const amounts = trades.map((trade) => toCents(trade.netPnl));
  return {
    count: trades.length,
    net: trades.reduce((sum, trade) => sum + toCents(trade.netPnl), BigInt(0)),
    wins: amounts.filter((amount) => amount > BigInt(0)).length,
    largestWin: amounts.reduce(largerCents, BigInt(0)),
    largestLoss: amounts.reduce(smallerCents, BigInt(0)),
  };
}
