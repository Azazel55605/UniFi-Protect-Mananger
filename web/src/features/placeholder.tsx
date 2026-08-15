import { Panel, PanelBody } from "@/components/ui/panel";

/**
 * An empty state that says what the view will do and what it needs, rather
 * than apologising. Every unbuilt view names the thing it is waiting on, so
 * the app never leaves you wondering whether something is broken.
 */
export function Placeholder({
  title,
  description,
  waitingOn,
}: {
  title: string;
  description: string;
  waitingOn: string;
}) {
  return (
    <Panel className="max-w-2xl">
      <PanelBody className="py-10 text-center">
        <h2 className="mb-2 text-base font-semibold">{title}</h2>
        <p className="mx-auto mb-5 max-w-md text-sm text-fg-dim">{description}</p>
        <p className="eyebrow">Waiting on {waitingOn}</p>
      </PanelBody>
    </Panel>
  );
}
