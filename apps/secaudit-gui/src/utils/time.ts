const TIME_LABEL_LOCALE = "zh-CN";

const TIME_LABEL_OPTIONS = {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
} satisfies Intl.DateTimeFormatOptions;
const SESSION_TIME_OPTIONS = {
  hour: "2-digit",
  minute: "2-digit",
} satisfies Intl.DateTimeFormatOptions;
const TRACE_TIME_OPTIONS = {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
} satisfies Intl.DateTimeFormatOptions;
const SESSION_SHORT_DATE_OPTIONS = {
  month: "2-digit",
  day: "2-digit",
} satisfies Intl.DateTimeFormatOptions;
const SESSION_FULL_DATE_OPTIONS = {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
} satisfies Intl.DateTimeFormatOptions;

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
const RECENT_DAYS = 6;
const relativeTimeFormatter = new Intl.RelativeTimeFormat(TIME_LABEL_LOCALE, {
  numeric: "auto",
});

export function formatTimeLabel(date: Date = new Date()): string {
  return date.toLocaleTimeString(TIME_LABEL_LOCALE, TIME_LABEL_OPTIONS);
}

export function formatTraceTimeLabel(value: string, now: Date = new Date()): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  if (isSameLocalDate(date, now)) {
    return date.toLocaleTimeString(TIME_LABEL_LOCALE, TRACE_TIME_OPTIONS);
  }
  return `${formatShortDate(date)} ${date.toLocaleTimeString(TIME_LABEL_LOCALE, TRACE_TIME_OPTIONS)}`;
}

export function formatSessionTimeLabel(value: string, now: Date = new Date()): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  const diffMs = now.getTime() - date.getTime();
  const absDiffMs = Math.abs(diffMs);
  if (absDiffMs < MINUTE_MS) {
    return "刚刚";
  }
  if (absDiffMs < HOUR_MS) {
    return relativeTimeFormatter.format(Math.round(-diffMs / MINUTE_MS), "minute");
  }
  if (isSameLocalDate(date, now)) {
    return `今天 ${formatSessionClock(date)}`;
  }
  if (isSameLocalDate(date, addDays(now, -1))) {
    return `昨天 ${formatSessionClock(date)}`;
  }
  if (absDiffMs < RECENT_DAYS * DAY_MS) {
    return `${relativeTimeFormatter.format(Math.round(-diffMs / DAY_MS), "day")} ${formatSessionClock(date)}`;
  }
  if (date.getFullYear() === now.getFullYear()) {
    return `${formatShortDate(date)} ${formatSessionClock(date)}`;
  }
  return `${formatFullDate(date)} ${formatSessionClock(date)}`;
}

export function formatSessionAbsoluteTimeLabel(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return `${formatFullDate(date)} ${formatSessionClock(date)}`;
}

function isSameLocalDate(left: Date, right: Date): boolean {
  return (
    left.getFullYear() === right.getFullYear() &&
    left.getMonth() === right.getMonth() &&
    left.getDate() === right.getDate()
  );
}

function addDays(date: Date, days: number): Date {
  const nextDate = new Date(date);
  nextDate.setDate(nextDate.getDate() + days);
  return nextDate;
}

function formatSessionClock(date: Date): string {
  return date.toLocaleTimeString(TIME_LABEL_LOCALE, SESSION_TIME_OPTIONS);
}

function formatShortDate(date: Date): string {
  return date.toLocaleDateString(TIME_LABEL_LOCALE, SESSION_SHORT_DATE_OPTIONS);
}

function formatFullDate(date: Date): string {
  return date.toLocaleDateString(TIME_LABEL_LOCALE, SESSION_FULL_DATE_OPTIONS);
}
