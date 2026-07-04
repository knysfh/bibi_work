import { QueryClient } from "@tanstack/react-query";

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 15_000,
        retry: (failureCount, error) => {
          const status = typeof error === "object" && error && "status" in error ? error.status : 0;
          if (status === 401 || status === 403) {
            return false;
          }
          return failureCount < 2;
        }
      },
      mutations: {
        retry: false
      }
    }
  });
}
