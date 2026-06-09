export type ModelCost = {
  name: string;
  cost: number;
  share: number;
};

export type DashboardData = {
  balance: number;
  topModels: ModelCost[];
  otherModels: ModelCost[];
};

export type DashboardState = {
  status: 'loading' | 'error' | 'ready';
  data?: DashboardData;
  message?: string;
  staleMessage?: string;
};
