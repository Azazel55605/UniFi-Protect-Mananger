import { useState } from "react";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { AppShell } from "@/components/AppShell";
import { LoginPage } from "@/features/auth/LoginPage";
import { SetupWizard } from "@/features/setup/SetupWizard";
import { OverviewPage } from "@/features/overview/OverviewPage";
import { EventsPage } from "@/features/events/EventsPage";
import { LogsPage } from "@/features/logs/LogsPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { Placeholder } from "@/features/placeholder";

export function App() {
  const [reconfiguring, setReconfiguring] = useState(false);

  const auth = useQuery({ queryKey: ["auth"], queryFn: api.authStatus });
  const setup = useQuery({
    queryKey: ["setup"],
    queryFn: api.setup,
    enabled: auth.data?.authenticated === true,
  });

  if (auth.isLoading) return <Splash />;

  if (auth.isError) {
    return (
      <div className="flex min-h-screen items-center justify-center p-6">
        <p className="max-w-sm text-center text-sm text-bad">
          Can't reach the server. Check the container is running, then reload.
        </p>
      </div>
    );
  }

  if (!auth.data?.authenticated) {
    return (
      <LoginPage
        configured={auth.data?.configured ?? false}
        onSignedIn={() => auth.refetch()}
      />
    );
  }

  if (setup.isLoading) return <Splash />;

  // Setup gates the app: without it, every view would be empty for reasons the
  // user can't see. Reachable again from Settings once complete.
  if (!setup.data?.complete || reconfiguring) {
    return (
      <SetupWizard
        onDone={() => {
          setReconfiguring(false);
          setup.refetch();
        }}
      />
    );
  }

  return (
    <BrowserRouter>
      <AppShell>
        <Routes>
          <Route path="/" element={<OverviewPage />} />
          <Route path="/events" element={<EventsPage />} />
          <Route
            path="/timeline"
            element={
              <Placeholder
                title="Timeline"
                description="A scrubbable day of footage with thumbnails, and clip playback in place."
                waitingOn="the event index and transcoding"
              />
            }
          />
          <Route
            path="/archive"
            element={
              <Placeholder
                title="Archive"
                description="What has been archived, what is due, and running an archive on demand or on a schedule."
                waitingOn="the archive runner"
              />
            }
          />
          <Route
            path="/storage"
            element={
              <Placeholder
                title="Capacity"
                description="Pool usage, growth over time, and how much footage is live versus archived."
                waitingOn="storage sampling"
              />
            }
          />
          <Route path="/logs" element={<LogsPage />} />
          <Route
            path="/settings"
            element={<SettingsPage onReconfigure={() => setReconfiguring(true)} />}
          />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </AppShell>
    </BrowserRouter>
  );
}

function Splash() {
  return (
    <div className="flex min-h-screen items-center justify-center">
      <span className="tally bg-signal tally-live" aria-hidden />
    </div>
  );
}
