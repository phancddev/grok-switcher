export type PeriodQuota = {
  kind: string;
  label: string;
  used: number;
  limit: number;
  percentUsed: number;
  periodStart: string;
  periodEnd: string;
  resetsAt: string;
  daysUntilReset: number;
  /** "api" | "tracked" */
  source: string;
};

export type QuotaInfo = {
  used: number;
  monthlyLimit: number;
  onDemandCap: number;
  billingPeriodStart: string;
  billingPeriodEnd: string;
  percentUsed: number;
  fetchedAt: string;
  periodKind?: string;
  periodLabel?: string;
  daysUntilReset?: number;
  resetsAt?: string;
  monthly?: PeriodQuota | null;
  weekly?: PeriodQuota | null;
};

export type AccountSummary = {
  userId: string;
  email: string;
  firstName?: string | null;
  lastName?: string | null;
  label?: string | null;
  isActive: boolean;
  lastUsed?: string | null;
  createdAt?: string | null;
  quota?: QuotaInfo | null;
  tier?: number | null;
  subscriptionTier?: string | null;
  planExpiresAt?: string | null;
};

export type Settings = {
  grokBinaryPath?: string | null;
  grokHome?: string | null;
};
