import { DateTime } from "luxon";
import type { Trade } from "@/features/trades/schemas/trade-file.schema";

export function calendarWeeks(month: DateTime): DateTime[][] {
  const first = month.startOf("month");
  const start = first.minus({ days: first.weekday - 1 });
  const last = month.endOf("month");
  const end = last.startOf("day");
  const weeks: DateTime[][] = [];
  for (let day = start; day.plus({ days: 6 }) <= end; day = day.plus({ weeks: 1 })) {
    weeks.push(
      Array.from({ length: 7 }, (_, index) => day.plus({ days: index })),
    );
  }
  return weeks;
}

export function groupByCloseDay(
  trades: Trade[],
  zone: string,
): Map<string, Trade[]> {
  const days = new Map<string, Trade[]>();
  for (const trade of trades) {
    const day = DateTime.fromISO(trade.closedAt, { setZone: true }).toISODate()!;
    const group = days.get(day) ?? [];
    group.push(trade);
    days.set(day, group);
  }
  return days;
}
