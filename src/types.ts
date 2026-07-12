export type QuotaInfo = {
  used: number;
  monthlyLimit: number;
  onDemandCap: number;
  billingPeriodStart: string;
  billingPeriodEnd: string;
  percentUsed: number;
  fetchedAt: string;
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
};

export type Settings = {
  grokBinaryPath?: string | null;
  grokHome?: string | null;
};
