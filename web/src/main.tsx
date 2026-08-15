import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { App } from "./App";
import { ApiError } from "@/lib/api";
import "./index.css";

const queryClient = new QueryClient({
  // A session expires after fourteen days, and browser tabs live longer than
  // that. Without this, an expired session turns every panel on the page into
  // its own error box while the app insists it is still signed in — so the one
  // query that decides that is refreshed, and the app falls back to the login
  // screen by its normal route.
  queryCache: new QueryCache({
    onError: (error, query) => {
      if (error instanceof ApiError && error.isAuthFailure && query.queryKey[0] !== "auth") {
        queryClient.invalidateQueries({ queryKey: ["auth"] });
      }
    },
  }),
  defaultOptions: {
    queries: {
      // Retrying a rejection cannot change its answer: the session is gone, the
      // clip is archived, setup is unfinished. Only faults and network trouble
      // are worth a second attempt.
      retry: (count, error) => {
        if (error instanceof ApiError) {
          return error.code === "internal" && count < 2;
        }
        return count < 2;
      },
      refetchOnWindowFocus: false,
      staleTime: 5_000,
    },
  },
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </StrictMode>,
);
