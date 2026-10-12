import Decimal from "decimal.js";
import { type Trade } from "@/features/trades/schemas/trade-file.schema";

export type Fill = {
  id: string;
  symbol: string;
  side: "BUY" | "SELL";
  time: number;
  price: string;
  quantity: string;
  fee: string;
  lane?: "LONG" | "SHORT" | "BOTH";
  closed?: string;
  pnl?: string;
  sequence?: string;
};

type Lot = { fill: Fill; quantity: Decimal; fee: Decimal };

const AccountingDecimal = Decimal.clone({ precision: 40 });

function toDecimal(value: string): Decimal {
  return new AccountingDecimal(value);
}

function toMoneyString(value: Decimal): string {
  return value.toDecimalPlaces(2, Decimal.ROUND_HALF_UP).toFixed(2);
}

function fillKey(fill: Fill): string {
  return `${fill.symbol}:${fill.id}`;
}

function uniqueFills(fills: Fill[]): Fill[] {
  return [...new Map(fills.map((fill) => [fillKey(fill), fill])).values()];
}

function compareIntegers(left: string, right: string): number {
  const leftValue = BigInt(left);
  const rightValue = BigInt(right);

  if (leftValue < rightValue) {
    return -1;
  }

  if (leftValue > rightValue) {
    return 1;
  }

  return 0;
}

function compareSequence(left: Fill, right: Fill): number {
  const leftOrder = left.sequence ?? left.id;
  const rightOrder = right.sequence ?? right.id;

  if (/^\d+$/.test(leftOrder) && /^\d+$/.test(rightOrder)) {
    return compareIntegers(leftOrder, rightOrder);
  }

  return leftOrder.localeCompare(rightOrder);
}

function isValidFill(fill: Fill): boolean {
  const price = toDecimal(fill.price);
  const quantity = toDecimal(fill.quantity);

  return (
    Number.isFinite(fill.time) &&
    price.isFinite() &&
    quantity.isFinite() &&
    price.gt(0) &&
    quantity.gt(0) &&
    toDecimal(fill.fee).isFinite()
  );
}

function explicitClosingQuantity(
  fill: Fill,
  quantity: Decimal,
  spot: boolean,
): Decimal | undefined {
  if (fill.closed !== undefined) {
    return toDecimal(fill.closed);
  }

  const closesLong = fill.lane === "LONG" && fill.side === "SELL";
  const closesShort = fill.lane === "SHORT" && fill.side === "BUY";
  const sellsSpot = spot && fill.side === "SELL";

  if (closesLong || closesShort || sellsSpot) {
    return quantity;
  }

  return undefined;
}

export function withBinanceClosingQuantities(
  fills: Fill[],
  endingPositions: Map<string, string>,
): Fill[] {
  const positions = new Map(
    [...endingPositions].map(([symbol, amount]) => [symbol, toDecimal(amount)]),
  );
  const unique = uniqueFills(fills);
  const annotated = new Map<string, Fill>();

  unique.sort(
    (left, right) =>
      left.time - right.time || compareIntegers(left.id, right.id),
  );

  for (const fill of unique.toReversed()) {
    if (fill.lane !== "BOTH") {
      continue;
    }

    const quantity = toDecimal(fill.quantity);

    if (!quantity.isFinite() || !quantity.gt(0)) {
      throw new Error("Exchange returned an invalid fill.");
    }

    const positionAfter = positions.get(fill.symbol);

    if (!positionAfter || !positionAfter.isFinite()) {
      throw new Error(
        "Binance position state is missing. Retry the import or use a file export.",
      );
    }

    let positionBefore: Decimal;
    let closesPosition: boolean;

    if (fill.side === "BUY") {
      positionBefore = positionAfter.minus(quantity);
      closesPosition = positionBefore.lt(0);
    } else {
      positionBefore = positionAfter.plus(quantity);
      closesPosition = positionBefore.gt(0);
    }

    let closed = toDecimal("0");

    if (closesPosition) {
      closed = AccountingDecimal.min(quantity, positionBefore.abs());
    }

    if (
      closed.isZero() &&
      fill.pnl !== undefined &&
      !toDecimal(fill.pnl).isZero()
    ) {
      throw new Error(
        "Binance position history could not be reconciled. Retry when trading is paused or use a file export.",
      );
    }

    annotated.set(fillKey(fill), { ...fill, closed: closed.toString() });
    positions.set(fill.symbol, positionBefore);
  }

  return fills.map((fill) => annotated.get(fillKey(fill)) ?? fill);
}

export function matchFills(fills: Fill[], prefix: string, spot: boolean) {
  const books = new Map<string, Lot[]>();
  const trades: Trade[] = [];
  const unique = uniqueFills(fills);
  let unmatched = 0;
  let exactCommission = toDecimal("0");
  let exactNet = toDecimal("0");
  let roundedCommission = toDecimal("0");
  let roundedNet = toDecimal("0");

  unique.sort(
    (left, right) => left.time - right.time || compareSequence(left, right),
  );

  for (const fill of unique) {
    if (!isValidFill(fill)) {
      throw new Error("Exchange returned an invalid fill.");
    }

    const isOneWayLane = !fill.lane || fill.lane === "BOTH";

    if (
      !spot &&
      fill.pnl !== undefined &&
      isOneWayLane &&
      fill.closed === undefined
    ) {
      throw new Error(
        "Binance one-way fills require verified closing quantities. Retry the import or use a file export.",
      );
    }

    const quantity = toDecimal(fill.quantity);
    const bookKey = `${fill.symbol}:${fill.lane ?? "BOTH"}`;
    const lots = books.get(bookKey) ?? [];
    let closedSide: Trade["side"] = "long";
    let priceDirection = 1;

    if (fill.side === "BUY") {
      closedSide = "short";
      priceDirection = -1;
    }

    books.set(bookKey, lots);

    const available = lots
      .filter((lot) => lot.fill.side !== fill.side)
      .reduce((sum, lot) => sum.plus(lot.quantity), toDecimal("0"));
    const explicit = explicitClosingQuantity(fill, quantity, spot);
    const closing = explicit ?? Decimal.min(quantity, available);

    if (!closing.isFinite() || closing.gt(quantity) || closing.isNegative()) {
      throw new Error("Exchange returned an invalid closing quantity.");
    }

    const reportsProfitWithoutOpening =
      !explicit &&
      fill.pnl !== undefined &&
      !toDecimal(fill.pnl).isZero() &&
      available.isZero();
    const isMissingOpeningFill =
      closing.gt(available) || reportsProfitWithoutOpening;

    if (isMissingOpeningFill) {
      unmatched += 1;

      for (const lot of [...lots]) {
        if (lot.fill.side !== fill.side) {
          lots.splice(lots.indexOf(lot), 1);
        }
      }

      continue;
    }

    let remaining = closing;

    for (const lot of [...lots]) {
      if (remaining.isZero() || lot.fill.side === fill.side) {
        continue;
      }

      const size = Decimal.min(remaining, lot.quantity);
      const entryFee = lot.fee.mul(size).div(lot.quantity);
      const exitFee = toDecimal(fill.fee).mul(size).div(quantity);
      const commission = entryFee.plus(exitFee).negated();
      let gross: Decimal;

      if (fill.pnl !== undefined) {
        gross = toDecimal(fill.pnl).mul(size).div(closing);
      } else {
        gross = toDecimal(fill.price)
          .minus(lot.fill.price)
          .mul(size)
          .mul(priceDirection);
      }

      exactCommission = exactCommission.plus(commission);
      exactNet = exactNet.plus(gross.plus(commission));

      const nextCommission = toDecimal(toMoneyString(exactCommission));
      const nextNet = toDecimal(toMoneyString(exactNet));
      const allocatedCommission = nextCommission.minus(roundedCommission);
      const allocatedNet = nextNet.minus(roundedNet);

      roundedCommission = nextCommission;
      roundedNet = nextNet;

      trades.push({
        id: `${prefix}:${fill.symbol}:${fill.id}:${lot.fill.id}`,
        symbol: fill.symbol,
        side: closedSide,
        volume: size.toNumber(),
        openedAt: new Date(lot.fill.time).toISOString(),
        closedAt: new Date(fill.time).toISOString(),
        entryPrice: toDecimal(lot.fill.price).toNumber(),
        exitPrice: toDecimal(fill.price).toNumber(),
        stopLoss: null,
        commission: toMoneyString(allocatedCommission),
        swap: "0.00",
        netPnl: toMoneyString(allocatedNet),
      });

      remaining = remaining.minus(size);
      lot.quantity = lot.quantity.minus(size);
      lot.fee = lot.fee.minus(entryFee);

      if (lot.quantity.isZero()) {
        lots.splice(lots.indexOf(lot), 1);
      }
    }

    const opening = quantity.minus(closing);

    if (opening.gt(0) && !(spot && fill.side === "SELL")) {
      lots.push({
        fill,
        quantity: opening,
        fee: toDecimal(fill.fee).mul(opening).div(quantity),
      });
    }
  }

  const openLots = [...books.values()].reduce(
    (sum, lots) => sum + lots.length,
    0,
  );

  return { trades, unmatched, openLots, fills: unique.length };
}
