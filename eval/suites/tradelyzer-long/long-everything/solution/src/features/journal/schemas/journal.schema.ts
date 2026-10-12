import { DateTime } from "luxon";
import { z } from "zod";
import { toCents } from "@/lib/money";

export const MAX_JOURNAL_TAGS = 10;

export const journalSchema = z.object({
  date: z.string().refine(isJournalDate, "Choose a valid date"),
  bias: z.enum(["neutral", "bullish", "bearish"]),
  levels: z.string().max(10000),
  maxLoss: z
    .string()
    .regex(
      /^(?:\d{1,15}(?:\.\d{1,2})?)?$/,
      "Use a positive dollar amount with at most two decimals",
    ),
  plan: z.string().max(10000),
  wentWell: z.string().max(10000),
  mistakes: z.string().max(10000),
  tags: z
    .array(z.string().trim().min(1).max(60))
    .max(MAX_JOURNAL_TAGS, "Choose up to 10 mistake tags."),
  mood: z.number().int().min(1).max(5).nullable(),
});
export function journalSchemaFor(previous?: { tags: string[] }) {
  if (!previous || previous.tags.length <= MAX_JOURNAL_TAGS)
    return journalSchema;
  return journalSchema.extend({
    tags: z
      .array(journalSchema.shape.tags.element)
      .max(30)
      .refine(
        (tags) =>
          tags.length <= MAX_JOURNAL_TAGS ||
          tags.every((tag) => previous.tags.includes(tag)),
        "Choose up to 10 mistake tags. Existing tags can be kept or removed.",
      ),
  });
}

export type JournalEntry = z.infer<typeof journalSchema>;
export const planTemplateSchema = journalSchema.pick({
  bias: true,
  levels: true,
  maxLoss: true,
  plan: true,
});
export type PlanTemplate = z.infer<typeof planTemplateSchema>;

export function isJournalDate(date: string) {
  return /^\d{4}-\d{2}-\d{2}$/.test(date) && DateTime.fromISO(date).isValid;
}

export function emptyJournal(
  date: string,
  template?: PlanTemplate,
): JournalEntry {
  return {
    date,
    bias: template?.bias ?? "neutral",
    levels: template?.levels ?? "",
    maxLoss: template?.maxLoss ?? "",
    plan: template?.plan ?? "",
    wentWell: "",
    mistakes: "",
    tags: [],
    mood: null,
  };
}

export function exceedsLossLimit(net: bigint, maxLoss = "") {
  return maxLoss !== "" && net < -toCents(maxLoss);
}
