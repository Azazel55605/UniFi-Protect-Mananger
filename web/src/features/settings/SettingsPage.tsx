import { useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import { AppearancePicker } from "@/components/AppearanceControls";
import { SettingsSummary } from "@/features/setup/SetupWizard";
import { WatchdogPanel } from "@/features/watchdog/WatchdogPanel";

export function SettingsPage({ onReconfigure }: { onReconfigure: () => void }) {
  const queryClient = useQueryClient();

  return (
    <div className="max-w-3xl space-y-4">
      <SettingsSummary />

      <WatchdogPanel />

      <Panel>
        <PanelHeader label="Appearance" />
        <PanelBody>
          <AppearancePicker />
        </PanelBody>
      </Panel>

      <div className="flex items-center gap-3">
        <Button onClick={onReconfigure}>Run setup again</Button>
        <Button
          variant="danger"
          onClick={async () => {
            await api.logout();
            queryClient.clear();
            location.reload();
          }}
        >
          Sign out
        </Button>
      </div>
    </div>
  );
}
