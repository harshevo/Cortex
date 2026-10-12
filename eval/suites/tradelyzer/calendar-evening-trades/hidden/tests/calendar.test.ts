import { describe, expect, it } from "vitest";
import { DateTime } from "luxon";
import {
  calendarWeeks,
  groupByCloseDay,
} from "@/features/calendar/utils/calendar";
import type { Trade } from "@/features/trades/schemas/trade-file.schema";

const trade = (closedAt: string) => ({ closedAt }) as Trade;

describe("Luxon calendar", () => {
  it("covers all dates of a six-week month including adjacent days", () => {
    const weeks = calendarWeeks(
      DateTime.fromISO("2026-03-01", { zone: "America/New_York" }),
    );
    expect(weeks).toHaveLength(6);
    expect(weeks[0][0].toISODate()).toBe("2026-02-23");
    expect(weeks[5][6].toISODate()).toBe("2026-04-05");
    expect(new Set(weeks.flat().map((day) => day.toISODate())).size).toBe(42);
    expect(weeks.flat().every((day) => day.hour === 0)).toBe(true);
  });
  it("handles leap February and December rollover", () => {
    expect(
      calendarWeeks(DateTime.fromISO("2028-02-01"))
        .flat()
        .some((day) => day.toISODate() === "2028-02-29"),
    ).toBe(true);
    expect(
      calendarWeeks(DateTime.fromISO("2026-12-01")).at(-1)!.at(-1)!.toISODate(),
    ).toBe("2027-01-03");
  });
  it("groups by local close day across midnight", () => {
    const groups = groupByCloseDay(
      [trade("2026-09-07T21:43:00Z")],
      "Asia/Kolkata",
    );
    expect(groups.get("2026-09-08")).toHaveLength(1);
    expect(groups.has("2026-09-07")).toBe(false);
  });
  it("groups both repeated DST hours on the same civil day", () => {
    const groups = groupByCloseDay(
      [trade("2026-11-01T05:30:00Z"), trade("2026-11-01T06:30:00Z")],
      "America/New_York",
    );
    expect(groups.get("2026-11-01")).toHaveLength(2);
  });
});
