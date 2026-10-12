const ZERO_CENTS = BigInt(0);
const CENTS_PER_UNIT = BigInt(100);

export function toCents(value: string): bigint {
  const [whole, fraction = ""] = value.split(".");

  return BigInt(whole) * CENTS_PER_UNIT + BigInt(fraction.padEnd(2, "0"));
}

function absoluteCents(cents: bigint): bigint {
  if (cents < ZERO_CENTS) {
    return -cents;
  }

  return cents;
}

function signPrefix(cents: bigint, signed: boolean): string {
  if (cents < ZERO_CENTS) {
    return "−";
  }

  if (signed && cents > ZERO_CENTS) {
    return "+";
  }

  return "";
}

export type Currency = "USD" | "USDT";

export function formatMoney(
  cents: bigint,
  signed = false,
  currency: Currency = "USD",
): string {
  const absolute = absoluteCents(cents);
  const sign = signPrefix(cents, signed);
  const units = (absolute / CENTS_PER_UNIT).toLocaleString("en-US");
  const fraction = (absolute % CENTS_PER_UNIT).toString().padStart(2, "0");
  const amount = `${units}.${fraction}`;

  if (currency === "USDT") {
    return `${sign}${amount} USDT`;
  }

  return `${sign}$${amount}`;
}

export function formatCompactMoney(cents: bigint): string {
  const absolute = Math.abs(Number(cents));
  let maximumFractionDigits = 2;

  if (absolute >= 10_000) {
    maximumFractionDigits = 1;
  }

  const formatter = new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits,
  });

  return `${signPrefix(cents, true)}${formatter.format(absolute / 100)}`;
}

export function largerCents(left: bigint, right: bigint): bigint {
  if (right > left) {
    return right;
  }

  return left;
}

export function smallerCents(left: bigint, right: bigint): bigint {
  if (right < left) {
    return right;
  }

  return left;
}
